//! 下载后端（SQLite 持久化 + HTTP 直链 / HLS 分片 + Tauri 命令）
//!
//! 设计要点：
//! - 任务与目录设置存在 `<app_data_dir>/downloads.db`（rusqlite bundled）；
//!   连接用 `Mutex<Option<Connection>>` 托管——打开失败时降级为内存库，
//!   功能仍可用，只是重启后任务丢失；
//! - 下载目录默认 `<文档目录>/VBox`（Windows / macOS 同为文档目录），
//!   由 lib.rs setup 传入 `app.path().document_dir()`；没有文档目录时回退
//!   `<app_data_dir>/downloads`。播放器的磁盘缓存目录 = `<下载目录>/cache`；
//! - 后台固定工作线程池轮询 queued 任务：并发数可配置（1-8，默认 1），并支持
//!   全局限速（KB/s，0=不限）；暂停/续传通过每任务的控制标志位（AtomicBool）+
//!   HTTP Range / HLS 跳过已存在分片实现；
//! - 任务变化通过 `download-progress` 事件推给前端（payload = 单个 DownloadTask），
//!   进度节流 ~400ms，状态变化立即发；
//! - 字段名与前端契约严格一致（camelCase）：`sizeBytes` 是总字节数，
//!   对应数据库列 `total_bytes`。
//!
//! 注意：命令名/参数名（Tauri 默认把 snake_case 参数映射为 camelCase）不要随意改：
//! `download_list` / `download_start(params)` / `download_control(id, action)` /
//! `download_remove(id, removeFile)` / `download_settings` / `download_set_dir(dir)` /
//! `download_pick_dir` / `download_open_dir`。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// 数据库文件名（位于 app_data_dir 下）
const DB_FILE: &str = "downloads.db";

/// 前端监听的事件名
pub const EVENT_NAME: &str = "download-progress";

/// 任务状态（与前端 `DownloadStatus` 对齐）
pub const STATUS_QUEUED: &str = "queued";
#[allow(dead_code)]
pub const STATUS_DOWNLOADING: &str = "downloading";
// 下面三个状态只由 SQL 直接写入（避免把状态字符串散落各处），常量供测试与外部读取。
#[allow(dead_code)]
pub const STATUS_PAUSED: &str = "paused";
#[allow(dead_code)]
pub const STATUS_COMPLETED: &str = "completed";
#[allow(dead_code)]
pub const STATUS_FAILED: &str = "failed";

/// 资源类型
pub const KIND_FILE: &str = "file";
pub const KIND_HLS: &str = "hls";
/// 缓存优先下载：HLS 下载先建本地代理会话复用播放缓存，只补缺失分片再合并；
/// 已缓存 100% 时零流量，完成后删除该集播放缓存（成品与缓存不重复占盘）。
pub const KIND_CACHED: &str = "cached";

/// 工作线程池大小（并发上限的物理上限）
const MAX_DOWNLOAD_WORKERS: usize = 8;
/// 默认并发数（保持旧版「一次下一个」的行为）
const DEFAULT_CONCURRENCY: u32 = 1;
/// 并发数上限（设置页可选范围）
const MAX_CONCURRENCY: u32 = 8;
/// 限速上限（KB/s）；0 = 不限速
const MAX_SPEED_LIMIT_KBPS: u32 = 1_048_576;

/// 建表/迁移脚本（幂等）。
const MIGRATION_SQL: &str = "
CREATE TABLE IF NOT EXISTS downloads (
    id               TEXT PRIMARY KEY,
    site_key         TEXT    NOT NULL DEFAULT '',
    site_name        TEXT,
    line             TEXT    NOT NULL DEFAULT '',
    vod_id           TEXT,
    ep_index         INTEGER NOT NULL DEFAULT 0,
    name             TEXT    NOT NULL DEFAULT '',
    ep_title         TEXT    NOT NULL DEFAULT '',
    cover            TEXT,
    url              TEXT    NOT NULL DEFAULT '',
    headers_json     TEXT    NOT NULL DEFAULT '{}',
    status           TEXT    NOT NULL DEFAULT 'queued',
    progress         REAL    NOT NULL DEFAULT 0,
    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
    total_bytes      INTEGER NOT NULL DEFAULT 0,
    speed_bps        REAL    NOT NULL DEFAULT 0,
    file_path        TEXT,
    kind             TEXT    NOT NULL DEFAULT 'file',
    priority         INTEGER NOT NULL DEFAULT 0,
    error            TEXT,
    created_at       INTEGER NOT NULL DEFAULT 0,
    updated_at       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_downloads_created_at ON downloads (created_at DESC);
CREATE TABLE IF NOT EXISTS download_settings (
    id        INTEGER PRIMARY KEY CHECK (id = 1),
    dir       TEXT NOT NULL,
    auto_save INTEGER NOT NULL DEFAULT 0,
    concurrency INTEGER NOT NULL DEFAULT 1,
    speed_limit_kbps INTEGER NOT NULL DEFAULT 0,
    auto_save_low_priority INTEGER NOT NULL DEFAULT 1,
    auto_save_limit_kbps INTEGER NOT NULL DEFAULT 0
);
";

/// 一条下载任务（序列化给前端，字段名 camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: String,
    #[serde(default)]
    pub site_key: String,
    pub site_name: Option<String>,
    /// 线路名（详情页选集上方的线路 tab；解析器下拉框不参与目录与去重）
    #[serde(default)]
    pub line: String,
    pub vod_id: Option<String>,
    #[serde(default)]
    pub ep_index: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ep_title: String,
    pub cover: Option<String>,
    pub status: String,
    pub progress: f64,
    pub speed_bps: f64,
    pub downloaded_bytes: i64,
    /// 总字节数（数据库列名 `total_bytes`，前端字段名 `sizeBytes`）
    pub size_bytes: i64,
    pub file_path: Option<String>,
    #[serde(default)]
    pub kind: String,
    /// 任务优先级：0=普通（手动下载），1=低优先级（播放时自动保存）
    #[serde(default)]
    pub priority: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 新建下载任务参数（与前端 `DownloadStartPayload` 一致）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStart {
    pub site_key: String,
    #[serde(default)]
    pub site_name: Option<String>,
    /// 线路名（线路 tab 的展示名）；空值落盘为「未命名线路」
    #[serde(default)]
    pub line: String,
    #[serde(default)]
    pub vod_id: Option<String>,
    #[serde(default)]
    pub ep_index: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub ep_title: String,
    #[serde(default)]
    pub cover: Option<String>,
    pub url: String,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    /// 0=普通下载；1=低优先级（播放时自动保存，正常下载时会自动让路）
    #[serde(default)]
    pub priority: Option<i64>,
}

/// 下载目录设置：`dir` 当前目录，`default_dir` 系统默认（文档/VBox），
/// `auto_save` = 播放时自动保存（前端字段 `autoSave`，默认 false）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    pub dir: String,
    pub default_dir: String,
    #[serde(default)]
    pub auto_save: bool,
    /// 同时下载任务数（1-8，默认 1）
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    /// 全局下载限速 KB/s（0 = 不限速）
    #[serde(default)]
    pub speed_limit_kbps: u32,
    /// 自动保存任务低优先级：普通下载进行时自动让路（默认 true）
    #[serde(default = "default_true")]
    pub auto_save_low_priority: bool,
    /// 自动保存任务单独限速 KB/s（0 = 不额外限制）
    #[serde(default)]
    pub auto_save_limit_kbps: u32,
}

fn default_true() -> bool {
    true
}

fn default_concurrency() -> u32 {
    DEFAULT_CONCURRENCY
}

/// 下载目录/缓存空间占用 + 磁盘余量（设置页「空间管理」）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStorageStatus {
    /// 当前下载目录
    pub dir: String,
    /// 所在磁盘总容量（字节，0=未知）
    pub disk_total_bytes: i64,
    /// 所在磁盘剩余空间（字节，0=未知）
    pub disk_free_bytes: i64,
    /// 下载目录总占用（字节）
    pub used_bytes: i64,
    /// 播放缓存目录占用（字节，下载目录/cache）
    pub cache_bytes: i64,
    /// `.part` 残留占用（字节）
    pub part_bytes: i64,
    /// 已完成文件总大小（字节，仅统计仍存在的文件）
    pub completed_bytes: i64,
    /// `.part` 残留文件数
    pub part_count: i64,
    /// 已完成记录数
    pub completed_count: i64,
}

/// 清理结果（删除文件数 / 释放字节）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCleanupResult {
    pub removed_files: i64,
    pub freed_bytes: i64,
}

/// 「边看边存」导出文件登记信息（与前端 DownloadStartPayload 的元信息一致）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFileInfo {
    #[serde(default)]
    pub site_key: String,
    #[serde(default)]
    pub site_name: Option<String>,
    /// 线路名（线路 tab 的展示名）
    #[serde(default)]
    pub line: String,
    #[serde(default)]
    pub vod_id: Option<String>,
    #[serde(default)]
    pub ep_index: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ep_title: String,
    #[serde(default)]
    pub cover: Option<String>,
}

/// 每任务的控制标志（命令线程置位，工作线程轮询）。
#[derive(Default)]
struct TaskControl {
    pause: AtomicBool,
    cancel: AtomicBool,
}

impl TaskControl {
    fn paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// 下载结果控制流（与真正的错误区分开）。
enum Outcome {
    Paused,
    Cancelled,
    Failed(String),
}

type StepResult<T> = Result<T, Outcome>;

impl From<std::io::Error> for Outcome {
    fn from(err: std::io::Error) -> Self {
        Outcome::Failed(format!("本地文件读写失败: {err}"))
    }
}

fn failed(message: impl Into<String>) -> Outcome {
    Outcome::Failed(message.into())
}

/// 判断 HLS 能否离线保存（不发起网络请求，便于单测）。
///
/// - DRM（FairPlay `skd://`、非 identity `KEYFORMAT`、SAMPLE-AES-CTR 等）：
///   没有授权密钥，无法离线解密，明确拒绝；
/// - SAMPLE-AES + fMP4（`#EXT-X-MAP`）：ffmpeg 的样本级解密路径不支持带初始化段的
///   fMP4，同样明确拒绝，避免下完分片才失败。
fn ensure_hls_downloadable(crypto: &crate::hls_util::MediaCrypto) -> Result<(), String> {
    if let Some(method) = &crypto.unsupported {
        return Err(format!(
            "暂不支持 {method} 加密的 HLS（DRM 类），无法离线解密分片"
        ));
    }
    if crypto.sample_aes && crypto.init.is_some() {
        return Err("SAMPLE-AES + fMP4（EXT-X-MAP）暂不支持离线解密".to_string());
    }
    Ok(())
}

/// 全局下载限速器（所有并发任务共用一份令牌桶）。
#[derive(Default)]
struct SpeedLimiter {
    state: Mutex<LimiterState>,
}

#[derive(Default)]
struct LimiterState {
    /// 目标速率（字节/秒）；0 = 不限速
    rate_bps: f64,
    /// 当前可用令牌（允许 1 秒突发）
    tokens: f64,
    last: Option<Instant>,
}

impl SpeedLimiter {
    /// 更新目标速率（KB/s）；运行中修改会对后续读取生效。
    fn set_rate(&self, kbps: u32) {
        let rate = f64::from(kbps) * 1024.0;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.rate_bps = rate;
        state.last = Some(Instant::now());
        if rate <= 0.0 {
            state.tokens = 0.0;
        } else {
            state.tokens = state.tokens.min(rate.max(64.0 * 1024.0));
        }
    }

    /// 阻塞式限速：按字节数消耗令牌，不足时 sleep 等待（只在下载线程调用）。
    fn throttle(&self, bytes: u64) {
        let need = bytes as f64;
        if need <= 0.0 {
            return;
        }
        loop {
            let wait = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let rate = state.rate_bps;
                if rate <= 0.0 {
                    return;
                }
                let now = Instant::now();
                let elapsed = state
                    .last
                    .map(|last| now.duration_since(last).as_secs_f64())
                    .unwrap_or(0.0);
                state.last = Some(now);
                // 允许 1 秒突发，至少 64 KiB，避免大分片起点卡死
                let cap = rate.max(64.0 * 1024.0);
                state.tokens = (state.tokens + elapsed * rate).min(cap);
                if state.tokens >= need {
                    state.tokens -= need;
                    return;
                }
                (need - state.tokens) / rate
            };
            std::thread::sleep(Duration::from_secs_f64(wait.clamp(0.001, 0.25)));
        }
    }
}

/// 内部共享状态（`DownloadStore` 是它的 Arc 句柄，工作线程也持有同一份）。
struct StoreInner {
    conn: Mutex<Option<Connection>>,
    /// 系统默认目录（文档/VBox，或回退 app_data/downloads）
    default_dir: PathBuf,
    /// 下载引擎的 HTTP 客户端（连接超时 15s，放行自签名证书）
    client: reqwest::blocking::Client,
    controls: Mutex<HashMap<String, Arc<TaskControl>>>,
    emitter: Mutex<Option<Arc<dyn Fn(DownloadTask) + Send + Sync>>>,
    /// 进度事件节流：id → 上次发送时间
    last_emit: Mutex<HashMap<String, Instant>>,
    worker_started: AtomicBool,
    /// 当前正在下载的任务数（工作线程池共享，用于判断并发上限）
    active: AtomicUsize,
    /// 全局下载限速器（所有并发任务共用）
    limiter: SpeedLimiter,
    /// 工作线程需要的 AppHandle（缓存优先下载要用 HlsProxy / 下载目录）；测试库可为空
    app: Mutex<Option<AppHandle>>,
    /// 正在执行「缓存优先下载」的任务 id：resume 时区分「运行中暂停」与「重启后暂停」
    active_cached: Mutex<HashSet<String>>,
}

/// 下载存储 + 后台工作线程句柄（Tauri managed state）。
#[derive(Clone)]
pub struct DownloadStore {
    inner: Arc<StoreInner>,
}

impl DownloadStore {
    /// 打开（必要时创建）`<app_data_dir>/downloads.db`。
    ///
    /// `default_documents_dir` 为系统文档目录（`app.path().document_dir()`）；
    /// 默认下载目录 = 文档目录/VBox，None 时回退 `<app_data_dir>/downloads`。
    pub fn new(app_data_dir: &Path, default_documents_dir: Option<PathBuf>) -> Self {
        let default_dir = match default_documents_dir {
            Some(dir) if !dir.as_os_str().is_empty() => dir.join("VBox"),
            _ => app_data_dir.join("downloads"),
        };
        let _ = fs::create_dir_all(app_data_dir);
        let path = app_data_dir.join(DB_FILE);
        match Connection::open(&path) {
            Ok(conn) => match init_connection(&conn, &default_dir) {
                Ok(()) => Self::with_connection(conn, default_dir),
                Err(err) => {
                    crate::logs::push(
                        "warn",
                        format!(
                            "下载数据库初始化失败（{}）：{err}，本次降级为内存库",
                            path.display()
                        ),
                    );
                    Self::memory_store(default_dir)
                }
            },
            Err(err) => {
                crate::logs::push(
                    "warn",
                    format!(
                        "下载数据库打开失败（{}）：{err}，本次降级为内存库",
                        path.display()
                    ),
                );
                Self::memory_store(default_dir)
            }
        }
    }

    /// 用已打开的连接构造（测试用内存库走的也是这里）。
    fn with_connection(conn: Connection, default_dir: PathBuf) -> Self {
        if let Err(err) = init_connection(&conn, &default_dir) {
            crate::logs::push("warn", format!("下载数据库初始化失败: {err}"));
        }
        let client = build_client();
        let store = Self {
            inner: Arc::new(StoreInner {
                conn: Mutex::new(Some(conn)),
                default_dir,
                client,
                controls: Mutex::new(HashMap::new()),
                emitter: Mutex::new(None),
                last_emit: Mutex::new(HashMap::new()),
                worker_started: AtomicBool::new(false),
                active: AtomicUsize::new(0),
                limiter: SpeedLimiter::default(),
                app: Mutex::new(None),
                active_cached: Mutex::new(HashSet::new()),
            }),
        };
        // 首次/迁移后确保目录存在
        let dir = store.media_dir();
        let _ = fs::create_dir_all(dir);
        store
    }

    /// 内存库降级（文件库打不开时仍能在本次会话内下载）。
    fn memory_store(default_dir: PathBuf) -> Self {
        match Connection::open_in_memory() {
            Ok(conn) => Self::with_connection(conn, default_dir),
            Err(err) => {
                crate::logs::push(
                    "error",
                    format!("内存下载库也不可用，本次下载功能不可用: {err}"),
                );
                let store = Self {
                    inner: Arc::new(StoreInner {
                        conn: Mutex::new(None),
                        default_dir,
                        client: build_client(),
                        controls: Mutex::new(HashMap::new()),
                        emitter: Mutex::new(None),
                        last_emit: Mutex::new(HashMap::new()),
                        worker_started: AtomicBool::new(false),
                        active: AtomicUsize::new(0),
                        limiter: SpeedLimiter::default(),
                        app: Mutex::new(None),
                        active_cached: Mutex::new(HashSet::new()),
                    }),
                };
                let _ = fs::create_dir_all(store.media_dir());
                store
            }
        }
    }

    /// 当前下载目录（每次重新读取设置，切换目录后新任务立即生效）。
    pub fn media_dir(&self) -> PathBuf {
        match self.settings() {
            Ok(settings) if !settings.dir.trim().is_empty() => PathBuf::from(settings.dir),
            _ => self.inner.default_dir.clone(),
        }
    }

    /// 播放器磁盘缓存目录（`<下载目录>/cache`）。
    pub fn cache_dir(&self) -> PathBuf {
        self.media_dir().join("cache")
    }

    /// 任务列表（按创建时间倒序）。
    pub fn list(&self) -> Result<Vec<DownloadTask>, String> {
        self.inner.with_conn(|conn| query_list(conn))
    }

    /// 读取目录设置。
    pub fn settings(&self) -> Result<DownloadSettings, String> {
        self.inner.read_settings()
    }

    /// 保存目录设置（创建目录并持久化），返回新设置。
    pub fn set_dir(&self, dir: &str) -> Result<DownloadSettings, String> {
        self.inner.set_dir(dir)
    }

    /// 保存「播放时自动保存」开关，返回最新设置。
    pub fn set_auto_save(&self, enabled: bool) -> Result<DownloadSettings, String> {
        self.inner.set_auto_save(enabled)
    }

    /// 保存下载并发数 / 全局限速（KB/s，0=不限），返回最新设置。
    pub fn set_network(
        &self,
        concurrency: u32,
        speed_limit_kbps: u32,
    ) -> Result<DownloadSettings, String> {
        self.inner.set_network(concurrency, speed_limit_kbps)
    }

    /// 保存自动保存任务策略（低优先级让路 / 单独限速），返回最新设置。
    pub fn set_auto_save_policy(
        &self,
        low_priority: bool,
        auto_save_limit_kbps: u32,
    ) -> Result<DownloadSettings, String> {
        self.inner
            .set_auto_save_policy(low_priority, auto_save_limit_kbps)
    }

    /// 单个任务（不存在返回 None）。
    pub fn get_task(&self, id: &str) -> Result<Option<DownloadTask>, String> {
        self.inner.get_task(id)
    }

    /// 登记一个已在本地生成好的完成文件（例如 HLS 边看边存导出）。
    ///
    /// 不经过下载队列：直接写一条 `completed` 记录，`file_path` 指向现成文件，
    /// 这样下载页能看到、能播放、也能删除（删除记录时可选择连文件一起删）。
    pub fn add_completed_local_file(
        &self,
        info: &SavedFileInfo,
        file_path: &Path,
    ) -> Result<DownloadTask, String> {
        static SAVED_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let size = fs::metadata(file_path)
            .ok()
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        let now = now_ms();
        let id = format!("saved-{now}-{:x}", SAVED_SEQ.fetch_add(1, Ordering::SeqCst));
        let task = DownloadTask {
            id: id.clone(),
            site_key: info.site_key.trim().to_string(),
            site_name: info.site_name.clone(),
            line: normalize_line(&info.line),
            vod_id: info.vod_id.clone(),
            ep_index: info.ep_index,
            name: if info.name.trim().is_empty() {
                "本地保存".to_string()
            } else {
                info.name.trim().to_string()
            },
            ep_title: info.ep_title.trim().to_string(),
            cover: info.cover.clone(),
            status: STATUS_COMPLETED.to_string(),
            progress: 100.0,
            speed_bps: 0.0,
            downloaded_bytes: size,
            size_bytes: size,
            file_path: Some(file_path.to_string_lossy().to_string()),
            kind: KIND_FILE.to_string(),
            priority: 0,
            error: None,
            created_at: now,
            updated_at: now,
        };
        self.inner.insert_task(&task, "", "{}")?;
        self.inner
            .get_task(&id)?
            .ok_or_else(|| "登记保存文件后记录丢失".to_string())
    }

    /// 是否已有**同集**的活动下载任务（排队 / 下载中）。
    ///
    /// 用于「播放时自动保存」去重：同一集已经由下载管理器在拉流时，跳过缓存导出，
    /// 避免同一份内容被两条链路各拉一遍（表现为 cache 与下载目录同时涨、产出两个文件）。
    pub fn has_active_task_for(&self, info: &SavedFileInfo) -> bool {
        let site = info.site_key.trim().to_string();
        let line = normalize_line(&info.line);
        let vod = info.vod_id.clone().unwrap_or_default();
        let name = info.name.trim().to_string();
        let ep = info.ep_title.trim().to_string();
        let by_identity = !site.is_empty() && !vod.is_empty();
        self.inner
            .with_conn(|conn| {
                let count: i64 = if by_identity {
                    conn.query_row(
                        "SELECT COUNT(*) FROM downloads
                         WHERE status IN ('queued','downloading')
                           AND site_key = ?1 AND line = ?2 AND vod_id = ?3 AND ep_index = ?4",
                        params![site, line, vod, info.ep_index],
                        |row| row.get(0),
                    )
                } else {
                    conn.query_row(
                        "SELECT COUNT(*) FROM downloads
                         WHERE status IN ('queued','downloading')
                           AND (?1 = '' OR site_key = ?1)
                           AND line = ?2 AND name = ?3 AND ep_title = ?4",
                        params![site, line, name, ep],
                        |row| row.get(0),
                    )
                }
                .map_err(err_str)?;
                Ok(count > 0)
            })
            .unwrap_or(false)
    }

    /// 查找该集可用的**本地成品**（completed 且文件仍存在），返回整条任务（含 file_path）。
    ///
    /// 详情页 / 历史续播起播前先查这里：有本地文件就直接播放，跳过在线解析与 HLS 缓存，
    /// 避免「已下载的集从历史记录点进来又开始新缓存」和同一集重复占磁盘。
    ///
    /// 匹配顺序：先按 `site_key + line + vod_id + ep_index` 精确匹配；同一线路里
    /// 集标题相同但全局 epIndex 对不上（换线路/历史记录）时，再退化为「线路 + 影片名 + 集标题」。
    /// 线路必须一致：不同线路的文件不能互相命中，否则线路 B 会播到线路 A 的成品。
    pub fn find_completed_file_for(&self, info: &SavedFileInfo) -> Option<DownloadTask> {
        let site = info.site_key.trim().to_string();
        let line = normalize_line(&info.line);
        let vod = info.vod_id.clone().unwrap_or_default();
        let name = info.name.trim().to_string();
        let ep = info.ep_title.trim().to_string();
        let by_identity = !site.is_empty() && !vod.is_empty();

        let query_identity = || -> Option<DownloadTask> {
            self.inner
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT {TASK_COLUMNS} FROM downloads
                         WHERE status = 'completed' AND file_path IS NOT NULL AND file_path <> ''
                           AND site_key = ?1 AND line = ?2 AND vod_id = ?3 AND ep_index = ?4
                         ORDER BY updated_at DESC LIMIT 1"
                    );
                    conn.query_row(&sql, params![site, line, vod, info.ep_index], row_to_task)
                        .optional()
                        .map_err(err_str)
                })
                .unwrap_or(None)
        };

        let query_name = || -> Option<DownloadTask> {
            self.inner
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT {TASK_COLUMNS} FROM downloads
                         WHERE status = 'completed' AND file_path IS NOT NULL AND file_path <> ''
                           AND (site_key = ?1 OR site_key = '')
                           AND line = ?2 AND name = ?3 AND ep_title = ?4
                         ORDER BY updated_at DESC LIMIT 1"
                    );
                    conn.query_row(&sql, params![site, line, name, ep], row_to_task)
                        .optional()
                        .map_err(err_str)
                })
                .unwrap_or(None)
        };

        // 记录可能还在、文件被用户手动删了：以磁盘实际存在为准，不存在就回退在线解析。
        let playable = |task: &DownloadTask| {
            task.file_path
                .as_deref()
                .map(|path| Path::new(path).is_file())
                .unwrap_or(false)
        };

        let mut task = if by_identity {
            query_identity().filter(playable)
        } else {
            None
        };
        if task.is_none() && !name.is_empty() {
            // 同一线路内退化为「影片名 + 集标题」精确匹配
            task = query_name().filter(playable);
        }
        if task.is_none() && !name.is_empty() {
            // 再退化一层：同线路 + 同影片 + 同集数（标题格式可能不同，如「第1集」vs「第01集」）
            task = self
                .find_by_name_and_episode_number(&site, &line, &name, &ep)
                .filter(playable);
        }
        task
    }

    /// 兜底匹配：同一线路、同一影片下，集标题里的数字相同即视为同一集。
    ///
    /// 「保存本集」登记的历史记录可能没有 vodId / epIndex，标题又可能写成
    /// 「第1集」「第01集」「EP1」，仅靠字符串相等会漏；这里按集数归一化比较。
    fn find_by_name_and_episode_number(
        &self,
        site: &str,
        line: &str,
        name: &str,
        ep_title: &str,
    ) -> Option<DownloadTask> {
        let wanted = episode_number(ep_title)?;
        let tasks = self
            .inner
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {TASK_COLUMNS} FROM downloads
                         WHERE status = 'completed' AND file_path IS NOT NULL AND file_path <> ''
                           AND (site_key = ?1 OR site_key = '')
                           AND line = ?2 AND name = ?3
                         ORDER BY updated_at DESC"
                    ))
                    .map_err(err_str)?;
                let rows = stmt
                    .query_map(params![site, line, name], row_to_task)
                    .map_err(err_str)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
            })
            .unwrap_or_default();
        tasks
            .into_iter()
            .find(|task| episode_number(&task.ep_title) == Some(wanted))
    }

    /// 是否已有该集的**可用成品**（completed 且文件仍存在）。
    ///
    /// 「播放时自动保存」导出前也要判断：如果「下载」已经从缓存产出了成品，
    /// 离开播放器时不应再导出第二份（否则同一集又变成两个文件）。
    pub fn has_completed_file_for(&self, info: &SavedFileInfo) -> bool {
        self.find_completed_file_for(info).is_some()
    }

    /// 「保存本集」导出开始：先登记一条 `downloading` 记录（下载页立即可见），
    /// 导出过程中由调用方更新进度，结束后改为 completed / failed。
    ///
    /// 旧实现只在导出**结束后**才登记 completed 记录，导出期间下载页什么都看不到，
    /// 用户会误以为「点保存没反应」（整集补下可能要几分钟）。
    pub fn begin_local_export(&self, info: &SavedFileInfo) -> Result<DownloadTask, String> {
        static EXPORT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = now_ms();
        let id = format!(
            "saving-{now}-{:x}",
            EXPORT_SEQ.fetch_add(1, Ordering::SeqCst)
        );
        let task = DownloadTask {
            id: id.clone(),
            site_key: info.site_key.trim().to_string(),
            site_name: info.site_name.clone(),
            line: normalize_line(&info.line),
            vod_id: info.vod_id.clone(),
            ep_index: info.ep_index,
            name: if info.name.trim().is_empty() {
                "本地保存".to_string()
            } else {
                info.name.trim().to_string()
            },
            ep_title: info.ep_title.trim().to_string(),
            cover: info.cover.clone(),
            status: STATUS_DOWNLOADING.to_string(),
            progress: 0.0,
            speed_bps: 0.0,
            downloaded_bytes: 0,
            size_bytes: 0,
            file_path: None,
            kind: KIND_FILE.to_string(),
            priority: 0,
            error: None,
            created_at: now,
            updated_at: now,
        };
        self.inner.insert_task(&task, "", "{}")?;
        let saved = self
            .inner
            .get_task(&id)?
            .ok_or_else(|| "登记保存任务后记录丢失".to_string())?;
        self.inner.emit(&saved, true);
        Ok(saved)
    }

    /// 更新「保存本集」导出进度（状态 / 已写字节 / 百分比 / 速度），并推送到下载页。
    pub fn update_local_export(
        &self,
        id: &str,
        status: &str,
        downloaded: i64,
        progress: f64,
        speed_bps: f64,
    ) -> Result<Option<DownloadTask>, String> {
        self.inner.with_conn(|conn| {
            conn.execute(
                "UPDATE downloads
                 SET status = ?1, downloaded_bytes = ?2, progress = ?3, speed_bps = ?4,
                     updated_at = ?5
                 WHERE id = ?6",
                params![
                    status,
                    downloaded.max(0),
                    progress.clamp(0.0, 100.0),
                    speed_bps.max(0.0),
                    now_ms(),
                    id
                ],
            )
            .map_err(err_str)?;
            Ok(())
        })?;
        let task = self.inner.get_task(id)?;
        if let Some(task) = &task {
            self.inner.emit(task, false);
        }
        Ok(task)
    }

    /// 「保存本集」导出完成：写入文件路径并标记 completed，推送到下载页。
    pub fn complete_local_export(
        &self,
        id: &str,
        file_path: &Path,
    ) -> Result<Option<DownloadTask>, String> {
        let size = fs::metadata(file_path)
            .ok()
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        self.inner.with_conn(|conn| {
            conn.execute(
                "UPDATE downloads
                 SET status = 'completed', progress = 100, downloaded_bytes = ?1,
                     total_bytes = ?1, speed_bps = 0, file_path = ?2, error = NULL,
                     updated_at = ?3
                 WHERE id = ?4",
                params![size, file_path.to_string_lossy().to_string(), now_ms(), id],
            )
            .map_err(err_str)?;
            Ok(())
        })?;
        let task = self.inner.get_task(id)?;
        if let Some(task) = &task {
            self.inner.emit(task, true);
        }
        Ok(task)
    }

    /// 「保存本集」导出失败：标记 failed 并写错误，避免下载页留下永远「下载中」的记录。
    pub fn fail_local_export(&self, id: &str, error: &str) -> Result<Option<DownloadTask>, String> {
        self.inner.set_status(id, STATUS_FAILED, Some(error))?;
        let task = self.inner.get_task(id)?;
        if let Some(task) = &task {
            self.inner.emit(task, true);
        }
        Ok(task)
    }

    /// 新建任务（status=queued，入库后立即返回）。
    pub fn create_task(&self, params: DownloadStart) -> Result<DownloadTask, String> {
        self.inner.create_task(params)
    }

    /// pause / resume / cancel / retry。
    pub fn control(&self, id: &str, action: &str) -> Result<Vec<DownloadTask>, String> {
        self.inner.control(id, action)
    }

    /// 删除任务记录（可选删除已下载文件），返回最新列表。
    pub fn remove(&self, id: &str, remove_file: bool) -> Result<Vec<DownloadTask>, String> {
        self.inner.remove(id, remove_file)
    }

    /// 确保后台工作线程池已启动（幂等）。
    ///
    /// 固定启动 [`MAX_DOWNLOAD_WORKERS`] 个线程，每轮读取设置里的 concurrency：
    /// 正在跑的任务数达到并发上限就等待，否则原子领取一条 queued 任务执行。
    /// 并发数运行中修改无需重启线程。
    pub fn start_worker(&self, app: AppHandle) {
        self.inner.set_app_handle(app.clone());
        self.inner.install_emitter(app.clone());
        if self.inner.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        for index in 0..MAX_DOWNLOAD_WORKERS {
            let inner = Arc::clone(&self.inner);
            let spawned = std::thread::Builder::new()
                .name(format!("vbox-download-{index}"))
                .spawn(move || {
                    loop {
                        let settings = inner.worker_settings();
                        let limit = settings.concurrency.max(1) as usize;
                        if inner.active.load(Ordering::SeqCst) >= limit {
                            std::thread::sleep(Duration::from_millis(120));
                            continue;
                        }
                        match inner.claim_next_queued() {
                            Ok(Some(task)) => {
                                inner.active.fetch_add(1, Ordering::SeqCst);
                                if let Err(err) = inner.run_claimed(task) {
                                    crate::logs::push_throttled(
                                        "download-worker",
                                        "error",
                                        format!("下载任务执行失败: {err}"),
                                        std::time::Duration::from_secs(30),
                                    );
                                }
                                inner.active.fetch_sub(1, Ordering::SeqCst);
                            }
                            Ok(None) => std::thread::sleep(Duration::from_millis(400)),
                            Err(err) => {
                                crate::logs::push_throttled(
                                    "download-worker",
                                    "error",
                                    format!("下载工作线程轮询失败: {err}"),
                                    std::time::Duration::from_secs(30),
                                );
                                std::thread::sleep(Duration::from_millis(800));
                            }
                        }
                    }
                });
            if let Err(err) = spawned {
                crate::logs::push(
                    "error",
                    format!("启动下载后台线程失败（下载不会开始）: {err}"),
                );
                self.inner.worker_started.store(false, Ordering::SeqCst);
                return;
            }
        }
    }

    /// 打开某个任务文件所在位置（Windows 资源管理器选中该文件）。
    ///
    /// 任务没有本地文件、文件已被删除时返回明确错误（前端提示），不静默失败。
    pub fn open_file_location(&self, id: &str) -> Result<(), String> {
        let task = self
            .get_task(id)?
            .ok_or_else(|| "下载任务不存在".to_string())?;
        let path = task
            .file_path
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| "该任务没有本地文件（尚未下载完成或文件已删除）".to_string())?;
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(format!("本地文件不存在: {}", path.display()));
        }
        open_in_file_manager(&path);
        Ok(())
    }

    /// 空间总览：下载目录/缓存/.part/已完成占用 + 磁盘总容量与剩余。
    pub fn storage_status(&self) -> Result<DownloadStorageStatus, String> {
        let dir = self.media_dir();
        let _ = fs::create_dir_all(&dir);
        let (used_bytes, _) = dir_usage(&dir);
        let (cache_bytes, _) = dir_usage(&dir.join("cache"));
        let mut parts = Vec::new();
        collect_part_files(&dir, &mut parts);
        let part_bytes: i64 = parts
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len() as i64)
            .sum();
        let completed_paths = self.completed_file_paths()?;
        let completed_bytes: i64 = completed_paths
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len() as i64)
            .sum();
        let (disk_total_bytes, disk_free_bytes) = disk_space(&dir);
        Ok(DownloadStorageStatus {
            dir: dir.to_string_lossy().to_string(),
            disk_total_bytes,
            disk_free_bytes,
            used_bytes,
            cache_bytes,
            part_bytes,
            completed_bytes,
            part_count: parts.len() as i64,
            completed_count: completed_paths.len() as i64,
        })
    }

    /// 清理下载目录里的 `.part` 残留（不动 cache 目录、不动正常文件）。
    pub fn cleanup_parts(&self) -> Result<DownloadCleanupResult, String> {
        let dir = self.media_dir();
        let mut parts = Vec::new();
        collect_part_files(&dir, &mut parts);
        let mut removed_files = 0i64;
        let mut freed_bytes = 0i64;
        for path in parts {
            let size = fs::metadata(&path)
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            if fs::remove_file(&path).is_ok() {
                removed_files += 1;
                freed_bytes += size;
            }
        }
        crate::logs::push(
            "info",
            format!("清理 .part 残留：{removed_files} 个文件，释放 {freed_bytes} 字节"),
        );
        Ok(DownloadCleanupResult {
            removed_files,
            freed_bytes,
        })
    }

    /// 清理已完成下载：删除已完成记录对应的本地文件，并移除这些记录。
    pub fn cleanup_completed(&self) -> Result<DownloadCleanupResult, String> {
        let paths = self.completed_file_paths()?;
        let mut removed_files = 0i64;
        let mut freed_bytes = 0i64;
        for path in paths {
            let size = fs::metadata(&path)
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            if fs::remove_file(&path).is_ok() {
                removed_files += 1;
                freed_bytes += size;
            }
        }
        let removed_records = self.inner.with_conn(|conn| {
            conn.execute("DELETE FROM downloads WHERE status = 'completed'", [])
                .map_err(err_str)
        })?;
        crate::logs::push(
            "info",
            format!(
                "清理已完成下载：{removed_records} 条记录 / {removed_files} 个文件，释放 {freed_bytes} 字节"
            ),
        );
        Ok(DownloadCleanupResult {
            removed_files,
            freed_bytes,
        })
    }

    /// 已完成记录里仍然存在的本地文件路径。
    fn completed_file_paths(&self) -> Result<Vec<PathBuf>, String> {
        self.inner.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT file_path FROM downloads WHERE status = 'completed'")
                .map_err(err_str)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Option<String>>(0))
                .map_err(err_str)?;
            let mut paths = Vec::new();
            for row in rows {
                if let Some(path) = row.map_err(err_str)? {
                    let path = PathBuf::from(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            Ok(paths)
        })
    }

    /// 打开下载目录（explorer / open / xdg-open，失败静默）。
    pub fn open_dir(&self) {
        let dir = self.media_dir();
        let _ = fs::create_dir_all(&dir);
        #[cfg(target_os = "windows")]
        let mut command = std::process::Command::new("explorer");
        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("open");
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        let mut command = std::process::Command::new("xdg-open");
        let _ = command.arg(&dir).spawn();
    }
}

/* ---------------- 内部实现 ---------------- */

impl StoreInner {
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Option<Connection>> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with_conn<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self.lock_conn();
        let conn = guard
            .as_mut()
            .ok_or_else(|| "下载数据库不可用（初始化失败）".to_string())?;
        action(conn)
    }

    fn set_app_handle(&self, app: AppHandle) {
        if let Ok(mut slot) = self.app.lock() {
            *slot = Some(app);
        }
    }

    fn app_handle(&self) -> Option<AppHandle> {
        self.app.lock().ok().and_then(|slot| slot.clone())
    }

    /// HLS 代理是否可用（可用时 HLS 下载走「缓存优先」而不是下载引擎直连）。
    fn cache_first_available(&self) -> bool {
        self.app_handle()
            .map(|app| {
                app.try_state::<std::sync::Arc<crate::hls_proxy::HlsProxy>>()
                    .is_some()
            })
            .unwrap_or(false)
    }

    fn install_emitter(&self, app: AppHandle) {
        let mut slot = self.emitter.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return;
        }
        *slot = Some(Arc::new(move |task: DownloadTask| {
            // AppHandle::emit 线程安全（内部转到主线程），工作线程直接调用即可
            let _ = app.emit(EVENT_NAME, task);
        }));
    }

    fn emit(&self, task: &DownloadTask, force: bool) {
        if !force {
            let mut last = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(at) = last.get(&task.id) {
                if at.elapsed() < Duration::from_millis(400) {
                    return;
                }
            }
            last.insert(task.id.clone(), Instant::now());
        } else {
            let mut last = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
            last.insert(task.id.clone(), Instant::now());
        }
        let callback = self
            .emitter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(callback) = callback {
            callback(task.clone());
        }
    }

    fn emit_by_id(&self, id: &str, force: bool) {
        if let Ok(Some(task)) = self.get_task(id) {
            self.emit(&task, force);
        }
    }

    fn read_settings(&self) -> Result<DownloadSettings, String> {
        let default_dir = self.default_dir.to_string_lossy().to_string();
        let stored: Option<(String, i64, i64, i64, i64, i64)> = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT dir, auto_save, concurrency, speed_limit_kbps,
                            auto_save_low_priority, auto_save_limit_kbps
                     FROM download_settings WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(err_str)
            })
            .unwrap_or(None);
        let (
            dir,
            auto_save,
            concurrency,
            speed_limit_kbps,
            auto_save_low_priority,
            auto_save_limit_kbps,
        ) = match stored {
            Some((dir, auto_save, concurrency, speed_limit_kbps, low_priority, auto_limit)) => (
                dir,
                auto_save != 0,
                concurrency,
                speed_limit_kbps,
                low_priority != 0,
                auto_limit,
            ),
            None => (
                default_dir.clone(),
                false,
                i64::from(DEFAULT_CONCURRENCY),
                0,
                true,
                0,
            ),
        };
        let dir = if dir.trim().is_empty() {
            default_dir.clone()
        } else {
            dir
        };
        let concurrency = concurrency.clamp(1, i64::from(MAX_CONCURRENCY)) as u32;
        let speed_limit_kbps = speed_limit_kbps.clamp(0, i64::from(MAX_SPEED_LIMIT_KBPS)) as u32;
        let auto_save_limit_kbps =
            auto_save_limit_kbps.clamp(0, i64::from(MAX_SPEED_LIMIT_KBPS)) as u32;
        Ok(DownloadSettings {
            dir,
            default_dir,
            auto_save,
            concurrency,
            speed_limit_kbps,
            auto_save_low_priority,
            auto_save_limit_kbps,
        })
    }

    /// 工作线程用：设置读取失败时退回默认值（不阻塞下载）。
    fn worker_settings(&self) -> DownloadSettings {
        match self.read_settings() {
            Ok(settings) => settings,
            Err(_) => DownloadSettings {
                dir: self.default_dir.to_string_lossy().to_string(),
                default_dir: self.default_dir.to_string_lossy().to_string(),
                auto_save: false,
                concurrency: DEFAULT_CONCURRENCY,
                speed_limit_kbps: 0,
                auto_save_low_priority: true,
                auto_save_limit_kbps: 0,
            },
        }
    }

    /// 当前下载目录（工作线程内部使用）。
    fn media_dir(&self) -> PathBuf {
        match self.read_settings() {
            Ok(settings) if !settings.dir.trim().is_empty() => PathBuf::from(settings.dir),
            _ => self.default_dir.clone(),
        }
    }

    /// 任务的原始下载地址（结构体不持有 url，仅入库保存）。
    fn task_url(&self, id: &str) -> Option<String> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT url FROM downloads WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(err_str)
        })
        .ok()
        .flatten()
    }

    fn set_dir(&self, dir: &str) -> Result<DownloadSettings, String> {
        let dir = dir.trim();
        if dir.is_empty() {
            return Err("下载目录不能为空".to_string());
        }
        let path = PathBuf::from(dir);
        fs::create_dir_all(&path).map_err(|e| format!("创建下载目录失败: {e}"))?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO download_settings (id, dir) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET dir = excluded.dir",
                params![dir],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.read_settings()
    }

    /// 保存并发数 / 限速（不动目录与 autoSave），返回最新设置。
    fn set_network(
        &self,
        concurrency: u32,
        speed_limit_kbps: u32,
    ) -> Result<DownloadSettings, String> {
        let concurrency = concurrency.clamp(1, MAX_CONCURRENCY) as i64;
        let speed_limit_kbps = speed_limit_kbps.clamp(0, MAX_SPEED_LIMIT_KBPS) as i64;
        let default_dir = self.default_dir.to_string_lossy().to_string();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO download_settings (id, dir, auto_save, concurrency, speed_limit_kbps)
                 VALUES (1, ?1, 0, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                     concurrency = excluded.concurrency,
                     speed_limit_kbps = excluded.speed_limit_kbps",
                params![default_dir, concurrency, speed_limit_kbps],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.read_settings()
    }

    /// 保存自动保存任务策略：低优先级 + 单独限速（不动其它设置）。
    fn set_auto_save_policy(
        &self,
        low_priority: bool,
        auto_save_limit_kbps: u32,
    ) -> Result<DownloadSettings, String> {
        let limit = auto_save_limit_kbps.min(MAX_SPEED_LIMIT_KBPS) as i64;
        let default_dir = self.default_dir.to_string_lossy().to_string();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO download_settings (
                     id, dir, auto_save, concurrency, speed_limit_kbps,
                     auto_save_low_priority, auto_save_limit_kbps
                 ) VALUES (1, ?1, 0, 1, 0, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                     auto_save_low_priority = excluded.auto_save_low_priority,
                     auto_save_limit_kbps = excluded.auto_save_limit_kbps",
                params![default_dir, low_priority as i64, limit],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.read_settings()
    }

    /// 保存自动保存开关（不动目录），返回最新设置。
    fn set_auto_save(&self, enabled: bool) -> Result<DownloadSettings, String> {
        let default_dir = self.default_dir.to_string_lossy().to_string();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO download_settings (id, dir, auto_save) VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET auto_save = excluded.auto_save",
                params![default_dir, enabled as i64],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.read_settings()
    }

    fn get_task(&self, id: &str) -> Result<Option<DownloadTask>, String> {
        self.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT {TASK_COLUMNS} FROM downloads WHERE id = ?1"),
                params![id],
                row_to_task,
            )
            .optional()
            .map_err(err_str)
        })
    }

    fn list(&self) -> Result<Vec<DownloadTask>, String> {
        self.with_conn(|conn| query_list(conn))
    }

    fn create_task(&self, params: DownloadStart) -> Result<DownloadTask, String> {
        let url = params.url.trim().to_string();
        if url.is_empty() {
            return Err("下载地址为空".to_string());
        }
        let name = fallback_name(&params.name, "未命名影片");
        let ep_index = params.ep_index.unwrap_or(0).max(0);
        let ep_title = fallback_name(
            &params.ep_title,
            &format!("第{}集", ep_index.saturating_add(1)),
        );
        let site_key = params.site_key.trim().to_string();
        let site_name = params
            .site_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| Some(site_key.clone()).filter(|value| !value.is_empty()));
        let line = normalize_line(&params.line);
        let vod_id = params
            .vod_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let cover = params
            .cover
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let headers = params.headers.unwrap_or_default();
        let priority = params.priority.unwrap_or(0).clamp(0, 1);
        let headers_json =
            serde_json::to_string(&headers).map_err(|e| format!("请求头序列化失败: {e}"))?;

        // 同集去重：手动点「下载」与「播放时自动保存」都走这里。
        // 命中 queued/downloading/paused/completed 直接返回已有任务；
        // 命中 failed 则复位为 queued（保留字节数/进度，靠 Range 续传）。
        if let Some(existing) =
            self.find_duplicate_task(&site_key, &line, vod_id.as_deref(), &name, ep_index)?
        {
            if existing.status == STATUS_FAILED {
                return self.reset_failed_task(&existing, &url, &headers_json);
            }
            return Ok(existing);
        }

        let now = now_ms();
        // HLS 优先「缓存优先下载」（复用播放缓存、只补缺失分片、完成后清缓存）；
        // 代理不可用（本地服务未起等）时退回原有 HLS 分片下载。
        let hls = is_hls_url(&url);
        let kind = if hls && self.cache_first_available() {
            KIND_CACHED
        } else if hls {
            KIND_HLS
        } else {
            KIND_FILE
        };
        let task = DownloadTask {
            id: new_task_id(),
            site_key,
            site_name,
            line,
            vod_id,
            ep_index,
            name,
            ep_title,
            cover,
            status: STATUS_QUEUED.to_string(),
            progress: 0.0,
            speed_bps: 0.0,
            downloaded_bytes: 0,
            size_bytes: 0,
            file_path: None,
            kind: kind.to_string(),
            priority,
            error: None,
            created_at: now,
            updated_at: now,
        };
        self.insert_task(&task, &url, &headers_json)?;
        self.control_for(&task.id);
        Ok(task)
    }

    /// 查找同集任务：匹配 `(site_key, line, vod_id, ep_index)`；vod_id 为空时退化为
    /// `(site_key, line, name, ep_index)`。优先返回非 failed 的任务（已完成的不要重复下载），
    /// 否则返回最近一次 failed 的任务。
    fn find_duplicate_task(
        &self,
        site_key: &str,
        line: &str,
        vod_id: Option<&str>,
        name: &str,
        ep_index: i64,
    ) -> Result<Option<DownloadTask>, String> {
        let matches = match vod_id {
            Some(vod_id) => self.with_conn(|conn| {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {TASK_COLUMNS} FROM downloads
                         WHERE site_key = ?1 AND line = ?2 AND vod_id = ?3 AND ep_index = ?4
                         ORDER BY created_at DESC, rowid DESC"
                    ))
                    .map_err(err_str)?;
                let rows = stmt
                    .query_map(params![site_key, line, vod_id, ep_index], row_to_task)
                    .map_err(err_str)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
            })?,
            None => self.with_conn(|conn| {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {TASK_COLUMNS} FROM downloads
                         WHERE site_key = ?1 AND line = ?2 AND (vod_id IS NULL OR vod_id = '')
                           AND name = ?3 AND ep_index = ?4
                         ORDER BY created_at DESC, rowid DESC"
                    ))
                    .map_err(err_str)?;
                let rows = stmt
                    .query_map(params![site_key, line, name, ep_index], row_to_task)
                    .map_err(err_str)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
            })?,
        };
        let mut fallback = None;
        for task in matches {
            if task.status != STATUS_FAILED {
                return Ok(Some(task));
            }
            if fallback.is_none() {
                fallback = Some(task);
            }
        }
        Ok(fallback)
    }

    /// failed 任务再次 start：复位为 queued，用新参数覆盖 url/headers，
    /// 保留 downloaded_bytes / progress 以便 Range 续传。
    fn reset_failed_task(
        &self,
        task: &DownloadTask,
        url: &str,
        headers_json: &str,
    ) -> Result<DownloadTask, String> {
        let control = self.control_for(&task.id);
        control.pause.store(false, Ordering::SeqCst);
        control.cancel.store(false, Ordering::SeqCst);
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE downloads
                 SET status = 'queued', error = NULL, speed_bps = 0,
                     url = ?2, headers_json = ?3, updated_at = ?4
                 WHERE id = ?1",
                params![task.id, url, headers_json, now_ms()],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.get_task(&task.id)?
            .ok_or_else(|| "复位失败任务时记录丢失".to_string())
    }

    fn insert_task(
        &self,
        task: &DownloadTask,
        url: &str,
        headers_json: &str,
    ) -> Result<(), String> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO downloads (
                     id, site_key, site_name, line, vod_id, ep_index, name, ep_title, cover,
                     url, headers_json, status, progress, downloaded_bytes, total_bytes,
                     speed_bps, file_path, kind, priority, error, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                           ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                params![
                    task.id,
                    task.site_key,
                    task.site_name,
                    task.line,
                    task.vod_id,
                    task.ep_index,
                    task.name,
                    task.ep_title,
                    task.cover,
                    url,
                    headers_json,
                    task.status,
                    task.progress,
                    task.downloaded_bytes,
                    task.size_bytes,
                    task.speed_bps,
                    task.file_path,
                    task.kind,
                    task.priority,
                    task.error,
                    task.created_at,
                    task.updated_at,
                ],
            )
            .map_err(err_str)
            .map(|_| ())
        })
    }

    fn delete_task(&self, id: &str) -> Result<(), String> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
                .map_err(err_str)
                .map(|_| ())
        })
    }

    fn control_for(&self, id: &str) -> Arc<TaskControl> {
        let mut map = self.controls.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id.to_string())
            .or_insert_with(|| Arc::new(TaskControl::default()))
            .clone()
    }

    fn control(&self, id: &str, action: &str) -> Result<Vec<DownloadTask>, String> {
        match action {
            "pause" => {
                self.control_for(id).pause.store(true, Ordering::SeqCst);
                self.with_conn(|conn| {
                    conn.execute(
                        "UPDATE downloads
                         SET status = 'paused', speed_bps = 0, updated_at = ?2
                         WHERE id = ?1 AND status IN ('queued', 'downloading')",
                        params![id, now_ms()],
                    )
                    .map_err(err_str)?;
                    Ok(())
                })?;
            }
            "resume" => {
                let control = self.control_for(id);
                // 继续：必须把暂停标志复位，否则 worker 重新取到任务后又会立刻暂停
                control.pause.store(false, Ordering::SeqCst);
                control.cancel.store(false, Ordering::SeqCst);
                // 缓存优先下载是「工作线程内阻塞导出」，暂停时 worker 没退出；
                // 继续时必须保持 downloading，否则状态回到 queued 会被其它 worker 重复领取。
                let still_running = self
                    .active_cached
                    .lock()
                    .map(|set| set.contains(id))
                    .unwrap_or(false);
                let next_status = if still_running {
                    "downloading"
                } else {
                    "queued"
                };
                self.with_conn(|conn| {
                    conn.execute(
                        "UPDATE downloads
                         SET status = ?1, error = NULL, speed_bps = 0, updated_at = ?3
                         WHERE id = ?2 AND status <> 'completed'",
                        params![next_status, id, now_ms()],
                    )
                    .map_err(err_str)?;
                    Ok(())
                })?;
            }
            "cancel" => {
                let control = self.control_for(id);
                control.cancel.store(true, Ordering::SeqCst);
                self.controls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(id);
                self.delete_task(id)?;
            }
            "retry" => {
                let control = self.control_for(id);
                control.pause.store(false, Ordering::SeqCst);
                control.cancel.store(false, Ordering::SeqCst);
                self.with_conn(|conn| {
                    conn.execute(
                        "UPDATE downloads
                         SET status = 'queued', error = NULL, speed_bps = 0, updated_at = ?2
                         WHERE id = ?1 AND status = 'failed'",
                        params![id, now_ms()],
                    )
                    .map_err(err_str)?;
                    Ok(())
                })?;
            }
            other => return Err(format!("未知的下载操作: {other}")),
        }
        self.list()
    }

    fn remove(&self, id: &str, remove_file: bool) -> Result<Vec<DownloadTask>, String> {
        let task = self.get_task(id)?;
        if let Some(control) = self
            .controls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        {
            control.cancel.store(true, Ordering::SeqCst);
        }
        self.delete_task(id)?;
        if remove_file {
            if let Some(task) = task {
                if let Some(path) = task.file_path.as_deref() {
                    let _ = fs::remove_file(Path::new(path));
                }
                // HLS：只删该集的中间目录，不能删 file_path 的 parent
                // （新结构里 parent 是线路目录，会误删整条线路的成品）。
                if task.kind == KIND_HLS {
                    let work_dir = hls_work_dir(&self.media_dir(), &task);
                    let _ = fs::remove_dir_all(work_dir);
                }
            }
        }
        self.list()
    }

    /* ---------------- 工作线程 ---------------- */

    /// 取最早的 queued 任务并同步执行一次；返回是否处理了任务。
    /// 原子领取一条 queued 任务并置为 downloading（工作线程池可并发调用）。
    fn claim_next_queued(&self) -> Result<Option<DownloadTask>, String> {
        self.with_conn(|conn| {
            let candidate: Option<(String, i64)> = conn
                .query_row(
                    "SELECT id, priority FROM downloads WHERE status = 'queued'
                     ORDER BY priority ASC, created_at ASC, rowid ASC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(err_str)?;
            let Some((id, priority)) = candidate else {
                return Ok(None);
            };
            // 低优先级（播放时自动保存）让路：有普通下载在跑时先不领取
            if priority > 0 {
                let low_priority_enabled: bool = conn
                    .query_row(
                        "SELECT auto_save_low_priority FROM download_settings WHERE id = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(err_str)?
                    .map(|value| value != 0)
                    .unwrap_or(true);
                if low_priority_enabled {
                    let running_normal: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM downloads
                             WHERE status = 'downloading' AND priority = 0",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(err_str)?;
                    if running_normal > 0 {
                        return Ok(None);
                    }
                }
            }
            let changed = conn
                .execute(
                    "UPDATE downloads
                     SET status = 'downloading', speed_bps = 0, updated_at = ?2
                     WHERE id = ?1 AND status = 'queued'",
                    params![id, now_ms()],
                )
                .map_err(err_str)?;
            if changed == 0 {
                return Ok(None);
            }
            conn.query_row(
                &format!("SELECT {TASK_COLUMNS} FROM downloads WHERE id = ?1"),
                params![id],
                row_to_task,
            )
            .optional()
            .map_err(err_str)
        })
    }

    /// 单步：领取并同步执行一条任务（测试 / 串行调试用）。
    #[cfg(test)]
    fn run_next_once(self: &Arc<Self>) -> Result<bool, String> {
        let Some(task) = self.claim_next_queued()? else {
            return Ok(false);
        };
        self.active.fetch_add(1, Ordering::SeqCst);
        let result = self.run_claimed(task);
        self.active.fetch_sub(1, Ordering::SeqCst);
        result.map(|_| true)
    }

    /// 缓存优先下载：建 HLS 代理会话 → 复用已缓存分片、只补缺失分片 → 合并成成品 → 清理缓存。
    ///
    /// - 播放过的分片直接命中 `cache/hls/<hash>`，满缓存时零流量；
    /// - 会话创建 / 播放列表解析失败时自动回退普通 HLS 下载（保证「能下就能下」）；
    /// - 暂停 / 取消 / 进度通过任务控制标志桥接到 [`crate::hls_proxy::ExportControl`]；
    /// - 成功后登记 completed 并删除播放缓存；失败保留缓存供续传。
    fn download_cached(
        self: &Arc<Self>,
        task: &DownloadTask,
        control: &Arc<TaskControl>,
    ) -> StepResult<()> {
        if control.cancelled() {
            return Err(Outcome::Cancelled);
        }
        let Some(app) = self.app_handle() else {
            return self.download_hls(task, control);
        };
        let Some(proxy) = app
            .try_state::<std::sync::Arc<crate::hls_proxy::HlsProxy>>()
            .map(|state| state.inner().clone())
        else {
            return self.download_hls(task, control);
        };
        let url = self
            .task_url(&task.id)
            .ok_or_else(|| failed("任务下载地址丢失"))?;
        let headers: Vec<(String, String)> = parse_headers(&task_headers_json(self, &task.id))
            .into_iter()
            .collect();
        let label = task_label(task);

        // 1) 建会话：缓存目录按「内容指纹」（媒体播放列表前几个分片 URL）确定，可直接命中播放时落盘的分片
        let session = match proxy.create_session(&url, &headers) {
            Ok(session) => session,
            Err(err) => {
                crate::logs::push(
                    "warn",
                    format!("缓存优先下载建会话失败，回退普通下载: {label} - {err}"),
                );
                return self.download_hls(task, control);
            }
        };
        // 2) 主动解析播放列表（正常播放时由本地服务解析；这里没有 mpv，需要自己拉一次）
        if let Err(err) = proxy.prime_tracks(&session.id) {
            crate::logs::push(
                "warn",
                format!("缓存优先下载解析播放列表失败，回退普通下载: {label} - {err}"),
            );
            proxy.close_session(&session.id);
            return self.download_hls(task, control);
        }

        crate::logs::push("info", format!("开始缓存优先下载: {label}"));
        let media_dir = self.media_dir();
        let dest_dir = episode_dir(&media_dir, task);
        if let Err(err) = ensure_dir(&dest_dir) {
            proxy.close_session(&session.id);
            return Err(err);
        }
        let name = episode_stem(&media_dir, task);

        // 3) 进度 / 暂停 / 取消桥接：任务控制标志 → 导出控制；分片进度 → 下载任务
        let export_control = crate::hls_proxy::ExportControl::new();
        {
            let mut set = self.active_cached.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(task.id.clone());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let progress_thread = {
            let inner = Arc::clone(self);
            let control = Arc::clone(control);
            let export = export_control.clone();
            let task_id = task.id.clone();
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("vbox-cached-download-progress".into())
                .spawn(move || {
                    let mut last_bytes = 0u64;
                    let mut last_at = Instant::now();
                    loop {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        if control.paused() {
                            export.pause();
                        } else {
                            export.resume();
                        }
                        if control.cancelled() {
                            export.cancel();
                        }
                        let (done, total, bytes) = export.progress();
                        let dt = last_at.elapsed().as_secs_f64().max(0.001);
                        let speed = bytes.saturating_sub(last_bytes) as f64 / dt;
                        last_bytes = bytes;
                        last_at = Instant::now();
                        inner.report_hls_progress(
                            &task_id,
                            bytes,
                            done as usize,
                            total as usize,
                            speed,
                        );
                        std::thread::sleep(Duration::from_millis(500));
                    }
                })
                .ok()
        };

        let export_result = proxy.export_with(&session.id, &dest_dir, &name, Some(&export_control));
        stop.store(true, Ordering::SeqCst);
        if let Some(handle) = progress_thread {
            let _ = handle.join();
        }
        if let Ok(mut set) = self.active_cached.lock() {
            set.remove(&task.id);
        }

        match export_result {
            Ok(path) => {
                if control.cancelled() {
                    // 取消发生在收尾阶段：删掉半成品，保留缓存供下次续传
                    let _ = std::fs::remove_file(&path);
                    proxy.close_session(&session.id);
                    return Err(Outcome::Cancelled);
                }
                let size = std::fs::metadata(&path)
                    .ok()
                    .map(|meta| meta.len() as i64)
                    .unwrap_or(0);
                if let Err(err) = self.mark_completed(&task.id, &path, size, size, KIND_FILE, None)
                {
                    return Err(failed(format!("登记下载完成失败: {err}")));
                }
                // 成品已落盘：清理该集播放缓存（避免成品 + 缓存各占一份磁盘）
                proxy.close_session_and_clean_cache(&session.id);
                crate::logs::push(
                    "info",
                    format!("缓存下载完成（已清理该集缓存）: {}", path.display()),
                );
                Ok(())
            }
            Err(err) => {
                if control.cancelled() {
                    proxy.close_session(&session.id);
                    return Err(Outcome::Cancelled);
                }
                proxy.close_session(&session.id);
                Err(failed(err))
            }
        }
    }

    /// 执行一条已领取的任务（状态已置 downloading）。
    fn run_claimed(self: &Arc<Self>, task: DownloadTask) -> Result<(), String> {
        let control = self.control_for(&task.id);
        // 竞态兜底：pause 命令先把状态写成 paused，worker 可能已经领取这条任务。
        // 此时绝不能开始下载，否则会出现「点了暂停又跑起来」/「暂停后 resume 不生效」。
        if control.paused() {
            self.set_status(&task.id, STATUS_PAUSED, None)?;
            self.emit_by_id(&task.id, true);
            return Ok(());
        }
        self.emit_by_id(&task.id, true);
        let label = task_label(&task);
        crate::logs::push("info", format!("开始下载: {label}"));

        // 限速：按当前设置更新全局令牌桶（运行中改设置，下一个任务生效）；
        // 自动保存任务（priority=1）在低优先级模式下使用独立的更小限速。
        if let Ok(settings) = self.read_settings() {
            let mut rate = settings.speed_limit_kbps;
            if task.priority > 0 && settings.auto_save_low_priority {
                let auto = settings.auto_save_limit_kbps;
                rate = match (rate, auto) {
                    (0, auto) => auto,
                    (global, 0) => global,
                    (global, auto) => global.min(auto),
                };
            }
            self.limiter.set_rate(rate);
        }

        let url = self.task_url(&task.id).unwrap_or_default();
        let result = if task.kind == KIND_CACHED {
            self.download_cached(&task, &control)
        } else if is_hls_url(&url) {
            self.download_hls(&task, &control)
        } else {
            self.download_http(&task, &control)
        };
        match result {
            Ok(()) => crate::logs::push("info", format!("下载完成: {label}")),
            Err(Outcome::Paused) => {
                // 只有仍在 downloading 时才写 paused：
                // 如果用户已经 resume（queued），这里不能覆盖回去。
                let _ = self.with_conn(|conn| {
                    conn.execute(
                        "UPDATE downloads
                         SET status = 'paused', speed_bps = 0, updated_at = ?2
                         WHERE id = ?1 AND status = 'downloading'",
                        params![task.id, now_ms()],
                    )
                    .map_err(err_str)
                    .map(|_| ())
                });
                self.emit_by_id(&task.id, true);
                crate::logs::push("info", format!("下载已暂停: {label}"));
            }
            Err(Outcome::Cancelled) => {
                // 任务记录通常已被删除；什么都不做
            }
            Err(Outcome::Failed(message)) => {
                let _ = self.with_conn(|conn| {
                    conn.execute(
                        "UPDATE downloads
                         SET status = 'failed', error = ?2, speed_bps = 0, updated_at = ?3
                         WHERE id = ?1 AND status = 'downloading'",
                        params![task.id, message, now_ms()],
                    )
                    .map_err(err_str)
                    .map(|_| ())
                });
                self.emit_by_id(&task.id, true);
                crate::logs::push("error", format!("下载失败: {label} - {message}"));
            }
        }
        Ok(())
    }

    fn set_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<(), String> {
        self.with_conn(|conn| {
            // 状态切换时速度清零：开始/暂停/失败都不应残留上一次的速度读数
            conn.execute(
                "UPDATE downloads SET status = ?1, error = ?2, speed_bps = 0, updated_at = ?3
                 WHERE id = ?4",
                params![status, error, now_ms(), id],
            )
            .map_err(err_str)
            .map(|_| ())
        })
    }

    /// 更新进度（每 ~500ms 调一次），并节流 emit。
    fn report_progress(
        &self,
        id: &str,
        downloaded: i64,
        total: i64,
        speed: f64,
        progress: f64,
    ) -> Result<(), String> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE downloads
                 SET downloaded_bytes = ?1, total_bytes = ?2, speed_bps = ?3,
                     progress = ?4, updated_at = ?5
                 WHERE id = ?6",
                params![
                    downloaded.max(0),
                    total.max(0),
                    speed.max(0.0),
                    progress.clamp(0.0, 100.0),
                    now_ms(),
                    id
                ],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.emit_by_id(id, false);
        Ok(())
    }

    /// HLS 进度上报。
    ///
    /// HLS 播放列表本身没有「整集总字节」，因此：
    /// - `progress` 用「已完成分片 / 总分片数」表示（暂停/续传也一致）；
    /// - `total_bytes` 保持 0（未知），**绝不能**把已下载字节兜底成总数，
    ///   否则前端按 downloaded/total 聚合会显示成 100%。
    /// 完成后由 [`Self::mark_completed`] 写入真实文件大小。
    fn report_hls_progress(
        &self,
        id: &str,
        downloaded: u64,
        done: usize,
        total_segments: usize,
        speed: f64,
    ) {
        let progress = if total_segments == 0 {
            0.0
        } else {
            done as f64 / total_segments as f64 * 100.0
        };
        let _ = self.report_progress(id, downloaded as i64, 0, speed, progress);
    }

    fn mark_completed(
        &self,
        id: &str,
        file_path: &Path,
        downloaded: i64,
        total: i64,
        kind: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        // 总字节未知时（如 HLS 合并失败只留 playlist）保持 0，不要兜底成已下载字节，
        // 否则前端按 downloaded/total 会算成 100%。
        let total_value = if total > 0 {
            total.max(downloaded).max(0)
        } else {
            0
        };
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE downloads
                 SET status = 'completed', progress = 100, speed_bps = 0,
                     downloaded_bytes = ?2, total_bytes = ?3, file_path = ?4,
                     kind = ?5, error = ?6, updated_at = ?7
                 WHERE id = ?1",
                params![
                    id,
                    downloaded.max(0),
                    total_value,
                    file_path.to_string_lossy().to_string(),
                    kind,
                    error,
                    now_ms()
                ],
            )
            .map_err(err_str)
            .map(|_| ())
        })?;
        self.emit_by_id(id, true);
        Ok(())
    }

    /* ---------------- HTTP 直链 ---------------- */

    /// 先按 URL 判断；URL 不像 m3u8 时再看响应 Content-Type 是否 mpegurl。
    fn download_http(&self, task: &DownloadTask, control: &TaskControl) -> StepResult<()> {
        let url = self
            .task_url(&task.id)
            .ok_or_else(|| failed("任务下载地址丢失"))?;
        let headers = parse_headers(&task_headers_json(self, &task.id));
        // 目标目录 = <下载目录>/<站点>/<剧名>/<线路>/
        let media_dir = self.media_dir();
        let target_dir = episode_dir(&media_dir, task);
        ensure_dir(&target_dir)?;

        let ext = url_extension(&url).unwrap_or_else(|| "mp4".to_string());
        let final_path = episode_output_path(&media_dir, task, &ext);
        let part_path = append_suffix(&final_path, ".part");

        // 暂停状态下的恢复请求：先不做网络请求，尽快让 worker 继续处理队列
        if control.paused() {
            return Err(Outcome::Paused);
        }

        let existing = file_len(&part_path).unwrap_or(0);
        let mut response = self
            .http_get(&url, &headers, Some(existing), None)
            .map_err(failed)?;

        // 服务端可能返回 m3u8（URL 没有扩展名/带签名参数）：转按 HLS 处理
        if is_mpegurl(&response) {
            return self.download_hls(task, control);
        }

        // 416：本地的 .part 已经覆盖整个文件（上次下完但没来得及 rename）
        if response.status().as_u16() == 416 && existing > 0 {
            let _ = fs::remove_file(&final_path);
            fs::rename(&part_path, &final_path)?;
            self.mark_completed(
                &task.id,
                &final_path,
                existing as i64,
                existing as i64,
                KIND_FILE,
                None,
            )
            .map_err(|e| failed(e))?;
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(failed(format!(
                "服务器返回 HTTP {}",
                response.status().as_u16()
            )));
        }

        let resumed = existing > 0 && response.status().as_u16() == 206;
        let base_len = if resumed { existing } else { 0 };
        if !resumed {
            let _ = fs::remove_file(&part_path);
        }
        let total = content_range_total(&response)
            .or_else(|| response.content_length().map(|len| base_len + len))
            .unwrap_or(0);

        let mut file = if resumed {
            OpenOptions::new().append(true).open(&part_path)?
        } else {
            File::create(&part_path)?
        };

        let mut downloaded = base_len;
        let mut last_report = Instant::now();
        let mut last_bytes = downloaded;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if control.cancelled() {
                return Err(Outcome::Cancelled);
            }
            if control.paused() {
                let _ = self.report_progress(
                    &task.id,
                    downloaded as i64,
                    total as i64,
                    0.0,
                    percent(downloaded, total),
                );
                return Err(Outcome::Paused);
            }
            let read = response
                .read(&mut buf)
                .map_err(|e| failed(format!("读取响应失败（已下载 {downloaded} 字节）: {e}")))?;
            if read == 0 {
                break;
            }
            file.write_all(&buf[..read])?;
            downloaded += read as u64;
            self.limiter.throttle(read as u64);

            if last_report.elapsed() >= Duration::from_millis(500) {
                let elapsed = last_report.elapsed().as_secs_f64().max(0.001);
                let speed = (downloaded.saturating_sub(last_bytes)) as f64 / elapsed;
                let _ = self.report_progress(
                    &task.id,
                    downloaded as i64,
                    total as i64,
                    speed,
                    percent(downloaded, total),
                );
                last_report = Instant::now();
                last_bytes = downloaded;
            }
        }
        file.flush()?;
        drop(file);

        // 覆盖同名文件（Windows rename 不覆盖）
        let _ = fs::remove_file(&final_path);
        fs::rename(&part_path, &final_path)?;
        self.mark_completed(
            &task.id,
            &final_path,
            downloaded as i64,
            total.max(downloaded) as i64,
            KIND_FILE,
            None,
        )
        .map_err(failed)?;
        Ok(())
    }

    fn http_get(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        range_start: Option<u64>,
        timeout: Option<Duration>,
    ) -> Result<reqwest::blocking::Response, String> {
        let mut request = self.client.get(url);
        // 分片/小文件请求加超时：避免个别卡死的连接把单线程下载 worker 永久占住
        // （表现为「暂停后再点继续一直不动」，其实 worker 卡在上一个请求里）。
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        for (key, value) in headers {
            if !key.trim().is_empty() && !value.trim().is_empty() {
                request = request.header(key.trim(), value.trim());
            }
        }
        if let Some(start) = range_start.filter(|start| *start > 0) {
            request = request.header("Range", format!("bytes={start}-"));
        }
        request.send().map_err(|e| format!("网络请求失败: {e}"))
    }

    fn fetch_text(&self, url: &str, headers: &HashMap<String, String>) -> StepResult<String> {
        let mut request = self.client.get(url).timeout(Duration::from_secs(30));
        for (key, value) in headers {
            if !key.trim().is_empty() && !value.trim().is_empty() {
                request = request.header(key.trim(), value.trim());
            }
        }
        let response = request
            .send()
            .map_err(|e| failed(format!("下载播放列表失败: {e}")))?;
        if !response.status().is_success() {
            return Err(failed(format!(
                "下载播放列表失败: HTTP {}",
                response.status().as_u16()
            )));
        }
        response
            .text()
            .map_err(|e| failed(format!("读取播放列表失败: {e}")))
    }

    /* ---------------- HLS ---------------- */

    fn download_hls(&self, task: &DownloadTask, control: &TaskControl) -> StepResult<()> {
        let url = self
            .task_url(&task.id)
            .ok_or_else(|| failed("任务下载地址丢失"))?;
        let headers = parse_headers(&task_headers_json(self, &task.id));
        let (media_url, text) = self.resolve_media_playlist(&url, &headers)?;

        // 加密分片：AES-128 可以离线解密；SAMPLE-AES/DRM 明确不支持
        let crypto = crate::hls_util::parse_media_playlist(&text, &media_url);
        // 在下载任何分片之前就拒绝 DRM / SAMPLE-AES + fMP4（见 ensure_hls_downloadable）
        ensure_hls_downloadable(&crypto).map_err(failed)?;

        let base = reqwest::Url::parse(&media_url)
            .map_err(|e| failed(format!("播放列表地址解析失败: {e}")))?;
        // 分片中间目录 = <下载目录>/<站点>/<剧名>/<线路>/<集名>/；
        // 合并结果写在上一级：<下载目录>/<站点>/<剧名>/<线路>/<集名>.<ext>
        let media_dir = self.media_dir();
        let task_dir = hls_work_dir(&media_dir, task);
        ensure_dir(&task_dir)?;

        // 暂停状态下的恢复请求：先不做网络请求，尽快让 worker 继续处理队列
        if control.paused() {
            return Err(Outcome::Paused);
        }

        // 解析：URI 行本地化；#EXT-X-MAP 的 URI 也本地化；其余标签原样保留。
        struct Job {
            url: String,
            file_name: String,
            is_init: bool,
        }
        let mut jobs: Vec<Job> = Vec::new();
        let mut out_lines: Vec<String> = Vec::new();
        let mut segment_index = 0usize;
        for raw in text.lines() {
            let line = raw.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            if line.starts_with("#EXT-X-MAP:") {
                if let Some(uri) = extract_uri_attr(line) {
                    let absolute = join_url(&base, &uri)
                        .map_err(|e| failed(format!("初始化分片地址无效: {e}")))?;
                    let file_name = format!(
                        "init_{:05}.{}",
                        jobs.len() + 1,
                        url_extension(&absolute).unwrap_or_else(|| "mp4".to_string())
                    );
                    jobs.push(Job {
                        url: absolute,
                        file_name: file_name.clone(),
                        is_init: true,
                    });
                    out_lines.push(replace_uri_attr(line, &file_name));
                    continue;
                }
                out_lines.push(line.to_string());
                continue;
            }
            if line.starts_with('#') {
                out_lines.push(line.to_string());
                continue;
            }
            let absolute =
                join_url(&base, line).map_err(|e| failed(format!("分片地址无效({line}): {e}")))?;
            segment_index += 1;
            let file_name = format!(
                "seg_{:05}.{}",
                segment_index,
                url_extension(&absolute).unwrap_or_else(|| "ts".to_string())
            );
            jobs.push(Job {
                url: absolute,
                file_name: file_name.clone(),
                is_init: false,
            });
            out_lines.push(file_name);
        }

        let total_segments = jobs.iter().filter(|job| !job.is_init).count();
        if total_segments == 0 {
            return Err(failed("播放列表里没有可下载的分片"));
        }

        // BYTERANGE：多个分片可能指向同一远端文件的不同区间；同一 URL 只下载一次，
        // 再按 (length, offset) 切片成本地分片文件，后续合并逻辑不变。
        if crypto.has_byterange {
            if crypto.ranges.len() != total_segments {
                return Err(failed("BYTERANGE 分片数量与播放列表不一致"));
            }
            let mut resources: HashMap<String, PathBuf> = HashMap::new();
            for (index, job) in jobs.iter().filter(|job| !job.is_init).enumerate() {
                let Some((length, start)) = crypto.ranges.get(index).copied().flatten() else {
                    return Err(failed("BYTERANGE 缺少区间信息"));
                };
                let resource = if let Some(path) = resources.get(&job.url) {
                    path.clone()
                } else {
                    let name = format!("res_{:04}.bin", resources.len() + 1);
                    let path = task_dir.join(&name);
                    if file_len(&path).unwrap_or(0) == 0 {
                        self.download_segment(&job.url, &headers, &path, control)?;
                    }
                    resources.insert(job.url.clone(), path.clone());
                    path
                };
                let dest = task_dir.join(&job.file_name);
                if file_len(&dest).unwrap_or(0) == 0 {
                    let raw = fs::read(&resource).map_err(|e| {
                        failed(format!(
                            "读取 BYTERANGE 资源失败({}): {e}",
                            resource.display()
                        ))
                    })?;
                    let begin = (start as usize).min(raw.len());
                    let end = begin.saturating_add(length as usize).min(raw.len());
                    if begin == raw.len() && length > 0 {
                        return Err(failed("BYTERANGE 起点超出文件长度"));
                    }
                    fs::write(&dest, &raw[begin..end])?;
                }
            }
        }

        let mut done = 0usize;
        let mut downloaded: u64 = 0;
        let mut last_report = Instant::now();
        let mut last_bytes: u64 = 0;
        for job in &jobs {
            if control.cancelled() {
                return Err(Outcome::Cancelled);
            }
            if control.paused() {
                self.report_hls_progress(&task.id, downloaded, done, total_segments, 0.0);
                return Err(Outcome::Paused);
            }
            let path = task_dir.join(&job.file_name);
            let existing = file_len(&path).unwrap_or(0);
            if existing > 0 {
                // 已存在且非空 → 续传时跳过（init 分片同理）
                downloaded = downloaded.saturating_add(existing);
                if !job.is_init {
                    done += 1;
                    // 续传时连续跳过分片：也推一次进度，让前端立刻看到「已恢复」
                    if last_report.elapsed() >= Duration::from_millis(500) {
                        self.report_hls_progress(&task.id, downloaded, done, total_segments, 0.0);
                        last_report = Instant::now();
                        last_bytes = downloaded;
                    }
                }
                continue;
            }
            let bytes = self.download_segment(&job.url, &headers, &path, control)?;
            downloaded = downloaded.saturating_add(bytes);
            if !job.is_init {
                done += 1;
                // 速度按「这一轮新增字节 / 这一轮耗时」统计（跳过的分片不算速度）
                let elapsed = last_report.elapsed().as_secs_f64().max(0.001);
                let speed = downloaded.saturating_sub(last_bytes) as f64 / elapsed;
                self.report_hls_progress(&task.id, downloaded, done, total_segments, speed);
                last_report = Instant::now();
                last_bytes = downloaded;
            }
        }

        // 分片与 init 的文件名列表（顺序与 jobs 一致）
        let init_files: Vec<String> = jobs
            .iter()
            .filter(|job| job.is_init)
            .map(|job| job.file_name.clone())
            .collect();
        let segment_files: Vec<String> = jobs
            .iter()
            .filter(|job| !job.is_init)
            .map(|job| job.file_name.clone())
            .collect();

        // 合并输出：<下载目录>/<影片名>/<集名>.<ext>（与分片目录同级，不放在分片目录里）
        let output_path = episode_output_path(
            &media_dir,
            task,
            &merged_extension(&init_files, &segment_files),
        );

        // 密钥：同一密钥 URL 只下载一次（AES-128 与 SAMPLE-AES 共用）
        let mut segment_crypto: Vec<Option<(PathBuf, [u8; 16])>> =
            Vec::with_capacity(segment_files.len());
        let mut key_file_names: HashMap<String, String> = HashMap::new();
        if crypto.encrypted() {
            if crypto.segments.len() != segment_files.len() {
                return Err(failed("加密播放列表的分片数与下载任务不一致"));
            }
            let mut key_files: HashMap<String, PathBuf> = HashMap::new();
            for (index, key_ref) in crypto.keys.iter().enumerate() {
                let Some(key_ref) = key_ref else {
                    segment_crypto.push(None);
                    continue;
                };
                let key_path = if let Some(path) = key_files.get(&key_ref.url) {
                    path.clone()
                } else {
                    let name = format!("key_{:04}.bin", key_files.len() + 1);
                    let path = task_dir.join(&name);
                    if file_len(&path).unwrap_or(0) == 0 {
                        self.download_segment(&key_ref.url, &headers, &path, control)?;
                    }
                    key_files.insert(key_ref.url.clone(), path.clone());
                    key_file_names.insert(key_ref.url.clone(), name);
                    path
                };
                let iv = key_ref.iv.unwrap_or_else(|| {
                    crate::hls_util::sequence_iv(crypto.media_sequence + index as u64)
                });
                segment_crypto.push(Some((key_path, iv)));
            }
        }

        // 本地播放列表（URI 全部为相对本目录的文件名）
        let playlist_path = task_dir.join("playlist.m3u8");
        let mut content = out_lines.join("\n");
        content.push('\n');
        fs::write(&playlist_path, content)?;

        // SAMPLE-AES：写本地分片+密钥的临时列表，交给 ffmpeg 按样本级解密（fMP4 暂不支持）
        if crypto.sample_aes {
            if !init_files.is_empty() {
                return Err(failed("SAMPLE-AES + fMP4（EXT-X-MAP）暂不支持离线解密"));
            }
            let local_playlist = crate::hls_util::build_local_playlist(
                &crypto,
                &segment_files,
                &key_file_names,
                crypto.media_sequence,
            )
            .map_err(failed)?;
            let sample_playlist = task_dir.join("sample_aes.m3u8");
            fs::write(&sample_playlist, local_playlist)?;
            // ffmpeg 按扩展名猜封装格式：先写到分片目录（.ts），成功后再挪到影片目录
            let build_path = task_dir.join("merged.ts");
            crate::hls_util::run_ffmpeg_hls_to_ts(
                &sample_playlist.to_string_lossy(),
                &build_path,
                Duration::from_secs(1800),
            )
            .map_err(failed)?;
            // 输出始终是 TS 封装，扩展名固定 .ts（不带分片原始扩展名）
            let sample_output = episode_output_path(&media_dir, task, "ts");
            let _ = fs::remove_file(&sample_output);
            fs::rename(&build_path, &sample_output).map_err(|e| {
                failed(format!(
                    "保存合并文件失败({} -> {}): {e}",
                    build_path.display(),
                    sample_output.display()
                ))
            })?;
            let merged_size = file_len(&sample_output).unwrap_or(0) as i64;
            cleanup_hls_fragments(&task_dir, &sample_output);
            self.mark_completed(
                &task.id,
                &sample_output,
                merged_size,
                merged_size,
                KIND_FILE,
                None,
            )
            .map_err(failed)?;
            return Ok(());
        }

        // 合并单文件：TS → <集名>.ts；fMP4（有 #EXT-X-MAP）→ <集名>.mp4。
        // 失败时回退为 playlist 行为（任务仍 completed，error 记录一条非 fatal 提示）。
        match merge_hls_files(
            &task_dir,
            &output_path,
            &init_files,
            &segment_files,
            &segment_crypto,
        ) {
            Ok(merged_path) => {
                let merged_size = file_len(&merged_path).unwrap_or(0) as i64;
                cleanup_hls_fragments(&task_dir, &merged_path);
                self.mark_completed(
                    &task.id,
                    &merged_path,
                    merged_size,
                    merged_size,
                    KIND_FILE,
                    None,
                )
                .map_err(failed)?;
            }
            Err(err) => {
                let message = format!("合并失败，已保留分片播放列表: {err}");
                // 分片播放列表不能当字节数来源：total 未知时保持 0，进度沿用分片比例
                self.mark_completed(
                    &task.id,
                    &playlist_path,
                    downloaded as i64,
                    0,
                    KIND_HLS,
                    Some(&message),
                )
                .map_err(failed)?;
            }
        }
        Ok(())
    }

    /// 解析 master playlist，返回最终媒体播放列表地址与内容。
    fn resolve_media_playlist(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> StepResult<(String, String)> {
        let mut current = url.to_string();
        for _ in 0..3 {
            let text = self.fetch_text(&current, headers)?;
            let Some(variant) = pick_variant(&text) else {
                return Ok((current, text));
            };
            let base = reqwest::Url::parse(&current)
                .map_err(|e| failed(format!("播放列表地址解析失败: {e}")))?;
            current = join_url(&base, &variant)
                .map_err(|e| failed(format!("子播放列表地址无效: {e}")))?;
        }
        Err(failed("HLS master playlist 嵌套过深"))
    }

    /// 下载单个分片/初始化段；支持断点（.part + Range）。
    fn download_segment(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        path: &Path,
        control: &TaskControl,
    ) -> StepResult<u64> {
        let part_path = PathBuf::from(format!("{}.part", path.display()));
        let existing = file_len(&part_path).unwrap_or(0);
        let mut response = self
            .http_get(url, headers, Some(existing), Some(Duration::from_secs(120)))
            .map_err(failed)?;
        if response.status().as_u16() == 416 && existing > 0 {
            fs::rename(&part_path, path)?;
            return Ok(existing);
        }
        if !response.status().is_success() {
            return Err(failed(format!(
                "分片下载失败: HTTP {}",
                response.status().as_u16()
            )));
        }
        let resumed = existing > 0 && response.status().as_u16() == 206;
        if !resumed {
            let _ = fs::remove_file(&part_path);
        }
        let mut file = if resumed {
            OpenOptions::new().append(true).open(&part_path)?
        } else {
            File::create(&part_path)?
        };
        let mut written = if resumed { existing } else { 0 };
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if control.cancelled() {
                return Err(Outcome::Cancelled);
            }
            if control.paused() {
                return Err(Outcome::Paused);
            }
            let read = response
                .read(&mut buf)
                .map_err(|e| failed(format!("读取分片失败（已写 {written} 字节）: {e}")))?;
            if read == 0 {
                break;
            }
            file.write_all(&buf[..read])?;
            written += read as u64;
            self.limiter.throttle(read as u64);
        }
        file.flush()?;
        drop(file);
        let _ = fs::remove_file(path);
        fs::rename(&part_path, path)?;
        Ok(written)
    }
}

/* ---------------- 辅助函数 ---------------- */

/// `downloads` 表查询列（`total_bytes` 映射到 `size_bytes`）。
const TASK_COLUMNS: &str = "id, site_key, site_name, line, vod_id, ep_index, name, ep_title, cover,
     url, headers_json, status, progress, downloaded_bytes, total_bytes, speed_bps,
     file_path, kind, priority, error, created_at, updated_at";

/// 递归统计目录占用（字节, 文件数）；目录不存在返回 (0, 0)。
fn dir_usage(path: &Path) -> (i64, i64) {
    let mut bytes = 0i64;
    let mut files = 0i64;
    let Ok(entries) = fs::read_dir(path) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            let (child_bytes, child_files) = dir_usage(&entry.path());
            bytes += child_bytes;
            files += child_files;
        } else if meta.is_file() {
            bytes += meta.len() as i64;
            files += 1;
        }
    }
    (bytes, files)
}

/// 递归收集下载目录里的 `.part` 残留（跳过播放缓存 cache 目录）。
fn collect_part_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            // 播放缓存由 playback_cache 自己管理，避免误删代理分片
            if path.file_name().and_then(|name| name.to_str()) == Some("cache") {
                continue;
            }
            collect_part_files(&path, out);
        } else if meta.is_file() {
            let is_part = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.ends_with(".part"))
                .unwrap_or(false);
            if is_part {
                out.push(path);
            }
        }
    }
}

/// 所在磁盘总容量/剩余空间（字节，失败返回 (0, 0)）。
fn disk_space(path: &Path) -> (i64, i64) {
    let total = fs2::total_space(path).map(|v| v as i64).unwrap_or(0);
    let free = fs2::available_space(path).map(|v| v as i64).unwrap_or(0);
    (total, free)
}

/// 从数据库读任务的 headers_json（结构体不暴露给前端，避免泄露签名参数）。
fn task_headers_json(inner: &StoreInner, id: &str) -> String {
    inner
        .with_conn(|conn| {
            conn.query_row(
                "SELECT headers_json FROM downloads WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(err_str)
            .map(|value| value.unwrap_or_else(|| "{}".to_string()))
        })
        .unwrap_or_else(|_| "{}".to_string())
}

fn parse_headers(json: &str) -> HashMap<String, String> {
    serde_json::from_str::<HashMap<String, String>>(json).unwrap_or_default()
}

fn task_ep_title(task: &DownloadTask) -> String {
    if task.ep_title.trim().is_empty() {
        format!("第{}集", task.ep_index.saturating_add(1))
    } else {
        task.ep_title.clone()
    }
}

fn fallback_name(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// 任务显示名：「剧名 集名」；集名为空时只显示剧名（运行日志用）。
fn task_label(task: &DownloadTask) -> String {
    let ep = task.ep_title.trim();
    if ep.is_empty() {
        task.name.clone()
    } else {
        format!("{} {ep}", task.name)
    }
}

/* ---------------- 落盘路径 ---------------- */

/// Windows 传统路径长度预算（MAX_PATH=260；Rust std 不会自动加 `\\?\` 前缀，
/// 超长时 CreateFile/MoveFileEx 会返回 `os error 3`「系统找不到指定的路径」）。
const WINDOWS_PATH_BUDGET: usize = 240;
/// 站点目录最多字符数
const SITE_COMPONENT_MAX: usize = 24;
/// 剧名目录最多字符数
const SHOW_COMPONENT_MAX: usize = 48;
/// 线路目录最多字符数
const LINE_COMPONENT_MAX: usize = 24;
/// 集名主干最多字符数
const EPISODE_COMPONENT_MAX: usize = 48;
/// 集名之后还要留 `.part` / `.mp4` / `merged.ts.part` 等后缀
const EPISODE_SUFFIX_RESERVE: usize = 16;
/// 站点目录之后：剧名/线路/集名/后缀/3 个分隔符
const SITE_PATH_RESERVE: usize =
    SHOW_COMPONENT_MAX + LINE_COMPONENT_MAX + EPISODE_COMPONENT_MAX + EPISODE_SUFFIX_RESERVE + 3;
/// 剧名目录之后：线路/集名/后缀/2 个分隔符
const SHOW_PATH_RESERVE: usize =
    LINE_COMPONENT_MAX + EPISODE_COMPONENT_MAX + EPISODE_SUFFIX_RESERVE + 2;
/// 线路目录之后：集名/后缀/1 个分隔符
const LINE_PATH_RESERVE: usize = EPISODE_COMPONENT_MAX + EPISODE_SUFFIX_RESERVE + 1;
/// 集名之后：扩展名 / `.part` 等后缀
const EPISODE_PATH_RESERVE: usize = EPISODE_SUFFIX_RESERVE;

/// 稳定短哈希（FNV-1a）：截断超长文件名时加在末尾，避免不同片名截断后互相覆盖。
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:06x}", hash & 0xff_ffff)
}

/// 字符串的 UTF-16 码元数（Windows 路径按 UTF-16 计数，emoji 占 2 个）。
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// 按 UTF-16 码元截断，避免把代理对（emoji）切成半个字符。
fn truncate_utf16(text: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if used + width > limit {
            break;
        }
        out.push(ch);
        used += width;
    }
    out
}

/// 路径长度（UTF-16 码元数：Windows MAX_PATH 按 UTF-16 计数，emoji 占 2 个）。
fn text_len(path: &Path) -> usize {
    utf16_len(&path.to_string_lossy())
}

/// 按剩余路径预算截断单个路径组件：优先保证不超长，截断时追加短哈希防冲突。
/// `cap` 是该层目录名的软上限，避免目录树在根目录很短时逐层膨胀。
fn fit_component(parent: &Path, component: &str, reserve: usize, cap: usize) -> String {
    let component = component.trim_end_matches(['.', ' ']).trim();
    let component = if component.is_empty() {
        "未命名"
    } else {
        component
    };
    let used = text_len(parent) + reserve + 1;
    let budget = WINDOWS_PATH_BUDGET.saturating_sub(used);
    let limit = budget.min(cap);
    if utf16_len(component) <= limit {
        return component.to_string();
    }
    let suffix = short_hash(component);
    // 预算不足 8 个码元时只保留短哈希，避免 `x-hash` 反而超出剩余预算。
    if limit <= 7 {
        return suffix.chars().take(limit.max(1)).collect();
    }
    // 给 `-{6 位哈希}` 留 7 个字符。
    let keep = limit - 7;
    let head = truncate_utf16(component, keep);
    let head = head.trim_end_matches(['.', ' ']).trim();
    if head.is_empty() {
        suffix
    } else {
        format!("{head}-{suffix}")
    }
}

/// 线路名归一化：空白统一落到「未命名线路」，保证去重与目录一致。
fn normalize_line(line: &str) -> String {
    let line = line.trim();
    if line.is_empty() {
        "未命名线路".to_string()
    } else {
        line.to_string()
    }
}

/// 站点展示名：冻结在任务里的 `site_name` 优先，空则回退 `site_key`，再空用占位名。
fn display_site_name(site_name: Option<&str>, site_key: &str) -> String {
    site_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let site_key = site_key.trim();
            if site_key.is_empty() {
                "未命名站点".to_string()
            } else {
                site_key.to_string()
            }
        })
}

/// 站点目录：`<下载目录>/<站点>/`。
fn site_component(media_dir: &Path, site_name: &str) -> String {
    fit_component(
        media_dir,
        &safe_component(site_name, "未命名站点"),
        SITE_PATH_RESERVE,
        SITE_COMPONENT_MAX,
    )
}

fn site_dir(media_dir: &Path, site_name: &str) -> PathBuf {
    media_dir.join(site_component(media_dir, site_name))
}

/// 剧名目录：`<下载目录>/<站点>/<剧名>/`。
fn show_component(site_dir: &Path, name: &str) -> String {
    fit_component(
        site_dir,
        &safe_component(name, "未命名影片"),
        SHOW_PATH_RESERVE,
        SHOW_COMPONENT_MAX,
    )
}

fn show_dir(media_dir: &Path, site_name: &str, name: &str) -> PathBuf {
    let site_dir = site_dir(media_dir, site_name);
    site_dir.join(show_component(&site_dir, name))
}

/// 线路目录：`<下载目录>/<站点>/<剧名>/<线路>/`。
fn line_component(show_dir: &Path, line: &str) -> String {
    fit_component(
        show_dir,
        &safe_component(&normalize_line(line), "未命名线路"),
        LINE_PATH_RESERVE,
        LINE_COMPONENT_MAX,
    )
}

fn episode_dir_for(media_dir: &Path, site_name: &str, name: &str, line: &str) -> PathBuf {
    let show_dir = show_dir(media_dir, site_name, name);
    show_dir.join(line_component(&show_dir, line))
}

/// 一集的目录：`<下载目录>/<站点>/<剧名>/<线路>/`。
fn episode_dir(media_dir: &Path, task: &DownloadTask) -> PathBuf {
    episode_dir_for(media_dir, &task_site_name(task), &task.name, &task.line)
}

/// 集名兜底：空标题按 `第N集`（N = ep_index + 1）生成。
fn episode_label(ep_title: &str, ep_index: i64) -> String {
    let ep_title = ep_title.trim();
    if ep_title.is_empty() {
        format!("第{}集", ep_index.saturating_add(1))
    } else {
        ep_title.to_string()
    }
}

fn episode_stem_in(
    media_dir: &Path,
    site_name: &str,
    name: &str,
    line: &str,
    ep_title: &str,
    ep_index: i64,
) -> String {
    let dir = episode_dir_for(media_dir, site_name, name, line);
    fit_component(
        &dir,
        &safe_component(&episode_label(ep_title, ep_index), "未命名"),
        EPISODE_PATH_RESERVE,
        EPISODE_COMPONENT_MAX,
    )
}

/// 集文件主干名（不含扩展名）：优先集标题（合集即 `第1集`），并保证不超路径预算。
fn episode_stem(media_dir: &Path, task: &DownloadTask) -> String {
    episode_stem_in(
        media_dir,
        &task_site_name(task),
        &task.name,
        &task.line,
        &task_ep_title(task),
        task.ep_index,
    )
}

/// 集输出文件路径：`<下载目录>/<站点>/<剧名>/<线路>/<集名>.<ext>`（直链结果与 HLS 合并结果同级）。
fn episode_output_path(media_dir: &Path, task: &DownloadTask, ext: &str) -> PathBuf {
    let ext = ext.trim().trim_start_matches('.');
    episode_dir(media_dir, task).join(format!(
        "{}.{}",
        episode_stem(media_dir, task),
        if ext.is_empty() { "mp4" } else { ext }
    ))
}

/// HLS 分片中间目录：`<下载目录>/<站点>/<剧名>/<线路>/<集名>/`（合并成功后整个目录删除）。
fn hls_work_dir(media_dir: &Path, task: &DownloadTask) -> PathBuf {
    episode_dir(media_dir, task).join(episode_stem(media_dir, task))
}

/// 「保存本集 / 自动保存」用：同一套四级目录。
pub fn episode_dir_for_info(media_dir: &Path, info: &SavedFileInfo) -> PathBuf {
    episode_dir_for(
        media_dir,
        &display_site_name(info.site_name.as_deref(), &info.site_key),
        &info.name,
        &info.line,
    )
}

/// 「保存本集 / 自动保存」用：同一套集名（不含扩展名）。
pub fn episode_stem_for_info(media_dir: &Path, info: &SavedFileInfo) -> String {
    episode_stem_in(
        media_dir,
        &display_site_name(info.site_name.as_deref(), &info.site_key),
        &info.name,
        &info.line,
        &info.ep_title,
        info.ep_index,
    )
}

/// 追加后缀（`.part`），不改变原扩展名。
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// `create_dir_all`：所有落盘目录在写之前都必须先建好，失败时错误里带上完整路径，
/// 便于排查 `os error 3（系统找不到指定的路径）` 这类问题。
fn ensure_dir(path: &Path) -> StepResult<()> {
    fs::create_dir_all(path).map_err(|e| failed(format!("创建目录失败({}): {e}", path.display())))
}

/// 站点展示名（任务行里冻结的 `site_name` 优先）。
fn task_site_name(task: &DownloadTask) -> String {
    display_site_name(task.site_name.as_deref(), &task.site_key)
}

fn build_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .danger_accept_invalid_certs(true)
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120 Safari/537.36",
        )
        .build()
        .unwrap_or_else(|err| {
            crate::logs::push(
                "error",
                format!("下载 HTTP 客户端创建失败，改用默认客户端: {err}"),
            );
            reqwest::blocking::Client::new()
        })
}

/// 在系统文件管理器里定位到文件（Windows 选中文件；macOS Finder 显示；Linux 打开父目录）。
fn open_in_file_manager(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        command.raw_arg(format!("/select,\"{}\"", path.display()));
        let _ = command.spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn init_connection(conn: &Connection, default_dir: &Path) -> Result<(), String> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=3000;")
        .map_err(err_str)?;
    conn.execute_batch(MIGRATION_SQL).map_err(err_str)?;
    // 老库升级：download_settings 补 auto_save 列（新库建表时已包含）
    ensure_settings_columns(conn)?;
    ensure_download_columns(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO download_settings (id, dir) VALUES (1, ?1)",
        params![default_dir.to_string_lossy().to_string()],
    )
    .map_err(err_str)?;
    // 上次退出时停在 downloading 的任务（进程被杀/崩溃）：重新排队，靠 Range 续传
    conn.execute(
        "UPDATE downloads SET status = 'queued', speed_bps = 0 WHERE status = 'downloading'",
        [],
    )
    .map_err(err_str)?;
    // 历史数据修复：HLS 下载中曾把「已下载字节」当成总大小写进 total_bytes，
    // 前端按 downloaded/total 会算成 100%。未完成且 total == downloaded 的按「未知」重置。
    conn.execute(
        "UPDATE downloads SET total_bytes = 0
         WHERE kind = 'hls' AND status <> 'completed'
           AND total_bytes > 0 AND total_bytes = downloaded_bytes",
        [],
    )
    .map_err(err_str)?;
    let _ = fs::create_dir_all(default_dir);
    Ok(())
}

/// 确保 `download_settings` 的新列存在（老库迁移，幂等）：
/// `auto_save`（自动保存）、`concurrency`（并发数）、`speed_limit_kbps`（限速）。
fn ensure_settings_columns(conn: &Connection) -> Result<(), String> {
    let mut columns: Vec<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(download_settings)")
            .map_err(err_str)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(err_str)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err_str)?
    };
    let mut add = |name: &str, ddl: &str| -> Result<(), String> {
        if columns.iter().any(|column| column == name) {
            return Ok(());
        }
        conn.execute(ddl, []).map_err(err_str)?;
        columns.push(name.to_string());
        Ok(())
    };
    add(
        "auto_save",
        "ALTER TABLE download_settings ADD COLUMN auto_save INTEGER NOT NULL DEFAULT 0",
    )?;
    add(
        "concurrency",
        "ALTER TABLE download_settings ADD COLUMN concurrency INTEGER NOT NULL DEFAULT 1",
    )?;
    add(
        "speed_limit_kbps",
        "ALTER TABLE download_settings ADD COLUMN speed_limit_kbps INTEGER NOT NULL DEFAULT 0",
    )?;
    add(
        "auto_save_low_priority",
        "ALTER TABLE download_settings ADD COLUMN auto_save_low_priority INTEGER NOT NULL DEFAULT 1",
    )?;
    add(
        "auto_save_limit_kbps",
        "ALTER TABLE download_settings ADD COLUMN auto_save_limit_kbps INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}

/// 确保 `downloads` 的新列存在（开发库补列，幂等）：`priority`（任务优先级）、`line`（线路）。
fn ensure_download_columns(conn: &Connection) -> Result<(), String> {
    let mut columns: Vec<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(downloads)")
            .map_err(err_str)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(err_str)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err_str)?
    };
    let mut add = |name: &str, ddl: &str| -> Result<(), String> {
        if columns.iter().any(|column| column == name) {
            return Ok(());
        }
        conn.execute(ddl, []).map_err(err_str)?;
        columns.push(name.to_string());
        Ok(())
    };
    add(
        "priority",
        "ALTER TABLE downloads ADD COLUMN priority INTEGER NOT NULL DEFAULT 0",
    )?;
    add(
        "line",
        "ALTER TABLE downloads ADD COLUMN line TEXT NOT NULL DEFAULT ''",
    )?;
    Ok(())
}

fn query_list(conn: &Connection) -> Result<Vec<DownloadTask>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM downloads ORDER BY created_at DESC, rowid DESC"
        ))
        .map_err(err_str)?;
    let rows = stmt.query_map([], row_to_task).map_err(err_str)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
}

/// 从剧集标题里取集数：`第12集` / `12` / `EP12` / `12集` 都能识别出 12。
/// 用于「保存本集」这类没有 siteKey / epIndex 的老记录按集数兜底匹配。
fn episode_number(title: &str) -> Option<u32> {
    let digits: String = title
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadTask> {
    Ok(DownloadTask {
        id: row.get("id")?,
        site_key: row.get("site_key")?,
        site_name: row.get("site_name")?,
        line: row.get("line")?,
        vod_id: row.get("vod_id")?,
        ep_index: row.get("ep_index")?,
        name: row.get("name")?,
        ep_title: row.get("ep_title")?,
        cover: row.get("cover")?,
        status: row.get("status")?,
        progress: row.get("progress")?,
        speed_bps: row.get("speed_bps")?,
        downloaded_bytes: row.get("downloaded_bytes")?,
        size_bytes: row.get("total_bytes")?,
        file_path: row.get("file_path")?,
        kind: row.get("kind")?,
        priority: row.get("priority")?,
        error: row.get("error")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn err_str(err: rusqlite::Error) -> String {
    format!("SQLite 错误: {err}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_task_id() -> String {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{:x}-{:x}", now_ms(), seq)
}

/// 文件名安全化：替换 Windows/macOS 非法字符与控制字符，处理保留名与长度。
pub fn sanitize_file_name(name: &str) -> String {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut out = String::with_capacity(name.len());
    let mut last_invalid = false;
    for ch in name.chars() {
        let invalid =
            matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control();
        if invalid {
            if !last_invalid {
                out.push('_');
            }
            last_invalid = true;
        } else {
            out.push(ch);
            last_invalid = false;
        }
    }
    let trimmed = out.trim().trim_end_matches(['.', ' ']).trim();
    if trimmed.is_empty() {
        return "未命名".to_string();
    }
    let mut result = trimmed.to_string();
    let base = result
        .split('.')
        .next()
        .unwrap_or(&result)
        .to_ascii_uppercase();
    if RESERVED.contains(&base.as_str()) {
        result.insert(0, '_');
    }
    if result.chars().count() > 120 {
        result = result.chars().take(120).collect();
    }
    // 截断/加前缀后可能又停在 '.'/' ' 上：Windows 文件名不允许以空格或点结尾
    let result = result.trim_end_matches(['.', ' ']).trim().to_string();
    if result.is_empty() {
        "未命名".to_string()
    } else {
        result
    }
}

fn safe_component(name: &str, fallback: &str) -> String {
    let sanitized = sanitize_file_name(name);
    if sanitized == "未命名" && name.trim().is_empty() {
        sanitize_file_name(fallback)
    } else {
        sanitized
    }
}

/// 从 URL 的 path 末尾取扩展名（小写、1-5 位字母数字），拿不到返回 None。
fn url_extension(url: &str) -> Option<String> {
    let without = url.split(['?', '#']).next().unwrap_or(url);
    let last = without.rsplit('/').next()?;
    let (_, ext) = last.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        Some(ext)
    } else {
        None
    }
}

fn is_hls_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower
        .split(['?', '#'])
        .next()
        .unwrap_or(&lower)
        .ends_with(".m3u8")
}

fn is_mpegurl(response: &reqwest::blocking::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("mpegurl") || value.contains("vnd.apple.mpegurl")
        })
        .unwrap_or(false)
}

/// 解析 `Content-Range: bytes 1024-2047/4096` 里的总长度。
fn content_range_total(response: &reqwest::blocking::Response) -> Option<u64> {
    let value = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let total = value.rsplit('/').next()?.trim();
    total.parse::<u64>().ok().filter(|total| *total > 0)
}

fn percent(downloaded: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
    }
}

fn file_len(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
}

/// 合并后的输出扩展名：fMP4（有 init 段）固定 `.mp4`；
/// 普通 HLS 按首个分片的扩展名（.ts → merged.ts，.aac → merged.aac…）。
fn merged_extension(init_files: &[String], segment_files: &[String]) -> String {
    if !init_files.is_empty() {
        return "mp4".to_string();
    }
    segment_files
        .first()
        .and_then(|name| url_extension(name))
        .unwrap_or_else(|| "ts".to_string())
}

/// 把分片按顺序二进制拼接成单个可播放文件。
///
/// - `task_dir`：分片/init 所在目录（合并成功后可整体清理）；
/// - `output_path`：最终输出路径（在影片目录下，与分片目录同级）；
/// - 有 `#EXT-X-MAP` init 段：先写 init，再写各分片 → `.mp4`；
/// - 普通 TS HLS：扩展名取自分片名（通常是 `.ts`）；
/// - 先写入 `task_dir/merged.<ext>.part` 再重命名，失败时清理半成品并返回可读错误
///   （调用方回退 playlist 方案）。
fn merge_hls_files(
    task_dir: &Path,
    output_path: &Path,
    init_files: &[String],
    segment_files: &[String],
    segment_crypto: &[Option<(PathBuf, [u8; 16])>],
) -> Result<PathBuf, String> {
    if segment_files.is_empty() {
        return Err("没有可合并的分片".to_string());
    }
    let ext = output_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("ts");
    let part_path = task_dir.join(format!("merged.{ext}.part"));

    let write_result = (|| -> Result<u64, String> {
        let mut output = File::create(&part_path).map_err(|e| format!("创建合并文件失败: {e}"))?;
        let mut total: u64 = 0;
        for name in init_files {
            let path = task_dir.join(name);
            let mut input =
                File::open(&path).map_err(|e| format!("打开分片失败({}): {e}", path.display()))?;
            let copied = std::io::copy(&mut input, &mut output)
                .map_err(|e| format!("写入合并文件失败({}): {e}", path.display()))?;
            total = total.saturating_add(copied);
        }
        for (index, name) in segment_files.iter().enumerate() {
            let path = task_dir.join(name);
            let crypto = segment_crypto.get(index).and_then(|slot| slot.as_ref());
            match crypto {
                Some((key_path, iv)) => {
                    let plain = crate::hls_util::decrypt_file(&path, key_path, iv)?;
                    output
                        .write_all(&plain)
                        .map_err(|e| format!("写入合并文件失败({}): {e}", path.display()))?;
                    total = total.saturating_add(plain.len() as u64);
                }
                None => {
                    let mut input = File::open(&path)
                        .map_err(|e| format!("打开分片失败({}): {e}", path.display()))?;
                    let copied = std::io::copy(&mut input, &mut output)
                        .map_err(|e| format!("写入合并文件失败({}): {e}", path.display()))?;
                    total = total.saturating_add(copied);
                }
            }
        }
        output
            .flush()
            .map_err(|e| format!("刷新合并文件失败: {e}"))?;
        Ok(total)
    })();

    match write_result {
        Ok(_) => {
            // Windows rename 不覆盖已有文件
            let _ = fs::remove_file(output_path);
            fs::rename(&part_path, output_path).map_err(|e| {
                let _ = fs::remove_file(&part_path);
                format!(
                    "合并文件重命名失败({} -> {}): {e}",
                    part_path.display(),
                    output_path.display()
                )
            })?;
            Ok(output_path.to_path_buf())
        }
        Err(err) => {
            let _ = fs::remove_file(&part_path);
            Err(err)
        }
    }
}

/// 合并成功后清理本集分片目录里的 playlist 与分片（失败忽略）。
fn cleanup_hls_fragments(task_dir: &Path, keep: &Path) {
    let _ = fs::remove_file(task_dir.join("playlist.m3u8"));
    let Ok(entries) = fs::read_dir(task_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep || !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "playlist.m3u8"
            || name == "sample_aes.m3u8"
            || name.starts_with("seg_")
            || name.starts_with("init_")
            || name.starts_with("key_")
            || name.starts_with("res_")
            || (name.starts_with("merged.") && name.ends_with(".part"))
        {
            let _ = fs::remove_file(&path);
        }
    }
    // 分片目录里只剩中间产物（含旧版本残留的 merged.*）：整体删掉，输出文件在上一级影片目录
    let _ = fs::remove_dir_all(task_dir);
}

/// 拼接 URL（支持相对路径与绝对 URL）。
fn join_url(base: &reqwest::Url, reference: &str) -> Result<String, String> {
    base.join(reference.trim())
        .map(|url| url.to_string())
        .map_err(|e| e.to_string())
}

/// 解析 `#EXT-X-STREAM-INF`，返回 BANDWIDTH 最高的子播放列表 URI。
fn pick_variant(playlist: &str) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    let mut pending: Option<u64> = None;
    for raw in playlist.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(parse_bandwidth(attrs).unwrap_or(0));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(bandwidth) = pending.take() {
            let replace = match &best {
                Some((current, _)) => bandwidth > *current,
                None => true,
            };
            if replace {
                best = Some((bandwidth, line.to_string()));
            }
        }
    }
    best.map(|(_, uri)| uri)
}

fn parse_bandwidth(attrs: &str) -> Option<u64> {
    for part in attrs.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("BANDWIDTH=") {
            return value.trim().parse::<u64>().ok();
        }
    }
    None
}

/// 提取 `URI="..."`（兼容单引号/无引号写法）。
fn extract_uri_attr(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("URI=")?;
    let rest = rest.trim_start();
    if let Some(rest) = rest.strip_prefix('"') {
        return rest.split('"').next().map(|value| value.to_string());
    }
    if let Some(rest) = rest.strip_prefix('\'') {
        return rest.split('\'').next().map(|value| value.to_string());
    }
    Some(rest.split(',').next().unwrap_or(rest).trim().to_string())
}

/// 把一行的 `URI=` 属性替换成本地文件名。
fn replace_uri_attr(line: &str, local_name: &str) -> String {
    let Some((prefix, rest)) = line.split_once("URI=") else {
        return line.to_string();
    };
    let rest = rest.trim_start();
    let (quote, content_start) = if let Some(content) = rest.strip_prefix('"') {
        ('"', content)
    } else if let Some(content) = rest.strip_prefix('\'') {
        ('\'', content)
    } else {
        return line.to_string();
    };
    let Some((_, tail)) = content_start.split_once(quote) else {
        return line.to_string();
    };
    format!("{prefix}URI={quote}{local_name}{quote}{tail}")
}

/* ---------------- Tauri 命令 ---------------- */

/// 任务列表（按创建时间倒序）。
#[tauri::command]
pub fn download_list(store: State<'_, DownloadStore>) -> Result<Vec<DownloadTask>, String> {
    store.list()
}

/// 新建下载任务（立即返回 queued 任务，后台线程随后开始下载）。
#[tauri::command]
pub fn download_start(
    app: AppHandle,
    store: State<'_, DownloadStore>,
    params: DownloadStart,
) -> Result<DownloadTask, String> {
    let task = store.create_task(params)?;
    store.start_worker(app);
    Ok(task)
}

/// 暂停 / 续传 / 取消 / 重试，返回最新列表。
#[tauri::command]
pub fn download_control(
    app: AppHandle,
    store: State<'_, DownloadStore>,
    id: String,
    action: String,
) -> Result<Vec<DownloadTask>, String> {
    let list = store.control(&id, &action)?;
    // 状态变化立即推给前端（cancel 后任务已不存在，get_task 返回 None）
    if let Ok(Some(task)) = store.get_task(&id) {
        store.inner.emit(&task, true);
    }
    store.start_worker(app);
    Ok(list)
}

/// 删除任务记录（`remove_file=true` 时同时删除已下载文件）。
#[tauri::command]
pub fn download_remove(
    app: AppHandle,
    store: State<'_, DownloadStore>,
    id: String,
    remove_file: bool,
) -> Result<Vec<DownloadTask>, String> {
    let list = store.remove(&id, remove_file)?;
    // 若队列里还有任务，确保工作线程活着（重启后首次操作也能恢复）
    store.start_worker(app);
    Ok(list)
}

/// 读取下载目录设置。
#[tauri::command]
pub fn download_settings(store: State<'_, DownloadStore>) -> Result<DownloadSettings, String> {
    store.settings()
}

/// 保存下载目录（创建目录并持久化），返回新设置。
#[tauri::command]
pub fn download_set_dir(
    store: State<'_, DownloadStore>,
    dir: String,
) -> Result<DownloadSettings, String> {
    store.set_dir(&dir)
}

/// 保存「播放时自动保存」开关，返回最新设置。
#[tauri::command]
pub fn download_set_auto_save(
    store: State<'_, DownloadStore>,
    enabled: bool,
) -> Result<DownloadSettings, String> {
    store.set_auto_save(enabled)
}

/// 保存下载并发数 / 全局限速（KB/s，0=不限），返回最新设置。
#[tauri::command]
pub fn download_set_network(
    store: State<'_, DownloadStore>,
    concurrency: u32,
    speed_limit_kbps: u32,
) -> Result<DownloadSettings, String> {
    store.set_network(concurrency, speed_limit_kbps)
}

/// 自动保存任务策略：低优先级让路 / 单独限速（KB/s，0=不额外限制）。
#[tauri::command]
pub fn download_set_auto_save_policy(
    store: State<'_, DownloadStore>,
    low_priority: bool,
    auto_save_limit_kbps: u32,
) -> Result<DownloadSettings, String> {
    store.set_auto_save_policy(low_priority, auto_save_limit_kbps)
}

/// 原生「选择文件夹」对话框（后台线程执行，避免阻塞主线程）。
#[tauri::command]
pub async fn download_pick_dir() -> Option<String> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("选择下载目录")
            .pick_folder()
    })
    .await
    .ok()
    .flatten();
    picked.map(|path| path.to_string_lossy().to_string())
}

/// 打开当前下载目录（失败静默）。
#[tauri::command]
pub fn download_open_dir(store: State<'_, DownloadStore>) -> Result<(), String> {
    store.open_dir();
    Ok(())
}

/// 打开指定任务文件所在位置（资源管理器/Finder 中选中该文件）。
#[tauri::command]
pub fn download_open_file_location(
    store: State<'_, DownloadStore>,
    id: String,
) -> Result<(), String> {
    store.open_file_location(&id)
}

/// 查询某集是否已有可用的本地成品（completed 且文件仍存在）。
///
/// 详情页 / 历史续播起播前先问这里：有本地文件就直接播放，跳过在线解析与 HLS 缓存，
/// 避免已下载的集「从历史记录点进来又开始新缓存」。没有返回 null（前端回退在线解析）。
#[tauri::command]
pub fn download_local_file(
    store: State<'_, DownloadStore>,
    info: SavedFileInfo,
) -> Option<DownloadTask> {
    store.find_completed_file_for(&info)
}

/// 下载空间总览（目录/缓存/.part/已完成占用 + 磁盘容量）。
#[tauri::command]
pub fn download_storage_status(
    store: State<'_, DownloadStore>,
) -> Result<DownloadStorageStatus, String> {
    store.storage_status()
}

/// 清理 `.part` 残留（不动正常文件与播放缓存）。
#[tauri::command]
pub fn download_cleanup_parts(
    store: State<'_, DownloadStore>,
) -> Result<DownloadCleanupResult, String> {
    store.cleanup_parts()
}

/// 清理已完成下载（删除文件 + 记录）。
#[tauri::command]
pub fn download_cleanup_completed(
    store: State<'_, DownloadStore>,
) -> Result<DownloadCleanupResult, String> {
    store.cleanup_completed()
}

/* ---------------- 单元测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tiny_http::{Header, Response, Server, StatusCode};

    static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "vbox-download-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            seq
        ));
        let dir = dir.join(tag);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 内存库 store（与正式库同一套 PRAGMA/建表逻辑）。
    fn test_store(media_dir: &Path) -> DownloadStore {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        DownloadStore::with_connection(conn, media_dir.to_path_buf())
    }

    /// 边看边存导出的文件登记为 completed 记录：下载页可见、字段一致、可被列表读到。
    #[test]
    fn register_saved_local_file_as_completed_task() {
        let dir = temp_dir("saved-register");
        let store = test_store(&dir);
        let file = dir.join("测试剧集 第1集.ts");
        fs::write(&file, b"0123456789").expect("写测试文件失败");

        let info = SavedFileInfo {
            site_key: "site-a".to_string(),
            site_name: Some("测试站点".to_string()),
            line: "hhyun".to_string(),
            vod_id: Some("vod-1".to_string()),
            ep_index: 2,
            name: "测试剧集".to_string(),
            ep_title: "第1集".to_string(),
            cover: Some("https://example.com/cover.jpg".to_string()),
        };
        let task = store
            .add_completed_local_file(&info, &file)
            .expect("登记保存文件失败");
        assert!(task.id.starts_with("saved-"), "{}", task.id);
        assert_eq!(task.status, STATUS_COMPLETED);
        assert_eq!(task.site_key, "site-a");
        assert_eq!(task.vod_id.as_deref(), Some("vod-1"));
        assert_eq!(task.ep_index, 2);
        assert_eq!(task.name, "测试剧集");
        assert_eq!(task.ep_title, "第1集");
        assert_eq!(task.size_bytes, 10);
        assert_eq!(task.downloaded_bytes, 10);
        assert_eq!(task.progress, 100.0);
        assert_eq!(task.kind, KIND_FILE);
        assert_eq!(task.cover.as_deref(), Some("https://example.com/cover.jpg"));
        assert_eq!(
            task.file_path.as_deref(),
            Some(file.to_string_lossy().as_ref())
        );

        let list = store.list().expect("读取列表失败");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, task.id);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 历史 / 详情起播查本地成品：completed 且文件存在才命中；未完成、别的集、文件被删都不命中。
    #[test]
    fn find_completed_local_file_for_episode() {
        let dir = temp_dir("find-completed");
        let store = test_store(&dir);
        let file = dir.join("折月亮 第1集.ts");
        fs::write(&file, b"0123456789").expect("写测试文件失败");
        let info = SavedFileInfo {
            site_key: "site-a".to_string(),
            site_name: Some("测试站点".to_string()),
            line: "hhyun".to_string(),
            vod_id: Some("vod-1".to_string()),
            ep_index: 0,
            name: "折月亮".to_string(),
            ep_title: "第1集".to_string(),
            cover: None,
        };
        assert!(
            store.find_completed_file_for(&info).is_none(),
            "空库不应命中本地成品"
        );

        let task = store.add_completed_local_file(&info, &file).unwrap();
        let found = store
            .find_completed_file_for(&info)
            .expect("已完成且文件存在时应命中");
        assert_eq!(found.id, task.id);
        assert_eq!(
            found.file_path.as_deref(),
            Some(file.to_string_lossy().as_ref())
        );
        assert!(
            store.has_completed_file_for(&info),
            "has_completed 应一致命中"
        );

        // 不同线路不能串用成品：即使片名 / 集标题相同也不命中
        let mut other_line = info.clone();
        other_line.line = "hhm3u8".to_string();
        assert!(
            store.find_completed_file_for(&other_line).is_none(),
            "别的线路不应命中本线路的成品"
        );

        // 别的影片（片名 / vodId 都不同）不命中
        let mut other_vod = info.clone();
        other_vod.vod_id = Some("vod-2".to_string());
        other_vod.name = "另一部剧".to_string();
        assert!(store.find_completed_file_for(&other_vod).is_none());

        // 「保存本集」老记录没有 siteKey / vodId，标题还可能是「第01集」：按影片名 + 集数兜底
        let legacy_info = SavedFileInfo {
            name: "旧剧".to_string(),
            ep_title: "第01集".to_string(),
            ..Default::default()
        };
        let legacy_file = dir.join("旧剧 第01集.ts");
        fs::write(&legacy_file, b"legacy").unwrap();
        let legacy = store
            .add_completed_local_file(&legacy_info, &legacy_file)
            .unwrap();
        let lookup = SavedFileInfo {
            site_key: "site-b".to_string(),
            vod_id: Some("vod-legacy".to_string()),
            ep_index: 0,
            name: "旧剧".to_string(),
            ep_title: "第1集".to_string(),
            ..Default::default()
        };
        let legacy_found = store
            .find_completed_file_for(&lookup)
            .expect("老记录无 siteKey 且标题格式不同时应按集数兜底");
        assert_eq!(legacy_found.id, legacy.id);

        // 文件被手动删除：记录还在也不命中（前端回退在线解析）
        fs::remove_file(&file).unwrap();
        assert!(
            store.find_completed_file_for(&info).is_none(),
            "文件不存在时不应命中"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 集数归一化：常见标题格式都能取到同一个集数，非剧集标题返回 None。
    #[test]
    fn episode_number_parses_common_titles() {
        assert_eq!(episode_number("第1集"), Some(1));
        assert_eq!(episode_number("第01集"), Some(1));
        assert_eq!(episode_number("EP12"), Some(12));
        assert_eq!(episode_number("12集 高清"), Some(12));
        assert_eq!(episode_number("正片"), None);
        assert_eq!(episode_number(""), None);
    }

    /// 「边下边播」自动保存去重：同集已有排队/下载中的下载任务时不应再导出（避免双份流量）。
    #[test]
    fn active_download_dedupes_auto_save_export() {
        let dir = temp_dir("dedupe-autosave");
        let store = test_store(&dir);
        let info = SavedFileInfo {
            site_key: "site-a".to_string(),
            site_name: Some("测试站点".to_string()),
            line: "hhyun".to_string(),
            vod_id: Some("vod-1".to_string()),
            ep_index: 0,
            name: "测试影片".to_string(),
            ep_title: "第1集".to_string(),
            cover: None,
        };
        assert!(!store.has_active_task_for(&info), "空库不应判定有活动任务");

        let task = store
            .create_task(start_payload("http://127.0.0.1:1/x.m3u8"))
            .expect("创建下载任务失败");
        assert!(
            store.has_active_task_for(&info),
            "同集下载中应触发自动保存去重"
        );

        // 不同集 / 不同影片不误判
        let other_ep = SavedFileInfo {
            ep_index: 1,
            ..info.clone()
        };
        assert!(!store.has_active_task_for(&other_ep));
        let other_vod = SavedFileInfo {
            vod_id: Some("vod-2".to_string()),
            ..info.clone()
        };
        assert!(!store.has_active_task_for(&other_vod));
        let other_line = SavedFileInfo {
            line: "hhm3u8".to_string(),
            ..info.clone()
        };
        assert!(!store.has_active_task_for(&other_line));

        // 删除任务后不再算活动任务
        store.remove(&task.id, false).expect("删除任务失败");
        assert!(!store.has_active_task_for(&info));
        let _ = fs::remove_dir_all(&dir);
    }

    fn start_payload(url: &str) -> DownloadStart {
        DownloadStart {
            site_key: "site-a".to_string(),
            site_name: Some("测试站点".to_string()),
            line: "hhyun".to_string(),
            vod_id: Some("vod-1".to_string()),
            ep_index: Some(0),
            name: "测试影片".to_string(),
            ep_title: "第1集".to_string(),
            cover: None,
            url: url.to_string(),
            headers: None,
            priority: None,
        }
    }

    /// 极简 HTTP 服务器：支持 Range（206），固定路径响应由闭包决定。
    fn spawn_server(
        route: impl Fn(&str) -> Option<(Vec<u8>, &'static str)> + Send + Sync + 'static,
    ) -> String {
        let server = Server::http("127.0.0.1:0").expect("启动本地 HTTP server 失败");
        let addr = server.server_addr().to_ip().expect("读取监听地址失败");
        let route = Arc::new(route);
        std::thread::spawn(move || {
            for request in server.incoming_requests().take(64) {
                let path = request.url().to_string();
                let Some((data, content_type)) = route(&path) else {
                    let _ =
                        request.respond(Response::from_string("not found").with_status_code(404));
                    continue;
                };
                let range = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Range"))
                    .and_then(|header| parse_range(header.value.as_str()));
                let (status, start, end) = match range {
                    Some(start) if start < data.len() => (206u16, start, data.len()),
                    Some(_) => {
                        let _ = request.respond(
                            Response::from_string("range not satisfiable")
                                .with_status_code(416)
                                .with_header(header(
                                    "Content-Range",
                                    &format!("bytes */{}", data.len()),
                                )),
                        );
                        continue;
                    }
                    None => (200u16, 0, data.len()),
                };
                let body = data[start..end].to_vec();
                let mut response = Response::from_data(body)
                    .with_status_code(status)
                    .with_header(header("Content-Type", content_type))
                    .with_header(header("Accept-Ranges", "bytes"));
                if status == 206 {
                    response = response.with_header(header(
                        "Content-Range",
                        &format!("bytes {}-{}/{}", start, data.len() - 1, data.len()),
                    ));
                }
                let _ = request.respond(response);
            }
        });
        format!("http://{addr}")
    }

    fn header(name: &str, value: &str) -> Header {
        Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("构造响应头失败")
    }

    /// 慢速 reader：把数据按 chunk 分片、每片之间 sleep，用于测试「下载中暂停/续传」。
    struct SlowReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
        delay: Duration,
    }

    impl Read for SlowReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let take = self.chunk.min(buf.len()).min(self.data.len() - self.pos);
            buf[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            std::thread::sleep(self.delay);
            Ok(take)
        }
    }

    /// 极简 HTTP 服务器：按 Range 返回 206，body 分片慢速发送（模拟真实网络）。
    fn spawn_slow_server(data: Vec<u8>, chunk: usize, delay_ms: u64) -> String {
        let server = Server::http("127.0.0.1:0").expect("启动本地 HTTP server 失败");
        let addr = server.server_addr().to_ip().expect("读取监听地址失败");
        std::thread::spawn(move || {
            for request in server.incoming_requests().take(64) {
                let range = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Range"))
                    .and_then(|header| parse_range(header.value.as_str()));
                let (status, start) = match range {
                    Some(start) if start < data.len() => (StatusCode(206), start),
                    Some(_) => {
                        let _ = request.respond(
                            Response::from_string("range not satisfiable").with_status_code(416),
                        );
                        continue;
                    }
                    None => (StatusCode(200), 0),
                };
                let body = data[start..].to_vec();
                let length = body.len();
                let mut headers = vec![
                    header("Content-Type", "video/mp4"),
                    header("Accept-Ranges", "bytes"),
                ];
                if status.0 == 206 {
                    headers.push(header(
                        "Content-Range",
                        &format!("bytes {}-{}/{}", start, data.len() - 1, data.len()),
                    ));
                }
                let response = Response::new(
                    status,
                    headers,
                    SlowReader {
                        data: body,
                        pos: 0,
                        chunk,
                        delay: Duration::from_millis(delay_ms),
                    },
                    Some(length),
                    None,
                );
                let _ = request.respond(response);
            }
        });
        format!("http://{addr}")
    }

    fn parse_range(value: &str) -> Option<usize> {
        let value = value.trim().strip_prefix("bytes=")?;
        let start = value.split('-').next()?.trim();
        start.parse::<usize>().ok()
    }

    fn sample_bytes() -> Vec<u8> {
        (0..256 * 1024).map(|i| (i % 251) as u8).collect()
    }

    /// 文件名安全化。
    #[test]
    fn download_sanitize_file_name() {
        assert_eq!(
            sanitize_file_name("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_file_name("   "), "未命名");
        assert_eq!(sanitize_file_name("CON"), "_CON");
        assert_eq!(sanitize_file_name("正常片名 第1集"), "正常片名 第1集");
        assert_eq!(sanitize_file_name(".."), "未命名");
        assert!(!sanitize_file_name("x".repeat(500).as_str()).is_empty());
    }

    /// 默认目录：文档/VBox；无文档目录时回退 <app_data>/downloads。
    #[test]
    fn download_default_dir_fallback() {
        let root = temp_dir("defaults");
        let data = root.join("data");
        let docs = root.join("docs");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&docs).unwrap();

        let store = DownloadStore::new(&data, Some(docs.clone()));
        let settings = store.settings().unwrap();
        assert_eq!(
            settings.default_dir,
            docs.join("VBox").to_string_lossy().to_string()
        );
        assert_eq!(settings.dir, settings.default_dir);
        assert!(docs.join("VBox").is_dir(), "默认目录应被创建");
        assert_eq!(store.cache_dir(), docs.join("VBox").join("cache"));

        let data2 = root.join("data2");
        fs::create_dir_all(&data2).unwrap();
        let store = DownloadStore::new(&data2, None);
        let settings = store.settings().unwrap();
        assert_eq!(
            settings.default_dir,
            data2.join("downloads").to_string_lossy().to_string()
        );
        assert!(data2.join("downloads").is_dir());
    }

    /// 前端契约：DownloadTask / DownloadSettings 的 JSON 字段必须是 camelCase。
    #[test]
    fn download_contract_serializes_camel_case() {
        let root = temp_dir("contract");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload("http://127.0.0.1:1/movie.mp4"))
            .unwrap();
        let value = serde_json::to_value(&task).unwrap();
        for key in [
            "id",
            "siteKey",
            "siteName",
            "vodId",
            "epIndex",
            "name",
            "epTitle",
            "cover",
            "status",
            "progress",
            "speedBps",
            "downloadedBytes",
            "sizeBytes",
            "filePath",
            "kind",
            "error",
            "createdAt",
            "updatedAt",
        ] {
            assert!(value.get(key).is_some(), "JSON 缺少字段 {key}: {value}");
        }
        assert!(
            value.get("size_bytes").is_none(),
            "不应出现 snake_case 字段"
        );
        let settings = serde_json::to_value(store.settings().unwrap()).unwrap();
        assert!(settings.get("dir").is_some());
        assert!(settings.get("defaultDir").is_some());
        assert_eq!(
            settings.get("autoSave"),
            Some(&serde_json::Value::Bool(false))
        );
    }

    /// 任务入库/列表/暂停/续传/重试/删除状态迁移。
    #[test]
    fn download_task_lifecycle_and_settings() {
        let root = temp_dir("lifecycle");
        let store = test_store(&root.join("media"));
        let task = store
            .create_task(start_payload("http://127.0.0.1:1/none.mp4"))
            .unwrap();
        assert_eq!(task.status, STATUS_QUEUED);
        assert_eq!(task.site_name.as_deref(), Some("测试站点"));
        assert_eq!(task.kind, KIND_FILE);

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, task.id);

        // pause → paused
        let list = store.control(&task.id, "pause").unwrap();
        assert_eq!(list[0].status, STATUS_PAUSED);
        // resume → queued
        let list = store.control(&task.id, "resume").unwrap();
        assert_eq!(list[0].status, STATUS_QUEUED);
        // 模拟失败后 retry → queued 且清 error
        store
            .inner
            .set_status(&task.id, STATUS_FAILED, Some("测试失败"))
            .unwrap();
        let list = store.control(&task.id, "retry").unwrap();
        assert_eq!(list[0].status, STATUS_QUEUED);
        assert_eq!(list[0].error, None);
        // 未知 action 报错
        assert!(store.control(&task.id, "explode").is_err());

        // 目录设置持久化
        let custom = root.join("custom");
        let settings = store.set_dir(custom.to_string_lossy().as_ref()).unwrap();
        assert_eq!(settings.dir, custom.to_string_lossy().to_string());
        assert!(custom.is_dir());
        assert_eq!(store.media_dir(), custom);

        // cancel → 记录删除
        assert!(store.control(&task.id, "cancel").unwrap().is_empty());
        assert!(store.list().unwrap().is_empty());
    }

    /// 直链完整下载（无续传）。
    #[test]
    fn download_file_completes() {
        let data = sample_bytes();
        let payload = data.clone();
        let base = spawn_server(move |_path| Some((payload.clone(), "video/mp4")));
        let root = temp_dir("file");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/movie.mp4")))
            .unwrap();

        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        assert_eq!(task.progress, 100.0);
        assert_eq!(task.downloaded_bytes, data.len() as i64);
        assert_eq!(task.size_bytes, data.len() as i64);
        assert_eq!(task.kind, KIND_FILE);

        let path = PathBuf::from(task.file_path.as_deref().unwrap());
        assert!(path.exists(), "完成的文件应存在");
        // 落盘结构：<下载目录>/<站点>/<剧名>/<线路>/<集名>.<ext>
        assert!(
            path.ends_with(Path::new("测试站点/测试影片/hhyun/第1集.mp4")),
            "四级路径不对: {}",
            path.display()
        );
        assert_eq!(fs::read(&path).unwrap(), data);
    }

    /// 断点续传：已有 .part 时用 Range 追加，内容不重复。
    #[test]
    fn download_file_resume_with_range() {
        let data = sample_bytes();
        let payload = data.clone();
        let base = spawn_server(move |_path| Some((payload.clone(), "video/mp4")));
        let root = temp_dir("resume");
        let store = test_store(&root);
        let url = format!("{base}/movie.mp4");
        let task = store.create_task(start_payload(&url)).unwrap();

        // 预先写入前 100KB 的 .part（模拟上次暂停）
        let dir = root.join("测试站点").join("测试影片").join("hhyun");
        fs::create_dir_all(&dir).unwrap();
        let part = dir.join("第1集.mp4.part");
        fs::write(&part, &data[..100 * 1024]).unwrap();

        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        assert_eq!(task.downloaded_bytes, data.len() as i64);
        let path = PathBuf::from(task.file_path.as_deref().unwrap());
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(!part.exists(), "完成后 .part 应被 rename 走");
    }

    /// HLS：master 选最高码率、分片合并为 merged.ts（playlist/分片被清理）。
    #[test]
    fn download_hls_segments_and_playlist_rewrite() {
        let seg1 = b"segment-one-data".to_vec();
        let seg2 = b"segment-two-data".to_vec();
        let expected_seg1 = seg1.clone();
        let expected_seg2 = seg2.clone();
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1920x1080\nhigh.m3u8\n".to_vec();
        let high_media = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n".to_vec();
        // 低码率子列表指向不存在的分片：如果选错码率，任务会失败（测试即可发现）
        let low_media = b"#EXTM3U\n#EXTINF:4.0,\nlowseg.ts\n#EXT-X-ENDLIST\n".to_vec();
        let high = high_media.clone();
        let base = spawn_server(move |path| match path.split('?').next().unwrap_or(path) {
            "/master.m3u8" => Some((master.clone(), "application/vnd.apple.mpegurl")),
            "/high.m3u8" => Some((high.clone(), "application/vnd.apple.mpegurl")),
            "/low.m3u8" => Some((low_media.clone(), "application/vnd.apple.mpegurl")),
            "/seg1.ts" => Some((seg1.clone(), "video/mp2t")),
            "/seg2.ts" => Some((seg2.clone(), "video/mp2t")),
            _ => None,
        });
        let root = temp_dir("hls");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/master.m3u8")))
            .unwrap();
        assert_eq!(task.kind, KIND_HLS);

        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        // 合并成功后 kind 改为 file，file_path 指向合并文件
        assert_eq!(task.kind, KIND_FILE);
        assert_eq!(task.progress, 100.0);

        let merged = PathBuf::from(task.file_path.as_deref().unwrap());
        assert!(
            merged.ends_with(Path::new("测试站点/测试影片/hhyun/第1集.ts")),
            "合并文件路径不对: {}",
            merged.display()
        );
        let expected = [&expected_seg1[..], &expected_seg2[..]].concat();
        assert_eq!(fs::read(&merged).unwrap(), expected);
        assert_eq!(task.size_bytes, expected.len() as i64);
        assert_eq!(task.downloaded_bytes, expected.len() as i64);

        // 分片目录被整体清理，只保留影片目录下的合并文件
        let movie_dir = merged.parent().unwrap();
        assert!(!movie_dir.join("第1集").exists(), "分片中间目录应被删除");
        assert!(
            !movie_dir.join("playlist.m3u8").exists(),
            "合并后 playlist 应被清理"
        );
        let leftovers: Vec<String> = fs::read_dir(movie_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(leftovers, vec!["第1集.ts".to_string()]);
    }

    /// HLS fMP4（含 #EXT-X-MAP）：merged.mp4 = init + 分片顺序拼接。
    #[test]
    fn download_hls_fmp4_merges_init_and_segments() {
        let init = b"ftyp-init-header".to_vec();
        let seg1 = b"fmp4-segment-one".to_vec();
        let seg2 = b"fmp4-segment-two".to_vec();
        let expected = [&init[..], &seg1[..], &seg2[..]].concat();
        let media = b"#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg1.m4s\n#EXTINF:4.0,\nseg2.m4s\n#EXT-X-ENDLIST\n".to_vec();
        let base = spawn_server(move |path| match path.split('?').next().unwrap_or(path) {
            "/media.m3u8" => Some((media.clone(), "application/vnd.apple.mpegurl")),
            "/init.mp4" => Some((init.clone(), "video/mp4")),
            "/seg1.m4s" => Some((seg1.clone(), "video/iso.segment")),
            "/seg2.m4s" => Some((seg2.clone(), "video/iso.segment")),
            _ => None,
        });
        let root = temp_dir("hls-fmp4");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/media.m3u8")))
            .unwrap();
        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        assert_eq!(task.kind, KIND_FILE);

        let merged = PathBuf::from(task.file_path.as_deref().unwrap());
        assert!(
            merged.ends_with(Path::new("测试站点/测试影片/hhyun/第1集.mp4")),
            "fMP4 应合并为 <集名>.mp4: {}",
            merged.display()
        );
        assert_eq!(fs::read(&merged).unwrap(), expected);
        assert_eq!(task.size_bytes, expected.len() as i64);

        let movie_dir = merged.parent().unwrap();
        assert!(!movie_dir.join("第1集").exists(), "分片中间目录应被删除");
        assert!(!movie_dir.join("playlist.m3u8").exists());
        assert!(!movie_dir.join("init_00001.mp4").exists());
        assert!(!movie_dir.join("seg_00001.m4s").exists());
        assert!(!movie_dir.join("seg_00002.m4s").exists());
    }

    /// AES-128 HLS：下载后离线解密并合并为明文文件。
    #[test]
    fn download_hls_decrypts_aes128() {
        let key = [9u8; 16];
        let iv = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let seg1 = crate::hls_util::encrypt_for_test(b"AAAASEGONE", &key, &iv);
        let seg2 = crate::hls_util::encrypt_for_test(b"BBBSEGTWO", &key, &iv);
        let media = b"#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n".to_vec();
        let base = spawn_server(move |path| match path.split('?').next().unwrap_or(path) {
            "/media.m3u8" => Some((media.clone(), "application/vnd.apple.mpegurl")),
            "/key.bin" => Some((key.to_vec(), "application/octet-stream")),
            "/seg1.ts" => Some((seg1.clone(), "video/mp2t")),
            "/seg2.ts" => Some((seg2.clone(), "video/mp2t")),
            _ => None,
        });
        let root = temp_dir("hls-aes");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/media.m3u8")))
            .unwrap();
        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        let merged = std::fs::read(task.file_path.as_deref().unwrap()).expect("读取合并文件失败");
        assert_eq!(merged, b"AAAASEGONEBBBSEGTWO");
    }

    /// BYTERANGE：同一资源的不同区间切片下载后合并为完整文件。
    #[test]
    fn download_hls_byterange_slices_and_merges() {
        let media = b"#EXTM3U\n#EXT-X-VERSION:4\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nall.bin\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10\nall.bin\n#EXT-X-ENDLIST\n".to_vec();
        let resource = b"0123456789ABCDEFGHIJ".to_vec();
        let base = spawn_server(move |path| match path.split('?').next().unwrap_or(path) {
            "/media.m3u8" => Some((media.clone(), "application/vnd.apple.mpegurl")),
            "/all.bin" => Some((resource.clone(), "video/mp2t")),
            _ => None,
        });
        let root = temp_dir("hls-range");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/media.m3u8")))
            .unwrap();
        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        let merged = std::fs::read(task.file_path.as_deref().unwrap()).expect("读取合并文件失败");
        assert_eq!(merged, b"0123456789ABCDEFGHIJ");
    }

    /// SAMPLE-AES + DRM（skd://）明确失败，并给出可读原因。
    #[test]
    fn download_hls_rejects_drm_sample_aes() {
        let media = b"#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://key\"\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n".to_vec();
        let base =
            spawn_server(move |_path| Some((media.clone(), "application/vnd.apple.mpegurl")));
        let root = temp_dir("hls-drm");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/media.m3u8")))
            .unwrap();
        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_FAILED);
        assert!(
            task.error.as_deref().unwrap_or("").contains("DRM"),
            "错误文案应说明 DRM 不支持: {:?}",
            task.error
        );
    }

    /// 纯函数校验：DRM / SAMPLE-AES + fMP4 在下载前就被拒绝，明文与可解密组合放行。
    #[test]
    fn hls_downloadable_guard_covers_drm_and_fmp4() {
        fn crypto(
            sample_aes: bool,
            init: Option<&str>,
            unsupported: Option<&str>,
        ) -> crate::hls_util::MediaCrypto {
            crate::hls_util::MediaCrypto {
                init: init.map(str::to_string),
                segments: vec!["https://cdn.example.com/seg1.ts".to_string()],
                keys: vec![None],
                ranges: vec![None],
                durations: vec![4.0],
                media_sequence: 0,
                unsupported: unsupported.map(str::to_string),
                sample_aes,
                has_byterange: false,
            }
        }

        let drm = crypto(false, None, Some("SAMPLE-AES + DRM（KEYFORMAT=skd）"));
        let err = ensure_hls_downloadable(&drm).expect_err("DRM 必须被拒绝");
        assert!(err.contains("DRM"), "{err}");

        let fmp4 = crypto(true, Some("https://cdn.example.com/init.mp4"), None);
        let err = ensure_hls_downloadable(&fmp4).expect_err("SAMPLE-AES + fMP4 必须被拒绝");
        assert!(err.contains("fMP4"), "{err}");

        // SAMPLE-AES + TS 可交给 ffmpeg；明文/纯 AES-128 一直支持
        assert!(ensure_hls_downloadable(&crypto(true, None, None)).is_ok());
        assert!(ensure_hls_downloadable(&crypto(false, None, None)).is_ok());
    }

    /// 直链但 Content-Type 是 mpegurl 时自动转 HLS。
    #[test]
    fn download_detects_hls_by_content_type() {
        let media = b"#EXTM3U\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n".to_vec();
        let seg = b"segment-data".to_vec();
        let base = spawn_server(move |path| match path.split('?').next().unwrap_or(path) {
            "/stream" => Some((media.clone(), "application/x-mpegURL")),
            "/seg1.ts" => Some((seg.clone(), "video/mp2t")),
            _ => None,
        });
        let root = temp_dir("hls-ct");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/stream")))
            .unwrap();
        assert_eq!(task.kind, KIND_FILE);
        assert!(store.inner.run_next_once().unwrap());
        let task = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, STATUS_COMPLETED, "错误: {:?}", task.error);
        // HLS 下载完成后合并为单个文件，kind 变为 file；输出在影片目录下（名称为集名）
        assert_eq!(task.kind, KIND_FILE);
        let path = task.file_path.as_deref().unwrap_or("");
        assert!(
            Path::new(path).ends_with(Path::new("测试站点/测试影片/hhyun/第1集.ts")),
            "合并文件路径不对: {path}"
        );
    }

    /// autoSave：默认 false，保存后可读回，且与目录设置互不影响。
    #[test]
    fn download_auto_save_setting() {
        let root = temp_dir("auto-save");
        let store = test_store(&root);
        let settings = store.settings().unwrap();
        assert!(!settings.auto_save, "autoSave 默认应为 false");

        let settings = store.set_auto_save(true).unwrap();
        assert!(settings.auto_save);
        assert!(store.settings().unwrap().auto_save);

        // 改目录不应重置 autoSave
        let custom = root.join("custom");
        store.set_dir(custom.to_string_lossy().as_ref()).unwrap();
        assert!(store.settings().unwrap().auto_save);

        let settings = store.set_auto_save(false).unwrap();
        assert!(!settings.auto_save);
        assert!(!store.settings().unwrap().auto_save);
    }

    /// 并发/限速设置：默认 1 路 + 不限速；越界夹取；不影响目录与 autoSave。
    #[test]
    fn download_network_settings_defaults_and_clamp() {
        let root = temp_dir("network-settings");
        let store = test_store(&root);
        let settings = store.settings().unwrap();
        assert_eq!(settings.concurrency, 1);
        assert_eq!(settings.speed_limit_kbps, 0);

        let settings = store.set_network(0, 0).unwrap();
        assert_eq!(settings.concurrency, 1, "并发数最小为 1");
        assert_eq!(settings.speed_limit_kbps, 0);

        let settings = store.set_network(99, 99_999_999).unwrap();
        assert_eq!(settings.concurrency, MAX_CONCURRENCY);
        assert_eq!(settings.speed_limit_kbps, MAX_SPEED_LIMIT_KBPS);
        assert_eq!(store.settings().unwrap().concurrency, MAX_CONCURRENCY);

        // 目录/autoSave 不受影响
        store.set_auto_save(true).unwrap();
        store.set_network(2, 2048).unwrap();
        let current = store.settings().unwrap();
        assert!(current.auto_save);
        assert_eq!(current.concurrency, 2);
        assert_eq!(current.speed_limit_kbps, 2048);
    }

    /// 打开文件所在位置：任务不存在 / 无本地文件 / 文件已被删除时都返回明确错误（不会弹资源管理器）。
    #[test]
    fn download_open_file_location_error_paths() {
        let root = temp_dir("open-location");
        let store = test_store(&root);
        assert!(store.open_file_location("missing-id").is_err());

        let task = store
            .create_task(DownloadStart {
                site_key: "s".to_string(),
                site_name: None,
                line: "hhyun".to_string(),
                vod_id: None,
                ep_index: Some(0),
                name: "测试影片".to_string(),
                ep_title: "第1集".to_string(),
                cover: None,
                url: "https://example.com/a.mp4".to_string(),
                headers: None,
                priority: None,
            })
            .unwrap();
        let err = store.open_file_location(&task.id).unwrap_err();
        assert!(err.contains("没有本地文件"), "{err}");

        // 模拟已完成但文件被删除：记录里有路径、磁盘上没有
        let missing = root.join("missing-file.mp4");
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE downloads SET file_path = ?2 WHERE id = ?1",
                    params![task.id, missing.to_string_lossy().to_string()],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();
        let err = store.open_file_location(&task.id).unwrap_err();
        assert!(err.contains("本地文件不存在"), "{err}");
    }

    /// 空间管理：统计目录/缓存/.part/已完成占用，清理 .part 与已完成文件（cache 不误删）。
    #[test]
    fn download_storage_status_and_cleanup() {
        let root = temp_dir("storage");
        let store = test_store(&root);
        let movie_dir = root.join("movie");
        let done = movie_dir.join("ep1.mp4");
        fs::create_dir_all(&movie_dir).unwrap();
        fs::write(&done, vec![1u8; 1000]).unwrap();
        let part = movie_dir.join("ep2.mp4.part");
        fs::write(&part, vec![2u8; 500]).unwrap();
        let cache_part = root.join("cache").join("hls.part");
        fs::create_dir_all(cache_part.parent().unwrap()).unwrap();
        fs::write(&cache_part, vec![3u8; 300]).unwrap();

        let now = now_ms();
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO downloads (
                         id, site_key, name, ep_title, url, headers_json, status, progress,
                         downloaded_bytes, total_bytes, speed_bps, file_path, kind, created_at, updated_at
                     ) VALUES ('done-1', 's', '影片', '第1集', 'https://x/a.mp4', '{}', 'completed',
                               100, 1000, 1000, 0, ?1, 'file', ?2, ?2)",
                    params![done.to_string_lossy().to_string(), now],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();

        let status = store.storage_status().unwrap();
        assert!(status.used_bytes >= 1800, "{}", status.used_bytes);
        assert_eq!(status.part_count, 1);
        assert_eq!(status.part_bytes, 500);
        assert_eq!(status.completed_count, 1);
        assert_eq!(status.completed_bytes, 1000);
        assert!(status.disk_total_bytes > 0);

        let cleaned = store.cleanup_parts().unwrap();
        assert_eq!(cleaned.removed_files, 1);
        assert_eq!(cleaned.freed_bytes, 500);
        assert!(!part.exists());
        assert!(cache_part.exists(), "cache 目录里的 .part 不应被清理");

        let cleaned = store.cleanup_completed().unwrap();
        assert_eq!(cleaned.removed_files, 1);
        assert!(!done.exists());
        assert!(store.get_task("done-1").unwrap().is_none());
        assert!(movie_dir.exists(), "只删文件，不删目录结构");
    }

    /// 自动保存策略：默认低优先级 + 不额外限速；越界夹取。
    /// 调度：普通任务优先领取；普通任务下载中低优先级任务让路；普通任务结束后才跑自动保存任务。
    #[test]
    fn download_auto_save_policy_and_priority_scheduling() {
        let root = temp_dir("auto-policy");
        let store = test_store(&root);
        let settings = store.settings().unwrap();
        assert!(settings.auto_save_low_priority, "默认应开启低优先级");
        assert_eq!(settings.auto_save_limit_kbps, 0);

        let settings = store.set_auto_save_policy(false, 99_999_999).unwrap();
        assert!(!settings.auto_save_low_priority);
        assert_eq!(settings.auto_save_limit_kbps, MAX_SPEED_LIMIT_KBPS);
        // 后续调度测试使用低优先级开启
        store.set_auto_save_policy(true, 256).unwrap();
        assert_eq!(store.settings().unwrap().auto_save_limit_kbps, 256);

        let make = |ep_index: i64, priority: Option<i64>| {
            store
                .create_task(DownloadStart {
                    site_key: "s".to_string(),
                    site_name: None,
                    line: "hhyun".to_string(),
                    vod_id: Some("vod-priority".to_string()),
                    ep_index: Some(ep_index),
                    name: "调度测试".to_string(),
                    ep_title: format!("第{}集", ep_index + 1),
                    cover: None,
                    url: format!("https://example.com/p{ep_index}.mp4"),
                    headers: None,
                    priority,
                })
                .unwrap()
        };
        let normal = make(0, None);
        let auto = make(1, Some(1));
        assert_eq!(normal.priority, 0);
        assert_eq!(auto.priority, 1);

        // 普通任务优先被领取（priority 升序）
        let claimed = store.inner.claim_next_queued().unwrap().unwrap();
        assert_eq!(claimed.id, normal.id);

        // 普通任务正在下载 → 自动保存任务让路
        assert!(
            store.inner.claim_next_queued().unwrap().is_none(),
            "低优先级任务不应与普通下载抢带宽"
        );

        // 普通任务结束后，自动保存任务可被领取
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE downloads SET status = 'completed' WHERE id = ?1",
                    params![normal.id],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();
        let claimed = store.inner.claim_next_queued().unwrap().unwrap();
        assert_eq!(claimed.id, auto.id);
    }

    /// 老库迁移：download_settings 没有 auto_save 列时自动 ALTER TABLE 补列。
    #[test]
    fn download_settings_migrates_old_schema() {
        let root = temp_dir("migrate");
        let custom = root.join("old-media");
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE download_settings (
                 id  INTEGER PRIMARY KEY CHECK (id = 1),
                 dir TEXT NOT NULL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO download_settings (id, dir) VALUES (1, ?1)",
            params![custom.to_string_lossy().to_string()],
        )
        .unwrap();

        // with_connection 会重新执行 init_connection（含迁移）
        let store = DownloadStore::with_connection(conn, root.join("default"));
        let settings = store.settings().unwrap();
        assert_eq!(settings.dir, custom.to_string_lossy().to_string());
        assert!(!settings.auto_save, "老库迁移后 autoSave 应为 false");

        let settings = store.set_auto_save(true).unwrap();
        assert!(settings.auto_save);
        let column: i64 = store
            .inner
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT auto_save FROM download_settings WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(err_str)
            })
            .unwrap();
        assert_eq!(column, 1);
    }

    /// download_start 同集去重：只有一条任务；failed 复位续传；completed 原样返回。
    #[test]
    fn download_dedupe_same_episode() {
        let root = temp_dir("dedupe");
        let store = test_store(&root.join("media"));

        let first = store
            .create_task(start_payload("http://127.0.0.1:1/one.mp4"))
            .unwrap();
        let second = store
            .create_task(start_payload("http://127.0.0.1:1/two.mp4"))
            .unwrap();
        assert_eq!(first.id, second.id, "同集重复 start 应返回同一任务");
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(
            store.inner.task_url(&first.id).as_deref(),
            Some("http://127.0.0.1:1/one.mp4"),
            "重复 start 不应覆盖已有任务的地址"
        );

        // 不同集 → 新建任务
        let mut other = start_payload("http://127.0.0.1:1/ep2.mp4");
        other.ep_index = Some(1);
        other.ep_title = "第2集".to_string();
        let ep2 = store.create_task(other).unwrap();
        assert_ne!(ep2.id, first.id);
        assert_eq!(store.list().unwrap().len(), 2);

        // completed → 直接返回已有任务（不重置进度）
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE downloads SET status = 'completed', progress = 100,
                         downloaded_bytes = 999, total_bytes = 999 WHERE id = ?1",
                    params![first.id],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();
        let again = store
            .create_task(start_payload("http://127.0.0.1:1/three.mp4"))
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.status, STATUS_COMPLETED);
        assert_eq!(again.downloaded_bytes, 999);
        assert_eq!(store.list().unwrap().len(), 2);

        // failed → 复位为 queued，保留字节数/进度，用新参数覆盖 url/headers
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE downloads SET status = 'failed', error = 'boom',
                         downloaded_bytes = 12345, progress = 42.5 WHERE id = ?1",
                    params![first.id],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();
        let retried = store
            .create_task(start_payload("http://127.0.0.1:1/four.mp4"))
            .unwrap();
        assert_eq!(retried.id, first.id);
        assert_eq!(retried.status, STATUS_QUEUED);
        assert_eq!(retried.error, None);
        assert_eq!(retried.downloaded_bytes, 12345, "复位应保留已下载字节数");
        assert_eq!(retried.progress, 42.5, "复位应保留进度");
        assert_eq!(
            store.inner.task_url(&first.id).as_deref(),
            Some("http://127.0.0.1:1/four.mp4"),
            "复位应覆盖为新地址"
        );
        assert_eq!(store.list().unwrap().len(), 2);

        // vod_id 为空 → 退化为 (site_key, name, ep_index)
        let mut no_vod = start_payload("http://127.0.0.1:1/no-vod.mp4");
        no_vod.vod_id = None;
        let a = store.create_task(no_vod.clone()).unwrap();
        let b = store.create_task(no_vod).unwrap();
        assert_eq!(a.id, b.id, "vod_id 为空时也应按集去重");
    }

    /// 合并失败（分片缺失）不留下半成品文件。
    #[test]
    fn download_merge_failure_cleans_partial_file() {
        let dir = temp_dir("merge-fail");
        fs::write(dir.join("seg_00001.ts"), b"data").unwrap();
        let output = dir.parent().unwrap().join("第1集.ts");
        let err = merge_hls_files(
            &dir,
            &output,
            &[],
            &["seg_00001.ts".to_string(), "missing.ts".to_string()],
            &[],
        )
        .unwrap_err();
        assert!(err.contains("打开分片失败"), "错误信息应可读: {err}");
        assert!(!output.exists(), "失败时不应留下合并文件");
        assert!(!dir.join("merged.ts.part").exists(), "失败时应清理 .part");
    }

    /// 历史数据修复：HLS 未完成任务的 total_bytes 曾被写成已下载字节（前端会算成 100%），
    /// 启动时应重置为「未知」。
    #[test]
    fn download_repairs_legacy_fake_total_bytes() {
        let root = temp_dir("repair-total");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload("http://127.0.0.1:1/media.m3u8"))
            .unwrap();
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE downloads
                     SET status = 'paused', kind = 'hls', progress = 2.2,
                         downloaded_bytes = 26042592, total_bytes = 26042592
                     WHERE id = ?1",
                    params![task.id],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();
        // 重新跑一次启动迁移（等价于重启应用）
        store
            .inner
            .with_conn(|conn| init_connection(conn, &root))
            .unwrap();
        let repaired = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(repaired.size_bytes, 0, "历史假总数应被重置为未知");
        assert_eq!(repaired.downloaded_bytes, 26_042_592);
        assert_eq!(repaired.progress, 2.2);

        // 已完成的 HLS 记录（total == downloaded 是正常的）不动
        store
            .inner
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE downloads
                     SET status = 'completed', progress = 100,
                         downloaded_bytes = 100, total_bytes = 100
                     WHERE id = ?1",
                    params![task.id],
                )
                .map_err(err_str)
                .map(|_| ())
            })
            .unwrap();
        store
            .inner
            .with_conn(|conn| init_connection(conn, &root))
            .unwrap();
        assert_eq!(
            store.get_task(&task.id).unwrap().unwrap().size_bytes,
            100,
            "已完成记录不应被修复逻辑改动"
        );
    }

    /// 下载路径按「站点/剧名/线路/集名」四级分层。
    #[test]
    fn download_paths_have_site_show_line_levels() {
        let root = temp_dir("paths");
        let store = test_store(&root);
        let mut payload = start_payload("http://127.0.0.1:1/movie.mp4");
        payload.site_name = Some("🎥┃豪华┃资源".to_string());
        let task = store.create_task(payload).unwrap();
        let media = store.media_dir();

        let site = media.join("🎥┃豪华┃资源");
        let show = site.join("测试影片");
        let line = show.join("hhyun");
        assert_eq!(episode_dir(&media, &task), line, "四级目录不对");
        assert_eq!(episode_stem(&media, &task), "第1集");
        assert_eq!(
            hls_work_dir(&media, &task),
            line.join("第1集"),
            "HLS 分片目录 = 站点/影片/线路/集名"
        );
        assert_eq!(
            episode_output_path(&media, &task, "ts"),
            line.join("第1集.ts"),
            "合并输出与分片目录同级"
        );

        // 不同站点：同名剧 / 同集落不同目录
        let mut other_site = start_payload("http://127.0.0.1:1/movie.mp4");
        other_site.site_key = "site-b".to_string();
        other_site.site_name = Some("🌙┃精品┃影视".to_string());
        let other_site = store.create_task(other_site).unwrap();
        assert_ne!(
            episode_dir(&media, &task),
            episode_dir(&media, &other_site),
            "不同站点的同集必须分开"
        );

        // 不同线路：同站同剧同集落不同目录
        let mut other_line = start_payload("http://127.0.0.1:1/movie.mp4");
        other_line.site_name = Some("🎥┃豪华┃资源".to_string());
        other_line.line = "hhm3u8".to_string();
        let other_line = store.create_task(other_line).unwrap();
        assert_ne!(
            episode_dir(&media, &task),
            episode_dir(&media, &other_line),
            "不同线路的同集必须分开"
        );

        // 合集：集标题为空时用「第N集」兜底，多个集不会互相覆盖
        let mut ep2 = start_payload("http://127.0.0.1:1/ep2.mp4");
        ep2.ep_index = Some(1);
        ep2.ep_title = String::new();
        let ep2 = store.create_task(ep2).unwrap();
        assert_eq!(episode_stem(&media, &ep2), "第2集");
        assert_ne!(
            episode_output_path(&media, &task, "ts"),
            episode_output_path(&media, &ep2, "ts")
        );
    }

    /// 路径安全化：`│`/emoji 本身合法应保留；非法字符、保留名、结尾空格/点要处理成合法路径。
    #[test]
    fn download_safe_component_handles_windows_edge_cases() {
        // 「🎥┃豪华┃资源」中的 ┃ 是普通 Unicode 字符（非 Windows 非法字符），保留
        assert_eq!(sanitize_file_name("🎥┃豪华┃资源"), "🎥┃豪华┃资源");
        assert_eq!(sanitize_file_name("杯雪一战成名"), "杯雪一战成名");
        // 结尾空格 / 点：Windows 不允许
        assert_eq!(sanitize_file_name("片名   "), "片名");
        assert_eq!(sanitize_file_name("片名..."), "片名");
        assert_eq!(sanitize_file_name("a\u{0}b"), "a_b");
        // 保留名（含带扩展名写法）加前缀
        assert_eq!(sanitize_file_name("NUL"), "_NUL");
        assert_eq!(sanitize_file_name("con.txt"), "_con.txt");
        assert_eq!(sanitize_file_name("PRN. "), "_PRN");
        // 超长名截断后仍合法
        let long = sanitize_file_name(&"长".repeat(300));
        assert_eq!(long.chars().count(), 120);
        assert!(!long.ends_with(' '));
    }

    /// Windows 路径预算：超长片名/集名要截断，避免 os error 3（系统找不到指定的路径）。
    #[test]
    fn download_paths_stay_within_windows_budget() {
        let root = temp_dir("path-budget");
        let store = test_store(&root);
        let mut payload = start_payload("http://127.0.0.1:1/movie.mp4");
        payload.name = "超长片名".repeat(60);
        payload.ep_title = "超长集名".repeat(60);
        let task = store.create_task(payload).unwrap();
        let media = store.media_dir();
        let output = episode_output_path(&media, &task, "mp4");
        assert!(
            text_len(&output) <= WINDOWS_PATH_BUDGET + 8,
            "输出路径过长({}): {}",
            text_len(&output),
            output.display()
        );

        // 不同超长片名截断后不能互相覆盖（带短哈希；注意前 120 字符不能相同，
        // sanitize_file_name 会先截到 120 字符）
        let mut other = start_payload("http://127.0.0.1:1/other.mp4");
        other.vod_id = Some("vod-2".to_string());
        other.name = format!("另一部{}", "超长片名".repeat(60));
        other.ep_title = format!("另一集{}", "超长集名".repeat(60));
        let other = store.create_task(other).unwrap();
        assert_ne!(
            episode_dir(&media, &task),
            episode_dir(&media, &other),
            "截断后的长片名应带哈希区分"
        );
        assert_ne!(short_hash("a"), short_hash("b"));
    }

    /// #4：进度/速度字段契约 ——
    /// HLS 未完成时 total_bytes 必须为 0（不能等于已下载字节），完成时才写真实大小；
    /// 速度按 report_progress 写入，状态切换时清零。
    #[test]
    fn download_hls_progress_keeps_unknown_total_until_completed() {
        let root = temp_dir("hls-progress");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload("http://127.0.0.1:1/media.m3u8"))
            .unwrap();
        let file = root.join("已完成.ts");
        fs::write(&file, vec![0u8; 4096]).unwrap();

        // 下载中：2/10 分片，已下载 12KB，速度 2KB/s
        store
            .inner
            .report_hls_progress(&task.id, 12_345, 2, 10, 2048.0);
        let running = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(running.progress, 20.0);
        assert_eq!(running.downloaded_bytes, 12_345);
        assert_eq!(running.size_bytes, 0, "HLS 未知总大小必须保持 0");
        assert_eq!(running.speed_bps, 2048.0, "下载速度应写入 speed_bps");

        // 暂停：速度清零，进度保留
        store
            .inner
            .report_hls_progress(&task.id, 12_345, 2, 10, 0.0);
        let paused = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(paused.speed_bps, 0.0);
        assert_eq!(paused.progress, 20.0);

        // 完成：写真实文件大小 + 100%
        store
            .inner
            .mark_completed(&task.id, &file, 4096, 4096, KIND_FILE, None)
            .unwrap();
        let done = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(done.status, STATUS_COMPLETED);
        assert_eq!(done.size_bytes, 4096);
        assert_eq!(done.downloaded_bytes, 4096);
        assert_eq!(done.progress, 100.0);
        assert_eq!(done.speed_bps, 0.0);

        // 合并失败回退 playlist（总大小未知）时也不能把已下载字节当总大小
        let task2 = store
            .create_task({
                let mut payload = start_payload("http://127.0.0.1:1/media2.m3u8");
                payload.ep_index = Some(1);
                payload.ep_title = "第2集".to_string();
                payload
            })
            .unwrap();
        let playlist = root.join("playlist.m3u8");
        fs::write(&playlist, b"#EXTM3U\n").unwrap();
        store
            .inner
            .mark_completed(&task2.id, &playlist, 12_000, 0, KIND_HLS, None)
            .unwrap();
        let fallback = store.get_task(&task2.id).unwrap().unwrap();
        assert_eq!(fallback.size_bytes, 0, "未知总大小不应兜底成已下载字节");
        assert_eq!(fallback.downloaded_bytes, 12_000);
    }

    /// #4：暂停 → 继续 必须真正恢复（排队阶段暂停不启动、下载中暂停可续传完成）。
    #[test]
    fn download_pause_then_resume_continues() {
        let data = sample_bytes();
        let total = data.len() as i64;
        let base = spawn_slow_server(data.clone(), 4096, 12);
        let root = temp_dir("pause-resume");
        let store = test_store(&root);
        let task = store
            .create_task(start_payload(&format!("{base}/movie.mp4")))
            .unwrap();

        // 1) 排队中暂停：worker 不应该取走这个任务
        store.control(&task.id, "pause").unwrap();
        assert!(
            !store.inner.run_next_once().unwrap(),
            "已暂停的任务不应被取走"
        );

        // 2) 继续 → worker 开始下载，中途再暂停
        store.control(&task.id, "resume").unwrap();
        let worker_store = store.clone();
        let handle = std::thread::spawn(move || worker_store.inner.run_next_once());
        std::thread::sleep(Duration::from_millis(220));
        store.control(&task.id, "pause").unwrap();
        assert!(handle.join().unwrap().unwrap());

        let paused = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(
            paused.status, STATUS_PAUSED,
            "暂停后应停在 paused: {:?}",
            paused.error
        );
        assert!(paused.downloaded_bytes > 0, "暂停前应已下载一部分");
        assert!(paused.downloaded_bytes < total, "暂停时不应已经下载完");
        assert_eq!(paused.speed_bps, 0.0, "暂停时速度应为 0");

        // 3) 再继续：从 .part 续传并完成（不能重复下载已下载部分）
        store.control(&task.id, "resume").unwrap();
        assert_eq!(
            store.get_task(&task.id).unwrap().unwrap().status,
            STATUS_QUEUED
        );
        assert!(store.inner.run_next_once().unwrap());
        let done = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(done.status, STATUS_COMPLETED, "错误: {:?}", done.error);
        assert_eq!(done.downloaded_bytes, total);
        let path = PathBuf::from(done.file_path.as_deref().unwrap());
        assert_eq!(
            fs::read(&path).unwrap(),
            data,
            "续传后的文件内容应完整且无重复"
        );
    }
}
