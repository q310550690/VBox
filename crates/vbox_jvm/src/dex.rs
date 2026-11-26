//! dex → jar：很多 TVBox 蜘蛛发布的是「只有 classes.dex 的伪装 jar」，
//! JVM 只能加载 `.class`，所以进宿主前必须先转。
//!
//! 设计取舍（对应计划 §6.6 缓存布局）：
//! - 转换器用 dex2jar（`jvm/vendor/dex2jar/*.jar` 随包 stage 到 `<runtime>/dex2jar/`）；
//! - 结果按**源文件 sha256** 命名缓存，二次进源零等待；
//! - 先写 `.part.jar` 再原子改名，避免进程中途被杀留下半成品；
//! - 只做「探测 + 转换」，不在这里判断蜘蛛能不能跑（那是宿主的事）。

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::errors::{JvmError, Result};
use crate::runtime::Runtime;

/// 转换超时：计划 §7 P3 要求 15 分钟（大 jar 首次转换实测 3–40 s，留足余量；
/// 超时后杀掉子进程并清掉 `.part.jar`，不会把半成品留在缓存里）。
pub const DEFAULT_CONVERT_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone)]
pub struct DexOptions {
    pub cache_dir: PathBuf,
    pub timeout: Duration,
}

impl DexOptions {
    /// 默认缓存目录：`VBOX_DATA_DIR/jvm-cache` → `LOCALAPPDATA/VBox/jvm-cache` → 系统临时目录。
    pub fn default_cache() -> PathBuf {
        if let Ok(dir) = std::env::var("VBOX_DATA_DIR") {
            if !dir.trim().is_empty() {
                return PathBuf::from(dir).join("jvm-cache");
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.trim().is_empty() {
                return PathBuf::from(local).join("VBox").join("jvm-cache");
            }
        }
        std::env::temp_dir().join("vbox-jvm-cache")
    }

    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        DexOptions {
            cache_dir: cache_dir.into(),
            timeout: DEFAULT_CONVERT_TIMEOUT,
        }
    }
}

/// zip 中央目录里的一项：只取「解码裸 dex」需要的字段。
#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    /// 数据区在文件中的起始偏移（已跳过本地文件头）。
    pub data_off: usize,
    pub comp_size: usize,
    /// 0 = stored，8 = deflate。
    pub method: u16,
}

/// 读 zip 中央目录（不依赖 zip crate；TVBox 的伪装 jar 都是标准 zip，够用即止）。
pub fn entries(path: &Path) -> Result<Vec<ZipEntry>> {
    let bytes = std::fs::read(path)?;
    let eocd = find_eocd(&bytes).ok_or_else(|| JvmError::Protocol {
        line: format!("不是合法 zip：{}", path.display()),
    })?;
    let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
    let mut off = u32::from_le_bytes([
        bytes[eocd + 16],
        bytes[eocd + 17],
        bytes[eocd + 18],
        bytes[eocd + 19],
    ]) as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if off + 46 > bytes.len() || &bytes[off..off + 4] != b"PK\x01\x02" {
            break;
        }
        let method = u16::from_le_bytes([bytes[off + 10], bytes[off + 11]]);
        let comp_size = u32::from_le_bytes([
            bytes[off + 20],
            bytes[off + 21],
            bytes[off + 22],
            bytes[off + 23],
        ]) as usize;
        let name_len = u16::from_le_bytes([bytes[off + 28], bytes[off + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[off + 30], bytes[off + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[off + 32], bytes[off + 33]]) as usize;
        let local = u32::from_le_bytes([
            bytes[off + 42],
            bytes[off + 43],
            bytes[off + 44],
            bytes[off + 45],
        ]) as usize;
        let name_start = off + 46;
        if name_start + name_len > bytes.len() || local + 30 > bytes.len() {
            break;
        }
        let name = String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).into_owned();
        let lname = u16::from_le_bytes([bytes[local + 26], bytes[local + 27]]) as usize;
        let lextra = u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]) as usize;
        out.push(ZipEntry {
            name,
            data_off: local + 30 + lname + lextra,
            comp_size,
            method,
        });
        off = name_start + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// 只读条目名（兼容旧调用点）。
pub fn entry_names(path: &Path) -> Result<Vec<String>> {
    Ok(entries(path)?.into_iter().map(|e| e.name).collect())
}

/// 从 jar 里取出第一个 `.dex` 写成裸文件——`DexJarRepair` 的 baksmali 需要独立 dex。
fn extract_dex(jar: &Path, out: &Path) -> Result<bool> {
    let bytes = std::fs::read(jar)?;
    let list = entries(jar)?;
    let mut pick = list.iter().find(|e| e.name == "classes.dex");
    if pick.is_none() {
        pick = list.iter().find(|e| e.name.to_ascii_lowercase().ends_with(".dex"));
    }
    let Some(entry) = pick else { return Ok(false) };
    let raw = if entry.method == 0 {
        let end = entry.data_off + entry.comp_size;
        if end > bytes.len() {
            return Ok(false);
        }
        bytes[entry.data_off..end].to_vec()
    } else if entry.method == 8 {
        use std::io::Read;
        let end = entry.data_off + entry.comp_size;
        if end > bytes.len() {
            return Ok(false);
        }
        let mut dec = flate2::read::DeflateDecoder::new(&bytes[entry.data_off..end]);
        let mut buf = Vec::new();
        dec.read_to_end(&mut buf)?;
        buf
    } else {
        return Ok(false);
    };
    if !(raw.starts_with(b"dex\n") || raw.starts_with(b"dey\n")) {
        return Ok(false);
    }
    std::fs::write(out, &raw)?;
    Ok(true)
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let start = bytes.len().saturating_sub(66 * 1024);
    (start..=bytes.len() - 22)
        .rev()
        .find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
}

/// 需要转换：含 `*.dex` 且不含 `*.class`（TVBox 常见形态，扩展名可能是 `.db`/`.dat`）。
///
/// 判型按 **zip 条目内容**而不是外层扩展名；对 stored 的 dex 再验一次魔数
/// （`dex\n` / `dey\n`），deflate 的条目不验（要解压才看得到，成本不值得）。
/// 转换产物文件名：`<sha256>.v4.jar`（v4 = 补资源版）。
fn product_name(digest: &str) -> String {
    format!("{digest}.v4.jar")
}

pub fn needs_conversion(path: &Path) -> Result<bool> {
    let list = entries(path)?;
    if list.iter().any(|e| e.name.ends_with(".class")) {
        return Ok(false);
    }
    let dex: Vec<&ZipEntry> = list
        .iter()
        .filter(|e| e.name.to_ascii_lowercase().ends_with(".dex"))
        .collect();
    if dex.is_empty() {
        return Ok(false);
    }
    let bytes = std::fs::read(path)?;
    for e in dex {
        if e.method == 0 {
            let end = (e.data_off + 4).min(bytes.len());
            if e.data_off + 4 <= bytes.len() {
                let head = &bytes[e.data_off..end];
                if head.starts_with(b"dex\n") || head.starts_with(b"dey\n") {
                    return Ok(true);
                }
            }
        } else {
            return Ok(true);
        }
    }
    // 全是 stored 但魔数都不对：不是 dex 包（可能是伪装成 .dex 的资源包）
    Ok(false)
}

/// 确保得到一个可被 JVM 加载的 jar：原样可用则返回原路径，否则返回转换后的缓存路径。
pub fn ensure_loadable(runtime: &Runtime, jar: &Path, opts: &DexOptions) -> Result<PathBuf> {
    if !needs_conversion(jar)? {
        return Ok(jar.to_path_buf());
    }
    std::fs::create_dir_all(&opts.cache_dir)?;
    // P1：Java 转换/修复进程的 java.io.tmpdir 必须落在可写缓存内——
    // Windows 沙箱/安全软件常把系统 TEMP 判为拒绝访问，导致 DexJarRepair 静默失败。
    let java_tmp = opts.cache_dir.join("tmpdir");
    let _ = std::fs::create_dir_all(&java_tmp);
    let digest = sha256_file(jar)?;
    // 产物版本：v4 起把源 jar 的 assets/*.so 等资源一并补进产物（加固壳 native 桥需要）。
    // 改产物格式时递增它，旧缓存自然会重建。
    let out = opts.cache_dir.join(product_name(&digest));
    // 缓存命中也要验质量：历史上出现过 22 字节空 zip 被当成成品（见下方 DexConvert 注释）。
    if out.is_file() && std::fs::metadata(&out)?.len() > 1024 {
        if class_count(&out).unwrap_or(0) > 0 {
            return Ok(out);
        }
        let _ = std::fs::remove_file(&out);
    }
    let tmp = opts.cache_dir.join(format!("{}.part.jar", product_name(&digest)));
    // 收编上次会话留下的半成品：dex2jar 已经跑完、只是进程被杀没来得及改名。
    if tmp.is_file() && std::fs::metadata(&tmp)?.len() > 1024 && class_count(&tmp).unwrap_or(0) > 0 {
        let _ = std::fs::rename(&tmp, &out);
        if std::env::var("VBOX_DEBUG_DEX").is_ok() {
            eprintln!("[dex] 收编上次残留 part.jar -> {}", out.display());
        }
        return Ok(out);
    }
    let _ = std::fs::remove_file(&tmp);

    let dex2jar = runtime.dex2jar_dir();
    if !dex2jar.is_dir() {
        return Err(JvmError::MissingRuntime { path: dex2jar });
    }
    let dex_cp = format!("{}/*", plain_path(&dex2jar).display());
    let stub_cp = format!(
        "{}{}{}",
        plain_path(&runtime.stubs_jar()).display(),
        cp_sep(),
        dex_cp
    );
    if std::env::var("VBOX_DEBUG_DEX").is_ok() {
        eprintln!(
            "[dex] java={} cp={} in={} out={}",
            runtime.java_bin().display(),
            stub_cp,
            jar.display(),
            tmp.display()
        );
    }

    let total = class_total(jar).unwrap_or(0);
    let progress = progress_begin(jar, &digest, total);
    // ① 优先走自研进程内转换器（跑在普通子 JVM 里）：它用 `FileOutputStream` 写 zip，
    //    完全绕开 `jdk.zipfs`，因此不受「目标文件系统可写性判定为只读」这类
    //    沙箱/安全软件误判影响；reader 配置也与 dex2jar 上游 `Dex2jar` 一致。
    let (mut ok, mut tail) = run_java(
        runtime,
        &stub_cp,
        &[
            "tvbox.dex.ConvertMain".to_string(),
            plain_path(jar).to_string_lossy().into_owned(),
            plain_path(&tmp).to_string_lossy().into_owned(),
        ],
        &java_tmp,
        opts.timeout,
        move |line| progress_line(progress, line),
    )?;

    // 自研转换器遇到某些 dex 会「部分失败」（逐类 try/catch 后仍返回成功）。
    // 用 dex 头的 class 总数做完整性闸门：明显偏少就回退官方 Dex2jarCmd 再转一次。
    if ok && total > 0 {
        let got = class_count(&tmp).unwrap_or(0);
        if (got as u64) * 10 < (total as u64) * 9 {
            eprintln!("[dex] 进程内转换只写出 {got}/{total} 个 class，回退 Dex2jarCmd 重转");
            ok = false;
        }
    }
    // ② 退回流式 `Dex2jarCmd`：兼容没有 stubs.jar 的自定义布局，或转换器本身出错的情形。
    if !ok || class_count(&tmp).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&tmp);
        let (ok2, tail2) = run_java(
            runtime,
            &dex_cp,
            &[
                "-Xmx1g".to_string(),
                "com.googlecode.dex2jar.tools.Dex2jarCmd".to_string(),
                "-f".to_string(),
                "-o".to_string(),
                plain_path(&tmp).to_string_lossy().into_owned(),
                plain_path(jar).to_string_lossy().into_owned(),
            ],
            &java_tmp,
            opts.timeout,
            |_line| {},
        )?;
        ok = ok2;
        tail = format!("{tail}\n--- Dex2jarCmd 回退 ---\n{tail2}");
    }
    let status_ok = ok;
    // 日志落盘：转换失败时能直接看 `<sha>.d2j.log`，不用重跑一遍（计划 §7 P3「日志落盘」）。
    let log_path = opts.cache_dir.join(format!("{digest}.d2j.log"));
    let _ = std::fs::write(&log_path, &tail);
    if !status_ok {
        let _ = std::fs::remove_file(&tmp);
        progress_finish(progress, false, Some(tail.clone()));
        return Err(JvmError::Exit {
            code: -1,
            stderr: tail,
        });
    }
    // 质量闸门：dex2jar 遇到「目标目录被判只读」会以 0 退出、只写 22 字节空 zip。
    // 这类半成品一旦进缓存，后续所有请求都会拿到「没有 Spider 类」的诡异错误，
    // 所以在落盘前必须验类数，失败就清掉并报 DexConvert（带 stderr 尾部便于定位）。
    let classes = class_count(&tmp).unwrap_or(0);
    if classes == 0 {
        let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&tmp);
        progress_finish(progress, false, Some(format!("产物 0 个 class（{size} 字节）")));
        return Err(JvmError::DexConvert {
            detail: format!(
                "{} 转换后 {} 字节 / 0 个 class（目标文件系统可写性判定可能被安全软件或沙箱拦截）；stderr 尾部：{}",
                jar.display(),
                size,
                tail
            ),
        });
    }
    std::fs::rename(&tmp, &out)?;
    progress_finish(progress, true, None);
    // 修 dex2jar 对 R8 `NEW` 的误译（MIT 上游 DexJarRepair）；失败不致命，只记日志。
    let _ = repair_new_sites(runtime, jar, &out, &digest, opts);
    if std::env::var("VBOX_DEBUG_DEX").is_ok() {
        eprintln!("[dex] ok classes={classes} out={}", out.display());
    }
    Ok(out)
}


/// 跑一个 java 转换子进程：`java -cp <cp> <extra…>`。
///
/// stderr 必须边跑边收：上千个类的「Detail Error」能超过管道缓冲区，等退出再读会把子进程堵死。
/// 返回 `(是否成功, stderr 尾部 80 行)`；超时会 kill 子进程并返回 `JvmError::Timeout`。
fn run_java(
    runtime: &Runtime,
    cp: &str,
    extra: &[String],
    tmpdir: &Path,
    timeout: Duration,
    mut on_stderr: impl FnMut(&str) + Send + 'static,
) -> Result<(bool, String)> {
    let mut child = Command::new(runtime.java_bin())
        .arg(format!("-Djava.io.tmpdir={}", plain_path(tmpdir).display()))
        // 子进程把 stderr 写到管道：显式钉 UTF-8，否则 Windows 控制台代码页会把
        // 中文日志变成乱码（`.d2j.log` 里就没法看了）。
        .args(["-Dfile.encoding=UTF-8", "-Dsun.stdout.encoding=UTF-8", "-Dsun.stderr.encoding=UTF-8"])
        .arg("-cp")
        .arg(cp)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr_pipe = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        if let Some(pipe) = stderr_pipe {
            for line in std::io::BufReader::new(pipe).lines().map_while(|l| l.ok()) {
                on_stderr(&line);
                if tail.len() >= 80 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
        tail.into_iter().collect::<Vec<String>>()
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(120));
    };
    let tail = stderr_handle.join().unwrap_or_default().join("\n");
    match status {
        Some(st) => Ok((st.success(), tail)),
        None => Err(JvmError::Timeout {
            ms: timeout.as_millis() as u64,
            stderr: tail,
        }),
    }
}

fn cp_sep() -> char {
    if cfg!(windows) {
        ';'
    } else {
        ':'
    }
}

/// 跑 `DexJarRepair`：dex2jar 会把「类被 R8 去掉了构造器」的 `NEW` 译成抽象类实例化，
/// 不修就在宿主里报 `InstantiationError`。失败保留原 jar（best-effort），日志落盘便于二查。
fn repair_new_sites(
    runtime: &Runtime,
    src_jar: &Path,
    converted: &Path,
    digest: &str,
    opts: &DexOptions,
) -> Option<String> {
    let tools = runtime.tools_jar();
    if !tools.is_file() {
        return None;
    }
    let dex2jar = runtime.dex2jar_dir();
    if !dex2jar.is_dir() {
        return None;
    }
    let dex_tmp = opts.cache_dir.join(format!("{digest}.dex"));
    match extract_dex(src_jar, &dex_tmp) {
        Ok(true) => {}
        _ => {
            let _ = std::fs::remove_file(&dex_tmp);
            return None;
        }
    }
    // 注意：**不要用 `dir/*` 通配符**。Windows 的 java 启动器会把恰好以 `*` 结尾的
    // **程序参数**也按类路径通配展开（不止 -cp），于是 DexJarRepair 收到的 `d2jCp`
    // 会变成一串独立 argv，args[3] 只剩第一个 jar → baksmali 子进程 CNFE。
    // 这里显式枚举 jar 并用 `;` 连接，作为单个参数传入最稳。
    let mut d2j_jars: Vec<String> = std::fs::read_dir(&dex2jar)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().map(|x| x == "jar").unwrap_or(false))
        .map(|p| plain_path(&p).display().to_string())
        .collect();
    d2j_jars.sort();
    if d2j_jars.is_empty() {
        return None;
    }
    let d2j_cp = d2j_jars.join(&cp_sep().to_string());
    let cp = format!(
        "{}{}{}",
        d2j_cp,
        cp_sep(),
        plain_path(&tools).display()
    );
    let java = runtime.java_bin();
    if std::env::var("VBOX_DEBUG_DEX").is_ok() {
        eprintln!("[dex] repair java={} d2jCp={}", java.display(), d2j_cp);
    }
    // 同 ensure_loadable：把 JDK 临时目录固定到可写缓存下（否则 DexJarRepair 创建临时目录被拒）。
    let repair_tmp = opts.cache_dir.join("tmpdir");
    let _ = std::fs::create_dir_all(&repair_tmp);
    let out = Command::new(&java)
        .arg(format!("-Djava.io.tmpdir={}", plain_path(&repair_tmp).display()))
        .arg("-Xmx1g")
        .arg("-cp")
        .arg(&cp)
        .arg("com.winbox.nativebridge.DexJarRepair")
        .arg(plain_path(&dex_tmp))
        .arg(plain_path(converted))
        .arg(plain_path(&java))
        .arg(&d2j_cp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let _ = std::fs::remove_file(&dex_tmp);
    match out {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(20);
            let summary = lines[start..].join("\n");
            let _ = std::fs::write(opts.cache_dir.join(format!("{digest}.fix.log")), &summary);
            // DexJarRepair 的 main 即便修复失败（前置条件不满足/ baksmali 失败）也以 exit 0 返回
            // 并打印 `fixed sites = -1`；必须按输出判断，否则会把「没修」误当成功写 .fix.ok。
            let repaired = o.status.success() && !summary.contains("fixed sites = -1");
            if repaired {
                let _ = std::fs::write(
                    opts.cache_dir.join(format!("{digest}.fix.ok")),
                    format!("{}\n", summary),
                );
                Some(summary)
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// 一次 dex → jar 转换的可观测进度（设置页/CLI 轮询用）。
///
/// 进度来自子进程 stderr 的 `[dex-stream] 已写出 N 个 class`；总数来自 dex 头的
/// `class_defs_size`（拿不到就是 0，前端显示「已转换 N 类」而不是百分比）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConvertProgress {
    pub key: String,
    pub jar: String,
    pub bytes: u64,
    pub total_classes: u32,
    pub classes_done: u32,
    pub started_ms: u128,
    pub elapsed_ms: u128,
    pub done: bool,
    pub ok: bool,
    pub error: Option<String>,
}

const PROGRESS_CAP: usize = 16;

fn progress_registry() -> &'static Mutex<VecDeque<ConvertProgress>> {
    static REG: OnceLock<Mutex<VecDeque<ConvertProgress>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 登记一次转换，返回句柄（注册表下标）。`total` 为 0 表示未知。
pub fn progress_begin(jar: &Path, digest: &str, total: u32) -> usize {
    let mut reg = progress_registry().lock().unwrap();
    if reg.len() >= PROGRESS_CAP {
        reg.pop_front();
    }
    reg.push_back(ConvertProgress {
        key: digest.to_string(),
        jar: jar.to_string_lossy().into_owned(),
        bytes: std::fs::metadata(jar).map(|m| m.len()).unwrap_or(0),
        total_classes: total,
        classes_done: 0,
        started_ms: now_ms(),
        elapsed_ms: 0,
        done: false,
        ok: false,
        error: None,
    });
    reg.len() - 1
}

fn progress_mut<R>(idx: usize, f: impl FnOnce(&mut ConvertProgress) -> R) -> Option<R> {
    let mut reg = progress_registry().lock().unwrap();
    reg.get_mut(idx).map(f)
}

/// 解析子进程 stderr 里的进度行。
pub fn progress_line(idx: usize, line: &str) {
    if let Some(rest) = line.split("已写出 ").nth(1) {
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num.parse::<u32>() {
            progress_mut(idx, |p| p.classes_done = p.classes_done.max(n));
        }
    }
    if let Some(rest) = line.split("CLASSES=").nth(1) {
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num.parse::<u32>() {
            progress_mut(idx, |p| {
                p.classes_done = n;
                if p.total_classes == 0 {
                    p.total_classes = n;
                }
            });
        }
    }
}

/// 结束一次转换（成功/失败都必须调用）。
pub fn progress_finish(idx: usize, ok: bool, error: Option<String>) {
    progress_mut(idx, |p| {
        p.done = true;
        p.ok = ok;
        p.error = error;
        p.elapsed_ms = now_ms().saturating_sub(p.started_ms);
    });
}

/// 最近若干次转换（最新在前）。
pub fn convert_progress() -> Vec<ConvertProgress> {
    let reg = progress_registry().lock().unwrap();
    let now = now_ms();
    let mut out: Vec<ConvertProgress> = reg
        .iter()
        .map(|p| {
            let mut p = p.clone();
            if !p.done {
                p.elapsed_ms = now.saturating_sub(p.started_ms);
            }
            p
        })
        .collect();
    out.reverse();
    out
}

/// 正在转换的任务数（设置页用它决定要不要短轮询）。
pub fn active_conversions() -> usize {
    progress_registry()
        .lock()
        .unwrap()
        .iter()
        .filter(|p| !p.done)
        .count()
}

fn dex_count_bytes(head: &[u8]) -> Option<u32> {
    if head.len() < 100 || &head[0..4] != b"dex\n" {
        return None;
    }
    Some(u32::from_le_bytes([head[96], head[97], head[98], head[99]]))
}

/// 估计一个 dex/伪装 jar 里的类总数（进度分母）；拿不到返回 None。
pub fn class_total(path: &Path) -> Option<u32> {
    let list = entries(path).ok()?;
    let dex = list
        .iter()
        .find(|e| e.name.to_ascii_lowercase().ends_with(".dex"))?;
    let mut f = std::fs::File::open(path).ok()?;
    if dex.method == 0 {
        f.seek(SeekFrom::Start((dex.data_off + 96) as u64)).ok()?;
        let mut four = [0u8; 4];
        f.read_exact(&mut four).ok()?;
        return Some(u32::from_le_bytes(four));
    }
    f.seek(SeekFrom::Start(dex.data_off as u64)).ok()?;
    let mut raw = vec![0u8; dex.comp_size];
    f.read_exact(&mut raw).ok()?;
    let mut head = Vec::new();
    flate2::read::DeflateDecoder::new(&raw[..])
        .take(112)
        .read_to_end(&mut head)
        .ok()?;
    dex_count_bytes(&head)
}

/// 解压一个 zip 到目录（用于 JRE 自愈等可信输入；拒绝路径逃逸）。
pub fn extract_zip(zip: &Path, dest: &Path) -> Result<usize> {
    let list = entries(zip)?;
    std::fs::create_dir_all(dest)?;
    let mut src = std::fs::File::open(zip)?;
    let mut written = 0usize;
    for e in list {
        if e.name.ends_with('/') || e.name.ends_with('\\') {
            continue;
        }
        let rel = e.name.replace('\\', "/");
        if rel.starts_with('/') || rel.split('/').any(|seg| seg == "..") {
            continue;
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        src.seek(SeekFrom::Start(e.data_off as u64))?;
        let mut raw = vec![0u8; e.comp_size];
        src.read_exact(&mut raw)?;
        let data = if e.method == 0 {
            raw
        } else {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(&raw[..]).read_to_end(&mut out)?;
            out
        };
        std::fs::write(&out, &data)?;
        written += 1;
    }
    Ok(written)
}

/// 数一个 jar 里的 `.class` 条目。
fn class_count(path: &Path) -> Result<usize> {
    let names = entry_names(path)?;
    Ok(names.iter().filter(|n| n.ends_with(".class")).count())
}

/// 规范化路径并去掉 Windows verbatim 前缀（`\\?\`）——
/// java.exe 的 classpath 通配符不认 verbatim 前缀，`-cp '\\\\?\C:\...\*'` 会直接找不到主类。
fn plain_path(path: &Path) -> PathBuf {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canon.to_string_lossy().into_owned();
    let trimmed = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(|rest| rest.to_string()))
        .unwrap_or(text);
    PathBuf::from(trimmed)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 自己拼一个最小 zip（本地头 + 中央目录 + EOCD，stored 不压缩），够测解析与判型。
    fn tiny_zip(items: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data) in items {
            let local_off = out.len() as u32;
            let mut h = vec![0u8; 30];
            h[0..4].copy_from_slice(b"PK\x03\x04");
            h[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
            h[18..22].copy_from_slice(&(data.len() as u32).to_le_bytes());
            h[22..26].copy_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            let mut e = vec![0u8; 46];
            e[0..4].copy_from_slice(b"PK\x01\x02");
            e[20..24].copy_from_slice(&(data.len() as u32).to_le_bytes());
            e[24..28].copy_from_slice(&(data.len() as u32).to_le_bytes());
            e[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            e[42..46].copy_from_slice(&local_off.to_le_bytes());
            e.extend_from_slice(name.as_bytes());
            cd.extend_from_slice(&e);
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        let mut eocd = vec![0u8; 22];
        eocd[0..4].copy_from_slice(b"PK\x05\x06");
        eocd[8..10].copy_from_slice(&(items.len() as u16).to_le_bytes());
        eocd[10..12].copy_from_slice(&(items.len() as u16).to_le_bytes());
        eocd[12..16].copy_from_slice(&(cd.len() as u32).to_le_bytes());
        eocd[16..20].copy_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&eocd);
        out
    }

    #[test]
    fn reads_entry_names() {
        // 测试目录放 workspace 的 target/tmp：Windows 上系统临时目录在受限进程里可能不可写。
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp/vbox_jvm")
            .join(format!("zip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dex = dir.join("a.zip");
        std::fs::write(
            &dex,
            tiny_zip(&[("classes.dex", b"dex\n035\0payload"), ("assets/x.so", b"\x7fELF")]),
        )
        .unwrap();
        let names = entry_names(&dex).unwrap();
        assert_eq!(names, vec!["classes.dex", "assets/x.so"]);
        assert!(needs_conversion(&dex).unwrap());

        // 有 .class 的普通 jar 不转；把 dex 换成非 dex 魔数也不转。
        let jar = dir.join("b.jar");
        std::fs::write(
            &jar,
            tiny_zip(&[("classes.dex", b"dex\n035\0x"), ("com/A.class", b"\xca\xfe")]),
        )
        .unwrap();
        assert!(!needs_conversion(&jar).unwrap());
        let fake = dir.join("c.dat");
        std::fs::write(&fake, tiny_zip(&[("classes.dex", b"not-a-dex!!")])).unwrap();
        assert!(!needs_conversion(&fake).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }
}
