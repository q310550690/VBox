//! 播放器状态与 Tauri 命令
//!
//! 渲染架构（对齐 soia 的做法）：
//! - libmpv 直接以 **主窗口句柄** 作为 `--wid`，把视频画在窗口本体上：
//!   Windows 用 HWND；macOS 用 `window.ns_view()` 拿到的 NSView 指针（libmpv 的 cocoa vo）；
//! - WebView 背景透明（窗口 `transparent: true`），前端在播放器窗格处用 clip-path 挖洞，
//!   洞内不绘制 HTML → 露出下层的 mpv 视频；控制条/浮层是 webview 层，盖在视频上方；
//! - 前端把窗格 rect 上报给 Rust，Rust 通过 `video-margin-ratio-*` 把 mpv 的视频区域
//!   限制在窗格内（否则会铺满整个窗口）；
//! - 这样不需要额外的原生子窗口：WebView2 的 DirectComposition 会盖住普通子窗口，
//!   而窗口本体的渲染在 DComp 之下，透明 webview 正好能把它露出来。
//!
//! macOS 注意：需要 `tauri` 的 `macos-private-api` feature + `app.macOSPrivateApi: true`
//! 才能有透明窗口；libmpv 从 `libmpv.2.dylib` / `libmpv.dylib` 动态加载（见 vbox_player::ffi）。
//!
//! 线程模型：
//! - 创建 libmpv（加载动态库 + 初始化）放在 `spawn_blocking`；
//! - 播放命令（pause/seek/volume…）直接调 libmpv，非阻塞；
//! - 事件线程 5Hz 轮询属性 → `player-state` 事件。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};
use vbox_player::{Player, PlayerOptions, PlayerSnapshot, RenderTarget, StateCallback};

/// 「保存本集」任务是否在跑 / 是否被用户暂停
static SAVE_ACTIVE: AtomicBool = AtomicBool::new(false);
static SAVE_CONTROL: std::sync::OnceLock<crate::hls_proxy::ExportControl> =
    std::sync::OnceLock::new();

fn save_control() -> &'static crate::hls_proxy::ExportControl {
    SAVE_CONTROL.get_or_init(crate::hls_proxy::ExportControl::new)
}

/// 保存任务结束时自动复位暂停/运行标记（含各种提前 return）。
struct SaveGuard;

impl Drop for SaveGuard {
    fn drop(&mut self) {
        SAVE_ACTIVE.store(false, Ordering::SeqCst);
        save_control().resume();
    }
}

/// 等待手动「保存本集」结束（最多 30 分钟，与导出超时一致）。
///
/// 关闭/切换会话时如果手动保存还在导出，立即 `close_session` 会删掉它正在用的缓存分片；
/// 等保存结束后再按「是否已落盘」决定清理缓存还是保留。
fn wait_save_idle() {
    let mut waited_ms = 0u32;
    while SAVE_ACTIVE.load(Ordering::SeqCst) && waited_ms < 30 * 60 * 1000 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        waited_ms += 100;
    }
}

/// 播放器托管状态。
pub struct PlayerHub {
    /// 最近一次状态快照（`player_state` 命令直接返回，避免每次读 mpv）
    state: Arc<Mutex<PlayerSnapshot>>,
    /// libmpv 实例（懒创建；换集复用同一个实例，loadfile replace）
    player: Mutex<Option<Arc<Player>>>,
    /// 前端上报的播放器窗格 rect（物理像素，相对窗口客户区）
    rect: Mutex<(i32, i32, i32, i32)>,
    /// 前端是否允许显示视频（滚出视口 / 离开页面时置 false）
    visible_allowed: Arc<std::sync::atomic::AtomicBool>,
    /// 当前 HLS 代理会话 id（非 HLS 播放时为 None）；关闭播放/换集时清理
    hls_session: Mutex<Option<String>>,
    /// 「播放时自动保存」目标：本集播放结束时把 HLS 缓存导出成本地文件
    autosave: Mutex<Option<crate::download::SavedFileInfo>>,
    /// 手动「保存本集」已成功落盘的 HLS 会话 id：会话关闭时连同播放缓存一起清理，
    /// 避免成品文件与缓存各占一份磁盘（播放中立刻删缓存会让回拖重新拉流，所以延后到关闭时）。
    saved_hls_sessions: Mutex<std::collections::HashSet<String>>,
}

impl Default for PlayerHub {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(PlayerSnapshot::default())),
            player: Mutex::new(None),
            rect: Mutex::new((0, 0, 960, 540)),
            visible_allowed: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            hls_session: Mutex::new(None),
            autosave: Mutex::new(None),
            saved_hls_sessions: Mutex::new(std::collections::HashSet::new()),
        }
    }
}

impl PlayerHub {
    fn snapshot(&self) -> PlayerSnapshot {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn current_rect(&self) -> (i32, i32, i32, i32) {
        *self.rect.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_rect(&self, rect: (i32, i32, i32, i32)) {
        *self.rect.lock().unwrap_or_else(|e| e.into_inner()) = rect;
    }

    pub(crate) fn player(&self) -> Option<Arc<Player>> {
        self.player
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone))
    }

    fn set_player(&self, player: Arc<Player>) {
        if let Ok(mut guard) = self.player.lock() {
            *guard = Some(player);
        }
    }

    /// 记录当前 HLS 代理会话（供 player_close / 换集时清理）。
    fn set_hls_session(&self, id: String) {
        if let Ok(mut guard) = self.hls_session.lock() {
            *guard = Some(id);
        }
    }

    /// 取出并清空 HLS 会话 id。
    fn take_hls_session(&self) -> Option<String> {
        self.hls_session
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }

    /// 读取当前 HLS 会话 id（不清理，供「保存到本地」使用）。
    fn hls_session_id(&self) -> Option<String> {
        self.hls_session.lock().ok().and_then(|guard| guard.clone())
    }

    /// 标记「手动保存本集」已成功落盘：会话关闭时连同播放缓存一起清理。
    fn mark_hls_session_saved(&self, id: &str) {
        if let Ok(mut guard) = self.saved_hls_sessions.lock() {
            guard.insert(id.to_string());
        }
    }

    /// 取出「该会话已手动保存过」标记（会话关闭时消费一次）。
    fn take_hls_session_saved(&self, id: &str) -> bool {
        self.saved_hls_sessions
            .lock()
            .ok()
            .map(|mut guard| guard.remove(id))
            .unwrap_or(false)
    }

    /// 记录「播放时自动保存」目标（本集结束时导出）。
    fn arm_autosave(&self, info: crate::download::SavedFileInfo) {
        if let Ok(mut guard) = self.autosave.lock() {
            *guard = Some(info);
        }
    }

    /// 取出并清空自动保存目标。
    fn take_autosave(&self) -> Option<crate::download::SavedFileInfo> {
        self.autosave.lock().ok().and_then(|mut guard| guard.take())
    }

    /// 把状态同步到 UI（事件 + 缓存）。
    fn apply_state(&self, app: &AppHandle, state: PlayerSnapshot) {
        if let Ok(mut slot) = self.state.lock() {
            *slot = state.clone();
        }
        let _ = app.emit("player-state", state);
    }

    /// 构造 libmpv 状态回调（事件线程调用）。
    fn state_callback(&self, app: AppHandle) -> StateCallback {
        let slot = Arc::clone(&self.state);
        Arc::new(move |state: PlayerSnapshot| {
            let previous = slot
                .lock()
                .map(|guard| guard.status.clone())
                .unwrap_or_default();
            if let Ok(mut guard) = slot.lock() {
                *guard = state.clone();
            }
            if previous != state.status {
                // hwdec-current 为空 ≠ 软解：#9 的关键。
                // - 解码器还没建立（idle/opening/换源缓冲）或属性暂不可用时为空 → 「等待硬解（未知）」；
                // - 已经有真实视频帧（video_ready）仍为空，才判定为真正回退软解。
                let hwdec_label = if !state.hwdec_current.is_empty() {
                    state.hwdec_current.clone()
                } else {
                    // 空值 = 未知（解码器未建立/属性暂不可用），把 requested 一并写出来，
                    // 避免日志里只看到「软解」而不知道实际请求的是什么硬解模式。
                    let requested = requested_hwdec(&app);
                    if !state.video_ready
                        || matches!(state.status.as_str(), "idle" | "opening" | "ended")
                    {
                        format!("等待硬解（解码器未建立/未知，requested={requested}）")
                    } else {
                        format!("软解（硬解未生效，requested={requested}）")
                    }
                };
                crate::logs::push(
                    "info",
                    format!(
                        "播放器状态={} pos={:.1}s dur={:.1}s size={}x{} fps={:.2} hwdec={} drops={}/{} ready={}{}",
                        state.status,
                        state.position_sec,
                        state.duration_sec,
                        state.width,
                        state.height,
                        state.container_fps,
                        hwdec_label,
                        state.frame_drops,
                        state.decoder_frame_drops,
                        state.video_ready,
                        if state.error.is_empty() {
                            String::new()
                        } else {
                            format!(" error={}", state.error)
                        }
                    ),
                );
                // 已有真实画面却没有硬件解码器：给一次限频原因说明，避免用户以为「一直都是软解」
                if state.status == "playing" && state.video_ready && state.hwdec_current.is_empty()
                {
                    log_hwdec_fallback(&app);
                }
            }
            let _ = app.emit("player-state", state);
        })
    }
}

/// 当前请求的硬解模式：优先读 mpv 实际生效的 `hwdec` 选项，读不到时回退到已保存的播放设置。
fn requested_hwdec(app: &AppHandle) -> String {
    if let Some(value) = app
        .state::<PlayerHub>()
        .player()
        .and_then(|player| player.property_string("hwdec"))
        .filter(|value| !value.trim().is_empty())
    {
        return value;
    }
    app.try_state::<crate::player_prefs::PlayerPrefsStore>()
        .map(|store| store.load().hwdec)
        .unwrap_or_else(|| "auto-safe".to_string())
}

/// 记录「硬解未生效、已回退软解」的排障原因（限频，避免刷屏）。
///
/// 只有在真实播放（有视频帧）且 `hwdec-current` 为空时调用；`hwdec=no` 是用户主动关硬解，
/// 不告警。读取的 `hwdec` 是 mpv 当前生效的配置值，`video-codec` / `pixelformat` 用来解释
/// 为什么某些流只能软解（10bit / 4:2:2 / 显卡驱动不支持等）。
fn log_hwdec_fallback(app: &AppHandle) {
    let hub = app.state::<PlayerHub>();
    let Some(player) = hub.player() else {
        return;
    };
    let requested = requested_hwdec(app);
    if requested.eq_ignore_ascii_case("no") {
        return;
    }
    let codec = player
        .property_string("video-codec")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "未知".to_string());
    let pixel_format = player
        .property_string("video-params/pixelformat")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "未知".to_string());
    crate::logs::push_throttled(
        "player-hwdec-soft-fallback",
        "warn",
        format!(
            "硬解未生效：requested={requested}，当前为软解（codec={codec}，pixfmt={pixel_format}）。\
             mpv 已自动回退软件解码，常见原因是该流编码/位深（如 10bit HEVC、4:2:2）或显卡驱动\
             不支持所选硬解；可在设置页改用「D3D11VA 硬解」或恢复「自动」后重试。"
        ),
        std::time::Duration::from_secs(30),
    );
}

/// 主窗口渲染句柄（作为 mpv 的 `wid`）。
///
/// - Windows：HWND（窗口本体 + DComp 之下，配合挖洞露出）
/// - macOS：方案 2 创建的专用 CAMetalLayer 视图（setup 时挂到 contentView 底部）；
///   视图没建好时退回 contentView，画面仍不可用，但音频/状态不受影响
#[cfg(windows)]
fn main_window_render_handle(app: &AppHandle) -> Result<usize, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    let hwnd = window
        .hwnd()
        .map_err(|e| format!("读取主窗口句柄失败: {e}"))?;
    Ok(hwnd.0 as usize)
}

#[cfg(target_os = "macos")]
fn main_window_render_handle(app: &AppHandle) -> Result<usize, String> {
    if let Some(ptr) = crate::macos_video::view_ptr() {
        return Ok(ptr);
    }
    // 兜底：视频视图创建失败时仍用 contentView，避免 macOS 上完全无法播放。
    crate::logs::push(
        "warn",
        "macOS 视频视图未创建，回退到窗口 contentView（可能仍无画面）",
    );
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    let view = window
        .ns_view()
        .map_err(|e| format!("读取主窗口 NSView 失败: {e}"))?;
    Ok(view as usize)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main_window_render_handle(_app: &AppHandle) -> Result<usize, String> {
    Err("当前平台暂不支持内嵌播放（仅 Windows / macOS）".to_string())
}

/// 把播放器窗格 rect 换算成 mpv 的 `video-margin-ratio-*`，让视频只画在窗格里。
fn apply_margins(app: &AppHandle, hub: &PlayerHub) {
    let Some(player) = hub.player() else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let width = size.width.max(1) as f64;
    let height = size.height.max(1) as f64;
    let (x, y, w, h) = hub.current_rect();
    let left = (x as f64 / width).clamp(0.0, 1.0);
    let top = (y as f64 / height).clamp(0.0, 1.0);
    let right = (1.0 - (x as f64 + w as f64) / width).clamp(0.0, 1.0);
    let bottom = (1.0 - (y as f64 + h as f64) / height).clamp(0.0, 1.0);
    player.set_video_margins(left, top, right, bottom);
}

/* ---------------- Tauri 命令 ---------------- */

/// 打开媒体并开始播放（m3u8/mp4 直链）。
#[tauri::command]
pub async fn player_open(
    app: AppHandle,
    url: String,
    headers: Option<HashMap<String, String>>,
    start_sec: Option<f64>,
) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("播放地址为空".to_string());
    }
    let hwnd = main_window_render_handle(&app)?;

    // 播放设置（硬解 / 上次音量 / 默认倍速）：来自 <app_data_dir>/player.json。
    // 必须在**创建播放器之前**读取：hwdec/volume/rate 要作为 mpv 初始化选项写入，
    // 保证第一个解码器/首帧就按用户设置工作（#8 上次音量、#9 硬解模式）。
    let prefs = app
        .try_state::<crate::player_prefs::PlayerPrefsStore>()
        .map(|store| store.load())
        .unwrap_or_default();

    let player = match app.state::<PlayerHub>().player() {
        Some(player) => player,
        None => {
            let callback = app.state::<PlayerHub>().state_callback(app.clone());
            let init_options = PlayerOptions {
                hwdec: prefs.hwdec.clone(),
                volume: prefs.volume,
                rate: prefs.rate,
            };
            let created = tauri::async_runtime::spawn_blocking(move || {
                Player::new_with_options(
                    RenderTarget::from_handle(hwnd, 1280, 720),
                    Some(callback),
                    &init_options,
                )
            })
            .await
            .map_err(|e| format!("创建播放器任务失败: {e}"))?
            .map_err(|e| e.to_string())?;
            crate::logs::push(
                "info",
                format!(
                    "播放器已初始化：hwdec={} volume={:.0} rate={:.2}",
                    prefs.hwdec, prefs.volume, prefs.rate
                ),
            );
            let player = Arc::new(created);
            app.state::<PlayerHub>().set_player(Arc::clone(&player));
            player
        }
    };

    // 先按当前窗格 rect 限定视频区域，再 loadfile，避免首帧铺满整个窗口
    apply_margins(&app, &app.state::<PlayerHub>());

    // 让 mpv 的磁盘缓存落在「下载/缓存目录」下（默认文档/VBox/cache），
    // 而不是系统临时目录。目录创建失败时忽略——只影响缓存位置，不影响播放。
    if let Some(store) = app.try_state::<crate::download::DownloadStore>() {
        let cache_dir = store.cache_dir();
        if std::fs::create_dir_all(&cache_dir).is_ok() {
            player.set_cache_dir(&cache_dir);
        }
    }

    // 播放器实例会在换集之间复用，用户可能刚在设置页改过硬解：每次打开都重新应用一次。
    // 失败不再静默忽略（#9：错误被吞会表现为「设置/日志与实际解码方式不一致」）。
    if let Err(err) = player.set_hwdec(&prefs.hwdec) {
        crate::logs::push(
            "warn",
            format!("应用硬解设置失败（hwdec={}）：{err}", prefs.hwdec),
        );
    }

    // 画质增强（Anime4K 超分）与插件（GLSL / lavfi）：内置 Anime4K 现在也是插件，
    // 旧版把档位存在 player.json（prefs.enhance），首次升级时迁移到插件状态。
    crate::plugins::migrate_bundled_enhance(&app, &prefs.enhance);
    if let Err(err) = crate::plugins::apply_render(&app) {
        crate::logs::push(
            "warn",
            format!("应用画质增强/插件失败（{}）：{err}", prefs.enhance),
        );
    }

    let header_list: Vec<(String, String)> = headers
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
        .collect();

    // HLS 走本地代理：真正边下边播（只等首段起播，后续按需拉取并落盘复用）。
    // 建会话失败（源不可达/不是标准 HLS）时回退为 mpv 直连，行为与之前一致。
    let mut play_url = url.clone();
    let mut play_headers = header_list.clone();
    let hub = app.state::<PlayerHub>();
    if crate::hls_proxy::is_hls_url(&url)
        && app
            .try_state::<std::sync::Arc<crate::hls_proxy::HlsProxy>>()
            .is_some()
    {
        // 换集前先收尾上一个会话（若开了自动保存，会在后台导出）
        if let Some(previous) = hub.take_hls_session() {
            finalize_hls_session(&app, previous);
        }
        if let Some(proxy) = app.try_state::<std::sync::Arc<crate::hls_proxy::HlsProxy>>() {
            // create_session 内部用 reqwest::blocking（自带 tokio runtime），
            // 若在 async worker 上创建/销毁会 panic（Cannot drop a runtime in a
            // context where blocking is not allowed），必须放到阻塞线程池。
            let proxy = std::sync::Arc::clone(proxy.inner());
            let session_url = url.clone();
            let session_headers = header_list.clone();
            let created = tauri::async_runtime::spawn_blocking(move || {
                proxy.create_session(&session_url, &session_headers)
            })
            .await;
            match created {
                Ok(Ok(session)) => {
                    crate::logs::push(
                        "info",
                        format!("HLS 会话已建立: {} -> {}", session.id, session.url),
                    );
                    play_url = session.url;
                    // 请求头已经由代理带上，mpv 直连本地服务不需要再带
                    play_headers.clear();
                    hub.set_hls_session(session.id);
                }
                Ok(Err(err)) => {
                    crate::logs::push("warn", format!("HLS 代理会话创建失败，回退直连: {err}"));
                }
                Err(err) => {
                    crate::logs::push("error", format!("HLS 代理会话任务失败，回退直连: {err}"));
                }
            }
        }
    }

    let start = start_sec.filter(|sec| sec.is_finite() && *sec > 0.0);
    tauri::async_runtime::spawn_blocking({
        let player = Arc::clone(&player);
        move || player.open(&play_url, &play_headers, start)
    })
    .await
    .map_err(|e| format!("打开媒体任务失败: {e}"))?
    .map_err(|e| e.to_string())?;

    // 起播后再应用上次音量/倍速，保证不被 open() 内部的重置覆盖。
    // 失败记录到运行日志，避免「音量记忆丢失」这类问题无迹可查（#8）。
    if let Err(err) = player.set_volume(prefs.volume) {
        crate::logs::push(
            "warn",
            format!("应用上次音量失败（volume={}）：{err}", prefs.volume),
        );
    }
    if let Err(err) = player.set_rate(prefs.rate) {
        crate::logs::push(
            "warn",
            format!("应用默认倍速失败（rate={}）：{err}", prefs.rate),
        );
    }
    Ok(())
}

/// 关闭当前播放：停止、状态回到 idle（保留 mpv 实例，便于下次快速起播）。
#[tauri::command]
pub fn player_close(app: AppHandle) -> Result<(), String> {
    // 先把当前音量写回 player.json：下次启动仍是用户上次调节的值（支持 0 / 100）。
    remember_volume(&app);
    let hub = app.state::<PlayerHub>();
    if let Some(player) = hub.player() {
        let _ = player.stop();
    }
    // 关闭 HLS 代理会话：若本集被标记为「自动保存」，先后台导出再清理
    if let Some(id) = hub.take_hls_session() {
        finalize_hls_session(&app, id);
    }
    // 没有会话也要清掉可能残留的自动保存目标
    let _ = hub.take_autosave();
    hub.apply_state(&app, PlayerSnapshot::default());
    Ok(())
}

/// 「播放时自动保存」：为本集 HLS 会话登记导出目标。
///
/// 返回 false 表示当前没有 HLS 代理会话（例如 mp4 直链或代理被降级），
/// 前端应回退到原来的「下载队列」路径。
#[tauri::command]
pub fn player_arm_autosave(
    app: AppHandle,
    info: crate::download::SavedFileInfo,
) -> Result<bool, String> {
    let hub = app.state::<PlayerHub>();
    if hub.hls_session_id().is_none() {
        return Ok(false);
    }
    hub.arm_autosave(info);
    Ok(true)
}

/// 收尾一个 HLS 会话：有自动保存目标就后台导出 + 登记下载记录，然后删缓存目录。
///
/// 导出可能还要补下未看的分片，不能阻塞 UI（player_close / 换集都是即时返回）。
fn finalize_hls_session(app: &AppHandle, id: String) {
    let target = app.state::<PlayerHub>().take_autosave();
    let Some(proxy) = app
        .try_state::<std::sync::Arc<crate::hls_proxy::HlsProxy>>()
        .map(|state| state.inner().clone())
    else {
        return;
    };
    let Some(info) = target else {
        // 手动保存已成功落盘：关闭会话时连播放缓存一起删，避免成品与缓存各占一份磁盘；
        // 否则保留缓存供下次播放/重启复用（close_session 会销毁 reqwest::blocking 客户端，
        // 从 async 上下文直接调用会 panic，统一丢给阻塞线程池）。
        // 手动「保存本集」可能还在导出：等它结束再决定清理/保留，否则删缓存会打断导出。
        let app = app.clone();
        let proxy = std::sync::Arc::clone(&proxy);
        tauri::async_runtime::spawn_blocking(move || {
            wait_save_idle();
            let was_saved = app.state::<PlayerHub>().take_hls_session_saved(&id);
            if was_saved {
                proxy.close_session_and_clean_cache(&id)
            } else {
                proxy.close_session(&id)
            }
        });
        return;
    };
    let store = app
        .try_state::<crate::download::DownloadStore>()
        .map(|state| state.inner().clone());
    // 去重：同一集已有排队 / 下载中的任务、或已有可用成品（下载已从缓存产出）时跳过导出，
    // 否则「下载」和「边下边播」会各产出一份文件（双份磁盘 / 双份流量）。
    if let Some(store) = &store {
        if store.has_active_task_for(&info) || store.has_completed_file_for(&info) {
            let label = if info.ep_title.trim().is_empty() {
                info.name.clone()
            } else {
                format!("{} {}", info.name, info.ep_title)
            };
            crate::logs::push(
                "info",
                format!(
                    "边下边播跳过导出：{label} 已有下载任务或成品（缓存已复用），避免重复导出；该集缓存同步清理"
                ),
            );
            let proxy = std::sync::Arc::clone(&proxy);
            tauri::async_runtime::spawn_blocking(move || proxy.close_session_and_clean_cache(&id));
            return;
        }
    }
    std::thread::Builder::new()
        .name("vbox-hls-autosave".into())
        .spawn(move || {
            let label = if info.ep_title.trim().is_empty() {
                info.name.clone()
            } else {
                format!("{} {}", info.name, info.ep_title)
            };
            let media_dir = store
                .as_ref()
                .map(|store| store.media_dir())
                .unwrap_or_else(|| std::env::temp_dir().join("VBox"));
            let dir = crate::download::episode_dir_for_info(&media_dir, &info);
            let stem = crate::download::episode_stem_for_info(&media_dir, &info);
            // 与手动「保存本集」串行化：手动保存可能正在进行（或刚完成）。
            // 拿到导出锁后再查一次成品，避免同一集导出第二份 / 争抢同名文件。
            let _export_guard = proxy.export_guard();
            if let Some(store) = &store {
                if store.has_completed_file_for(&info) {
                    crate::logs::push(
                        "info",
                        format!("自动保存跳过：{label} 已有可用成品（避免重复导出），缓存同步清理"),
                    );
                    proxy.close_session_and_clean_cache(&id);
                    return;
                }
            }
            crate::logs::push("info", format!("开始自动保存: {label}"));
            match proxy.export_with_locked(&id, &dir, &stem, None) {
                Ok(path) => {
                    crate::logs::push("info", format!("自动保存完成: {}", path.display()));
                    if let Some(store) = &store {
                        match store.add_completed_local_file(&info, &path) {
                            Ok(task) => crate::logs::push(
                                "info",
                                format!("自动保存已登记下载记录 {}", task.id),
                            ),
                            Err(err) => crate::logs::push(
                                "warn",
                                format!("自动保存已导出但登记下载记录失败: {err}"),
                            ),
                        }
                    }
                    // 成品已落盘：清理该集播放缓存，避免同一集同时留「成品 + 缓存」两份
                    proxy.close_session_and_clean_cache(&id);
                }
                Err(err) => {
                    crate::logs::push("error", format!("自动保存失败: {err}"));
                    // 导出失败保留缓存，方便重试/继续观看
                    proxy.close_session(&id);
                }
            }
        })
        .ok();
}

/// 把当前播放内容保存到下载目录（能播就能存）。
///
/// - HLS 代理会话：复用已缓存分片，未看部分按序补下，支持 AES-128 / SAMPLE-AES / 分离音轨 / BYTERANGE；
/// - 非 HLS（mp4 直链 / DASH / 无 `.m3u8` 后缀的流）：用 ffmpeg 直接读播放地址重封装；
/// - 保存成功后登记为一条 `completed` 下载记录（下载页可见、可播放/删除）。
///
/// 命令这一层只负责「开始 / 结束 / 失败」三条运行日志，实际逻辑在 `save_current_inner`。
#[tauri::command]
pub async fn player_save_current(
    app: AppHandle,
    info: crate::download::SavedFileInfo,
) -> Result<String, String> {
    // 同一时刻只允许一个手动保存：并发导出同一集会争抢同名文件（曾出现「保存完成」后
    // 另一个导出 rename 报 os error 2）。前端按钮虽有三态，但页面重载 / HMR 后状态会丢。
    if SAVE_ACTIVE
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("已有保存任务进行中，请等待完成或先暂停".to_string());
    }
    let label = save_label(&info.name, Some(&info.ep_title));
    crate::logs::push("info", format!("开始保存本集: {label}"));
    let result = save_current_inner(app, info).await;
    match &result {
        Ok(path) => crate::logs::push("info", format!("保存本集完成: {path}")),
        Err(err) => crate::logs::push("error", format!("保存本集失败: {label} - {err}")),
    }
    result
}

/// 剧集标签：「剧名 第N集」；任意一侧为空时退化，避免出现多余空格。
fn save_label(name: &str, episode: Option<&str>) -> String {
    let series = name.trim();
    let ep = episode.unwrap_or_default().trim();
    if series.is_empty() {
        ep.to_string()
    } else if ep.is_empty() {
        series.to_string()
    } else {
        format!("{series} {ep}")
    }
}

/// 「保存本集」进度上报线程：把 HLS 导出的分片进度写进下载记录并推送到下载页。
/// Drop 时停止线程（成功 / 失败 / 提前 return 都会触发）。
struct ExportProgressMonitor {
    stop: std::sync::Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ExportProgressMonitor {
    fn start(
        store: crate::download::DownloadStore,
        task_id: String,
        control: crate::hls_proxy::ExportControl,
    ) -> Self {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let monitor_stop = std::sync::Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("vbox-save-progress".to_string())
            .spawn(move || {
                let mut last_bytes = 0u64;
                let mut last_at = std::time::Instant::now();
                loop {
                    if monitor_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    if monitor_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let (done, total, bytes) = control.progress();
                    let progress = if total > 0 {
                        (done as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };
                    let dt = last_at.elapsed().as_secs_f64().max(0.001);
                    let speed = bytes.saturating_sub(last_bytes) as f64 / dt;
                    last_bytes = bytes;
                    last_at = std::time::Instant::now();
                    let status = if control.is_paused() {
                        "paused"
                    } else {
                        "downloading"
                    };
                    let _ =
                        store.update_local_export(&task_id, status, bytes as i64, progress, speed);
                }
            })
            .ok();
        Self { stop, handle }
    }
}

impl Drop for ExportProgressMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 非 HLS（或没有代理会话）兜底：交给 ffmpeg 把播放地址重封装成 ts/mp4。
/// 与 HLS 导出共用同一个「保存任务」记录（进度按未知处理，完成/失败由调用方落库）。
async fn remux_current_url(
    dir: std::path::PathBuf,
    stem: String,
    url: String,
    control: crate::hls_proxy::ExportControl,
) -> Result<std::path::PathBuf, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("当前播放没有可保存的网络地址".to_string());
    }
    let ext = if url
        .split('?')
        .next()
        .unwrap_or(&url)
        .to_ascii_lowercase()
        .ends_with(".mp4")
    {
        "mp4"
    } else {
        "ts"
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建保存目录失败: {e}"))?;
    let safe_stem = crate::hls_proxy::sanitize_file_name(&stem);
    let dest = crate::hls_proxy::unique_output_path(&dir, &safe_stem, ext);
    let part = crate::hls_proxy::unique_part_path(&dest);
    let part_clone = part.clone();
    let input = url.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::hls_util::run_ffmpeg_remux(&input, &part_clone, std::time::Duration::from_secs(1800))
    })
    .await
    .map_err(|err| format!("保存任务失败: {err}"))??;
    // ffmpeg 无法中途暂停：处于暂停态时先挂起再落盘
    let pause_control = control.clone();
    tauri::async_runtime::spawn_blocking(move || {
        while pause_control.is_paused() {
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
    })
    .await
    .map_err(|err| format!("保存任务失败: {err}"))?;
    crate::hls_proxy::commit_output(&part, &dest)
}

async fn save_current_inner(
    app: AppHandle,
    info: crate::download::SavedFileInfo,
) -> Result<String, String> {
    let hub = app.state::<PlayerHub>();
    let session_id = hub.hls_session_id();
    // 会话 id 副本：保存成功后标记；会话关闭时据此清理播放缓存（成品与缓存不重复占盘）
    let saved_session_id = session_id.clone();
    let current_url = hub.snapshot().url;

    // 下载页可见性：保存开始就先登记一条 downloading 记录，导出过程中更新进度，
    // 完成 / 失败再落到对应状态。旧实现只在导出结束后才登记，导出期间下载页
    // 什么都没有，用户会误以为「点了保存没反应」（整集补下可能要几分钟）。
    let store = app
        .try_state::<crate::download::DownloadStore>()
        .map(|state| state.inner().clone());
    let media_dir = store
        .as_ref()
        .map(|store| store.media_dir())
        .unwrap_or_else(|| std::env::temp_dir().join("VBox"));
    let dir = crate::download::episode_dir_for_info(&media_dir, &info);
    let stem = crate::download::episode_stem_for_info(&media_dir, &info);

    let export_task_id = match &store {
        Some(store) => match store.begin_local_export(&info) {
            Ok(task) => Some(task.id),
            Err(err) => {
                crate::logs::push(
                    "warn",
                    format!("登记保存任务失败（继续保存，完成后仍会登记）: {err}"),
                );
                None
            }
        },
        None => None,
    };

    // 保存任务开始：清掉上次暂停状态（SAVE_ACTIVE 已由 player_save_current 置位，
    // 这里由 SaveGuard 在结束时复位，含各种提前 return）。
    let control = save_control().clone();
    control.resume();
    let _save_guard = SaveGuard;

    // 进度上报线程：提前 return 时由 Drop 停止
    let _progress_monitor = match (&store, &export_task_id) {
        (Some(store), Some(task_id)) => Some(ExportProgressMonitor::start(
            store.clone(),
            task_id.clone(),
            control.clone(),
        )),
        _ => None,
    };

    let export_result: Result<std::path::PathBuf, String> = match session_id {
        Some(id) => {
            let Some(proxy) = app
                .try_state::<std::sync::Arc<crate::hls_proxy::HlsProxy>>()
                .map(|state| state.inner().clone())
            else {
                return Err("HLS 代理未启用".to_string());
            };
            let task_control = control.clone();
            tauri::async_runtime::spawn_blocking(move || {
                proxy.export_with(&id, &dir, &stem, Some(&task_control))
            })
            .await
            .map_err(|err| format!("保存任务失败: {err}"))
            .and_then(|result| result)
        }
        None => remux_current_url(dir, stem, current_url.trim().to_string(), control.clone()).await,
    };

    let path = match export_result {
        Ok(path) => path,
        Err(err) => {
            // 导出失败：把下载页那条「保存中」记录改成 failed（不再静默消失）
            if let (Some(store), Some(task_id)) = (&store, &export_task_id) {
                let _ = store.fail_local_export(task_id, &err);
            }
            return Err(err);
        }
    };

    // 完成登记：优先把开始时的「保存中」记录改成 completed；
    // 下载库不可用 / 登记失败时退回原路径（直接登记一条 completed 记录）。
    match (&store, &export_task_id) {
        (Some(store), Some(task_id)) => match store.complete_local_export(task_id, &path) {
            Ok(_) => crate::logs::push(
                "info",
                format!("保存本集已登记下载记录 {task_id} -> {}", path.display()),
            ),
            Err(err) => crate::logs::push(
                "warn",
                format!("保存本集已导出但更新下载记录失败（文件仍在）: {err}"),
            ),
        },
        _ => {
            if let Some(store) = &store {
                match store.add_completed_local_file(&info, &path) {
                    Ok(task) => crate::logs::push(
                        "info",
                        format!("保存本集已登记下载记录 {} -> {}", task.id, path.display()),
                    ),
                    Err(err) => crate::logs::push(
                        "warn",
                        format!("保存本集已导出但登记下载记录失败（文件仍在）: {err}"),
                    ),
                }
            }
        }
    }
    // 手动保存已落盘：标记会话，等播放器关闭时连同播放缓存一起清理
    // （播放中立刻删缓存会让回拖重新拉流，所以延后到会话关闭）。
    if let Some(id) = saved_session_id.as_deref() {
        hub.mark_hls_session_saved(id);
    }
    // 清掉自动保存目标，避免关闭时再导出一份
    let _ = hub.take_autosave();

    Ok(path.to_string_lossy().to_string())
}

/// 暂停当前「保存本集」任务（返回 true 表示状态已变为暂停）。
#[tauri::command]
pub fn player_save_pause() -> bool {
    if !SAVE_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    if save_control().is_paused() {
        return false;
    }
    save_control().pause();
    crate::logs::push("info", "保存本集已暂停（已下的分片保留，可点「继续」恢复）");
    true
}

/// 继续当前「保存本集」任务（返回 true 表示状态已恢复为进行中）。
#[tauri::command]
pub fn player_save_resume() -> bool {
    if !SAVE_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    if !save_control().is_paused() {
        return false;
    }
    save_control().resume();
    crate::logs::push("info", "保存本集继续");
    true
}

#[tauri::command]
pub fn player_play(app: AppHandle) -> Result<(), String> {
    with_player(&app, |player| player.play())
}

#[tauri::command]
pub fn player_pause(app: AppHandle) -> Result<(), String> {
    with_player(&app, |player| player.pause())
}

/// 切换播放/暂停，返回切换后是否暂停。
#[tauri::command]
pub fn player_toggle_pause(app: AppHandle) -> Result<bool, String> {
    let player = require_player(&app)?;
    let paused = player.toggle_pause().map_err(|e| e.to_string())?;
    let snapshot = player.snapshot();
    app.state::<PlayerHub>().apply_state(&app, snapshot);
    Ok(paused)
}

#[tauri::command]
pub fn player_seek(app: AppHandle, sec: f64) -> Result<(), String> {
    with_player(&app, |player| player.seek(sec))
}

#[tauri::command]
pub fn player_seek_relative(app: AppHandle, delta: f64) -> Result<(), String> {
    with_player(&app, |player| player.seek_relative(delta))
}

#[tauri::command]
pub fn player_set_volume(app: AppHandle, volume: f64) -> Result<(), String> {
    with_player(&app, |player| player.set_volume(volume))
}

/// 把 mpv 当前音量写回 `player.json`（关闭播放 / 退出应用时调用）。
///
/// 设置页已不再提供「默认音量」：音量由用户在播放时调节，这里记住最后一次的值，
/// 下次创建播放器时作为初始音量恢复（音量 0 / 100 也会被记住）。
pub fn remember_volume(app: &AppHandle) {
    let Some(volume) = app
        .state::<PlayerHub>()
        .player()
        .and_then(|player| player.property_f64("volume"))
    else {
        return;
    };
    if !volume.is_finite() {
        return;
    }
    let Some(store) = app.try_state::<crate::player_prefs::PlayerPrefsStore>() else {
        return;
    };
    let current = store.load();
    if (current.volume - volume).abs() < 0.05 {
        return;
    }
    let mut next = current;
    next.volume = volume;
    if let Err(err) = store.save(&next) {
        eprintln!("[vbox] 保存上次音量失败: {err}");
    }
}

#[tauri::command]
pub fn player_set_mute(app: AppHandle, muted: bool) -> Result<(), String> {
    with_player(&app, |player| player.set_mute(muted))
}

#[tauri::command]
pub fn player_set_rate(app: AppHandle, rate: f64) -> Result<(), String> {
    with_player(&app, |player| player.set_rate(rate))
}

/// 播放器页面「画质」开关：驱动内置 Anime4K 插件（启用 + 预设），并把选择写回
/// `player.json`（兼容旧版/其它读取方）。
///
/// - 模式：`off` / `lite` / `standard` / `strong`（非法值按 off 处理）；
/// - 播放器还没创建时也会更新插件状态，下一次 `player_open` 会按插件状态应用；
/// - 返回实际生效的模式（便于前端把 UI 回正）。
#[tauri::command]
pub fn player_set_enhance(app: AppHandle, mode: String) -> Result<String, String> {
    let normalized = crate::player_prefs::normalize_enhance(&mode);
    if let Some(store) = app.try_state::<crate::player_prefs::PlayerPrefsStore>() {
        let mut prefs = store.load();
        prefs.enhance = normalized.clone();
        if let Err(err) = store.save(&prefs) {
            crate::logs::push("warn", format!("保存画质增强设置失败：{err}"));
        }
    }
    crate::plugins::set_bundled_enhance(&app, &normalized)?;
    Ok(normalized)
}

/// 切换音轨（id 为 mpv track id 字符串；"no" / 空串表示关闭）。
///
/// 设置后立即重建一次快照并广播 `player-state`，让 UI 马上高亮新选中轨道，
/// 不用等事件线程的下一拍。
#[tauri::command]
pub fn player_set_audio_track(app: AppHandle, id: String) -> Result<(), String> {
    let player = require_player(&app)?;
    player.set_audio_track(&id).map_err(|e| e.to_string())?;
    player.refresh();
    Ok(())
}

/// 切换字幕轨（id 为 mpv track id 字符串；"no" / 空串表示关闭）。
#[tauri::command]
pub fn player_set_subtitle_track(app: AppHandle, id: String) -> Result<(), String> {
    let player = require_player(&app)?;
    player.set_subtitle_track(&id).map_err(|e| e.to_string())?;
    player.refresh();
    Ok(())
}

/// 加载外挂字幕（本地文件路径）；成功后自动选中新字幕轨并广播状态。
#[tauri::command]
pub fn player_add_subtitle(app: AppHandle, path: String) -> Result<(), String> {
    let player = require_player(&app)?;
    player.add_subtitle(&path).map_err(|e| e.to_string())?;
    // `add_subtitle` 内部已 refresh（会回调一次），这里再同步一次缓存，保证
    // `player_state` 命令和事件与 UI 看到的一致。
    app.state::<PlayerHub>()
        .apply_state(&app, player.snapshot());
    Ok(())
}

/// 原生「选择字幕文件」对话框（后台线程执行，避免阻塞主线程；取消返回 null）。
#[tauri::command]
pub async fn player_pick_subtitle() -> Option<String> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("选择字幕文件")
            .add_filter(
                "字幕文件",
                &[
                    "srt", "ass", "ssa", "vtt", "sub", "sup", "smi", "lrc", "ttml", "dfxp",
                ],
            )
            .pick_file()
    })
    .await
    .ok()
    .flatten()
    .map(|path| path.to_string_lossy().to_string())
}

/// 上报播放器窗格位置（物理像素，相对窗口客户区）；同时把视频区域限制到该 rect。
#[tauri::command]
pub fn player_set_rect(
    app: AppHandle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let hub = app.state::<PlayerHub>();
    hub.set_rect((x, y, width.max(1), height.max(1)));
    apply_margins(&app, &hub);
    Ok(())
}

/// 手动显隐（滚出视口/离开页面时前端置 false；是否绘制由前端洞决定）。
#[tauri::command]
pub fn player_set_visible(app: AppHandle, visible: bool) -> Result<(), String> {
    let hub = app.state::<PlayerHub>();
    hub.visible_allowed
        .store(visible, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// 当前状态快照。
#[tauri::command]
pub fn player_state(app: AppHandle) -> PlayerSnapshot {
    app.state::<PlayerHub>().snapshot()
}

/* ---------------- 内部工具 ---------------- */

fn require_player(app: &AppHandle) -> Result<Arc<Player>, String> {
    app.state::<PlayerHub>()
        .player()
        .ok_or_else(|| "播放器尚未初始化（请先选择剧集）".to_string())
}

fn with_player<F>(app: &AppHandle, action: F) -> Result<(), String>
where
    F: FnOnce(&Player) -> Result<(), vbox_player::PlayerError>,
{
    let player = require_player(app)?;
    action(&player).map_err(|e| e.to_string())?;
    let snapshot = player.snapshot();
    app.state::<PlayerHub>().apply_state(app, snapshot);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::SavedFileInfo;

    /// 自动保存目标：登记后可取出，取出后清空（不会在关闭时重复导出）。
    #[test]
    fn autosave_target_roundtrip() {
        let hub = PlayerHub::default();
        assert!(hub.take_autosave().is_none(), "默认不应有自动保存目标");
        hub.arm_autosave(SavedFileInfo {
            site_key: "site-a".to_string(),
            vod_id: Some("vod-1".to_string()),
            ep_index: 3,
            name: "测试剧集".to_string(),
            ep_title: "第3集".to_string(),
            ..Default::default()
        });
        let info = hub.take_autosave().expect("应能取到自动保存目标");
        assert_eq!(info.name, "测试剧集");
        assert_eq!(info.ep_title, "第3集");
        assert_eq!(info.ep_index, 3);
        assert!(hub.take_autosave().is_none(), "取出后应清空");
    }

    /// HLS 会话 id：读取不清空，take 才清空。
    #[test]
    fn hls_session_id_read_does_not_clear() {
        let hub = PlayerHub::default();
        assert!(hub.hls_session_id().is_none());
        hub.set_hls_session("abc-123".to_string());
        assert_eq!(hub.hls_session_id().as_deref(), Some("abc-123"));
        assert_eq!(hub.hls_session_id().as_deref(), Some("abc-123"));
        assert_eq!(hub.take_hls_session().as_deref(), Some("abc-123"));
        assert!(hub.hls_session_id().is_none());
    }
}
