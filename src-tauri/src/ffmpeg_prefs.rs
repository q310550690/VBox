//! 外部工具 FFmpeg：随应用集成 + 一键下载 + 路径配置 + 可用性检测
//!
//! 为什么需要：SAMPLE-AES 样本级解密、分离音轨合并、非 HLS（DASH/MP4/FLV）兜底保存
//! 都依赖 ffmpeg（与 mpv 同一套 libavformat）。这些能力是「能播就能存」的兜底，
//! 但不能把「自己装 ffmpeg」的成本丢给用户，所以 VBox 采用三级集成：
//!
//! 1. **随包集成**：发布包通过 `bundle.resources` 带出 `resources/ffmpeg/ffmpeg.exe`
//!    （构建前跑 `scripts/fetch-ffmpeg.ps1`，安装包开箱即用）；
//! 2. **一键下载**：安装包里没带、或旧版安装，可在设置页点「一键下载并集成」，
//!    下载固定版本的 Windows 静态构建到 `<app_data_dir>/tools/ffmpeg/`，
//!    并用内置 SHA-256 校验（开发/非 Windows 环境同样可用）；
//! 3. **兼容外部**：用户指定路径 → `VBOX_FFMPEG` → 系统 `PATH`（老用户/自建 ffmpeg 不受影响）。
//!
//! 解析顺序：用户指定 → 已下载副本 → 随包集成 → `VBOX_FFMPEG` → PATH。
//! 配置持久化在 `<app_data_dir>/ffmpeg.json`。

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};

use crate::embedded_tools::{path_text, strip_verbatim_prefix};

/// 随包集成的目录（相对 Tauri resource_dir；与 `tauri.conf.json` 的 bundle.resources 对应）
pub const BUNDLED_SUBDIR: &str = "resources/ffmpeg";

/// 固定版本的 Windows x64 静态构建（GyanD essentials，GPLv3）。
///
/// 用 GitHub Release 的**固定 tag**（而不是 gyan.dev 的滚动 URL）：tag 不变、
/// 资产不变，SHA-256 可复现；国内直连 GitHub 常常不可用，所以默认走 gh-proxy 镜像，
/// ghproxy.net 作为备用，最后才是 GitHub 直连。
///
/// 换版本时同步更新 `scripts/fetch-ffmpeg.ps1` 里的同名常量。
const DOWNLOAD_URL: &str = "https://github.com/GyanD/codexffmpeg/releases/download/2026-11-02-git-b034eb5e7e/ffmpeg-2026-11-02-git-b034eb5e7e-essentials_build.zip";
const DOWNLOAD_SHA256: &str =
    "ff646d864bd9b0fa6e0be55acaa2e1f0ad7210c309325f13853c89bf7afd79e7";
/// 压缩包大小（仅用于没有 Content-Length 时给前端一个进度分母）
const DOWNLOAD_SIZE_HINT: u64 = 117_638_417;
/// GitHub 镜像前缀（按速度排序；内容由 SHA-256 锁定，镜像不可信也不会被接受）
const DOWNLOAD_MIRRORS: &[&str] = &["https://gh-proxy.com/", "https://ghproxy.net/"];

/// 下载进度事件名（前端 `listen` 同名事件）
const PROVISION_EVENT: &str = "ffmpeg-provision";

/// 平台可执行文件名
const EXE_NAME: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

/// 持久化内容（损坏/缺失时视为「自动」）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FfmpegPrefs {
    #[serde(default)]
    path: String,
}

/// 给前端的探测结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegStatus {
    /// 用户配置的路径（空串 = 自动）
    pub path: String,
    /// 实际解析到的可执行文件（配置 / 已下载 / 随包 / 环境变量 / PATH）
    pub effective: String,
    /// 来源：configured / managed / bundled / env / path
    pub source: String,
    /// `ffmpeg -version` 是否成功
    pub available: bool,
    /// 版本首行（可用时）
    pub version: Option<String>,
    /// 失败原因（不可用时）
    pub error: Option<String>,
    /// 随包集成的副本路径（存在时；发布安装包一般都有）
    pub bundled: Option<String>,
    /// 已下载集成到本机的副本路径（存在时）
    pub managed: Option<String>,
    /// 是否正在下载集成中
    pub provisioning: bool,
    /// 当前平台是否支持一键下载集成（目前只有 Windows x64 构建）
    pub can_provision: bool,
}

/// 下载进度（`ffmpeg-provision` 事件负载）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionProgress {
    /// download / extract / verify / done
    phase: String,
    /// 已下载字节
    downloaded: u64,
    /// 总字节（未知时为 0）
    total: u64,
    /// 给用户看的一句话
    message: String,
}

/// FFmpeg 设置存储（Tauri managed state；内部 Arc 便于在后台线程持有）
#[derive(Clone)]
pub struct FfmpegPrefsStore {
    inner: Arc<Inner>,
}

struct Inner {
    file: PathBuf,
    data_dir: PathBuf,
    path: Mutex<String>,
    /// 随包集成副本（启动时探测一次）
    bundled: Option<PathBuf>,
    provisioning: AtomicBool,
}

impl FfmpegPrefsStore {
    /// 读取 `<app_data_dir>/ffmpeg.json`，探测随包副本，并把解析结果应用到全局。
    ///
    /// `resource_dir` 为 Tauri 的 `path().resource_dir()`（拿不到时传 None，
    /// 仍会尝试 exe 同目录与仓库内开发期路径）。
    pub fn new(data_dir: &Path, resource_dir: Option<PathBuf>) -> Self {
        let file = data_dir.join("ffmpeg.json");
        let prefs = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<FfmpegPrefs>(&text).ok())
            .unwrap_or_default();
        let path = strip_verbatim_prefix(prefs.path.trim());
        let bundled = find_bundled(resource_dir.as_deref());
        let store = Self {
            inner: Arc::new(Inner {
                file,
                data_dir: data_dir.to_path_buf(),
                path: Mutex::new(path),
                bundled,
                provisioning: AtomicBool::new(false),
            }),
        };
        store.apply();
        store
    }

    fn configured(&self) -> String {
        self.inner
            .path
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 已下载集成的可执行文件位置：`<app_data_dir>/tools/ffmpeg/ffmpeg[.exe]`
    pub fn managed_bin(&self) -> PathBuf {
        self.inner
            .data_dir
            .join("tools")
            .join("ffmpeg")
            .join(EXE_NAME)
    }

    /// 当前解析结果与来源。
    fn resolve(&self) -> (String, &'static str) {
        let configured = self.configured();
        if !configured.is_empty() {
            return (configured, "configured");
        }
        let managed = self.managed_bin();
        if managed.is_file() {
            return (path_text(&managed), "managed");
        }
        if let Some(bundled) = self.inner.bundled.as_ref()
            && bundled.is_file()
        {
            return (path_text(bundled), "bundled");
        }
        if let Ok(env) = std::env::var("VBOX_FFMPEG")
            && !env.trim().is_empty()
        {
            return (strip_verbatim_prefix(env.trim()), "env");
        }
        ("ffmpeg".to_string(), "path")
    }

    /// 把解析结果同步给 `hls_util`（下载引擎与播放器保存都走这里）。
    fn apply(&self) {
        let (effective, source) = self.resolve();
        let override_path = matches!(source, "configured" | "managed" | "bundled")
            .then_some(effective);
        crate::hls_util::set_ffmpeg_override(override_path);
    }

    fn persist(&self, path: &str) -> Result<(), String> {
        let prefs = FfmpegPrefs {
            path: path.to_string(),
        };
        let text = serde_json::to_string_pretty(&prefs)
            .map_err(|e| format!("序列化 FFmpeg 设置失败: {e}"))?;
        if let Some(parent) = self.inner.file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建设置目录失败: {e}"))?;
        }
        let tmp = self.inner.file.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("写入 FFmpeg 设置失败: {e}"))?;
        let _ = std::fs::remove_file(&self.inner.file);
        std::fs::rename(&tmp, &self.inner.file)
            .map_err(|e| format!("保存 FFmpeg 设置失败: {e}"))
    }

    /// 保存并应用路径（None / 空串 = 自动：已下载 → 随包 → 环境变量 → PATH）。
    ///
    /// 保存前去掉 Windows `\\?\` 前缀：这个前缀只对 Win32 API 有意义，存下来会让
    /// 设置页展示成 `\\?\D:\...`。功能上普通路径等价，不影响启动子进程。
    pub fn set_path(&self, path: Option<String>) -> Result<FfmpegStatus, String> {
        let path = strip_verbatim_prefix(path.unwrap_or_default().trim());
        if !path.is_empty() {
            let candidate = Path::new(&path);
            if !candidate.is_file() {
                return Err(format!("不是有效的可执行文件: {path}"));
            }
        }
        self.persist(&path)?;
        if let Ok(mut guard) = self.inner.path.lock() {
            *guard = path;
        }
        self.apply();
        Ok(self.status())
    }

    /// 当前解析结果 + `-version` 探测。
    pub fn status(&self) -> FfmpegStatus {
        let (effective, source) = self.resolve();
        let (available, version, error) = probe_ffmpeg(&effective);
        let managed = self.managed_bin();
        FfmpegStatus {
            path: self.configured(),
            effective,
            source: source.to_string(),
            available,
            version,
            error,
            bundled: self.inner.bundled.as_ref().map(|path| path_text(path)),
            managed: managed.is_file().then(|| path_text(&managed)),
            provisioning: self.inner.provisioning.load(Ordering::SeqCst),
            can_provision: cfg!(windows),
        }
    }

    fn emit(&self, app: &AppHandle, progress: ProvisionProgress) {
        let _ = app.emit(PROVISION_EVENT, progress);
    }

    /// 下载并集成 FFmpeg 到 `<app_data_dir>/tools/ffmpeg/`（阻塞；由命令放到后台线程）。
    ///
    /// 成功后解析顺序会自动切到「已下载副本」，无需用户再做任何事。
    fn provision(&self, app: &AppHandle) -> Result<FfmpegStatus, String> {
        if self
            .inner
            .provisioning
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("FFmpeg 正在下载中，请稍候".to_string());
        }
        let result = self.provision_inner(app);
        self.inner.provisioning.store(false, Ordering::SeqCst);
        if let Err(err) = &result {
            self.emit(
                app,
                ProvisionProgress {
                    phase: "error".to_string(),
                    downloaded: 0,
                    total: 0,
                    message: err.clone(),
                },
            );
        }
        result
    }

    fn provision_inner(&self, app: &AppHandle) -> Result<FfmpegStatus, String> {
        if !cfg!(windows) {
            return Err("一键集成目前只提供 Windows x64 的 FFmpeg 构建；当前平台请在系统包管理器安装 ffmpeg，或在设置页指定可执行文件".to_string());
        }
        let dir = self
            .inner
            .data_dir
            .join("tools")
            .join("ffmpeg");
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建工具目录失败: {e}"))?;
        let zip_path = dir.join("ffmpeg.zip");
        let bin = self.managed_bin();

        // 已有可用副本：直接返回，避免重复下载
        if bin.is_file() && probe_ffmpeg(&bin.to_string_lossy()).0 {
            self.apply();
            return Ok(self.status());
        }

        let sources = download_sources();
        let mut emit = |progress: ProvisionProgress| self.emit(app, progress);

        emit(ProvisionProgress {
            phase: "download".to_string(),
            downloaded: 0,
            total: DOWNLOAD_SIZE_HINT,
            message: "开始下载 FFmpeg（约 112 MB）…".to_string(),
        });
        fetch_package(&sources, DOWNLOAD_SHA256, &zip_path, &mut emit)?;

        emit(ProvisionProgress {
            phase: "extract".to_string(),
            downloaded: DOWNLOAD_SIZE_HINT,
            total: DOWNLOAD_SIZE_HINT,
            message: "解压并安装…".to_string(),
        });
        extract_ffmpeg(&zip_path, &bin)?;
        let _ = std::fs::remove_file(&zip_path);

        let (available, version, error) = probe_ffmpeg(&bin.to_string_lossy());
        if !available {
            return Err(format!(
                "FFmpeg 已下载但无法运行：{}",
                error.unwrap_or_else(|| "未知原因".to_string())
            ));
        }

        // 之前手动指定过、但已经不可用的路径：集成成功后清掉，让「已下载副本」生效。
        // （用户点「一键集成」的意图就是要一个能用的 FFmpeg）
        if !self.configured().is_empty() {
            let _ = self.persist("");
            if let Ok(mut guard) = self.inner.path.lock() {
                guard.clear();
            }
        }
        self.apply();
        let mut status = self.status();
        status.version = version;
        self.emit(
            app,
            ProvisionProgress {
                phase: "done".to_string(),
                downloaded: DOWNLOAD_SIZE_HINT,
                total: DOWNLOAD_SIZE_HINT,
                message: "FFmpeg 已集成，无需再手动安装".to_string(),
            },
        );
        Ok(status)
    }
}

/// 候选的随包集成路径（按优先级）：
/// 1. `VBOX_FFMPEG_BUNDLED`（自定义/调试）
/// 2. `resource_dir/resources/ffmpeg/`（Tauri 2 打包布局）
/// 3. `resource_dir/ffmpeg/`、`resource_dir/`（手工归档布局）
/// 4. exe 同目录及其 `resources/` 子目录（安装版常见布局）
/// 5. 开发期：仓库内 `src-tauri/resources/ffmpeg/`（`cargo run` 时 exe 在 target/debug）
fn find_bundled(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(custom) = std::env::var_os("VBOX_FFMPEG_BUNDLED") {
        candidates.push(PathBuf::from(strip_verbatim_prefix(
            &custom.to_string_lossy(),
        )));
    }
    if let Some(dir) = resource_dir {
        // Tauri resource_dir 在 Windows 上可能带 `\\?\`；先还原成普通路径再拼候选。
        let dir = PathBuf::from(strip_verbatim_prefix(&dir.to_string_lossy()));
        candidates.push(dir.join(BUNDLED_SUBDIR).join(EXE_NAME));
        candidates.push(dir.join("ffmpeg").join(EXE_NAME));
        candidates.push(dir.join(EXE_NAME));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(BUNDLED_SUBDIR).join(EXE_NAME));
        candidates.push(dir.join("resources").join("ffmpeg").join(EXE_NAME));
        candidates.push(dir.join("ffmpeg").join(EXE_NAME));
        if let Some(root) = dir.parent().and_then(Path::parent) {
            candidates.push(root.join(BUNDLED_SUBDIR).join(EXE_NAME));
        }
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("ffmpeg")
            .join(EXE_NAME),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| PathBuf::from(strip_verbatim_prefix(&path.to_string_lossy())))
}

/// 下载地址候选：`VBOX_FFMPEG_URL`（自定义镜像/离线包）→ gh-proxy → ghproxy.net →
/// GitHub 直连。内容由 `DOWNLOAD_SHA256` 锁定，镜像只影响速度，不影响正确性。
fn download_sources() -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    if let Ok(custom) = std::env::var("VBOX_FFMPEG_URL")
        && !custom.trim().is_empty()
    {
        sources.push(custom.trim().to_string());
    }
    for prefix in DOWNLOAD_MIRRORS {
        sources.push(format!("{prefix}{DOWNLOAD_URL}"));
    }
    sources.push(DOWNLOAD_URL.to_string());
    sources
}

/// 逐个镜像下载 + 校验；任一镜像成功即返回，全失败时返回最后一个错误。
fn fetch_package(
    sources: &[String],
    sha256: &str,
    zip_path: &Path,
    progress: &mut dyn FnMut(ProvisionProgress),
) -> Result<(), String> {
    let mut last_error = String::new();
    for source in sources {
        progress(ProvisionProgress {
            phase: "download".to_string(),
            downloaded: 0,
            total: DOWNLOAD_SIZE_HINT,
            message: format!("开始下载 FFmpeg（{source}）…"),
        });
        if let Err(err) = download_zip(progress, source, zip_path) {
            last_error = err;
            let _ = std::fs::remove_file(zip_path);
            continue;
        }
        progress(ProvisionProgress {
            phase: "verify".to_string(),
            downloaded: DOWNLOAD_SIZE_HINT,
            total: DOWNLOAD_SIZE_HINT,
            message: "校验文件完整性…".to_string(),
        });
        match sha256_file(zip_path) {
            Ok(actual) if actual.eq_ignore_ascii_case(sha256) => return Ok(()),
            Ok(actual) => {
                last_error =
                    format!("FFmpeg 安装包校验失败（期望 {sha256}，实际 {actual}）");
                let _ = std::fs::remove_file(zip_path);
            }
            Err(err) => {
                last_error = err;
                let _ = std::fs::remove_file(zip_path);
            }
        }
    }
    Err(format!(
        "下载 FFmpeg 失败：{last_error}。可稍后重试，或用 VBOX_FFMPEG_URL 指定一个可用镜像"
    ))
}

/// 下载到 `dest`：优先用系统 curl（Windows 10+ / macOS / Linux 自带）。
///
/// curl 支持 `--speed-limit/--speed-time`：连接卡死时自动放弃并换镜像，
/// 不会像原生 HTTP 读那样永久阻塞；没有 curl 时回退 `reqwest`。
fn download_zip(
    progress: &mut dyn FnMut(ProvisionProgress),
    url: &str,
    dest: &Path,
) -> Result<(), String> {
    match download_zip_curl(progress, url, dest) {
        Err(err) if err == CURL_MISSING => download_zip_http(progress, url, dest),
        result => result,
    }
}

/// 表示本机没有 curl（用于回退，不是用户可见错误）
const CURL_MISSING: &str = "__vbox_curl_missing__";

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn download_zip_curl(
    progress: &mut dyn FnMut(ProvisionProgress),
    url: &str,
    dest: &Path,
) -> Result<(), String> {
    let _ = std::fs::remove_file(dest);
    let mut command = Command::new("curl");
    command
        .arg("-L")
        .arg("--fail")
        .arg("--connect-timeout")
        .arg("20")
        .arg("--max-time")
        .arg("3600")
        // 60 秒内低于 10KB/s 视为卡死，直接换镜像
        .arg("--speed-limit")
        .arg("10240")
        .arg("--speed-time")
        .arg("60")
        .arg("-A")
        .arg("VBox/0.1 (ffmpeg provisioning)")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CURL_MISSING.to_string());
        }
        Err(err) => return Err(format!("启动 curl 失败: {err}")),
    };
    let mut last_emit = Instant::now();
    loop {
        if last_emit.elapsed() >= Duration::from_millis(300) {
            let downloaded = file_len(dest);
            progress(ProvisionProgress {
                phase: "download".to_string(),
                downloaded,
                total: DOWNLOAD_SIZE_HINT,
                message: format!("正在下载 FFmpeg… {}", percent_text(downloaded, DOWNLOAD_SIZE_HINT)),
            });
            last_emit = Instant::now();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(format!(
                        "下载失败（curl exit={:?}；连接超时或速度过低）",
                        status.code()
                    ));
                }
                let downloaded = file_len(dest);
                if downloaded == 0 {
                    return Err("下载失败：文件为空".to_string());
                }
                progress(ProvisionProgress {
                    phase: "download".to_string(),
                    downloaded,
                    total: DOWNLOAD_SIZE_HINT,
                    message: "下载完成".to_string(),
                });
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(err) => {
                let _ = child.kill();
                return Err(format!("等待 curl 失败: {err}"));
            }
        }
    }
}

/// 没有 curl 时的回退：`reqwest` 阻塞下载（整体超时兜底，不做卡死检测）。
fn download_zip_http(
    progress: &mut dyn FnMut(ProvisionProgress),
    url: &str,
    dest: &Path,
) -> Result<(), String> {
    let _ = std::fs::remove_file(dest);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(1200))
        .user_agent("VBox/0.1 (ffmpeg provisioning)")
        .build()
        .map_err(|e| format!("初始化下载器失败: {e}"))?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("下载 FFmpeg 失败: {e}（可设 VBOX_FFMPEG_URL 换镜像）"))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载 FFmpeg 失败: HTTP {}（可设 VBOX_FFMPEG_URL 换镜像）",
            response.status()
        ));
    }
    let total = response
        .content_length()
        .filter(|value| *value > 0)
        .unwrap_or(DOWNLOAD_SIZE_HINT);
    let file = std::fs::File::create(dest).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut out = BufWriter::new(file);
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut last_emit = Instant::now();
    loop {
        let read = response
            .read(&mut buf)
            .map_err(|e| format!("下载中断: {e}"))?;
        if read == 0 {
            break;
        }
        out.write_all(&buf[..read])
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        downloaded += read as u64;
        if last_emit.elapsed() >= Duration::from_millis(250) {
            progress(ProvisionProgress {
                phase: "download".to_string(),
                downloaded,
                total,
                message: format!("正在下载 FFmpeg… {}", percent_text(downloaded, total)),
            });
            last_emit = Instant::now();
        }
    }
    out.flush().map_err(|e| format!("写入临时文件失败: {e}"))?;
    if downloaded == 0 {
        return Err("下载失败：文件为空".to_string());
    }
    progress(ProvisionProgress {
        phase: "download".to_string(),
        downloaded,
        total,
        message: "下载完成".to_string(),
    });
    Ok(())
}

fn percent_text(downloaded: u64, total: u64) -> String {
    if total == 0 {
        return format!("{:.1} MB", downloaded as f64 / 1_048_576.0);
    }
    format!(
        "{:.0}%（{:.1}/{:.1} MB）",
        downloaded as f64 * 100.0 / total as f64,
        downloaded as f64 / 1_048_576.0,
        total as f64 / 1_048_576.0
    )
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取安装包失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|e| format!("读取安装包失败: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 从 zip 里取出 `ffmpeg.exe`（静态构建包内路径为 `ffmpeg-<ver>-essentials_build/bin/ffmpeg.exe`）。
fn extract_ffmpeg(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开安装包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("解析安装包失败: {e}"))?;
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let index = names
        .iter()
        .position(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with("/ffmpeg.exe") || lower == "ffmpeg.exe"
        })
        .ok_or_else(|| "安装包里没有 ffmpeg.exe".to_string())?;
    let mut entry = archive
        .by_index(index)
        .map_err(|e| format!("读取 ffmpeg.exe 失败: {e}"))?;

    let tmp = dest.with_file_name(format!("{EXE_NAME}.tmp"));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let mut out = std::fs::File::create(&tmp).map_err(|e| format!("写入 ffmpeg 失败: {e}"))?;
    std::io::copy(&mut entry, &mut out).map_err(|e| format!("解压 ffmpeg 失败: {e}"))?;
    out.flush().map_err(|e| format!("解压 ffmpeg 失败: {e}"))?;
    drop(out);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    if dest.is_file() {
        std::fs::remove_file(dest).map_err(|e| format!("替换旧 ffmpeg 失败: {e}"))?;
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("安装 ffmpeg 失败: {e}"))
}

/// 执行 `ffmpeg -version`，返回 (可用, 版本首行, 错误)。
fn probe_ffmpeg(bin: &str) -> (bool, Option<String>, Option<String>) {
    let mut command = Command::new(bin);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match command.output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let first = text.lines().next().unwrap_or("").trim().to_string();
            let version = if first.chars().count() > 160 {
                first.chars().take(160).collect()
            } else {
                first
            };
            (true, Some(version), None)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = stderr.lines().last().unwrap_or("").trim().to_string();
            (
                false,
                None,
                Some(format!(
                    "执行失败（退出码 {:?}）{}",
                    output.status.code(),
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(": {tail}")
                    }
                )),
            )
        }
        Err(err) => (false, None, Some(format!("无法执行 {bin}: {err}"))),
    }
}

/// 当前 FFmpeg 状态。
#[tauri::command]
pub fn ffmpeg_status(store: State<'_, FfmpegPrefsStore>) -> FfmpegStatus {
    store.status()
}

/// 保存 FFmpeg 路径并返回最新状态（`path` 为 null/空串表示恢复自动）。
#[tauri::command]
pub fn ffmpeg_set_path(
    store: State<'_, FfmpegPrefsStore>,
    path: Option<String>,
) -> Result<FfmpegStatus, String> {
    store.set_path(path)
}

/// 下载并集成 FFmpeg（后台线程执行；进度通过 `ffmpeg-provision` 事件推送）。
#[tauri::command]
pub async fn ffmpeg_provision(
    app: AppHandle,
    store: State<'_, FfmpegPrefsStore>,
) -> Result<FfmpegStatus, String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.provision(&app))
        .await
        .map_err(|e| format!("下载任务异常: {e}"))?
}

/// 原生「选择 ffmpeg 可执行文件」对话框（后台线程，取消返回 null）。
#[tauri::command]
pub async fn ffmpeg_pick() -> Option<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut dialog = rfd::FileDialog::new().set_title("选择 ffmpeg 可执行文件");
        if cfg!(windows) {
            dialog = dialog.add_filter("可执行文件", &["exe", "bat", "cmd"]);
        }
        dialog.pick_file()
    })
    .await
    .ok()
    .flatten()
    .map(|path| path.to_string_lossy().to_string())
}

/// 打开 ffmpeg 官方下载页（安装引导按钮）。
#[tauri::command]
pub fn ffmpeg_open_download(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url("https://ffmpeg.org/download.html", None::<&str>)
        .map_err(|e| format!("打开下载页失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vbox-ffmpeg-prefs-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    #[test]
    fn default_is_auto_and_invalid_path_rejected() {
        let dir = temp_dir("default");
        let store = FfmpegPrefsStore::new(&dir, None);
        assert_eq!(store.configured(), "");
        assert!(
            store
                .set_path(Some(dir.join("nope.exe").to_string_lossy().to_string()))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persists_and_restores_configured_path() {
        let dir = temp_dir("persist");
        // 用一个空文件当「已存在的可执行文件」：只验证路径存取，不真正运行它
        let fake = dir.join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        std::fs::write(&fake, b"").expect("写测试文件失败");
        let store = FfmpegPrefsStore::new(&dir, None);
        let status = store
            .set_path(Some(fake.to_string_lossy().to_string()))
            .expect("保存失败");
        assert_eq!(status.source, "configured");
        assert_eq!(status.path, fake.to_string_lossy());

        let reloaded = FfmpegPrefsStore::new(&dir, None);
        assert_eq!(reloaded.configured(), fake.to_string_lossy());

        // 恢复自动
        let auto = reloaded.set_path(None).expect("恢复自动失败");
        assert_eq!(auto.path, "");
        assert_ne!(auto.source, "configured");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 随包副本的探测：把假 ffmpeg 放进 `resources/ffmpeg/`，应被识别为 bundled。
    #[test]
    fn detects_bundled_copy_and_prefers_managed() {        let dir = temp_dir("bundled");
        let resources = dir.join("resources");
        let bundled_dir = resources.join("ffmpeg");
        std::fs::create_dir_all(&bundled_dir).expect("创建资源目录失败");
        let bundled = bundled_dir.join(EXE_NAME);
        std::fs::write(&bundled, b"").expect("写随包副本失败");

        let store = FfmpegPrefsStore::new(&dir, Some(resources.clone()));
        let status = store.status();
        assert_eq!(status.source, "bundled");
        assert_eq!(
            status.bundled.as_deref(),
            Some(bundled.to_string_lossy().as_ref())
        );

        // 已下载副本优先级高于随包副本
        let managed = store.managed_bin();
        std::fs::create_dir_all(managed.parent().expect("managed 无父目录"))
            .expect("创建 managed 目录失败");
        std::fs::write(&managed, b"").expect("写 managed 副本失败");
        let status = store.status();
        assert_eq!(status.source, "managed");
        assert_eq!(
            status.managed.as_deref(),
            Some(managed.to_string_lossy().as_ref())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 路径展示：保存/返回的路径都不能带 Windows `\\?\` 前缀（即使来源是 Verbatim 路径）。
    #[test]
    fn strips_verbatim_prefix_from_reported_paths() {
        let dir = temp_dir("verbatim-configured");
        let fake = dir.join(EXE_NAME);
        std::fs::write(&fake, b"").expect("写测试文件失败");

        // 以 Verbatim 路径保存，状态里应还原成普通路径。
        let store = FfmpegPrefsStore::new(&dir, None);
        let verbatim = format!(r"\\?\{}", fake.to_string_lossy());
        let status = store.set_path(Some(verbatim)).expect("保存失败");
        assert_eq!(status.path, fake.to_string_lossy());
        assert!(!status.path.contains(r"\\?\"), "path 不应带前缀: {:?}", status.path);
        let _ = std::fs::remove_dir_all(&dir);

        // 随包路径同样要还原（Tauri resource_dir 在 Windows 上是 canonicalize 出来的）。
        let dir = temp_dir("verbatim-bundled");
        let resources = dir.join("resources");
        let bundled_dir = resources.join("ffmpeg");
        std::fs::create_dir_all(&bundled_dir).expect("创建资源目录失败");
        let bundled = bundled_dir.join(EXE_NAME);
        std::fs::write(&bundled, b"").expect("写随包副本失败");
        let verbatim_resources =
            PathBuf::from(format!(r"\\?\{}", resources.to_string_lossy()));
        let store = FfmpegPrefsStore::new(&dir, Some(verbatim_resources));
        let status = store.status();
        assert_eq!(status.source, "bundled");
        assert_eq!(status.bundled.as_deref(), Some(bundled.to_string_lossy().as_ref()));
        assert!(
            !status.effective.contains(r"\\?\"),
            "effective 不应带前缀: {:?}",
            status.effective
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 本机装有 ffmpeg 时，默认解析应命中 PATH 且能读出内部版本首行。
    #[test]
    fn detects_ffmpeg_from_path_when_installed() {
        if std::env::var("VBOX_FFMPEG").is_ok() || std::env::var("VBOX_FFMPEG_BUNDLED").is_ok() {
            eprintln!("skip: 环境变量已设置");
            return;
        }
        let probe = Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !probe.map(|status| status.success()).unwrap_or(false) {
            eprintln!("skip: 未安装 ffmpeg");
            return;
        }
        let dir = temp_dir("detect");
        let store = FfmpegPrefsStore::new(&dir, None);
        let status = store.status();
        // 开发机可能把 ffmpeg 放在仓库 resources/ffmpeg/，这时 source 是 bundled，
        // 但无论如何都应是可用状态。
        assert!(
            status.available || status.source != "path" || status.error.is_some(),
            "解析结果异常: {:?}",
            status
        );
        if status.source == "path" {
            assert!(status.available, "应检测到 ffmpeg: {:?}", status.error);
            assert!(
                status.version.as_deref().unwrap_or("").contains("ffmpeg"),
                "版本串异常: {:?}",
                status.version
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// zip 解包：合成一个含 `bin/ffmpeg.exe` 的压缩包，验证能取出并落到目标位置。
    #[test]
    fn extracts_ffmpeg_from_zip() {
        let dir = temp_dir("zip");
        let zip_path = dir.join("pkg.zip");
        std::fs::write(&zip_path, build_zip(b"fake ffmpeg binary")).expect("写 zip 失败");
        let dest = dir.join("tools").join(EXE_NAME);
        extract_ffmpeg(&zip_path, &dest).expect("解压失败");
        assert!(dest.is_file());
        assert_eq!(
            std::fs::read(&dest).expect("读解压结果失败"),
            b"fake ffmpeg binary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 合成一个真实 zip 包（`.../bin/ffmpeg.exe` 内含 `payload`）。
    fn build_zip(payload: &[u8]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer
                .start_file("ffmpeg-1.0-essentials_build/bin/ffmpeg.exe", options)
                .expect("写 zip 条目失败");
            writer.write_all(payload).expect("写内容失败");
            writer.finish().expect("完成 zip 失败");
        }
        buffer.into_inner()
    }

    /// 起一个只服务一次的本地 HTTP 服务（模拟镜像），返回下载地址。
    fn serve_once(body: Vec<u8>) -> String {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let addr = listener.local_addr().expect("读取本地端口失败");
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        });
        format!("http://{addr}/ffmpeg.zip")
    }

    /// 端到端：本地 HTTP → 下载 → SHA-256 校验 → 解包，得到已安装的可执行文件。
    #[test]
    fn downloads_verifies_and_installs_from_local_server() {
        let dir = temp_dir("e2e");
        let payload = b"fake ffmpeg from local server";
        let zip_bytes = build_zip(payload);
        let sha = hex::encode(Sha256::digest(&zip_bytes));
        let url = serve_once(zip_bytes);
        let zip_path = dir.join("ffmpeg.zip");
        let bin = dir.join(EXE_NAME);

        let mut phases: Vec<String> = Vec::new();
        fetch_package(&[url], &sha, &zip_path, &mut |progress| {
            phases.push(progress.phase)
        })
        .expect("下载/校验失败");
        assert!(zip_path.is_file(), "应留下已校验的安装包");
        extract_ffmpeg(&zip_path, &bin).expect("解包失败");
        assert_eq!(std::fs::read(&bin).expect("读取安装结果失败"), payload);
        assert!(phases.iter().any(|phase| phase == "download"));
        assert!(phases.iter().any(|phase| phase == "verify"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 校验失败时必须拒绝并删掉安装包，不能把可疑文件装进来。
    #[test]
    fn rejects_wrong_checksum() {
        let dir = temp_dir("bad-sha");
        let zip_bytes = build_zip(b"tampered payload");
        let url = serve_once(zip_bytes);
        let zip_path = dir.join("ffmpeg.zip");
        let bin = dir.join(EXE_NAME);

        let err = fetch_package(
            &[url],
            "0000000000000000000000000000000000000000000000000000000000000000",
            &zip_path,
            &mut |_| {},
        )
        .expect_err("哈希不匹配必须失败");
        assert!(err.contains("校验失败"), "错误信息异常: {err}");
        assert!(!zip_path.is_file(), "校验失败应删除安装包");
        assert!(!bin.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_matches_known_vector() {
        let dir = temp_dir("sha");
        let file = dir.join("payload.bin");
        std::fs::write(&file, b"abc").expect("写测试文件失败");
        assert_eq!(
            sha256_file(&file).expect("计算哈希失败"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
