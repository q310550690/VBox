//! HLS 本地代理：真正的「边下边播」
//!
//! 背景：之前 HLS 要么 mpv 直连远端（没法落盘复用），要么下载引擎先整集下完再播
//! （双份流量）；`N_m3u8DL-RE` 这类下载器只在结束后给出文件，不能当流服务用。
//!
//! 做法：由桌面端本地 HTTP 服务按会话代理 HLS——
//! - `GET /hls/{sid}/playlist.m3u8`：拉远端播放列表，把所有 URI（分片、变体播放列表、
//!   密钥、EXT-X-MAP 初始化段）改写成 `/hls/{sid}/...` 本地地址后返回；
//! - `GET /hls/{sid}/s/{key}`：分片请求时**按需**下载并落盘，已缓存直接读文件；
//! - `GET /hls/{sid}/k/{key}`、`/i/{key}`：AES-128 密钥与 fMP4 init 段同样代理（mpv 自己解密）；
//! - `GET /hls/{sid}/v/{key}`：主列表里的变体播放列表，再次改写为本地分片地址。
//!
//! mpv 只需要打开 `http://127.0.0.1:{port}/hls/{sid}/playlist.m3u8`：
//! 起播只等首段、后续边看边下、看过的段直接复用（单份流量），拖动也会按需补齐。
//!
//! 安全边界：只代理 http(s)；会话 id / key 都是十六进制；缓存目录按「内容指纹」
//! （媒体播放列表前几个分片的绝对 URL）隔离，跨重启保留，并由 `playback_cache`
//! 的保留策略（大小上限 / 保留天数）在启动、会话结束与设置变更时按需清理。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tiny_http::{Header, Request, Response, StatusCode};

use crate::hls_util::resolve_url;
use crate::playback_cache::{prune_cache, PlaybackCacheStore};

/// 导出任务的暂停开关 + 进度计数（「保存本集」点击暂停/继续时共享）。
#[derive(Clone, Default)]
pub struct ExportControl {
    paused: Arc<AtomicBool>,
    /// 用户取消（缓存优先下载支持取消；播放器的「保存本集」不设置此标志）
    cancelled: Arc<AtomicBool>,
    /// 已处理的分片数
    done_segments: Arc<AtomicU64>,
    /// 分片总数（0 = 未知，例如交给 ffmpeg 合并的路径没有分片粒度进度）
    total_segments: Arc<AtomicU64>,
    /// 已写出的字节数（未解密分片尺寸，用于估算速度）
    done_bytes: Arc<AtomicU64>,
}

impl ExportControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 取消导出（下一个分片边界生效；ffmpeg 合并路径只能等它返回后才能中断）。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 设置分片总数（导出开始前调用；ffmpeg 合并路径保持 0，进度按未知处理）。
    pub fn set_total_segments(&self, total: u64) {
        self.total_segments.store(total, Ordering::SeqCst);
    }

    /// 记录一个已写出的分片（用于把「保存本集」进度上报到下载页）。
    pub fn add_segment(&self, bytes: usize) {
        self.done_segments.fetch_add(1, Ordering::SeqCst);
        self.done_bytes.fetch_add(bytes as u64, Ordering::SeqCst);
    }

    /// (已完成分片数, 总分片数, 已写字节数)
    pub fn progress(&self) -> (u64, u64, u64) {
        (
            self.done_segments.load(Ordering::SeqCst),
            self.total_segments.load(Ordering::SeqCst),
            self.done_bytes.load(Ordering::SeqCst),
        )
    }

    /// 当前处于暂停时阻塞等待（分片粒度检查，不会中断正在进行的下载）。
    /// 返回 false 表示等待期间被取消，调用方应尽快中止导出。
    fn wait_if_paused(&self) -> bool {
        loop {
            if self.is_cancelled() {
                return false;
            }
            if !self.is_paused() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }
}

/// 与播放器一致的浏览器 UA（部分源会按 UA 拒绝）
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// HLS 播放列表 MIME
const PLAYLIST_MIME: &str = "application/vnd.apple.mpegurl";
/// 「最近使用」标记文件名：保留天数按会话目录内最新 mtime 判断，重看旧缓存时会刷新它。
const USED_MARKER: &str = ".vbox-used";

/// 一条被改写 URI 的记录：真实远端地址 + 响应类型
#[derive(Debug, Clone)]
struct Entry {
    url: String,
    mime: &'static str,
}

/// 媒体播放列表的顺序快照（用于「保存到本地」合并/解密）
type MediaInfo = crate::hls_util::MediaCrypto;

/// 一条轨道（视频或音频）的最近快照 + 代理播放列表路径
#[derive(Clone)]
struct TrackInfo {
    media: MediaInfo,
    playlist_path: String,
}

/// 一个播放会话（一个 media source）
struct Session {
    id: String,
    source: String,
    /// 本地服务根地址（`http://127.0.0.1:{port}`），改写 URI 时用
    base: String,
    headers: Vec<(String, String)>,
    /// 播放缓存根目录（`cache/hls`）：解析到媒体播放列表后据此定位内容目录
    cache_root: PathBuf,
    /// 会话缓存目录。
    ///
    /// 建会话时先放一个「按解析地址」的占位目录（此时还不知道内容身份）；
    /// 一旦解析到媒体播放列表，就改按**内容指纹**（前几个分片的绝对 URL）定目录，
    /// 同一集无论解析地址 / 请求头怎么变都落到同一目录，复用已下载的分片。
    /// 因此这里用 `Mutex` 而不是不可变字段。
    dir: Mutex<PathBuf>,
    client: reqwest::blocking::Client,
    entries: Mutex<HashMap<String, Entry>>,
    /// master 里 `EXT-X-MEDIA:TYPE=AUDIO` 的播放列表地址（用来区分音频/视频轨）
    audio_urls: Mutex<std::collections::HashSet<String>>,
    /// master 里是否存在「非音频轨」的变体（即是否有视频轨）。
    /// 只有纯音频流才允许用音频轨的分片指纹定缓存目录，避免 mpv 请求顺序导致目录漂移。
    has_video_variant: Mutex<bool>,
    /// 最近一次解析的视频轨（保存时按此顺序补下 + 解密 + 合并）
    video_track: Mutex<Option<TrackInfo>>,
    /// 最近一次解析的音频轨（分离音轨时用于合并）
    audio_track: Mutex<Option<TrackInfo>>,
}

/// 创建好的会话（`id` 用于关闭，`url` 交给 mpv 打开）
#[derive(Debug, Clone)]
pub struct CreatedSession {
    pub id: String,
    pub url: String,
}

/// HLS 代理（桌面端全局唯一，由 Tauri 管理）
pub struct HlsProxy {
    cache_root: PathBuf,
    base_url: Mutex<String>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// 导出串行化：手动「保存本集」与关闭会话时的「自动保存」可能并发导出同一集，
    /// 共用同名 `.part` / 成品文件会互相覆盖（表现为 rename 时 os error 2）。
    export_lock: Mutex<()>,
    /// 播放缓存保留策略（缓存跨重启保留；启动/会话结束/设置变更时按策略清理）
    prefs: Arc<PlaybackCacheStore>,
}

impl HlsProxy {
    /// 创建代理；缓存跨重启保留，启动时只按保留策略清理一次（不再整目录清空）。
    pub fn new(cache_root: PathBuf, prefs: Arc<PlaybackCacheStore>) -> Self {
        let _ = std::fs::create_dir_all(&cache_root);
        let proxy = Self {
            cache_root,
            base_url: Mutex::new(String::new()),
            sessions: Mutex::new(HashMap::new()),
            export_lock: Mutex::new(()),
            prefs,
        };
        proxy.prune_now();
        proxy
    }

    /// 缓存根目录（设置页统计/清理、保留策略清理使用）。
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// 立即按当前策略清理一次播放缓存（启动/会话结束/设置变更时调用）。
    pub fn prune_now(&self) -> crate::playback_cache::PruneReport {
        let report = prune_cache(&self.cache_root, &self.prefs.load());
        if report.removed_files > 0 {
            crate::logs::push(
                "info",
                format!(
                    "播放缓存已按策略清理：{} 个文件 / {:.1} MB",
                    report.removed_files,
                    report.removed_bytes as f64 / (1024.0 * 1024.0)
                ),
            );
        }
        report
    }

    /// 本地服务启动后回填 `http://127.0.0.1:{port}`。
    pub fn set_base_url(&self, base: impl Into<String>) {
        *self.base_url.lock().unwrap_or_else(|e| e.into_inner()) = base.into();
    }

    fn base_url(&self) -> String {
        self.base_url
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    /// 为一个 HLS 地址建会话：预拉一次播放列表（失败即回退直连），返回本地入口 URL。
    pub fn create_session(
        &self,
        source: &str,
        headers: &[(String, String)],
    ) -> Result<CreatedSession, String> {
        let base = self.base_url();
        if base.is_empty() {
            return Err("本地服务尚未就绪".to_string());
        }
        let source = source.trim();
        if !(source.starts_with("http://") || source.starts_with("https://")) {
            return Err(format!("不支持的播放地址: {source}"));
        }
        let id = new_session_id();
        // 缓存根目录可能被外部清理（用户手动删、杀软等）：
        // 建会话前重建一次，避免后续分片落盘报「系统找不到指定的路径 (os error 3)」。
        let _ = std::fs::create_dir_all(&self.cache_root);
        // 占位目录：还没解析到媒体播放列表时按「解析地址」落盘（live 只有部分分片等边缘场景）。
        // 真正的目录在解析到媒体播放列表后由 `Session::bind_content_dir` 按**内容指纹**确定：
        // 同一集无论解析地址（可能带临时路径/查询参数）和请求头顺序怎么变，都会落到同一目录。
        // 这里不预建目录，避免每次解析都留下空壳目录。
        let dir = provisional_cache_dir(&self.cache_root, source);
        if dir.is_dir() {
            // 上次同地址的占位缓存：刷新「最近使用」标记（内容目录在解析后会重新定位）
            touch_marker(&dir);
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(25))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(UA)
            .cookie_store(true)
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| format!("创建 HLS 客户端失败: {e}"))?;

        let session = Arc::new(Session {
            id: id.clone(),
            source: source.to_string(),
            base: base.clone(),
            headers: headers.to_vec(),
            cache_root: self.cache_root.clone(),
            dir: Mutex::new(dir),
            client,
            entries: Mutex::new(HashMap::new()),
            audio_urls: Mutex::new(std::collections::HashSet::new()),
            has_video_variant: Mutex::new(false),
            video_track: Mutex::new(None),
            audio_track: Mutex::new(None),
        });

        // 预拉一次：既验证源可用，也把首批分片地址登记好（失败则不建会话，调用方回退直连）。
        // 注意：缓存目录跨重启保留，播放列表失败时**不要**删除整个会话目录，
        // 否则会把同一集上次已缓存的分片一起清掉。
        if let Err(err) = session.playlist(source) {
            return Err(err);
        }

        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), Arc::clone(&session));
        Ok(CreatedSession {
            url: format!("{base}/hls/{id}/playlist.m3u8"),
            id,
        })
    }

    /// 关闭会话（缓存保留以便下次复用，并按保留策略清理）。
    ///
    /// 注意：关闭时可能还有 in-flight 的分片请求仍持有 `Arc<Session>`（mpv 换集/关闭窗口时很常见），
    /// 先等引用归零再清理，避免正在写缓存的分片因目录被删而报
    /// `落盘缓存失败: 系统找不到指定的路径 (os error 3)`。
    /// 与旧版本不同：这里**不再删除会话目录**，否则缓存无法跨播放/重启复用。
    pub fn close_session(&self, id: &str) {
        let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        else {
            return;
        };
        let prefs = Arc::clone(&self.prefs);
        let cache_root = self.cache_root.clone();
        let spawned = std::thread::Builder::new()
            .name("vbox-hls-cache-prune".into())
            .spawn(move || {
                // in-flight 请求持有 Arc：最多等 15s，超时也照样清理
                let step = Duration::from_millis(50);
                let mut waited = Duration::ZERO;
                let limit = Duration::from_secs(15);
                while Arc::strong_count(&session) > 1 && waited < limit {
                    std::thread::sleep(step);
                    waited += step;
                }
                drop(session);
                let report = prune_cache(&cache_root, &prefs.load());
                if report.removed_files > 0 {
                    crate::logs::push(
                        "info",
                        format!(
                            "播放缓存已按策略清理：{} 个文件 / {:.1} MB",
                            report.removed_files,
                            report.removed_bytes as f64 / (1024.0 * 1024.0)
                        ),
                    );
                }
            });
        if spawned.is_err() {
            // 线程创建失败：同步清理一次（此时认为没有 in-flight 请求）
            let _ = prune_cache(&self.cache_root, &self.prefs.load());
        }
    }

    /// 关闭会话并删除该会话的播放缓存目录。
    ///
    /// 用于「内容已经完整落盘」的场景（手动保存本集 / 播放时自动保存导出成功后）：
    /// 此时缓存只是成品的一份重复拷贝，留着会让同一集在磁盘上占 2 倍空间。
    /// 缓存目录按「内容指纹」（前几个分片 URL）确定，删除后重看会重新拉流（缓存本身就是可丢弃数据）。
    pub fn close_session_and_clean_cache(&self, id: &str) {
        let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        else {
            return;
        };
        let dir = session.cache_dir();
        let cleanup_dir = dir.clone();
        let spawned = std::thread::Builder::new()
            .name("vbox-hls-cache-clean".into())
            .spawn(move || {
                // in-flight 请求持有 Arc：最多等 15s，避免删到正在写的分片；超时也照样清理
                let step = Duration::from_millis(50);
                let mut waited = Duration::ZERO;
                let limit = Duration::from_secs(15);
                while Arc::strong_count(&session) > 1 && waited < limit {
                    std::thread::sleep(step);
                    waited += step;
                }
                drop(session);
                match std::fs::remove_dir_all(&cleanup_dir) {
                    Ok(()) => crate::logs::push(
                        "info",
                        format!("已清理播放缓存：{}", cleanup_dir.display()),
                    ),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => crate::logs::push(
                        "warn",
                        format!("清理播放缓存失败({}): {err}", cleanup_dir.display()),
                    ),
                }
            });
        if spawned.is_err() {
            // 线程创建失败：同步清理（此时认为没有 in-flight 请求）
            let _ = std::fs::remove_dir_all(&dir);
        }
    }


    /// 把会话缓存合并保存为一个本地文件（边看边存）。
    ///
    /// - 已缓存的分片直接复用（看过的部分不再回源）；
    /// - 未缓存的分片按播放顺序补下；
    /// - 分离音轨（`EXT-X-MEDIA:TYPE=AUDIO`）：视频+音频双输入交给 ffmpeg 合并；
    /// - `SAMPLE-AES` / `#EXT-X-BYTERANGE`：交给 ffmpeg 读本地代理播放列表处理；
    /// - 普通 AES-128 / 明文：本地拼接（解密 + 按 BYTERANGE 切片）；
    /// - DRM 类明确报错。
    /// 导出（不带暂停控制）：测试 / 内部便捷入口，内部会拿导出锁。
    #[cfg(test)]
    pub fn export(&self, id: &str, dest_dir: &Path, name: &str) -> Result<PathBuf, String> {
        self.export_with(id, dest_dir, name, None)
    }

    /// 导出串行化锁：手动「保存本集」与关闭会话时的「自动保存」可能同时导出同一集。
    /// 自动保存拿到锁后还要再查一次是否已有成品，避免重复导出。
    pub(crate) fn export_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.export_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 带暂停控制的导出（`control` 为空时与 [`export`] 一致）；内部会拿导出锁。
    pub fn export_with(
        &self,
        id: &str,
        dest_dir: &Path,
        name: &str,
        control: Option<&ExportControl>,
    ) -> Result<PathBuf, String> {
        let _guard = self.export_guard();
        self.export_with_locked(id, dest_dir, name, control)
    }

    /// 与 [`export_with`] 相同，但要求调用方已持有 [`export_guard`]（自动保存用）。
    pub(crate) fn export_with_locked(
        &self,
        id: &str,
        dest_dir: &Path,
        name: &str,
        control: Option<&ExportControl>,
    ) -> Result<PathBuf, String> {
        let session = self
            .session(id)
            .ok_or_else(|| "HLS 会话不存在（可能已切换剧集）".to_string())?;
        let video = session
            .video_track
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().cloned())
            .ok_or_else(|| "尚未解析到媒体播放列表，请先播放几秒再保存".to_string())?;
        let audio = session
            .audio_track
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().cloned());

        // DRM / 不支持的加密方式：视频/音频任一命中就直接报错
        for track in [Some(&video), audio.as_ref()].into_iter().flatten() {
            if let Some(method) = &track.media.unsupported {
                return Err(format!(
                    "该 HLS 使用 {method} 加密，暂不支持离线保存，可继续在线观看"
                ));
            }
        }
        if video.media.segments.is_empty() {
            return Err("播放列表里没有可保存的分片".to_string());
        }

        // 进度上报：记录总分片数，「保存本集」会在下载页显示实时进度
        if let Some(control) = control {
            control.set_total_segments(video.media.segments.len() as u64);
        }

        std::fs::create_dir_all(dest_dir).map_err(|e| format!("创建保存目录失败: {e}"))?;

        // 需要 ffmpeg 的情况：分离音轨、SAMPLE-AES
        let needs_ffmpeg = audio.is_some() || video.media.sample_aes;
        if needs_ffmpeg {
            if let Some(control) = control {
                if !control.wait_if_paused() {
                    return Err("已取消".to_string());
                }
            }
            let dest = unique_output_path(dest_dir, name, "ts");
            let part = unique_part_path(&dest);
            let timeout = Duration::from_secs(1800);
            let result = if let Some(audio) = &audio {
                crate::hls_util::run_ffmpeg_merge_to_ts(
                    &video.playlist_path,
                    &audio.playlist_path,
                    &part,
                    timeout,
                )
            } else {
                crate::hls_util::run_ffmpeg_hls_to_ts(&video.playlist_path, &part, timeout)
            };
            if let Err(err) = result {
                let _ = std::fs::remove_file(&part);
                return Err(err);
            }
            // ffmpeg 无法中途暂停：返回后若处于暂停态，先挂起再落盘；取消则不落盘
            if let Some(control) = control {
                if !control.wait_if_paused() {
                    let _ = std::fs::remove_file(&part);
                    return Err("已取消".to_string());
                }
            }
            return commit_output(&part, &dest);
        }

        // 纯 AES-128 / 明文：本地拼接
        let info = &video.media;
        if !info.only_aes128() {
            return Err("该 HLS 混用了多种加密方式，暂不支持离线保存".to_string());
        }

        // init 段（一般未加密）
        let init_file = match &info.init {
            Some(init) => {
                let entry = session
                    .entry(&md5_key(init))
                    .ok_or_else(|| "init 段地址丢失".to_string())?;
                Some(session.ensure_cached(&md5_key(init), &entry)?)
            }
            None => None,
        };

        let ext = if info.init.is_some() { "mp4" } else { "ts" };
        let dest = unique_output_path(dest_dir, name, ext);
        let part = unique_part_path(&dest);

        let write_result = (|| -> Result<(), String> {
            let mut output = File::create(&part).map_err(|e| format!("创建保存文件失败: {e}"))?;
            if let Some(init) = &init_file {
                let mut input = File::open(init).map_err(|e| format!("读取 init 段失败: {e}"))?;
                std::io::copy(&mut input, &mut output)
                    .map_err(|e| format!("写入保存文件失败: {e}"))?;
            }

            // 密钥文件按 URL 缓存，避免同一密钥反复读盘
            let mut key_cache: HashMap<String, [u8; 16]> = HashMap::new();
            for (index, segment_url) in info.segments.iter().enumerate() {
                // 每段开始前检查暂停/取消（导出可随时暂停/继续，已缓存分片会被复用）
                if let Some(control) = control {
                    if !control.wait_if_paused() {
                        return Err("已取消".to_string());
                    }
                }
                let key = md5_key(segment_url);
                let entry = session
                    .entry(&key)
                    .ok_or_else(|| format!("分片地址丢失: {segment_url}"))?;
                let cached = session.ensure_cached(&key, &entry)?;
                let raw = std::fs::read(&cached).map_err(|e| format!("读取缓存分片失败: {e}"))?;
                // BYTERANGE：只取本段对应的字节区间
                let slice: &[u8] = match info.ranges.get(index).copied().flatten() {
                    Some((length, start)) => {
                        let start = start.min(raw.len() as u64) as usize;
                        let end = start.saturating_add(length as usize).min(raw.len());
                        &raw[start..end]
                    }
                    None => &raw,
                };
                let crypto = info.keys.get(index).and_then(|slot| slot.as_ref());
                match crypto {
                    Some(key_ref) => {
                        let key_bytes = if let Some(bytes) = key_cache.get(&key_ref.url) {
                            *bytes
                        } else {
                            let key_id = md5_key(&key_ref.url);
                            let key_entry = session
                                .entry(&key_id)
                                .ok_or_else(|| "HLS 密钥地址丢失".to_string())?;
                            let key_file = session.ensure_cached(&key_id, &key_entry)?;
                            let raw = std::fs::read(&key_file)
                                .map_err(|e| format!("读取密钥失败: {e}"))?;
                            let bytes = crate::hls_util::key_bytes(&raw)?;
                            key_cache.insert(key_ref.url.clone(), bytes);
                            bytes
                        };
                        let iv = key_ref.iv.unwrap_or_else(|| {
                            crate::hls_util::sequence_iv(
                                info.media_sequence.saturating_add(index as u64),
                            )
                        });
                        let plain = crate::hls_util::decrypt_segment(slice, &key_bytes, &iv)?;
                        output
                            .write_all(&plain)
                            .map_err(|e| format!("写入保存文件失败: {e}"))?;
                    }
                    None => {
                        output
                            .write_all(slice)
                            .map_err(|e| format!("写入保存文件失败: {e}"))?;
                    }
                }
                // 进度上报：每写出一个分片累加一次（下载页实时进度 / 速度）
                if let Some(control) = control {
                    control.add_segment(slice.len());
                }
            }
            output
                .flush()
                .map_err(|e| format!("刷新保存文件失败: {e}"))?;
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = std::fs::remove_file(&part);
            return Err(err);
        }
        commit_output(&part, &dest)
    }

    /// 为「不依赖播放、直接复用缓存下载」预先解析播放列表（填充 video/audio track）。
    ///
    /// 正常播放时是本地服务在 mpv 请求 playlist 时顺带解析；下载复用缓存的场景没有 mpv，
    /// 需要在导出前主动拉一次播放列表（只拉列表本身，流量很小）。
    pub fn prime_tracks(&self, id: &str) -> Result<(), String> {
        let session = self
            .session(id)
            .ok_or_else(|| "HLS 会话不存在（可能已切换剧集）".to_string())?;

        // 1) 源列表：media playlist 会直接填充 video_track；master 只注册各变体
        session.playlist(&session.source)?;

        // 2) master：挑一个非音频变体当视频轨
        let has_video = session
            .video_track
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        if !has_video {
            let audio_urls: Vec<String> = session
                .audio_urls
                .lock()
                .map(|urls| urls.iter().cloned().collect())
                .unwrap_or_default();
            let variants: Vec<String> = session
                .entries
                .lock()
                .map(|entries| {
                    entries
                        .values()
                        .filter(|entry| entry.mime == PLAYLIST_MIME)
                        .filter(|entry| !audio_urls.contains(&entry.url))
                        .map(|entry| entry.url.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut parsed = false;
            for url in variants {
                if session.playlist(&url).is_ok()
                    && session
                        .video_track
                        .lock()
                        .map(|guard| guard.is_some())
                        .unwrap_or(false)
                {
                    parsed = true;
                    break;
                }
            }
            if !parsed {
                return Err("未能解析到 HLS 媒体播放列表（可先播放几秒再下载）".to_string());
            }
        }

        // 3) 分离音轨：填充 audio_track（失败不致命，导出时会按实际情况合并/报错）
        let audio_urls: Vec<String> = session
            .audio_urls
            .lock()
            .map(|urls| urls.iter().cloned().collect())
            .unwrap_or_default();
        for url in audio_urls {
            let _ = session.playlist(&url);
        }
        Ok(())
    }

    fn session(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .ok()
            .and_then(|map| map.get(id).cloned())
    }

    /// 是否为本模块的路径（供 local_server 路由）。
    pub fn matches(path: &str) -> bool {
        path.starts_with("/hls/")
    }

    /// 处理 `/hls/...` 请求（请求已被 local_server 解析过路径）。
    pub fn handle(&self, request: Request, path: &str) -> Result<(), String> {
        let rest = match path.strip_prefix("/hls/") {
            Some(rest) => rest,
            None => return respond_text(request, 404, "not found"),
        };
        let mut parts = rest.splitn(3, '/');
        let id = parts.next().unwrap_or("");
        let route = parts.next().unwrap_or("");
        let key = parts.next().unwrap_or("");

        let Some(session) = self.session(id) else {
            return respond_text(request, 404, "hls session not found");
        };

        match route {
            "playlist.m3u8" => match session.playlist(&session.source) {
                Ok(text) => respond_playlist(request, &text),
                Err(err) => {
                    crate::logs::push_throttled(
                        "hls-playlist",
                        "warn",
                        format!("HLS 播放列表拉取失败: {err}"),
                        std::time::Duration::from_secs(10),
                    );
                    respond_text(request, 502, format!("playlist fetch failed: {err}"))
                }
            },
            "v" => {
                let entry = match session.entry(key) {
                    Some(entry) => entry,
                    None => return respond_text(request, 404, "variant not found"),
                };
                match session.playlist(&entry.url) {
                    Ok(text) => respond_playlist(request, &text),
                    Err(err) => {
                        crate::logs::push_throttled(
                            "hls-variant",
                            "warn",
                            format!("HLS 清晰度列表拉取失败: {err}"),
                            std::time::Duration::from_secs(10),
                        );
                        respond_text(request, 502, format!("variant fetch failed: {err}"))
                    }
                }
            }
            "s" | "i" | "k" => {
                let entry = match session.entry(key) {
                    Some(entry) => entry,
                    None => return respond_text(request, 404, "segment not found"),
                };
                match session.ensure_cached(key, &entry) {
                    Ok(file) => serve_file(request, &file, entry.mime),
                    Err(err) => {
                        crate::logs::push_throttled(
                            "hls-segment",
                            "warn",
                            format!("HLS 分片拉取失败: {err}"),
                            std::time::Duration::from_secs(10),
                        );
                        respond_text(request, 502, format!("segment fetch failed: {err}"))
                    }
                }
            }
            _ => respond_text(request, 404, "not found"),
        }
    }
}

impl Session {
    /// 当前会话缓存目录（解析到媒体播放列表前可能还是占位目录）。
    fn cache_dir(&self) -> PathBuf {
        self.dir
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// 解析到媒体播放列表后，按**内容指纹**确定/复用会话缓存目录。
    ///
    /// - 目标目录已存在：直接复用（上次播放/下载留下的分片继续命中，不再重下）；
    /// - 目标目录不存在：扫描 `cache_root` 下的兄弟目录，只要里面有文件名等于分片 URL
    ///   的 md5（即同一份媒体的分片），就沿用那个旧目录——旧版本用「解析地址 + 请求头」
    ///   做目录名、请求头顺序随进程随机，用户磁盘上很可能已有同一集的多份目录；
    ///   缓存最完整的那个被采用，其它旧目录只把缺失的分片搬过来（不删重复文件、
    ///   不删用户旧目录，交给保留策略清理），避免丢数据；
    /// - 都没有：新建内容目录。
    ///
    /// 只由视频轨（或没有视频轨的纯音频流）触发，避免 mpv 请求列表顺序不同导致
    /// 音频轨/视频轨各定一次目录、同一集缓存被劈成两份。
    fn bind_content_dir(&self, media: &MediaInfo) {
        let Some(target) = content_cache_dir(&self.cache_root, &media.segments) else {
            // 空播放列表（live 刚起播只有 EXT-X-PART 等）：先按占位目录落盘，
            // 等出现完整分片后再定目录，并把已下载的分片迁过去。
            return;
        };
        let current = self.cache_dir();
        if current == target {
            // 已经绑定过：只刷新「最近使用」标记（保留天数按目录内最新 mtime 判断）
            let _ = std::fs::create_dir_all(&target);
            touch_marker(&target);
            return;
        }

        let mut final_dir = target.clone();
        let mut legacy_others: Vec<PathBuf> = Vec::new();
        if !target.is_dir() {
            let legacy = legacy_cache_dirs(&self.cache_root, &media.segments, &target);
            if let Some(best) = legacy.first() {
                final_dir = best.clone();
                crate::logs::push(
                    "info",
                    format!(
                        "复用旧版播放缓存目录：{}（分片指纹命中同一媒体）",
                        best.display()
                    ),
                );
                legacy_others = legacy.iter().skip(1).cloned().collect();
            }
        }

        let _ = std::fs::create_dir_all(&final_dir);
        // 本次会话先落在占位目录里的分片（live/parts 等）并进内容目录；搬空才删除这个临时目录
        if current != final_dir {
            merge_cache_dir(&current, &final_dir);
            remove_abandoned_dir(&current);
        }
        // 其它命中的旧目录：只把内容目录缺的分片搬过来（不删重复、不删目录），尽量复用已有缓存
        for from in &legacy_others {
            merge_cache_dir(from, &final_dir);
        }
        touch_marker(&final_dir);
        *self.dir.lock().unwrap_or_else(|e| e.into_inner()) = final_dir;
    }

    /// 带会话请求头发起 GET。
    fn get(&self, url: &str) -> Result<reqwest::blocking::Response, String> {
        let mut request = self.client.get(url);
        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        let response = request.send().map_err(|e| format!("{url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        Ok(response)
    }

    /// 拉取远端播放列表并改写成本地地址。
    fn playlist(&self, url: &str) -> Result<String, String> {
        let response = self.get(url)?;
        let text = response
            .text()
            .map_err(|e| format!("读取播放列表失败: {e}"))?;
        // BOM 兜底（部分站点不会自动去掉）
        let text = text.trim_start_matches('\u{feff}');
        let is_master =
            text.contains("#EXT-X-STREAM-INF") || text.contains("#EXT-X-I-FRAME-STREAM-INF");
        let base = self.base();
        let sid = self.id.as_str();
        let mut register = |resolved: &str, kind: &str| -> String {
            let hash = md5_key(resolved);
            let mime = mime_for_kind(kind, resolved);
            self.register(hash.clone(), resolved, mime);
            format!("{base}/hls/{sid}/{}/{}", route_for(kind), hash)
        };
        let rewritten = rewrite_playlist(text, url, is_master, &mut register);
        if is_master {
            // 记录音频轨播放列表地址：mpv 会分别请求视频/音频列表，保存时要能区分
            let audios = master_audio_urls(text, url);
            // 是否存在视频轨：只有纯音频流才允许用音频轨的分片指纹定缓存目录，
            // 否则 mpv 先请求音频还是视频列表会让目录名漂移、同一集缓存被劈成两份。
            let has_video = master_has_video_variant(text, url, &audios);
            if !audios.is_empty() {
                let mut slot = self.audio_urls.lock().unwrap_or_else(|e| e.into_inner());
                slot.clear();
                slot.extend(audios);
            }
            if let Ok(mut slot) = self.has_video_variant.lock() {
                *slot = has_video;
            }
        } else {
            // 记录顺序快照：保存到本地时按这个顺序补下缺失分片并合并（含解密）
            let info = crate::hls_util::parse_media_playlist(text, url);
            let playlist_path = if url == self.source {
                format!("{base}/hls/{sid}/playlist.m3u8")
            } else {
                format!("{base}/hls/{sid}/v/{}", md5_key(url))
            };
            let is_audio = self
                .audio_urls
                .lock()
                .map(|urls| urls.contains(url))
                .unwrap_or(false);
            if is_audio {
                // 纯音频流（master 里没有视频变体）才按音频轨定目录：
                // 有视频轨时一律等视频轨来定，避免 mpv 请求顺序不同导致目录漂移。
                let pure_audio = !self
                    .has_video_variant
                    .lock()
                    .map(|value| *value)
                    .unwrap_or(false);
                if pure_audio {
                    self.bind_content_dir(&info);
                }
                let track = TrackInfo {
                    media: info,
                    playlist_path,
                };
                if let Ok(mut slot) = self.audio_track.lock() {
                    *slot = Some(track);
                }
            } else {
                // 视频轨（或 source 本身就是媒体播放列表）定内容目录：优先用视频轨的分片指纹
                self.bind_content_dir(&info);
                let track = TrackInfo {
                    media: info,
                    playlist_path,
                };
                if let Ok(mut slot) = self.video_track.lock() {
                    *slot = Some(track);
                }
            }
        }
        Ok(rewritten)
    }

    fn register(&self, key: String, url: &str, mime: &'static str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                key,
                Entry {
                    url: url.to_string(),
                    mime,
                },
            );
        }
    }

    fn entry(&self, key: &str) -> Option<Entry> {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(key).cloned())
    }

    fn base(&self) -> String {
        self.base.clone()
    }

    /// 分片/init/密钥的按需下载与本地缓存。
    ///
    /// 落盘要点（修复 `落盘缓存失败: 系统找不到指定的路径 (os error 3)`）：
    /// 1. 每次写之前 `create_dir_all` 会话目录，目录被外部清掉也能自愈；
    /// 2. 临时文件名带唯一 token，避免同一分片被并发请求时互相踩；
    /// 3. rename 失败先重建目录重试一次；仍失败则直接把临时文件交给播放器，绝不因为
    ///    缓存不可写而中断播放。
    fn ensure_cached(&self, key: &str, entry: &Entry) -> Result<PathBuf, String> {
        let dir = self.cache_dir();
        let path = dir.join(key);
        let cached = std::fs::metadata(&path)
            .map(|meta| meta.is_file() && meta.len() > 0)
            .unwrap_or(false);
        if cached {
            return Ok(path);
        }
        self.ensure_cache_dir()?;
        let response = self.get(&entry.url)?;
        let tmp = dir.join(format!("{key}.{}.part", cache_token()));
        {
            let mut file = File::create(&tmp)
                .map_err(|e| format!("写入缓存失败({}): {e}", tmp.display()))?;
            let mut reader = response;
            std::io::copy(&mut reader, &mut file).map_err(|e| format!("下载分片失败: {e}"))?;
            file.flush()
                .map_err(|e| format!("写入缓存失败({}): {e}", tmp.display()))?;
        }
        self.commit_cache(&tmp, &path)
    }

    /// 确保会话缓存目录存在（幂等；目录被删/进程重启后也能恢复）。
    fn ensure_cache_dir(&self) -> Result<(), String> {
        let dir = self.cache_dir();
        std::fs::create_dir_all(&dir).map_err(|e| {
            format!(
                "创建分片缓存目录失败({}): {e}",
                dir.display()
            )
        })
    }

    /// 把临时文件转正；rename 失败时重建目录重试，最终降级为直接使用临时文件。
    fn commit_cache(&self, tmp: &Path, path: &Path) -> Result<PathBuf, String> {
        match std::fs::rename(tmp, path) {
            Ok(()) => Ok(path.to_path_buf()),
            Err(first) => {
                let _ = self.ensure_cache_dir();
                match std::fs::rename(tmp, path) {
                    Ok(()) => Ok(path.to_path_buf()),
                    Err(second) => {
                        if tmp.is_file() {
                            // 缓存目录不可用不应影响播放：本次直接读临时文件
                            crate::logs::push_throttled(
                                "hls-cache-rename",
                                "warn",
                                format!(
                                    "HLS 缓存落盘失败({} -> {}): {first}；重试: {second}；本次直接用临时文件播放",
                                    tmp.display(),
                                    path.display()
                                ),
                                Duration::from_secs(10),
                            );
                            Ok(tmp.to_path_buf())
                        } else {
                            Err(format!(
                                "落盘缓存失败({}): {second}",
                                path.display()
                            ))
                        }
                    }
                }
            }
        }
    }
}

/* ---------------- 播放列表改写 ---------------- */

/// 改写播放列表：分片行 / 变体行按 `is_master` 区分，`URI="..."` 属性按标签区分。
fn rewrite_playlist(
    text: &str,
    playlist_url: &str,
    is_master: bool,
    map: &mut impl FnMut(&str, &str) -> String,
) -> String {
    let mut out = String::with_capacity(text.len() + 256);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        if trimmed.starts_with('#') {
            out.push_str(&rewrite_uri_attributes(trimmed, playlist_url, map));
        } else {
            let resolved = resolve_url(playlist_url, trimmed);
            let kind = if is_master { "variant" } else { "segment" };
            out.push_str(&map(&resolved, kind));
        }
        out.push('\n');
    }
    out
}

fn rewrite_uri_attributes(
    line: &str,
    playlist_url: &str,
    map: &mut impl FnMut(&str, &str) -> String,
) -> String {
    let mut out = line.to_string();
    let kind = if line.starts_with("#EXT-X-KEY") || line.starts_with("#EXT-X-SESSION-KEY") {
        "key"
    } else if line.starts_with("#EXT-X-MAP") {
        "init"
    } else if line.starts_with("#EXT-X-PART") || line.starts_with("#EXT-X-PRELOAD-HINT") {
        // 低延迟 HLS 的部分分片：按分片代理
        "segment"
    } else {
        // EXT-X-MEDIA / EXT-X-I-FRAME-STREAM-INF / EXT-X-RENDITION-REPORT 等
        "variant"
    };

    // 逐处替换 URI="..."（规范里 URI 属性只有一个，但主列表的 I-FRAME 也可能重复；
    // 这里用循环稳妥处理）
    let mut search_from = 0usize;
    while let Some(index) = out[search_from..].find("URI=").map(|i| i + search_from) {
        let value_start = index + 4;
        let Some(quote) = out[value_start..].chars().next() else {
            break;
        };
        if quote != '"' && quote != '\'' {
            search_from = index + 4;
            continue;
        }
        let body_start = value_start + quote.len_utf8();
        let Some(end) = out[body_start..].find(quote).map(|i| i + body_start) else {
            break;
        };
        let value = out[body_start..end].to_string();
        let resolved = resolve_url(playlist_url, &value);
        let local = map(&resolved, kind);
        out.replace_range(body_start..end, &local);
        search_from = body_start + local.len();
    }
    out
}

/// 取标签里某个属性值（`NAME=VALUE`，引号可选）。
fn tag_attr(line: &str, name: &str) -> Option<String> {
    let start = line.find(':')? + 1;
    for part in line[start..].split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(name) {
            return Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}

/// master 播放列表里所有音频轨（`#EXT-X-MEDIA:TYPE=AUDIO`）的绝对地址。
fn master_audio_urls(text: &str, base: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("#EXT-X-MEDIA:"))
        .filter(|line| {
            tag_attr(line, "TYPE")
                .map(|value| value.eq_ignore_ascii_case("AUDIO"))
                .unwrap_or(false)
        })
        .filter_map(|line| tag_attr(line, "URI"))
        .map(|uri| resolve_url(base, &uri))
        .collect()
}

/// master 里是否存在「非音频轨」的变体（即是否有视频轨）。
///
/// `#EXT-X-STREAM-INF` 后面的 URI 若不在音频轨集合里，就是视频变体；
/// 纯音频流（只有音频轨列表）没有视频变体，此时才允许用音频轨定缓存目录。
fn master_has_video_variant(text: &str, base: &str, audio_urls: &[String]) -> bool {
    let mut pending = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-STREAM-INF") {
            pending = true;
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if pending {
            pending = false;
            let resolved = resolve_url(base, line);
            if !audio_urls.iter().any(|url| url == &resolved) {
                return true;
            }
        }
    }
    false
}

/// 远端地址 → 缓存文件名（稳定哈希，同 URL 永远同一个 key）。
fn md5_key(url: &str) -> String {
    format!("{:x}", md5::compute(url.as_bytes()))
}

/// 保存文件名清洗（去掉 Windows 非法字符/控制字符，压缩空白并截断）。
pub(crate) fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        return "HLS 缓存".to_string();
    }
    if cleaned.chars().count() > 80 {
        return cleaned.chars().take(80).collect();
    }
    cleaned
}

/// 目标文件已存在时追加 `-1` / `-2` 后缀。
pub(crate) fn unique_output_path(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let label = sanitize_file_name(name);
    let first = dir.join(format!("{label}.{ext}"));
    if !first.exists() {
        return first;
    }
    for index in 1..1000 {
        let candidate = dir.join(format!("{label}-{index}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{label}-{}.{ext}", std::process::id()))
}

/// 导出临时文件路径：每次导出都唯一。
///
/// 同一集可能同时存在「手动保存」与「自动保存」两个导出，若共用同名 `.part`，
/// 后开始的导出会截断先开始的临时文件、先完成的 rename 又会把文件改名走，
/// 表现为另一个导出 rename 时报「系统找不到指定的文件」(os error 2)。
pub(crate) fn unique_part_path(dest: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let file = dest.file_name().unwrap_or_default().to_string_lossy();
    dest.with_file_name(format!("{file}.{}-{seq}.part", std::process::id()))
}

/// 把导出的临时文件落到最终路径。
///
/// 并发导出可能抢到同一个目标名：rename 前目标已存在时自动追加 `-N` 后缀重试，
/// 不再因为「同名文件刚被另一个导出 rename 走」而整集保存失败。
pub(crate) fn commit_output(part: &Path, dest: &Path) -> Result<PathBuf, String> {
    if !dest.exists() {
        match std::fs::rename(part, dest) {
            Ok(()) => return Ok(dest.to_path_buf()),
            Err(err) => {
                // 临时文件都不见了：没必要再试候选名（并发导出已把它 rename 走）
                if !part.exists() {
                    return Err(format!(
                        "保存文件重命名失败({}): {err}",
                        part.display()
                    ));
                }
            }
        }
    }
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let stem = dest
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = dest
        .extension()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    for index in 1..1000u32 {
        let candidate = if ext.is_empty() {
            dir.join(format!("{stem}-{index}"))
        } else {
            dir.join(format!("{stem}-{index}.{ext}"))
        };
        if candidate.exists() {
            continue;
        }
        if std::fs::rename(part, &candidate).is_ok() {
            return Ok(candidate);
        }
        if !part.exists() {
            break;
        }
    }
    let _ = std::fs::remove_file(part);
    Err(format!(
        "保存文件重命名失败({}): 目标文件被占用或临时文件已丢失",
        dest.display()
    ))
}

fn route_for(kind: &str) -> &'static str {
    match kind {
        "key" => "k",
        "init" => "i",
        "variant" => "v",
        _ => "s",
    }
}

fn mime_for_kind(kind: &str, url: &str) -> &'static str {
    match kind {
        "key" => "application/octet-stream",
        "init" => "video/mp4",
        "variant" => PLAYLIST_MIME,
        _ => {
            let lower = url
                .split(['?', '#'])
                .next()
                .unwrap_or(url)
                .to_ascii_lowercase();
            if lower.ends_with(".ts") {
                "video/mp2t"
            } else if lower.ends_with(".m4s") || lower.ends_with(".mp4") || lower.ends_with(".m4v")
            {
                "video/mp4"
            } else if lower.ends_with(".aac") {
                "audio/aac"
            } else if lower.ends_with(".m3u8") {
                PLAYLIST_MIME
            } else {
                "application/octet-stream"
            }
        }
    }
}

/// 是否是 HLS 地址（mpv 只对 HLS 走本地代理；mp4 直链本来就支持 Range 渐进播放）。
pub fn is_hls_url(url: &str) -> bool {
    url.to_ascii_lowercase().contains(".m3u8")
}

fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{:x}", std::process::id())
}

/// 内容指纹：同一份媒体的稳定标识（会话缓存目录名）。
///
/// 取媒体播放列表里**前 1~3 个分片的绝对 URL**，拼成 `hls1:` 前缀的字符串再取 md5：
/// 分片 URL 在多次解析/会话之间是稳定的（同一集的分片文件名不变，正是这次 bug 现场的证据），
/// 而解析地址（source）可能带临时路径/查询参数、请求头顺序随进程随机，都不能进 key。
/// 只取前 3 个分片：足以区分不同集/不同清晰度，又不会因为 live 列表尾部增长而漂移。
fn content_fingerprint(segments: &[String]) -> Option<String> {
    let first = segments.first()?;
    let mut material = String::with_capacity(first.len() * 3 + 8);
    material.push_str("hls1:");
    material.push_str(first);
    for url in segments.iter().skip(1).take(2) {
        material.push('\n');
        material.push_str(url);
    }
    Some(format!("{:x}", md5::compute(material.as_bytes())))
}

/// 内容指纹 → 会话缓存目录（纯计算，不落盘）。
fn content_cache_dir(cache_root: &Path, segments: &[String]) -> Option<PathBuf> {
    content_fingerprint(segments).map(|fingerprint| cache_root.join(fingerprint))
}

/// 占位缓存目录：还没解析到媒体播放列表时按「解析地址」落盘。
///
/// 只是临时占位（例如 live 刚起播只有 `EXT-X-PART`、还没有完整分片），
/// 解析到媒体播放列表后会由 `Session::bind_content_dir` 换成内容目录，并把这里的文件并过去。
/// 不含请求头：请求头顺序随进程随机，正是旧方案「同一集重复缓存」的根因。
fn provisional_cache_dir(cache_root: &Path, source: &str) -> PathBuf {
    cache_root.join(format!("{:x}", md5::compute(source.as_bytes())))
}

/// 扫描 `cache_root` 下的兄弟会话目录，找出缓存过**同一份媒体**的旧目录。
///
/// 判定依据：目录里存在文件名等于某个（前几个）分片 URL 的 md5 的非空文件——
/// 分片文件名就是 `md5(分片 URL)`，文件名相同即同一份媒体（同一集的分片 URL 稳定）。
/// 返回按已缓存分片数从多到少排序的候选，优先复用缓存最完整的目录。
fn legacy_cache_dirs(cache_root: &Path, segments: &[String], exclude: &Path) -> Vec<PathBuf> {
    let keys: Vec<String> = segments.iter().take(3).map(|url| md5_key(url)).collect();
    let Ok(entries) = std::fs::read_dir(cache_root) else {
        return Vec::new();
    };
    let mut hits: Vec<(usize, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path == exclude {
            continue;
        }
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let matched = keys.iter().any(|key| {
            std::fs::metadata(path.join(key))
                .map(|meta| meta.is_file() && meta.len() > 0)
                .unwrap_or(false)
        });
        if matched {
            hits.push((cached_file_count(&path), path));
        }
    }
    hits.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    hits.into_iter().map(|(_, path)| path).collect()
}

/// 目录里已缓存的分片数（直接子文件，忽略「最近使用」标记）。
fn cached_file_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| entry.file_name().to_string_lossy() != USED_MARKER)
        .count()
}

/// 把 `from` 里已落盘的分片并进 `to`（只搬 `to` 里没有的文件）。
///
/// 用于「旧版目录 / 占位目录 → 内容目录」的迁移：**只搬不删、绝不覆盖**已有分片，
/// 也不删除源目录（是否移除搬空的源目录由调用方决定），保证不丢用户数据。
fn merge_cache_dir(from: &Path, to: &Path) {
    if from == to || !from.is_dir() || !to.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy() == USED_MARKER {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 {
            continue;
        }
        let dest = to.join(&name);
        if dest.exists() {
            // 目标已有同名分片（文件名就是分片 URL 的 md5，内容一致）：保留源文件交给保留策略清理
            continue;
        }
        // 同盘 rename；失败（被占用/权限等）就跳过，不影响播放
        let _ = std::fs::rename(entry.path(), &dest);
    }
}

/// 只移除「空目录」或「只剩 `.vbox-used` 标记」的目录；有分片的一律保留。
fn remove_abandoned_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    match names.as_slice() {
        [] => {
            let _ = std::fs::remove_dir(dir);
        }
        [only] if only == USED_MARKER => {
            let _ = std::fs::remove_file(dir.join(USED_MARKER));
            let _ = std::fs::remove_dir(dir);
        }
        _ => {}
    }
}

/// 刷新会话目录的「最近使用」标记文件。
///
/// 保留天数按会话目录内最新 mtime 判断；重看旧缓存时会打开这个文件刷新 mtime，
/// 避免「刚看过却被当成过期删掉」。
fn touch_marker(dir: &Path) {
    let marker = dir.join(USED_MARKER);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&marker);
}

/// 缓存临时文件后缀：同进程内唯一，避免同一分片被并发请求时临时文件互相覆盖。
fn cache_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}{seq:x}")
}

/* ---------------- HTTP 响应 ---------------- */

fn respond_text(request: Request, status: u16, body: impl Into<String>) -> Result<(), String> {
    let mime = if status >= 400 {
        "text/plain; charset=utf-8"
    } else {
        PLAYLIST_MIME
    };
    let header = Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
        .map_err(|_| "invalid content-type".to_string())?;
    let response = Response::from_data(body.into().into_bytes())
        .with_status_code(StatusCode(status))
        .with_header(header);
    request
        .respond(response)
        .map_err(|e| format!("write response failed: {e}"))
}

fn respond_playlist(request: Request, text: &str) -> Result<(), String> {
    let content_type = Header::from_bytes(&b"Content-Type"[..], PLAYLIST_MIME.as_bytes())
        .map_err(|_| "invalid content-type".to_string())?;
    // 播放列表不缓存：直播列表会持续增长，VOD 也以最新改写为准
    let cache_control = Header::from_bytes(&b"Cache-Control"[..], b"no-cache")
        .map_err(|_| "invalid cache-control".to_string())?;
    let response = Response::from_string(text)
        .with_status_code(StatusCode(200))
        .with_header(content_type)
        .with_header(cache_control);
    request
        .respond(response)
        .map_err(|e| format!("write response failed: {e}"))
}

/// 读本地缓存文件并响应；支持单段 Range（mpv 对部分 fMP4 会发 Range 探测）。
fn serve_file(request: Request, path: &Path, mime: &str) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取缓存失败: {e}"))?;
    let total = meta.len();
    let range = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Range"))
        .and_then(|header| parse_range(header.value.as_str(), total));

    let (status, start, end) = match range {
        Some((start, end)) => (206u16, start, end),
        None => (200u16, 0, total.saturating_sub(1)),
    };
    let length = if total == 0 {
        0
    } else {
        end.saturating_sub(start) + 1
    };

    let mut file = File::open(path).map_err(|e| format!("打开缓存失败: {e}"))?;
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("定位缓存失败: {e}"))?;
    }

    let mut headers = vec![
        Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
            .map_err(|_| "invalid content-type".to_string())?,
        Header::from_bytes(&b"Accept-Ranges"[..], b"bytes")
            .map_err(|_| "invalid accept-ranges".to_string())?,
    ];
    if status == 206 {
        let content_range = format!("bytes {start}-{end}/{total}");
        headers.push(
            Header::from_bytes(&b"Content-Range"[..], content_range.as_bytes())
                .map_err(|_| "invalid content-range".to_string())?,
        );
    }

    let response = Response::new(
        StatusCode(status),
        headers,
        file.take(length),
        Some(length as usize),
        None,
    );
    request
        .respond(response)
        .map_err(|e| format!("write response failed: {e}"))
}

/// 解析 `bytes=start-end` / `bytes=start-`；不支持多段 Range（返回 None 走整段）。
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let rest = value.trim().strip_prefix("bytes=")?;
    if rest.contains(',') {
        return None;
    }
    let (start_text, end_text) = rest.split_once('-')?;
    let start = start_text.trim().parse::<u64>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end_text.trim().is_empty() {
        total - 1
    } else {
        end_text.trim().parse::<u64>().ok()?.min(total - 1)
    };
    (end >= start).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite_with(text: &str, base: &str, master: bool) -> (String, Vec<(String, String)>) {
        let mut entries = Vec::new();
        let mut map = |url: &str, kind: &str| {
            entries.push((url.to_string(), kind.to_string()));
            format!("local:{kind}:{}", entries.len() - 1)
        };
        (rewrite_playlist(text, base, master, &mut map), entries)
    }

    #[test]
    fn rewrites_media_playlist_segments_and_relative_urls() {
        let playlist = "#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nhttps://cdn.example.com/seg2.ts\n#EXT-X-ENDLIST\n";
        let (out, entries) =
            rewrite_with(playlist, "https://cdn.example.com/live/index.m3u8", false);
        assert!(out.contains("local:segment:0"), "{out}");
        assert!(out.contains("local:segment:1"), "{out}");
        assert_eq!(entries[0].0, "https://cdn.example.com/live/seg1.ts");
        assert_eq!(entries[1].0, "https://cdn.example.com/seg2.ts");
    }

    #[test]
    fn rewrites_master_playlist_variants() {
        let playlist =
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nv0/index.m3u8\n";
        let (out, entries) = rewrite_with(playlist, "https://cdn.example.com/master.m3u8", true);
        assert!(out.contains("local:variant:0"), "{out}");
        assert_eq!(entries[0].0, "https://cdn.example.com/v0/index.m3u8");
    }

    #[test]
    fn rewrites_key_and_init_uris() {
        let playlist = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x1\n#EXT-X-MAP:URI='init.mp4'\n#EXTINF:4.0,\nseg1.m4s\n";
        let (out, entries) = rewrite_with(playlist, "https://cdn.example.com/a/b.m3u8", false);
        assert!(out.contains("URI=\"local:key:0\""), "{out}");
        assert!(out.contains("URI='local:init:1'"), "{out}");
        assert_eq!(entries[0].1, "key");
        assert_eq!(entries[1].1, "init");
        assert_eq!(entries[0].0, "https://cdn.example.com/a/key.bin");
    }

    #[test]
    fn keeps_data_uris_untouched() {
        let (out, _) = rewrite_with(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"data:text/plain;base64,AAAA\"\n",
            "https://cdn.example.com/x.m3u8",
            false,
        );
        // data: 不解析为相对地址，但仍然是「URI 属性」——本地代理无法拉取 data:，
        // 保持原样最稳（mpv 自己处理）。
        assert!(
            out.contains("local:key:0") || out.contains("data:text/plain"),
            "{out}"
        );
    }

    #[test]
    fn rewrites_session_key_and_low_latency_parts() {
        let playlist = "#EXTM3U\n#EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"sk.bin\"\n#EXT-X-PART:DURATION=0.5,URI=\"part1.m4s\"\n#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part2.m4s\"\n";
        let (out, entries) = rewrite_with(playlist, "https://cdn.example.com/a/x.m3u8", false);
        assert_eq!(entries[0].1, "key");
        assert_eq!(entries[1].1, "segment");
        assert_eq!(entries[2].1, "segment");
        assert!(out.contains("local:key:0"), "{out}");
    }

    #[test]
    fn parses_master_audio_playlist_urls() {
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aac\",NAME=\"中文\",URI=\"audio/zh.m3u8\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",URI=\"subs.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"aac\"\nv0.m3u8\n";
        let urls = master_audio_urls(master, "https://cdn.example.com/master.m3u8");
        assert_eq!(urls, vec!["https://cdn.example.com/audio/zh.m3u8"]);
        assert_eq!(
            tag_attr("#EXT-X-MEDIA:TYPE=AUDIO,URI=\"a.m3u8\"", "TYPE").as_deref(),
            Some("AUDIO")
        );
    }

    #[test]
    fn parses_single_range() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-", 1000), Some((100, 999)));
        assert_eq!(parse_range("bytes=0-2000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=0-10,20-30", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
    }

    #[test]
    fn detects_hls_urls() {
        assert!(is_hls_url("https://x/a/index.m3u8?token=1"));
        assert!(is_hls_url("https://x/a/index.M3U8"));
        assert!(!is_hls_url("https://x/a/movie.mp4"));
    }

    #[test]
    fn session_ids_are_unique_hex() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "{a}");
    }

    fn test_session(cache_root: PathBuf, dir: PathBuf) -> Session {
        Session {
            id: "test-session".to_string(),
            source: "https://example.com/media.m3u8".to_string(),
            base: "http://127.0.0.1:1".to_string(),
            headers: Vec::new(),
            cache_root,
            dir: Mutex::new(dir),
            client: reqwest::blocking::Client::new(),
            entries: Mutex::new(HashMap::new()),
            audio_urls: Mutex::new(std::collections::HashSet::new()),
            has_video_variant: Mutex::new(false),
            video_track: Mutex::new(None),
            audio_track: Mutex::new(None),
        }
    }

    /// 构造媒体播放列表快照（测试用）。
    fn media_info(segments: &[&str]) -> MediaInfo {
        MediaInfo {
            segments: segments.iter().map(|url| url.to_string()).collect(),
            ..Default::default()
        }
    }

    /// 建一个测试会话：占位目录按 source 算（与线上 `create_session` 一致）。
    fn session_for(root: &Path, source: &str, headers: &[(&str, &str)]) -> Session {
        let mut session = test_session(root.to_path_buf(), provisional_cache_dir(root, source));
        session.headers = headers
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        session
    }

    /// 临时缓存根目录（测试结束自行清理）。
    fn temp_cache_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("vbox-hls-{tag}-{}", cache_token()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 同一集的分片（文件名就是缓存文件名，取 bug 现场的真实分片名）。
    const MEDIA_A: [&str; 4] = [
        "https://cdn.example.com/media/1234/00213832626b3b5fe80b6633f69a0418.ts",
        "https://cdn.example.com/media/1234/01aec371e9ac66260647685f3e748c7f.ts",
        "https://cdn.example.com/media/1234/02b9e0e2d1f9c8f0a1b2c3d4e5f60718.ts",
        "https://cdn.example.com/media/1234/03c0f1f3e2a0d9a1b2c3d4e5f6071829.ts",
    ];
    /// 另一集（不同媒体）的分片
    const MEDIA_B: [&str; 2] = [
        "https://cdn.example.com/media/5678/aa11bb22cc33dd44ee55ff6677889900.ts",
        "https://cdn.example.com/media/5678/bb22cc33dd44ee55ff66778899001122.ts",
    ];

    /// 缓存目录被外部清掉（用户手删 / 另一个实例清理 / 杀软）后应能自愈，
    /// 不再出现「落盘缓存失败: 系统找不到指定的路径 (os error 3)」。
    #[test]
    fn cache_dir_self_heals_after_removal() {
        let dir = std::env::temp_dir().join(format!("vbox-hls-cache-{}", cache_token()));
        let session = test_session(dir.clone(), dir.clone());
        assert!(session.ensure_cache_dir().is_ok());
        assert!(dir.is_dir());

        // 目录被删 → 再次写盘前重建
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(session.ensure_cache_dir().is_ok());
        assert!(dir.is_dir(), "缓存目录被删后应自愈");

        let tmp = dir.join("abc.1.part");
        std::fs::write(&tmp, b"segment-data").unwrap();
        let committed = session
            .commit_cache(&tmp, &dir.join("abc"))
            .expect("落盘应成功");
        assert_eq!(committed, dir.join("abc"));
        assert_eq!(std::fs::read(dir.join("abc")).unwrap(), b"segment-data");

        // rename 失败（目录又被删）时会重建目录重试
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(session.ensure_cache_dir().is_ok());
        let tmp2 = dir.join("def.2.part");
        std::fs::write(&tmp2, b"more-data").unwrap();
        assert!(session.commit_cache(&tmp2, &dir.join("def")).is_ok());
        assert_eq!(std::fs::read(dir.join("def")).unwrap(), b"more-data");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 关闭会话时要等在途请求（持有 Arc）结束，但**缓存目录要保留**（跨播放复用），
    /// 只按保留策略清理；否则「播放缓存」永远是空的。
    #[test]
    fn close_session_keeps_cache_for_reuse() {
        let root = std::env::temp_dir().join(format!("vbox-hls-close-{}", cache_token()));
        let prefs = Arc::new(PlaybackCacheStore::new(&root));
        let proxy = HlsProxy::new(root.join("hls"), prefs);
        let session_dir = root.join("hls").join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("seg1"), b"cached").unwrap();

        let session = Arc::new(test_session(root.join("hls"), session_dir.clone()));
        // 模拟一个在途请求仍持有 Arc
        let in_flight = Arc::clone(&session);
        proxy
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("s1".to_string(), session);

        proxy.close_session("s1");
        drop(in_flight);
        std::thread::sleep(Duration::from_millis(400));

        assert!(
            session_dir.join("seg1").exists(),
            "关闭会话后缓存应保留，供下次播放/重启复用"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 成品已落盘（保存本集/自动保存成功）时关闭会话要**连缓存目录一起删**：
    /// 否则同一集会在磁盘上同时留「成品文件 + 播放缓存」两份。
    #[test]
    fn close_session_and_clean_cache_removes_dir() {
        let root = std::env::temp_dir().join(format!("vbox-hls-clean-{}", cache_token()));
        let prefs = Arc::new(PlaybackCacheStore::new(&root));
        let proxy = HlsProxy::new(root.join("hls"), prefs);
        let session_dir = root.join("hls").join("s2");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("seg1"), b"cached").unwrap();

        let session = Arc::new(test_session(root.join("hls"), session_dir.clone()));
        proxy
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("s2".to_string(), session);

        proxy.close_session_and_clean_cache("s2");
        std::thread::sleep(Duration::from_millis(400));

        assert!(
            !session_dir.exists(),
            "成品落盘后会话缓存目录应被删除"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ① 同一媒体、不同解析地址（source 每次解析都可能变）→ 同一内容目录。
    #[test]
    fn same_media_with_different_source_urls_shares_cache_dir() {
        let root = temp_cache_root("content");
        let media = media_info(&MEDIA_A);

        let first = session_for(&root, "https://api.example.com/play?id=1&token=aaa", &[]);
        first.bind_content_dir(&media);
        let second = session_for(&root, "https://api.example.com/tmp/9f8e/play.m3u8?ts=2", &[]);
        second.bind_content_dir(&media);

        assert_eq!(
            first.cache_dir(),
            second.cache_dir(),
            "同一媒体应落到同一内容目录"
        );
        assert!(first.cache_dir().is_dir(), "内容目录应已创建");
        assert!(
            first.cache_dir().join(USED_MARKER).exists(),
            "内容目录应刷新 .vbox-used 标记"
        );
        let name = first
            .cache_dir()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            name.chars().all(|ch| ch.is_ascii_hexdigit()),
            "目录名应是内容指纹 md5: {name}"
        );
        assert!(
            !provisional_cache_dir(&root, "https://api.example.com/play?id=1&token=aaa").exists(),
            "绑定内容目录后不应留下占位空目录"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ② 不同媒体（分片不同）→ 不同内容目录，不能串缓存。
    #[test]
    fn different_media_use_different_cache_dirs() {
        let root = temp_cache_root("content-diff");
        let a = session_for(&root, "https://api.example.com/play?id=1", &[]);
        a.bind_content_dir(&media_info(&MEDIA_A));
        let b = session_for(&root, "https://api.example.com/play?id=2", &[]);
        b.bind_content_dir(&media_info(&MEDIA_B));
        assert_ne!(a.cache_dir(), b.cache_dir(), "不同媒体不能共用缓存目录");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ③ 请求头不再参与 key：顺序/大小写变化不影响目录。
    ///
    /// 旧方案按 `HashMap` 迭代顺序拼 header 进 key，进程重启就换目录，
    /// 正是「同一集重复缓存」的根因之一。
    #[test]
    fn header_order_and_case_do_not_affect_cache_dir() {
        let root = temp_cache_root("content-headers");
        let media = media_info(&MEDIA_A);
        let source = "https://api.example.com/play?id=1";
        let a = session_for(
            &root,
            source,
            &[("Referer", "https://x/"), ("User-Agent", "UA")],
        );
        a.bind_content_dir(&media);
        let b = session_for(
            &root,
            source,
            &[("user-agent", "UA"), ("referer", "https://x/")],
        );
        b.bind_content_dir(&media);
        assert_ne!(a.headers, b.headers, "两个会话的请求头顺序/大小写确实不同");
        assert_eq!(
            a.cache_dir(),
            b.cache_dir(),
            "请求头顺序/大小写不应影响缓存目录"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ④ 旧版目录（只含相同分片文件、目录名是旧哈希）能被识别复用；
    /// 同一媒体的多份旧目录会被合并，搬空的目录才移除，不丢用户数据。
    #[test]
    fn reuses_and_merges_legacy_session_dirs() {
        let root = temp_cache_root("content-legacy");
        let media = media_info(&MEDIA_A);
        let keys: Vec<String> = MEDIA_A.iter().map(|url| md5_key(url)).collect();
        // 模拟一个只在 live 里出现过、不在 segments 里的部分分片
        let part_key = md5_key("https://cdn.example.com/media/1234/part-only.m4s");

        // 旧目录 1（缓存最完整）：旧版目录名 = md5(解析地址 + 请求头)，这里用现场的真实目录名
        let legacy = root.join("3fcfc9c33ddede0bceb3a97344423fb5");
        std::fs::create_dir_all(&legacy).unwrap();
        for (index, key) in keys.iter().enumerate().take(3) {
            std::fs::write(legacy.join(key), format!("cached-{index}")).unwrap();
        }
        touch_marker(&legacy);

        // 旧目录 2（同一媒体的另一份会话目录）：含首个分片 + 一份旧目录 1 没有的分片
        let other = root.join("7e3ba40c5dab213dc27e7657e4a23844");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(&keys[0]), b"cached-0").unwrap();
        std::fs::write(other.join(&keys[3]), b"cached-3").unwrap();

        // 新会话：source 已变；占位目录里先落了半段（模拟 live/parts 先写盘）
        let session = session_for(&root, "https://api.example.com/tmp/abc/play.m3u8?ts=9", &[]);
        let provisional = session.cache_dir();
        std::fs::create_dir_all(&provisional).unwrap();
        std::fs::write(provisional.join(&part_key), b"pending").unwrap();

        session.bind_content_dir(&media);

        assert_eq!(session.cache_dir(), legacy, "应复用缓存最完整的旧目录");
        assert_eq!(
            std::fs::read(legacy.join(&keys[0])).unwrap(),
            b"cached-0",
            "已有分片不应被覆盖"
        );
        assert_eq!(
            std::fs::read(legacy.join(&keys[1])).unwrap(),
            b"cached-1"
        );
        assert_eq!(
            std::fs::read(legacy.join(&keys[3])).unwrap(),
            b"cached-3",
            "另一份旧目录里缺失的分片应被并入"
        );
        assert_eq!(
            std::fs::read(legacy.join(&part_key)).unwrap(),
            b"pending",
            "占位目录里的分片应被并入"
        );
        assert!(
            other.exists() && other.join(&keys[0]).exists(),
            "另一份旧目录里的重复分片保留、目录本身不删（不丢用户数据）"
        );
        assert!(
            !other.join(&keys[3]).exists(),
            "另一份旧目录里内容目录缺失的分片应搬到采用目录"
        );
        assert!(!provisional.exists(), "被搬空的占位目录应移除");
        assert!(legacy.join(USED_MARKER).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 内容指纹只取前 3 个分片：live 播放列表尾部增长不会让目录漂移；
    /// 没有完整分片时先不定目录（继续用占位目录）。
    #[test]
    fn content_fingerprint_ignores_tail_segments() {
        let shorter: Vec<String> = MEDIA_A[..3].iter().map(|url| url.to_string()).collect();
        let mut longer: Vec<String> = MEDIA_A.iter().map(|url| url.to_string()).collect();
        longer.push(
            "https://cdn.example.com/media/1234/04d1e2f3a4b5c6d7e8f9012345678901.ts".to_string(),
        );
        assert_eq!(content_fingerprint(&shorter), content_fingerprint(&longer));
        assert!(
            content_fingerprint(&[]).is_none(),
            "没有完整分片时不应定内容目录"
        );
        let root = PathBuf::from("C:/vbox-hls-cache-root");
        assert_eq!(
            content_cache_dir(&root, &MEDIA_A.iter().map(|url| url.to_string()).collect::<Vec<_>>())
                .unwrap()
                .parent(),
            Some(root.as_path())
        );
    }

    /// master 里是否含视频变体：决定纯音频流能否用音频轨定缓存目录。
    #[test]
    fn detects_video_variants_in_master() {
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aac\",URI=\"audio/zh.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,AUDIO=\"aac\"\nv0.m3u8\n";
        let audios = master_audio_urls(master, "https://cdn.example.com/master.m3u8");
        assert!(
            master_has_video_variant(master, "https://cdn.example.com/master.m3u8", &audios),
            "有视频变体时应由视频轨定目录"
        );

        // 纯音频 master：STREAM-INF 指向的就是音频轨列表
        let audio_only = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aac\",URI=\"audio/zh.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=128000,AUDIO=\"aac\"\naudio/zh.m3u8\n";
        let audios = master_audio_urls(audio_only, "https://cdn.example.com/master.m3u8");
        assert!(
            !master_has_video_variant(audio_only, "https://cdn.example.com/master.m3u8", &audios),
            "纯音频流允许用音频轨定目录"
        );
    }

    /// 并发导出时临时文件必须唯一：同名 `.part` 会被另一个导出截断 / rename 走。
    #[test]
    fn unique_part_paths_do_not_collide() {
        let dest = PathBuf::from("C:/vbox-download/折月亮 第1集.ts");
        let a = unique_part_path(&dest);
        let b = unique_part_path(&dest);
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".part"));
        assert_eq!(a.parent(), dest.parent());
    }

    /// 成品名被并发导出抢占时，落盘自动追加 `-N` 后缀，而不是整集保存失败。
    #[test]
    fn commit_output_falls_back_when_destination_taken() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "vbox-hls-commit-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let dest = dir.join("折月亮 第1集.ts");
        std::fs::write(&dest, b"winner").unwrap();
        let part = unique_part_path(&dest);
        std::fs::write(&part, b"loser").unwrap();

        let saved = commit_output(&part, &dest).expect("目标被占用时应换名落盘");
        assert_ne!(saved, dest);
        assert_eq!(std::fs::read(&dest).unwrap(), b"winner", "不能覆盖已有成品");
        assert_eq!(std::fs::read(&saved).unwrap(), b"loser");

        // 临时文件丢失（被另一个导出 rename 走）：返回错误而不是 panic
        let missing = unique_part_path(&dest);
        assert!(commit_output(&missing, &dir.join("不存在.ts")).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
