//! 常驻宿主进程（`SpiderHost --serve`）。
//!
//! 协议（每行一个 JSON）：
//! ```text
//! → {"id":"7","className":"com.foo.Spider","method":"homeContent","args":["<ext>"]}
//! ← {"id":"7","ok":true,"data":"{\"class\":[...]}","pp":45231}
//! ```
//!
//! 约定：蜘蛛 jar 走**命令行参数**（`--serve <jar1;jar2>`），不进 classpath —— 宿主内部用
//! `URLClassLoader` 隔离加载；`args[0]` 固定是 `ext`（`init(Context, ext)` 消费），真实参数从
//! `args[1]` 开始。服务端是 24 线程池 + 实例池，但 Rust 侧本 crate 只做**串行**调用：
//! 阻塞 API 的并发度由上层按站点粒度开多个宿主进程来拿，避免一个 JVM 内互相干扰。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::errors::{JvmError, Result};
use crate::proto::Envelope;
use crate::runtime::{default_jvm_args, Runtime};

/// 拉起宿主的参数。
#[derive(Debug, Clone)]
pub struct HostOptions {
    pub jars: Vec<PathBuf>,
    /// shim（加固壳影子 jar / 解密载荷目录），按顺序排 classpath 最前。
    pub shims: Vec<PathBuf>,
    pub heap_mb: u32,
    pub extra_jvm_args: Vec<String>,
    /// 单次调用超时（蜘蛛在做网络 IO，给足）。
    pub call_timeout: Duration,
    /// 等 `setupEnv` 完成（类加载 + Init/App 初始化）的上限。
    pub start_timeout: Duration,
    /// dex→jar 转换参数（None 时用默认缓存目录 + 5 分钟超时）。
    pub dex: Option<crate::DexOptions>,
    /// 宿主进程的 CWD（蜘蛛会往里写 cookie/缓存）；`None` = `<dex 缓存>/work`。
    pub work_dir: Option<PathBuf>,
}

impl HostOptions {
    pub fn new(jars: Vec<PathBuf>) -> Self {
        HostOptions {
            jars,
            shims: Vec::new(),
            heap_mb: crate::runtime::DEFAULT_HEAP_MB,
            extra_jvm_args: Vec::new(),
            call_timeout: Duration::from_secs(30),
            start_timeout: Duration::from_secs(15),
            dex: None,
            work_dir: None,
        }
    }

    /// 追加 classpath 最前的 shim / 载荷目录。
    pub fn with_shims(mut self, shims: Vec<PathBuf>) -> Self {
        self.shims = shims;
        self
    }

    /// 指定 dex 转换缓存目录（测试/多实例隔离用）。
    pub fn with_dex_cache(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dex = Some(crate::DexOptions::new(dir));
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

}

/// 一次方法调用。`args[0]` 是 ext（`init` 消费），真实参数从下标 1 开始。
#[derive(Debug, Clone)]
pub struct Call {
    pub class: String,
    pub method: String,
    pub args: Vec<String>,
}

impl Call {
    pub fn new(class: impl Into<String>, method: impl Into<String>, args: Vec<String>) -> Self {
        Call { class: class.into(), method: method.into(), args }
    }

    pub fn home_content(class: impl Into<String>, ext: &str) -> Self {
        Call::new(class, "homeContent", vec![ext.to_string()])
    }

    pub fn category_content(
        class: impl Into<String>,
        ext: &str,
        filter: &str,
        page: &str,
        extend: &str,
    ) -> Self {
        Call::new(
            class,
            "categoryContent",
            vec![ext.to_string(), filter.to_string(), page.to_string(), extend.to_string()],
        )
    }

    pub fn detail_content(class: impl Into<String>, ext: &str, ids: &[String]) -> Self {
        Call::new(class, "detailContent", vec![ext.to_string(), ids.join(",")])
    }

    pub fn search_content(class: impl Into<String>, ext: &str, keyword: &str, quick: &str) -> Self {
        Call::new(class, "searchContent", vec![ext.to_string(), keyword.to_string(), quick.to_string()])
    }

    pub fn player_content(
        class: impl Into<String>,
        ext: &str,
        flag: &str,
        id: &str,
        vip_flags: &[String],
    ) -> Self {
        // dispatchMethod 里 r[0]=vodId、r[1]=flag（见 SpiderHost.dispatchMethod）。
        Call::new(
            class,
            "playerContent",
            vec![ext.to_string(), id.to_string(), flag.to_string(), vip_flags.join(",")],
        )
    }
}

/// 一次调用的结果。
#[derive(Debug, Clone)]
pub struct Reply {
    pub ok: bool,
    /// 成功时是蜘蛛返回的 catvod JSON 文本；失败时是 `异常类: 消息`。
    pub data: String,
    /// 宿主内代理端口（`-Dtvbox.proxy.port` 生效或自绑成功时才有）。
    pub pp: Option<u16>,
    pub elapsed: Duration,
    /// 宿主给出的稳定错误码（成功时为 `None`）。
    pub code: Option<String>,
}

impl Reply {
    pub fn data_json(&self) -> Result<serde_json::Value> {
        if !self.ok {
            return Err(JvmError::Spider {
                code: self
                    .code
                    .clone()
                    .unwrap_or_else(|| "spider_threw".into()),
                detail: self.data.clone(),
            });
        }
        if self.data.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        Ok(serde_json::from_str(&self.data)?)
    }
}

/// 常驻宿主：一个 JVM 对应一个 jar 集合。
pub struct HostProcess {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// 本次宿主 stderr 的落盘文件（`<cache>/logs/host-*.log`）；创建失败则为 None。
    log_path: Option<PathBuf>,
    next_id: u64,
    dead: bool,
    pp: Option<u16>,
    /// 宿主工作目录：加固壳首次解密后 Java 侧把内层载荷导出到这里。
    work: PathBuf,
    /// jar 内容 sha256（payloads/<sha>/ 的键；空表示取不到，不做自动安装）。
    jar_sha: String,
    /// dex/payload 缓存根目录（安装载荷用）。
    cache_dir: PathBuf,
    /// 本宿主是否已启用 shell-shim（启用时不会再产出内层载荷）。
    shim_enabled: bool,
}

/// 把 jar 列表拼成宿主 `--serve` 的单个参数（`;` 分隔）。
/// 选一个真的能写的宿主工作目录：缓存目录 → 系统临时目录 → 当前目录下的 `.vbox-jvm-work`。
///
/// Windows 上受限进程（沙箱/安全软件）可能拒绝在 `%LOCALAPPDATA%` 建目录，
/// 这里逐个尝试而不是直接失败。
fn pick_work_dir(preferred: &std::path::Path) -> std::io::Result<(PathBuf, PathBuf)> {
    // 「父目录能建」不代表「子目录能建」：安全软件/沙箱会按路径白名单放行。
    // 直接用 `spider/` 子目录能否建出来当判据，失败就换下一个候选。
    let mut candidates: Vec<PathBuf> = vec![preferred.to_path_buf()];
    if let Some(dir) = std::env::var_os("VBOX_JVM_WORK") {
        candidates.push(PathBuf::from(dir));
    }
    // 开发态：`cargo test` 的进程树常常只能写 workspace（沙箱把 %LOCALAPPDATA%/TEMP
    // 一律判否），所以把仓库 target/tmp 也列进候选。
    if let Some(manifest) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(
            PathBuf::from(manifest)
                .join("..")
                .join("..")
                .join("target")
                .join("tmp")
                .join("vbox-jvm-work"),
        );
    }
    candidates.push(std::env::temp_dir().join("vbox-jvm-work"));
    candidates.push(PathBuf::from(".vbox-jvm-work"));
    let mut last: Option<std::io::Error> = None;
    for dir in candidates {
        let spider = dir.join("spider");
        match std::fs::create_dir_all(&spider) {
            Ok(()) => return Ok((dir, spider)),
            Err(err) => last = Some(err),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "没有可写的宿主工作目录")
    }))
}

/// Windows `canonicalize` 会带 `\\?\` verbatim 前缀；宿主 `--serve` 拿到这种路径时
/// URLClassLoader 解不开（表现为 ClassNotFoundException），这里统一剥掉。
fn host_plain_path(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("\\\\?\\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

fn jars_arg_for(jars: &[PathBuf]) -> String {
    jars.iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(";")
}

impl HostProcess {
    /// 拉起宿主并等 `__ping__` 通过（`setupEnv` 已完成）。
    pub fn spawn(rt: &Runtime, opts: &HostOptions) -> Result<HostProcess> {
        if opts.jars.is_empty() {
            return Err(JvmError::Protocol { line: "HostOptions.jars 为空".into() });
        }
        rt.validate()?;
        // dex 样本先转成 class（按 sha256 缓存，二次进源零等待）。
        let dex_opts = match &opts.dex {
            Some(o) => o.clone(),
            None => crate::DexOptions::new(crate::DexOptions::default_cache()),
        };
        let mut jars = Vec::with_capacity(opts.jars.len());
        for jar in &opts.jars {
            // 相对路径必须先绝对化：宿主的 CWD 是 work 目录，`--serve` 收到相对路径会解析错，
            // 表现为 ClassNotFoundException（类其实在 jar 里）。
            let abs = host_plain_path(std::fs::canonicalize(jar).unwrap_or_else(|_| jar.clone()));
            jars.push(crate::ensure_loadable(rt, &abs, &dex_opts)?);
        }
        // 内层载荷导出/自动安装需要：jar 内容 sha256（`payloads/<sha>/` 的键）+ 缓存目录。
        let jar_sha = crate::sha256_file(&opts.jars[0]).unwrap_or_default();
        // 兼容性遥测（L1，本地 spool）：每只 jar 只记录一次「是什么家族/什么结构」，
        // 用于后续按家族排适配优先级。默认不上传，见 `crate::telemetry`。
        if !jar_sha.is_empty() {
            if let Ok(fp) = crate::shell_fingerprint(&opts.jars[0]) {
                let _ = crate::telemetry::note_jar_once(&opts.jars[0], &jar_sha, &fp);
            }
        }
        // B4：shell-shim（纯 Java 影子 DexNative）。加固壳先用原生桥解出内层 dex（一次性，
        // `vbox_cli jvm-shell --jar … --payload …` 安装到 payloads/<sha>/），之后同一只 jar 的宿主
        // 自动改走「影子 DexNative + 载荷 jar」的纯 Java 快路径，不再启动 unidbg。
        // 关闭方式：VBOX_SHELL_SHIM=0。
        let shim_off = std::env::var("VBOX_SHELL_SHIM")
            .map(|v| v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
            .unwrap_or(false);
        let mut shim_classes: Option<String> = std::env::var("VBOX_SHELL_SHIM_CLASSES")
            .ok()
            .filter(|v| !v.trim().is_empty());
        if shim_classes.is_none() && !shim_off {
            // 自动发现：`<cache>/payloads/<jar_sha>/inner.jar` 存在就用它（见 shell::install_payload）。
            if !jar_sha.is_empty() {
                if let Some(payload) = crate::shell::resolve(&dex_opts.cache_dir, &jar_sha, None) {
                    if let Some(first) = payload.paths.into_iter().next() {
                        shim_classes = Some(first.to_string_lossy().into_owned());
                    }
                }
            }
        }
        let shim_enabled = !shim_off && shim_classes.is_some() && rt.shell_shim_jar().is_file();
        let mut shim_paths: Vec<PathBuf> = Vec::new();
        if shim_enabled {
            // 影子类必须排在 stubs.jar 之前（类加载顺序覆盖壳里的 native DexNative）。
            shim_paths.push(rt.shell_shim_jar());
        }
        shim_paths.extend(opts.shims.iter().cloned());
        let classpath = rt.classpath_with_shims(shim_paths.iter().map(|p| p.as_path()).collect())?;
        let mut args = default_jvm_args(opts.heap_mb);
        args.extend(opts.extra_jvm_args.iter().cloned());
        args.push("-cp".into());
        args.push(classpath);
        args.push("SpiderHost".into());
        args.push("--serve".into());
        args.push(jars_arg_for(&jars));

        // 蜘蛛（Push/PikPak 一类）会往 CWD 和 Context 目录写 cookie/缓存；给每个运行时
        // 一个固定工作目录，既不污染安装目录，也避免只读目录在 init 阶段直接失败。
        let (work, spider_cache) = match opts.work_dir.clone() {
            Some(dir) => {
                let spider = dir.join("spider");
                std::fs::create_dir_all(&spider).map_err(|e| {
                    JvmError::Io(std::io::Error::new(
                        e.kind(),
                        format!("创建宿主工作目录失败 {}：{e}", spider.display()),
                    ))
                })?;
                (dir, spider)
            }
            None => pick_work_dir(&dex_opts.cache_dir.join("work")).map_err(|e| {
                JvmError::Io(std::io::Error::new(
                    e.kind(),
                    format!(
                        "没有可写的宿主工作目录（候选起点 {}）：{e}",
                        dex_opts.cache_dir.join("work").display()
                    ),
                ))
            })?,
        };
        args.insert(
            0,
            format!("-Dtvbox.spiderCacheDir={}", spider_cache.to_string_lossy()),
        );
        // 原生桥（加固壳）：载荷改写产物落 dex 缓存，桥工作目录独立；显式钉 unicorn2
        // 后端（dynarmic jar 不随包分发，缺类时桥会自己回退，但先钉住省一次异常探测）。
        args.insert(
            0,
            format!("-Dtvbox.d2j.cache={}", work.join("runtime-dex").to_string_lossy()),
        );
        // 壳内的 dalvik.system.BaseDexClassLoader 需要「自带 dex2jar」才能转换解密出来的裸 dex。
        if let Some(java) = rt.java_bin().to_str() {
            args.insert(0, format!("-Dtvbox.d2j.java={java}"));
        }
        let d2j: Vec<String> = rt
            .dex2jar_dir()
            .read_dir()
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|x| x == "jar").unwrap_or(false))
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if !d2j.is_empty() {
            args.insert(0, format!("-Dtvbox.d2j.cp={}", d2j.join(";")));
        }
        if shim_enabled {
            if let Some(classes) = shim_classes.as_deref() {
                args.insert(0, format!("-Dtvbox.shellShimClasses={classes}"));
            }
            // 告诉宿主：影子 DexNative 已接管，别再启 unidbg 桥（否则会继续装载加密 .so 并崩宿主）。
            args.insert(0, "-Dtvbox.shellShim.enabled=true".to_string());
            // 壳内层代码会 System.exit(0)（防调试/配置为空路径）；常驻宿主必须拦下来。
            args.insert(0, "-Dtvbox.blockExit=true".to_string());
            args.insert(0, "-Djava.security.manager=allow".to_string());
            if std::env::var("VBOX_SHELL_SHIM_DEBUG").map(|v| v == "1").unwrap_or(false) {
                args.insert(0, "-Dtvbox.shellShim.debug=true".to_string());
            }
        }
        // unidbg 通过 scijava nativelib 落地 native 库；受限环境下默认 java.io.tmpdir 不可写。
        let _ = std::fs::create_dir_all(work.join("tmp"));
        args.insert(
            0,
            format!("-Djava.io.tmpdir={}", work.join("tmp").to_string_lossy()),
        );
        args.insert(
            0,
            format!("-Dtvbox.native.work={}", work.join("native").to_string_lossy()),
        );
        // 加固壳 jar 内自带 ARM .so 时，SpiderHost 走「静态改写」路径（nativeJarOf →
        // prepareNativeBridge）；这条路径要求 tvbox.native.patch 指向可写目录，
        // 否则改写产物不生成、classpath 里没有覆盖目录 → 仍走原始 DexNative（加载 ARM .so 失败）。
        args.insert(
            0,
            format!("-Dtvbox.native.patch={}", work.join("native-patch").to_string_lossy()),
        );
        // 加固壳首次解密成功后：Java 侧（BaseDexClassLoader）把内层 jar 导出到
        // `<work>/inner-payload.jar`，Rust 侧 `harvest_payload()` 自动安装成
        // `payloads/<sha>/inner.jar`；此后同一只 jar 的宿主改走纯 Java shim，不再重复解密。
        if !shim_enabled {
            args.insert(
                0,
                format!(
                    "-Dtvbox.payload.out={}",
                    work.join("inner-payload.jar").to_string_lossy()
                ),
            );
        }
        // 模拟后端：默认 dynarmic（JIT，随包分发 unidbg-dynarmic.jar）；架包/平台不支持时桥内自动回退 Unicorn2。
        // 可用 `VBOX_NATIVE_BACKEND=unicorn2` 强制旧后端排查。
        let backend = std::env::var("VBOX_NATIVE_BACKEND")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "dynarmic".to_string());
        args.insert(0, format!("-Dtvbox.native.backend={backend}"));
        if std::env::var("VBOX_JVM_DEBUG_CMD").map(|v| v == "1").unwrap_or(false) {
            eprintln!("[jvm-debug] cwd={} java={} args={:?}", work.display(), rt.java_bin().display(), args);
        }
        let java = rt.java_runtime()?.java;
        let java_display = java.display().to_string();
        let mut child = Command::new(&java)
            .args(&args)
            .current_dir(&work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                JvmError::Io(std::io::Error::new(
                    e.kind(),
                    format!(
                        "拉起 {} 失败（cwd={}）：{e}",
                        java_display,
                        work.display()
                    ),
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| JvmError::Protocol { line: "stdin 未捕获".into() })?;
        let stdout = child.stdout.take().ok_or_else(|| JvmError::Protocol { line: "stdout 未捕获".into() })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| JvmError::Protocol { line: "stderr 未捕获".into() })?;

        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let stderr_writer = Arc::clone(&stderr);
        // stderr 落盘（计划 §7 C 项）：蜘蛛的 `Log.e`、壳的调试输出都在 stderr，
        // 崩溃后只有 tail 不够定位；这里按「启动时刻 + jar 指纹」落一个文件，
        // 并只保留最近 20 个，避免长期使用把缓存目录撑爆。
        let log_dir = opts
            .dex
            .clone()
            .map(|d| d.cache_dir)
            .unwrap_or_else(crate::dex::DexOptions::default_cache)
            .join("logs");
        let log_path = prepare_host_log(&log_dir, &opts.jars);
        let log_writer = log_path.as_ref().and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
                .map(|f| Arc::new(Mutex::new(f)))
        });
        std::thread::spawn(move || {
            use std::io::Write;
            for line in BufReader::new(stderr_pipe).lines().map_while(|l| l.ok()) {
                if let Some(file) = &log_writer {
                    let mut f = file.lock().unwrap();
                    let _ = writeln!(f, "{line}");
                    let _ = f.flush();
                }
                let mut buf = stderr_writer.lock().unwrap();
                if buf.len() >= 400 {
                    buf.pop_front();
                }
                buf.push_back(line);
            }
        });

        let mut host = HostProcess {
            child,
            stdin,
            rx,
            stderr,
            log_path,
            next_id: 1,
            dead: false,
            pp: None,
            work: work.clone(),
            jar_sha,
            cache_dir: dex_opts.cache_dir.clone(),
            shim_enabled,
        };
        host.handshake(opts.start_timeout)?;
        Ok(host)
    }

    /// `__ping__`：确认 `setupEnv` 完成（类加载器等已就绪）。
    pub fn handshake(&mut self, timeout: Duration) -> Result<()> {
        let id = self.next_id_string();
        let req = serde_json::json!({"id": id, "className": "", "method": "__ping__", "args": []});
        let env = self.request(&id, &req.to_string(), timeout)?;
        if !env.ok {
            return Err(JvmError::Protocol { line: format!("__ping__ 失败：{}", env.data) });
        }
        if let Some(pp) = env.pp {
            self.pp = Some(pp);
        }
        Ok(())
    }

    /// 预热：加载蜘蛛类并预建实例（`args[0]` = ext）。
    pub fn warm(&mut self, class: &str, ext: &str, timeout: Duration) -> Result<()> {
        let id = self.next_id_string();
        let req = serde_json::json!({"id": id, "className": class, "method": "__warm__", "args": [ext]});
        let env = self.request(&id, &req.to_string(), timeout)?;
        if !env.ok {
            return Err(JvmError::Spider {
                code: env
                    .err
                    .as_ref()
                    .map(|e| e.code.clone())
                    .unwrap_or_else(|| "spider_threw".into()),
                detail: env.data,
            });
        }
        Ok(())
    }

    pub fn call(&mut self, call: &Call, timeout: Duration) -> Result<Reply> {
        let started = Instant::now();
        let id = self.next_id_string();
        let req = serde_json::json!({
            "id": id,
            "className": call.class,
            "method": call.method,
            "args": call.args,
        });
        let env = self.request(&id, &req.to_string(), timeout)?;
        let elapsed = started.elapsed();
        if let Some(pp) = env.pp {
            self.pp = Some(pp);
        }
        let code = env.err.as_ref().map(|e| e.code.clone());
        Ok(Reply {
            ok: env.ok,
            data: env.data,
            pp: env.pp,
            elapsed,
            code,
        })
    }

    pub fn pp(&self) -> Option<u16> {
        self.pp
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// 本次宿主的 stderr 落盘路径（诊断用）。
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// 加固壳首次解密成功后，把 Java 侧导出的内层载荷自动安装成 payload。
    ///
    /// 返回安装后的 `payloads/<jar sha>/inner.jar` 路径；没有可导出产物（非壳 / 已启用 shim /
    /// 还没解密）时返回 `None`。只在走原生桥（`shim_enabled == false`）的宿主上生效。
    pub fn harvest_payload(&mut self) -> Option<PathBuf> {
        if self.shim_enabled || self.jar_sha.is_empty() {
            return None;
        }
        let src = self.work.join("inner-payload.jar");
        let meta = std::fs::metadata(&src).ok()?;
        if !meta.is_file() || meta.len() < 1024 {
            return None;
        }
        match crate::shell::install_payload(&self.cache_dir, &self.jar_sha, &src) {
            Ok(inner) => {
                eprintln!(
                    "[payload] 已自动安装内层载荷（{} B）→ {}；下次进源走纯 Java shim",
                    meta.len(),
                    inner.display()
                );
                Some(inner)
            }
            Err(err) => {
                eprintln!("[payload] 自动安装载荷失败: {err}");
                None
            }
        }
    }

    /// 最近 N 行 stderr（诊断用）。
    pub fn stderr_tail(&self, n: usize) -> Vec<String> {
        let buf = self.stderr.lock().unwrap();
        buf.iter().rev().take(n).rev().cloned().collect()
    }

    fn next_id_string(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        id.to_string()
    }

    /// 发一行请求，等 `id` 匹配的信封；日志泄漏行会被跳过。
    fn request(&mut self, id: &str, line: &str, timeout: Duration) -> Result<Envelope> {
        if self.dead {
            return Err(JvmError::Protocol { line: "宿主已标记死亡".into() });
        }
        if let Err(err) = writeln!(self.stdin, "{line}").and_then(|_| self.stdin.flush()) {
            self.dead = true;
            return Err(JvmError::Exit { code: -1, stderr: format!("stdin 写入失败：{err}") });
        }
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                self.dead = true;
                return Err(JvmError::Timeout {
                    ms: timeout.as_millis() as u64,
                    stderr: self.stderr_tail(20).join("\n"),
                });
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(line) => {
                    let Some(env) = crate::proto::parse_envelope(&line) else {
                        continue;
                    };
                    if env.id.as_deref() == Some(id) {
                        return Ok(env);
                    }
                }
                Err(_) => {
                    // 通道断开：进程已退出或 stdout 关了。
                    let status = self.child.try_wait().ok().flatten().and_then(|s| s.code());
                    self.dead = true;
                    return Err(JvmError::Exit {
                        code: status.unwrap_or(-1),
                        stderr: self.stderr_tail(20).join("\n"),
                    });
                }
            }
        }
    }

    /// 让宿主退出（可重复调用；`self` 仍保留，供池化句柄使用）。
    pub fn quit(&mut self, grace: Duration) {
        if self.dead {
            return;
        }
        self.dead = true;
        let _ = self.stdin.write_all(b"quit\n");
        let _ = self.stdin.flush();
        let deadline = Instant::now() + grace;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return;
                }
            }
        }
    }

    pub fn shutdown(mut self, grace: Duration) {
        self.quit(grace);
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if !self.dead {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Runtime;

    fn ext_of(class: &str) -> String {
        format!("{{\"class\":\"{class}\"}}")
    }

    /// 需要真实运行时 + 真实 jar；用环境变量给样本，缺就跳过。
    #[test]
    fn serve_home_content_against_sample_jar() {
        let Ok(jar) = std::env::var("VBOX_JVM_SAMPLE_JAR") else {
            eprintln!("跳过：未设置 VBOX_JVM_SAMPLE_JAR");
            return;
        };
        let Ok(class) = std::env::var("VBOX_JVM_SAMPLE_CLASS") else {
            eprintln!("跳过：未设置 VBOX_JVM_SAMPLE_CLASS");
            return;
        };
        let Some(rt) = Runtime::locate() else {
            eprintln!("跳过：没有找到 JVM 运行时");
            return;
        };
        let opts = HostOptions::new(vec![PathBuf::from(jar)]).with_timeout(Duration::from_secs(20));
        let mut host = HostProcess::spawn(&rt, &opts).expect("拉起宿主失败");
        let reply = host
            .call(&Call::home_content(class, &ext_of("")), Duration::from_secs(20))
            .expect("调用失败");
        assert!(reply.ok, "homeContent 失败：{}；stderr：{:?}", reply.data, host.stderr_tail(10));
        assert!(reply.data_json().is_ok(), "返回不是 JSON：{}", reply.data);
        host.shutdown(Duration::from_secs(5));
    }
}

impl crate::pool::HostItem for HostProcess {
    fn is_dead(&self) -> bool {
        HostProcess::is_dead(self)
    }

    fn shutdown(&mut self) {
        let _ = self.quit(Duration::from_secs(2));
    }
}

/// 为一次宿主启动准备 stderr 日志文件：`<dir>/host-<epoch_ms>-<jar sha 前 8 位>.log`。
///
/// 同时做保留策略：只看 `host-*.log`，按文件名（含时间戳）排序后删掉最旧的，
/// 只留 `KEEP_HOST_LOGS` 个。目录建不出来就返回 `None`（日志落盘是诊断增强，
/// 不能在沙箱/只读环境下把宿主拖死）。
fn prepare_host_log(dir: &Path, jars: &[PathBuf]) -> Option<PathBuf> {
    const KEEP_HOST_LOGS: usize = 20;
    std::fs::create_dir_all(dir).ok()?;
    let sha = jars
        .first()
        .and_then(|j| crate::dex::sha256_file(j).ok())
        .unwrap_or_default();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let short = sha.chars().take(8).collect::<String>();
    let path = dir.join(format!("host-{stamp}-{short}.log"));
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut logs: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("host-") && n.ends_with(".log"))
                    .unwrap_or(false)
            })
            .collect();
        logs.sort();
        while logs.len() >= KEEP_HOST_LOGS {
            let oldest = logs.remove(0);
            let _ = std::fs::remove_file(oldest);
        }
    }
    Some(path)
}
