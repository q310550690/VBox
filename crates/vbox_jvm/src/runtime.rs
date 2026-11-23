use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::errors::{JvmError, Result};
use crate::proto::{parse_probe, Probe};
use crate::{HOST_JAR, JRE_DIR, JRE_MARKER, LIBS_DIR, SHIMS_MANIFEST, STUBS_JAR};

/// classpath 分隔符：Windows 用 `;`，其余 `:`。
const PATH_SEP: char = if cfg!(windows) { ';' } else { ':' };

/// 默认探测超时。冷启动目标 < 2.5 s，探针给足余量但不允许无限等。
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// 默认堆上限（与参考实现一致：蜘蛛是 IO 密集，256 MB 足够且省内存）。
pub const DEFAULT_HEAP_MB: u32 = 256;

/// 拉起宿主时固定携带的 JVM 参数（顺序稳定，便于测试断言）。
pub fn default_jvm_args(heap_mb: u32) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-noverify".into(),
        format!("-Xmx{heap_mb}m"),
        "-XX:+UseSerialGC".into(),
        "-XX:TieredStopAtLevel=1".into(),
        "-Dfile.encoding=UTF-8".into(),
        "-Dsun.stdout.encoding=UTF-8".into(),
        "-Dsun.stderr.encoding=UTF-8".into(),
    ];
    // 蜘蛛普遍用反射摸 JDK 内部（尤其网盘签名、时间格式化）；17 起必须显式 open。
    for pkg in [
        "java.base/java.lang",
        "java.base/java.lang.reflect",
        "java.base/java.io",
        "java.base/java.net",
        "java.base/java.nio",
        "java.base/java.util",
        "java.base/java.util.concurrent",
        "java.base/java.util.regex",
        "java.base/java.text",
        "java.base/java.time",
        // unidbg 会用反射调用 JDK 内部的 JarURLInputStream.close()（壳从 jar/内存流读载荷时）。
        "java.base/sun.net.www.protocol.jar",
        // 少数壳用 URL/HttpURLConnection 反射拿内部字段（net 系已开，这里补 sun.net 实现包）。
        "java.base/sun.net.www.protocol.http",
        "java.base/sun.net.www.protocol.https",
        // 反射调用 NativeLibrary/NIO 内部（unidbg 的 Java 代理会碰）。
        "java.base/sun.nio.ch",
    ] {
        args.push(format!("--add-opens={pkg}=ALL-UNNAMED"));
    }
    args
}

/// `src-tauri/resources/jvm/manifest.json`：构建期产物清单（`scripts/build-jvm.mjs` 写出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeManifest {
    #[serde(default)]
    pub rev: String,
    #[serde(default)]
    pub built_at: String,
    #[serde(default)]
    pub release: u32,
    #[serde(default)]
    pub javac: String,
    #[serde(default)]
    pub stubs: JarInfo,
    #[serde(default)]
    pub host: JarInfo,
    #[serde(default)]
    pub libs: Vec<LibInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JarInfo {
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub classes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub sha256: String,
}

/// JRE 侧标记（`scripts/fetch-jre.ps1` 写出）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JreMarker {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub java_version: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub built_at: String,
}

/// 一个已 staged 的 JVM 运行时目录：`<root>/{stubs.jar,spider-host.jar,libs/,jre/,manifest.json,marker.json}`。
#[derive(Debug, Clone)]
pub struct Runtime {
    root: PathBuf,
}

impl Runtime {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Runtime { root: root.into() }
    }

    /// 按优先级寻找运行时目录：
    /// 1. `VBOX_JVM_ROOT`（测试/调试直指）；
    /// 2. 可执行文件旁：`<exe>/resources/jvm`、`<exe>/jvm`、macOS `<exe>/../Resources/jvm`；
    /// 3. 开发态：workspace 的 `src-tauri/resources/jvm`；
    /// 4. 数据目录：`VBOX_DATA_DIR/jvm`（P2 运行时外置下载的落点）。
    pub fn locate() -> Option<Runtime> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(v) = std::env::var("VBOX_JVM_ROOT") {
            if !v.trim().is_empty() {
                candidates.push(PathBuf::from(v));
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("resources").join("jvm"));
                candidates.push(dir.join("jvm"));
                if let Some(parent) = dir.parent() {
                    candidates.push(parent.join("Resources").join("jvm"));
                }
            }
        }
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("src-tauri")
                .join("resources")
                .join("jvm"),
        );
        if let Ok(data) = std::env::var("VBOX_DATA_DIR") {
            if !data.trim().is_empty() {
                candidates.push(PathBuf::from(data).join("jvm"));
            }
        }
        candidates
            .into_iter()
            .map(Runtime::at)
            .find(|rt| rt.looks_ready())
    }

    /// 目录形状是否像运行时（不校验哈希，只判断关键文件在不在）。
    /// 目录像不像一个可用的运行时：`stubs.jar` + `spider-host.jar` 必须在。
    ///
    /// 不再强制要求内置 JRE —— 计划 §7 C 项要求「JRE 缺失自愈」：
    /// 允许回退到 `VBOX_JAVA_HOME` / `JAVA_HOME` / PATH 上已有的 17+ JDK。
    pub fn looks_ready(&self) -> bool {
        self.root.join(STUBS_JAR).is_file() && self.root.join(HOST_JAR).is_file()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn java_bin(&self) -> PathBuf {
        let name = if cfg!(windows) { "java.exe" } else { "java" };
        self.root.join(JRE_DIR).join("bin").join(name)
    }

    /// 找到实际可用的 java：内置 JRE 优先；缺失时按
    /// `VBOX_JVM_JRE_ZIP`（解压自愈）→ `VBOX_JAVA_HOME` → `JAVA_HOME` → `PATH` 依次回退。
    ///
    /// 只接受主版本 ≥ 17 的 JDK/JRE —— 兼容层 class 文件版本是 61，低版本 java
    /// 会在启动时报 `UnsupportedClassVersionError`，不如在这里给出可读错误。
    pub fn java_runtime(&self) -> Result<JavaRuntime> {
        let bundled = self.java_bin();
        if bundled.is_file() {
            let version = read_java_version(&bundled);
            return Ok(JavaRuntime {
                java: bundled,
                source: "bundled".into(),
                version,
            });
        }
        // 自愈 1：给了 JRE 压缩包就直接解到运行时目录里。
        if let Ok(zip) = std::env::var("VBOX_JVM_JRE_ZIP") {
            let zip = PathBuf::from(zip.trim());
            if zip.is_file() {
                if let Some(java) = self.extract_jre(&zip)? {
                    return Ok(JavaRuntime {
                        java,
                        source: "VBOX_JVM_JRE_ZIP".into(),
                        version: None,
                    });
                }
            }
        }
        // 自愈 2：环境里已有的 17+ JDK。
        for var in ["VBOX_JAVA_HOME", "JAVA_HOME"] {
            if let Ok(root) = std::env::var(var) {
                if root.trim().is_empty() {
                    continue;
                }
                let java = PathBuf::from(root.trim()).join("bin").join(java_exe_name());
                if java.is_file() {
                    let version = read_java_version(&java);
                    if java_major(version.as_deref()).map(|m| m >= 17).unwrap_or(false) {
                        return Ok(JavaRuntime {
                            java,
                            source: var.into(),
                            version,
                        });
                    }
                }
            }
        }
        if let Some(java) = find_java_on_path() {
            let version = read_java_version(&java);
            if java_major(version.as_deref()).map(|m| m >= 17).unwrap_or(false) {
                return Ok(JavaRuntime {
                    java,
                    source: "PATH".into(),
                    version,
                });
            }
        }
        Err(JvmError::MissingRuntime { path: bundled })
    }

    /// 把 `VBOX_JVM_JRE_ZIP` 解压到**数据目录**（不是安装目录）并返回找到的 java。
    ///
    /// 解到安装目录是错的：打包安装后 `resources/jvm` 位于 Program Files，普通用户进程
    /// 没有写权限。数据目录在受限环境下也可能不可写（沙箱），此时返回 None 继续回退。
    fn extract_jre(&self, zip: &Path) -> Result<Option<PathBuf>> {
        let dest = crate::dex::DexOptions::default_cache().join("jre");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest)?;
        crate::dex::extract_zip(zip, &dest)?;
        // 常见布局：`<dest>/jre/bin/java[.exe]`、`<dest>/<某目录>/bin/java[.exe]`。
        let direct = dest.join(JRE_DIR).join("bin").join(java_exe_name());
        if direct.is_file() {
            return Ok(Some(direct));
        }
        let mut found = None;
        collect_java_bins(&dest, 0, &mut found);
        Ok(found)
    }

    /// 加固壳 ARM native 桥（可选产物；没有时宿主遇到 ARM .so 只会报 unsupported）。
    pub fn bridge_jar(&self) -> PathBuf {
        self.root.join(crate::NATIVE_BRIDGE_JAR)
    }

    /// 桥依赖的 unidbg jar（目录不存在时为空）。
    pub fn unidbg_jars(&self) -> Vec<PathBuf> {
        let dir = self.root.join(crate::UNIDBG_DIR);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut jars: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "jar").unwrap_or(false))
            .collect();
        jars.sort();
        jars
    }

    pub fn stubs_jar(&self) -> PathBuf {
        self.root.join(STUBS_JAR)
    }

    pub fn host_jar(&self) -> PathBuf {
        self.root.join(HOST_JAR)
    }

    pub fn libs_dir(&self) -> PathBuf {
        self.root.join(LIBS_DIR)
    }

    /// dex2jar 转换器所在目录（`<root>/dex2jar/*.jar`，由打包脚本 stage）。
    pub fn dex2jar_dir(&self) -> PathBuf {
        self.root.join(crate::DEX2JAR_DIR)
    }

    /// `decode2jar` 之后跑 `DexJarRepair`（修 dex2jar 对 R8 `NEW` 的误译）用的工具 jar。
    pub fn tools_jar(&self) -> PathBuf {
        self.root.join(crate::TOOLS_JAR)
    }

    /// 纯 Java 影子 `DexNative`（加固壳解密出内层 dex 后，用它在**没有 native** 的情况下
    /// 从「已转好的载荷 jar」加载真实 Spider；见 host.rs 的 `VBOX_SHELL_SHIM`）。
    pub fn shell_shim_jar(&self) -> PathBuf {
        self.root.join(crate::SHELL_SHIM_JAR)
    }

    /// `dex2jar/` 下的 jar（进程内转换 `tvbox.dex.DexStreamConverter` 的编译/运行依赖）。
    pub fn dex2jar_jars(&self) -> Vec<PathBuf> {
        let dir = self.dex2jar_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("jar"))
                    == Some(true)
            })
            .collect();
        out.sort();
        out
    }

    /// `libs/` 下的 jar，按文件名排序（顺序稳定才能复现 classpath）。
    pub fn lib_jars(&self) -> Result<Vec<PathBuf>> {
        let dir = self.libs_dir();
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&dir).map_err(|_| JvmError::MissingRuntime {
            path: dir.clone(),
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("jar")) == Some(true) {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// 拼 classpath：`[shim] ; stubs.jar ; spider-host.jar ; libs/*.jar`。
    ///
    /// shim（加固壳影子 jar）必须排最前，否则 stubs.jar 里的同名类会先被命中。
    pub fn classpath(&self, shim: Option<&Path>) -> Result<String> {
        self.classpath_with_shims(shim.into_iter().collect())
    }

    /// 多 shim 版本：按给定顺序排在最前（加固壳载荷目录在前，影子 jar 在后）。
    pub fn classpath_with_shims(&self, shims: Vec<&Path>) -> Result<String> {
        let mut parts: Vec<PathBuf> = Vec::new();
        for shim in &shims {
            // 不存在的条目由 java 自己忽略；载荷目录的存在性在 shell::resolve 里已过滤。
            parts.push(shim.to_path_buf());
        }
        parts.push(self.stubs_jar());
        parts.push(self.host_jar());
        parts.extend(self.lib_jars()?);
        // 进程内 dex 转换器（stubs.jar 里的 tvbox.dex.DexStreamConverter）需要 dex2jar + asm 才能工作；
        // 这些 jar 只在真正转换时才被加载，放在 classpath 尾部不拖慢启动。
        parts.extend(self.dex2jar_jars());
        // jvm-tools.jar：DexJarRepair / JarRewriter（转换产物收尾要反射调用 DexJarRepair 修 R8 NEW 误译）。
        let tools = self.tools_jar();
        if tools.exists() {
            parts.push(tools);
        }
        let bridge = self.bridge_jar();
        if bridge.exists() {
            parts.push(bridge);
            parts.extend(self.unidbg_jars());
        }
        Ok(parts
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(&PATH_SEP.to_string()))
    }

    pub fn manifest(&self) -> Result<RuntimeManifest> {
        read_json(&self.root.join(SHIMS_MANIFEST))
    }

    pub fn jre_marker(&self) -> Result<JreMarker> {
        read_json(&self.root.join(JRE_MARKER))
    }

    /// 校验运行时：关键文件 + 清单可读。返回清单供上层做版本展示。
    pub fn validate(&self) -> Result<RuntimeManifest> {
        // 找得到 java（内置或自愈）才算可用；找不到时由这里统一报 MissingRuntime。
        self.java_runtime()?;
        for jar in [self.stubs_jar(), self.host_jar()] {
            if !jar.is_file() {
                return Err(JvmError::MissingRuntime { path: jar });
            }
        }
        let manifest = self.manifest()?;
        if manifest.release != 0 && manifest.release != 17 {
            return Err(JvmError::BadManifest {
                path: self.root.join(SHIMS_MANIFEST),
                reason: format!("release={}，期望 17", manifest.release),
            });
        }
        Ok(manifest)
    }

    /// 跑一次 `--probe`，返回宿主自报的装配情况（阻塞，带超时）。
    pub fn probe(&self) -> Result<Probe> {
        self.probe_with(DEFAULT_PROBE_TIMEOUT)
    }

    pub fn probe_with(&self, timeout: Duration) -> Result<Probe> {
        self.validate()?;
        let classpath = self.classpath(None)?;
        let mut args = default_jvm_args(DEFAULT_HEAP_MB);
        args.push("-cp".into());
        args.push(classpath);
        args.push("SpiderHost".into());
        args.push("--probe".into());
        let java = self.java_runtime()?.java;
        let mut child = Command::new(java)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        collect_probe(&mut child, timeout)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    if !path.is_file() {
        return Err(JvmError::MissingRuntime {
            path: path.to_path_buf(),
        });
    }
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| JvmError::BadManifest {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// 读 stdout 直到拿到一行可解析的探针结果；stderr 单独收集尾部用于报错。
fn collect_probe(child: &mut Child, timeout: Duration) -> Result<Probe> {
    let stdout = child.stdout.take().ok_or_else(|| JvmError::Protocol {
        line: "stdout 未捕获".into(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| JvmError::Protocol {
        line: "stderr 未捕获".into(),
    })?;

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_tail = std::thread::spawn(move || {
        let mut tail: VecDeque<String> = VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            if tail.len() >= 200 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>()
    });

    let deadline = Instant::now() + timeout;
    let mut found: Option<Probe> = None;
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(line) => {
                if let Some(p) = parse_probe(&line) {
                    found = Some(p);
                    break;
                }
            }
            Err(_) => break,
        }
    }
    // 探针自己会退出；这里兜底杀掉，避免僵尸进程占住管道。
    let _ = child.kill();
    let status = child.wait().ok();
    let stderr_lines = stderr_tail.join().unwrap_or_default();
    let stderr_text = stderr_lines.join("\n");

    match found {
        Some(probe) => Ok(probe),
        None => {
            let code = status.and_then(|s| s.code());
            match code {
                Some(0) => Err(JvmError::Protocol {
                    line: format!("宿主正常退出但没有探针输出；stderr：{stderr_text}"),
                }),
                Some(code) => Err(JvmError::Exit {
                    code,
                    stderr: stderr_text,
                }),
                None => Err(JvmError::Timeout {
                    ms: timeout.as_millis() as u64,
                    stderr: stderr_text,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试目录放在 workspace 的 `target/tmp` 下：Windows 上临时目录可能被
    /// 沙箱/ACL 拒绝写入（`os error 5`），target 目录一定可写。
    fn scratch(tag: &str) -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/vbox_jvm");
        std::fs::create_dir_all(&base).expect("创建测试目录");
        base.join(format!("{tag}_{}", std::process::id()))
    }

    fn fake_runtime(tag: &str) -> PathBuf {
        let dir = scratch(tag);
        std::fs::create_dir_all(dir.join(JRE_DIR).join("bin")).unwrap();
        std::fs::create_dir_all(dir.join(LIBS_DIR)).unwrap();
        std::fs::write(dir.join(JRE_DIR).join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }), b"")
            .unwrap();
        std::fs::write(dir.join(STUBS_JAR), b"x").unwrap();
        std::fs::write(dir.join(HOST_JAR), b"x").unwrap();
        dir
    }

    #[test]
    fn classpath_order_is_shim_stubs_host_libs() {
        let dir = fake_runtime("cp");
        std::fs::write(dir.join(LIBS_DIR).join("b.jar"), b"").unwrap();
        std::fs::write(dir.join(LIBS_DIR).join("a.jar"), b"").unwrap();
        let rt = Runtime::at(&dir);
        let cp = rt.classpath(None).unwrap();
        let parts: Vec<String> = cp.split(PATH_SEP).map(|s| s.to_string()).collect();
        assert!(parts[0].ends_with(STUBS_JAR), "{parts:?}");
        assert!(parts[1].ends_with(HOST_JAR), "{parts:?}");
        assert!(parts[2].ends_with("a.jar"), "{parts:?}");
        assert!(parts[3].ends_with("b.jar"), "{parts:?}");

        let shim = dir.join("shell-shim.jar");
        let cp = rt.classpath(Some(&shim)).unwrap();
        assert!(cp.starts_with(&shim.to_string_lossy().into_owned()), "{cp}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_args_include_heap_and_opens() {
        let args = default_jvm_args(256);
        assert!(args.iter().any(|a| a == "-Xmx256m"));
        assert!(args.iter().any(|a| a.starts_with("--add-opens=java.base/java.lang=")));
        assert!(args.iter().any(|a| a == "-noverify"));
    }

    #[test]
    fn validate_rejects_empty_runtime() {
        let dir = scratch("bad");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let err = Runtime::at(&dir).validate().unwrap_err();
        assert!(matches!(err, JvmError::MissingRuntime { .. }), "{err:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 需要真实运行时（`VBOX_JVM_ROOT` 或 workspace 里已 staged）；没有就跳过而不是失败。
    #[test]
    fn probe_against_real_runtime() {
        let Some(rt) = Runtime::locate() else {
            eprintln!("跳过：没有找到 JVM 运行时（先跑 scripts/build-jvm.mjs 与 fetch-jre）");
            return;
        };
        let probe = rt.probe().expect("probe 失败");
        assert!(probe.is_healthy(), "缺项：{:?}", probe.missing());
    }
}

/// 一次解析出来的 java 运行时信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRuntime {
    pub java: PathBuf,
    /// `bundled` / `VBOX_JAVA_HOME` / `JAVA_HOME` / `PATH` / `VBOX_JVM_JRE_ZIP`
    pub source: String,
    pub version: Option<String>,
}

fn java_exe_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// 解析 `java -version` 输出里的版本号（它写 stderr）。
pub fn read_java_version(java: &Path) -> Option<String> {
    let out = Command::new(java)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    for line in text.lines() {
        for prefix in ["openjdk version \"", "java version \""] {
            if let Some(rest) = line.trim().strip_prefix(prefix) {
                return Some(rest.trim_end_matches('"').to_string());
            }
        }
    }
    None
}

/// 版本号里的主版本：`17.0.21` → 17，`1.8.0_402` → 8。
pub fn java_major(version: Option<&str>) -> Option<u32> {
    let v = version?;
    let mut parts = v.split(['.', '-', '+', '_']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn find_java_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(java_exe_name());
        if candidate.is_file() {
            return Some(std::fs::canonicalize(&candidate).unwrap_or(candidate));
        }
    }
    None
}

fn collect_java_bins(dir: &Path, depth: u32, found: &mut Option<PathBuf>) {
    if found.is_some() || depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_java_bins(&path, depth + 1, found);
        } else if path.file_name().and_then(|n| n.to_str()) == Some(java_exe_name()) {
            if let Some(parent) = path.parent() {
                if parent.file_name().and_then(|n| n.to_str()) == Some("bin") {
                    *found = Some(path);
                    return;
                }
            }
        }
    }
}
