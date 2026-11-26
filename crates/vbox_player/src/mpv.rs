//! 播放器实例（libmpv 薄封装）
//!
//! 负责：动态加载 libmpv、把渲染目标（HWND）通过 `wid` 交给 mpv、发送播放命令、
//! 在独立线程里 `mpv_wait_event` + 轮询属性，产出统一的 [`PlayerSnapshot`] 给上层。
//!
//! 线程模型：
//! - `mpv_wait_event` 只在事件线程调用（libmpv 要求单线程消费事件）；
//! - 播放命令（loadfile / pause / seek / volume / speed）可以从任意线程调用
//!   （libmpv 的 handle 是线程安全的），因此不需要每条命令都排队；
//! - 上层通过 `StateCallback` 拿到 5Hz 的状态快照（位置/时长/暂停/缓冲/错误…）。

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::ffi::{self, MpvApi, MpvEventEndFile};
use crate::render::RenderTarget;

/// 播放状态（与前端 `PlayerState.status` 对齐）
pub const STATUS_IDLE: &str = "idle";
pub const STATUS_OPENING: &str = "opening";
pub const STATUS_PLAYING: &str = "playing";
pub const STATUS_PAUSED: &str = "paused";
pub const STATUS_BUFFERING: &str = "buffering";
pub const STATUS_ENDED: &str = "ended";
pub const STATUS_ERROR: &str = "error";

/// 播放器可选轨道（音轨 / 字幕轨），由 mpv `track-list` 枚举得到。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerTrack {
    /// 轨道标识（选择时回传；对应 mpv track id 的十进制字符串）
    pub id: String,
    /// 展示名：优先 `title`，其次 `lang`，最后「音轨 N / 字幕 N」
    pub label: String,
    /// 语言标记（`lang`，缺失为空串）
    pub lang: String,
    /// 编码格式（`codec`，缺失为空串）
    pub codec: String,
    /// 是否外挂轨道（`external`）
    pub external: bool,
}

/// 播放器状态快照（序列化给前端）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerSnapshot {
    /// idle / opening / playing / paused / buffering / ended / error
    pub status: String,
    pub url: String,
    pub position_sec: f64,
    pub duration_sec: f64,
    pub paused: bool,
    pub buffering: bool,
    pub volume: f64,
    pub muted: bool,
    pub rate: f64,
    pub width: i64,
    pub height: i64,
    pub error: String,
    /// 当前播放位置之后已缓冲/缓存的秒数（`demuxer-cache-time`，未知或负值归一为 0）
    pub cache_sec: f64,
    /// 缓存下载速度（字节/秒，`cache-speed`，未知或负值归一为 0）
    pub buffer_speed_bps: f64,
    /// 是否已经有过真实视频帧输出（VIDEO_RECONFIG 后为 true）；
    /// 前端据此决定何时才在窗口上「挖洞」露出 mpv，避免缓冲等待期看到透明窗口。
    pub video_ready: bool,
    /// 片源帧率（`container-fps`，未知为 0）
    pub container_fps: f64,
    /// 估算的视频帧率（`estimated-vf-fps`，未知为 0）
    pub estimated_fps: f64,
    /// 当前硬件解码器（`hwdec-current`）。
    ///
    /// 空串表示「解码器未建立 / 属性暂不可用」（未知），**不能直接当成软解**；
    /// 真正软解时 mpv 一般返回 `no`（上层展示/日志应结合 video_ready 判断）。
    pub hwdec_current: String,
    /// 显示刷新率（`display-fps`，未知为 0）
    pub display_fps: f64,
    /// 累计丢帧数（`frame-drop-count`，仅排障参考）
    pub frame_drops: i64,
    /// 解码器丢帧数（`decoder-frame-drop-count`，仅排障参考）
    pub decoder_frame_drops: i64,
    /// 视频色彩原色（`video-params/primaries`，如 `bt.709` / `bt.2020`；空串 = 未知）
    pub video_primaries: String,
    /// 视频传输函数（`video-params/gamma`，如 `bt.1886` / `pq` / `hlg`；空串 = 未知）
    pub video_gamma: String,
    /// HDR 信号峰值（`video-params/sig-peak`，SDR 通常为 1.0；未知/负值归一为 0）
    pub video_sig_peak: f64,
    /// 可选音轨（来自 mpv `track-list`；无媒体时为空）
    pub audio_tracks: Vec<PlayerTrack>,
    /// 可选字幕轨（来自 mpv `track-list`；无媒体时为空）
    pub subtitle_tracks: Vec<PlayerTrack>,
    /// 当前音轨 id（"no" = 关闭 / 无音轨）
    pub active_audio_id: String,
    /// 当前字幕轨 id（"no" = 关闭 / 无字幕）
    pub active_subtitle_id: String,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            status: String::new(),
            url: String::new(),
            position_sec: 0.0,
            duration_sec: 0.0,
            paused: false,
            buffering: false,
            volume: 0.0,
            muted: false,
            rate: 0.0,
            width: 0,
            height: 0,
            error: String::new(),
            cache_sec: 0.0,
            buffer_speed_bps: 0.0,
            video_ready: false,
            container_fps: 0.0,
            estimated_fps: 0.0,
            hwdec_current: String::new(),
            display_fps: 0.0,
            frame_drops: 0,
            decoder_frame_drops: 0,
            video_primaries: String::new(),
            video_gamma: String::new(),
            video_sig_peak: 0.0,
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            // 前端契约：没有选中轨道时统一用 "no"（表示关闭），而不是空串。
            active_audio_id: "no".to_string(),
            active_subtitle_id: "no".to_string(),
        }
    }
}

/// 状态回调（src-tauri 用它把快照转成 Tauri 事件）
pub type StateCallback = Arc<dyn Fn(PlayerSnapshot) + Send + Sync>;

/// 内部可变状态（事件线程与命令线程共享）
#[derive(Default)]
struct Inner {
    url: String,
    /// 当前文件是否已成功加载（FILE_LOADED）
    loaded: bool,
    /// loadfile 已发出、尚未 FILE_LOADED
    opening: bool,
    /// 播放结束（EOF）
    ended: bool,
    /// 错误文案（空 = 无错误）
    error: String,
    /// 是否已有真实视频帧输出（START_FILE 复位，VIDEO_RECONFIG 置位）
    video_ready: bool,
}

/// 播放器初始化选项（必须在 `mpv_initialize` **之前** 写入）。
///
/// 为什么要抽出来：`hwdec` 只有在初始化前作为 option 设置，才能保证第一个解码器
/// 就按用户设置的模式建立；仅靠 initialize 之后 `set_property("hwdec", ...)` 在
/// 部分 libmpv 版本/时序下可能静默不生效，表现为「偶尔硬解、多数软解」（#9）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerOptions {
    /// 硬解模式（auto-safe / auto / d3d11va / no；空串回退 auto-safe）
    pub hwdec: String,
    /// 初始音量（0-130）
    pub volume: f64,
    /// 初始倍速（0.25-4.0）
    pub rate: f64,
}

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            hwdec: "auto-safe".to_string(),
            volume: 80.0,
            rate: 1.0,
        }
    }
}

impl PlayerOptions {
    /// 归一化后的 hwdec 值（空串 → auto-safe）。
    fn normalized_hwdec(&self) -> String {
        normalize_hwdec(&self.hwdec)
    }
}

/// hwdec 归一：空串/空白回退 `auto-safe`，其余保留调用方设置（白名单由上层 prefs 保证）。
fn normalize_hwdec(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "auto-safe".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 播放器实例。
pub struct Player {
    api: Arc<MpvApi>,
    handle: *mut c_void,
    inner: Arc<Mutex<Inner>>,
    snapshot: Arc<Mutex<PlayerSnapshot>>,
    stop: Arc<AtomicBool>,
    event_thread: Option<JoinHandle<()>>,
    callback: Option<StateCallback>,
}

// libmpv handle 本身线程安全；这里由 `Player` 统一持有，命令线程只调用 libmpv 函数。
unsafe impl Send for Player {}
unsafe impl Sync for Player {}

impl Player {
    /// 绑定到渲染目标并初始化 libmpv。
    ///
    /// `target.handle` 必须是 Windows HWND（0 表示让 mpv 自建独立窗口，仅供调试）。
    pub fn new(target: RenderTarget, callback: Option<StateCallback>) -> Result<Self, PlayerError> {
        Self::new_with_options(target, callback, &PlayerOptions::default())
    }

    /// 带初始化选项的构造：hwdec / volume / rate 在 `mpv_initialize` 之前写入。
    ///
    /// 上层（Tauri 命令层）应把用户播放设置传进来，保证**第一个解码器**就按设置建立，
    /// 而不是等到 initialize 之后再去改属性（后者在部分时序下会静默失败）。
    pub fn new_with_options(
        target: RenderTarget,
        callback: Option<StateCallback>,
        options: &PlayerOptions,
    ) -> Result<Self, PlayerError> {
        let api = Arc::new(MpvApi::load_default()?);
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(PlayerError::Api("mpv_create 失败".into()));
        }

        // 选项：无内置 OSD/OSC（控制条由前端绘制）、idle 常驻
        let defaults: [(&str, &str); 15] = [
            ("idle", "yes"),
            ("keep-open", "no"),
            ("osc", "no"),
            ("osd-level", "0"),
            ("osd-bar", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("terminal", "no"),
            ("config", "no"),
            // HDR / 杜比视界输出：libplacebo 的 gpu-next 才能解析 HDR10/HLG 元数据与
            // 杜比视界 RPU；target-colorspace-hint 让 Windows 11 HDR 显示器直接按 HDR 输出。
            // 老版本 mpv 不认识某个选项时 set_option 会失败，这里统一忽略、退回默认 vo。
            ("vo", "gpu-next"),
            ("gpu-api", "d3d11"),
            ("target-colorspace-hint", "yes"),
            ("hdr-compute-peak", "yes"),
            ("audio-client-name", "VBox"),
            ("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36"),
        ];
        for (name, value) in defaults {
            // 个别选项在旧版 mpv 上可能不存在，忽略失败即可
            let _ = api.set_option(handle, name, value);
        }
        // 影视源常见自签名证书 / 代理劫持 / 过期证书（TVBox 的 okhttp 同样放行）：
        // 关闭 TLS 校验，否则分片会全部握手失败（表现为一直缓冲、拿不到时长）
        let _ = api.set_option(handle, "tls-verify", "no");
        // 不需要 youtube-dl 解析（省一层子进程与超时）
        let _ = api.set_option(handle, "ytdl", "no");
        // 排障：VBOX_MPV_LOG=<path> 时把 mpv 日志写到文件（默认不写）
        if let Some(path) = std::env::var_os("VBOX_MPV_LOG") {
            let _ = api.set_option(handle, "log-file", &path.to_string_lossy());
            let _ = api.set_option(handle, "log-level", "debug");
        }
        // 硬解模式：必须在 initialize 之前写入，保证**第一个解码器**就按用户设置建立。
        // 这里不在之后覆盖成 auto-safe，否则用户选了 d3d11va/no 也会被改回自动。
        let hwdec = options.normalized_hwdec();
        let hwdec_set = api.set_option(handle, "hwdec", &hwdec);
        // 默认音量/倍速同样在初始化前写入（用户设置优先于 mpv 默认值）。
        let volume = options.volume.clamp(0.0, 130.0);
        let _ = api.set_option(handle, "volume", &format!("{volume:.1}"));
        let rate = options.rate.clamp(0.25, 4.0);
        let _ = api.set_option(handle, "speed", &format!("{rate:.2}"));
        if let Err(err) = &hwdec_set {
            // 不阻断创建：部分旧版 mpv 不认识某些白名单值，mpv 会按默认策略解码
            eprintln!("[vbox][player] 初始化 hwdec={hwdec} 失败（回退 mpv 默认）：{err}");
        }
        if target.handle != 0 {
            let _ = api.set_option(handle, "wid", &target.handle.to_string());
            let _ = api.set_option(handle, "force-window", "yes");
        }

        let rc = unsafe { (api.initialize)(handle) };
        if rc < 0 {
            let message = api.error_text(rc);
            unsafe { (api.terminate_destroy)(handle) };
            return Err(PlayerError::Api(format!("mpv_initialize 失败: {message}")));
        }

        let inner = Arc::new(Mutex::new(Inner::default()));
        let snapshot = Arc::new(Mutex::new(PlayerSnapshot {
            status: STATUS_IDLE.to_string(),
            volume,
            rate,
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));

        let event_thread = {
            let api = Arc::clone(&api);
            let inner = Arc::clone(&inner);
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let callback = callback.clone();
            let handle_addr = handle as usize;
            std::thread::Builder::new()
                .name("vbox-mpv-events".into())
                .spawn(move || {
                    event_loop(
                        api,
                        handle_addr as *mut c_void,
                        inner,
                        snapshot,
                        stop,
                        callback,
                    )
                })
                .map_err(|e| PlayerError::Api(format!("启动 mpv 事件线程失败: {e}")))?
        };

        Ok(Self {
            api,
            handle,
            inner,
            snapshot,
            stop,
            event_thread: Some(event_thread),
            callback,
        })
    }

    /// 打开媒体地址（m3u8/mp4）。`headers` 会作为 `http-header-fields` 透传给请求。
    pub fn open(
        &self,
        url: &str,
        headers: &[(String, String)],
        start_sec: Option<f64>,
    ) -> Result<(), PlayerError> {
        if url.trim().is_empty() {
            return Err(PlayerError::Api("播放地址为空".into()));
        }
        {
            let mut inner = self.lock_inner();
            inner.url = url.to_string();
            inner.loaded = false;
            inner.opening = true;
            inner.ended = false;
            inner.error.clear();
        }
        let header_fields = headers
            .iter()
            .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
            .map(|(k, v)| format!("{}: {}", k.trim(), v.trim()))
            .collect::<Vec<_>>()
            .join("\r\n");
        let _ = self
            .api
            .set_property(self.handle, "http-header-fields", &header_fields);
        match start_sec {
            Some(sec) if sec > 0.0 => {
                let _ = self
                    .api
                    .set_property(self.handle, "start", &format!("{sec}"));
            }
            _ => {
                let _ = self.api.set_property(self.handle, "start", "0");
            }
        }
        // 新片从 1.0x 开始，避免上一部片的倍速残留
        let _ = self.api.set_property(self.handle, "speed", "1.0");
        self.api
            .command(self.handle, &["loadfile", url, "replace"])?;
        self.publish();
        Ok(())
    }

    pub fn play(&self) -> Result<(), PlayerError> {
        self.api
            .set_property(self.handle, "pause", "no")
            .map(|_| ())
    }

    pub fn pause(&self) -> Result<(), PlayerError> {
        self.api
            .set_property(self.handle, "pause", "yes")
            .map(|_| ())
    }

    /// 切换播放/暂停，返回切换后是否处于暂停态。
    pub fn toggle_pause(&self) -> Result<bool, PlayerError> {
        let paused = self
            .api
            .get_flag(self.handle, "pause")
            .unwrap_or(false);
        let next = !paused;
        self.api
            .set_property(self.handle, "pause", if next { "yes" } else { "no" })?;
        Ok(next)
    }

    pub fn seek(&self, sec: f64) -> Result<(), PlayerError> {
        self.api
            .command(self.handle, &["seek", &format!("{sec:.3}"), "absolute", "exact"])
    }

    pub fn seek_relative(&self, delta_sec: f64) -> Result<(), PlayerError> {
        self.api
            .command(self.handle, &["seek", &format!("{delta_sec:.3}"), "relative"])
    }

    pub fn set_volume(&self, volume: f64) -> Result<(), PlayerError> {
        let clamped = volume.clamp(0.0, 130.0);
        self.api
            .set_property(self.handle, "volume", &format!("{clamped:.1}"))
    }

    pub fn set_mute(&self, muted: bool) -> Result<(), PlayerError> {
        self.api
            .set_property(self.handle, "mute", if muted { "yes" } else { "no" })
    }

    /// 把视频输出限制在窗口内的一个子矩形（0..1 比例），用于把画面只画在播放器窗格里。
    ///
    /// 依赖 mpv 的 ideo-margin-ratio-left/top/right/bottom（运行时属性，可随时改）。
    pub fn set_video_margins(&self, left: f64, top: f64, right: f64, bottom: f64) {
        for (name, value) in [
            ("video-margin-ratio-left", left),
            ("video-margin-ratio-top", top),
            ("video-margin-ratio-right", right),
            ("video-margin-ratio-bottom", bottom),
        ] {
            let _ = self
                .api
                .set_property(self.handle, name, &format!("{:.6}", value.clamp(0.0, 1.0)));
        }
    }

    /// 设置 libmpv 的缓存目录（与「下载目录」共用），但**默认关闭磁盘缓存**。
    ///
    /// 说明：`cache-on-disk=yes` 会把网络包持续写盘，播放时一边写一边读，
    /// 在机械盘 / 高码率 HLS 上容易造成掉帧与拖动卡顿（实测表现为「帧率很低」）。
    /// 因此这里只设置 `cache-dir`（供下载/临时文件与将来可选开启），网络缓冲仍走内存；
    /// 需要落盘的完整文件由下载引擎负责。
    ///
    /// 注意：这些是 **选项**，`mpv_set_option` 在 `mpv_initialize` 之后通常不可用；
    /// 运行时同时按属性设置一次，失败则忽略，不影响播放。
    pub fn set_cache_dir(&self, dir: &std::path::Path) {
        let dir = dir.to_string_lossy().to_string();
        if dir.trim().is_empty() {
            return;
        }
        for (name, value) in [
            ("cache-dir", dir.as_str()),
            ("cache-on-disk", "no"),
        ] {
            if self.api.set_option(self.handle, name, value).is_err() {
                let _ = self.api.set_property(self.handle, name, value);
            }
        }
    }

    pub fn set_rate(&self, rate: f64) -> Result<(), PlayerError> {
        let clamped = rate.clamp(0.25, 4.0);
        self.api
            .set_property(self.handle, "speed", &format!("{clamped:.2}"))
    }

    /// 设置硬解模式（运行时 `hwdec` 属性；空串回退 `auto-safe`）。
    ///
    /// 个别平台/旧版 mpv 可能拒绝运行时切换硬解，调用方可以忽略错误
    /// （当前解码仍按原方式继续，不影响播放）。
    pub fn set_hwdec(&self, hwdec: &str) -> Result<(), PlayerError> {
        let value = hwdec.trim();
        let value = if value.is_empty() { "auto-safe" } else { value };
        self.api.set_property(self.handle, "hwdec", value)
    }

    /// 设置用户着色器链（mpv `glsl-shaders`，例如 Anime4K 超分）。
    ///
    /// - 传空切片表示关闭全部用户着色器（属性设为空串即清空列表）；
    /// - `glsl-shaders` 是 **path list** 选项：Windows 用 `;`、其它平台用 `:` 分隔；
    /// - 动态修改会触发渲染链重建（播放中切换会有一次短暂重编译，属正常现象）；
    /// - 需要 `vo=gpu` / `vo=gpu-next`，其它 VO 下 mpv 只会记录警告、画面不生效。
    pub fn set_glsl_shaders(&self, paths: &[String]) -> Result<(), PlayerError> {
        let split = if cfg!(windows) { ';' } else { ':' };
        let joined = paths
            .iter()
            .map(|path| escape_path_list_item(path, split))
            .collect::<Vec<_>>()
            .join(&split.to_string());
        self.api
            .set_property(self.handle, "glsl-shaders", &joined)
    }

    /// 设置视频滤镜链（mpv `vf`，空串清空）。
    ///
    /// 滤镜包由 `plugins.rs` 做白名单校验后生成，这里只负责写入；
    /// 非法滤镜会由 mpv 异步报错（日志），调用方按失败回退。
    pub fn set_video_filters(&self, filters: &str) -> Result<(), PlayerError> {
        self.api.set_property(self.handle, "vf", filters)
    }

    /// 设置音频滤镜链（mpv `af`，空串清空）。
    pub fn set_audio_filters(&self, filters: &str) -> Result<(), PlayerError> {
        self.api.set_property(self.handle, "af", filters)
    }

    /// 切换音轨：`aid` 属性（`"no"` = 关闭；空串按关闭处理）。
    pub fn set_audio_track(&self, id: &str) -> Result<(), PlayerError> {
        self.api
            .set_property(self.handle, "aid", normalize_track_id(id))
    }

    /// 切换字幕轨：`sid` 属性（`"no"` = 关闭；空串按关闭处理）。
    pub fn set_subtitle_track(&self, id: &str) -> Result<(), PlayerError> {
        self.api
            .set_property(self.handle, "sid", normalize_track_id(id))
    }

    /// 加载外挂字幕文件（mpv `sub-add`，默认选中新加载的字幕轨）。
    ///
    /// - `path` 必须是**已经存在**的本地文件，否则直接返回错误（不把错误留给 mpv 异步抛）；
    /// - 加载成功后立即重建一次快照并回调（`track-list` 里会多出一条 `external: true` 的字幕轨），
    ///   UI 不用等事件线程的下一拍；
    /// - 外挂轨跟随当前媒体文件，`loadfile replace` 换集时由 mpv 自动清掉。
    pub fn add_subtitle(&self, path: &str) -> Result<(), PlayerError> {
        let path = validate_subtitle_path(path)?;
        self.api
            .command(self.handle, &["sub-add", path, "select"])?;
        self.refresh();
        Ok(())
    }

    /// 停止播放并回到 idle（保留播放器实例，便于下次快速 loadfile）。
    pub fn stop(&self) -> Result<(), PlayerError> {
        self.api.command(self.handle, &["stop"])?;
        {
            let mut inner = self.lock_inner();
            inner.loaded = false;
            inner.opening = false;
            inner.ended = false;
            inner.url.clear();
            inner.error.clear();
            inner.video_ready = false;
        }
        self.publish();
        Ok(())
    }

    /// 当前状态快照。
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// 媒体是否已加载（供上层判断是否显示视频窗口）。
    pub fn has_media(&self) -> bool {
        let inner = self.lock_inner();
        inner.loaded && !inner.ended && inner.error.is_empty()
    }

    /// 读取任意字符串属性（排障用：`hwdec` / `hwdec-current` / `video-codec` /
    /// `video-params/pixelformat` 等）。
    ///
    /// 读取失败返回 `None`，**绝不兜底成「软解」之类的假值**：`hwdec-current` 在
    /// 解码器尚未建立时可能为空/不可用，这与真正软解是两回事（#9）。
    pub fn property_string(&self, name: &str) -> Option<String> {
        self.api.get_string(self.handle, name)
    }

    /// 读取任意数值属性（排障/测试用；失败返回 `None`）。
    pub fn property_f64(&self, name: &str) -> Option<f64> {
        self.api.get_double(self.handle, name)
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(&self) {
        if let Some(callback) = &self.callback {
            callback(self.snapshot());
        }
    }

    /// 让事件循环尽快刷新一次（命令后立即反映到 UI，而不是等 200ms）。
    pub fn refresh(&self) {
        let snapshot = build_snapshot(&self.api, self.handle, &self.inner);
        if let Ok(mut slot) = self.snapshot.lock() {
            *slot = snapshot.clone();
        }
        if let Some(callback) = &self.callback {
            callback(snapshot);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // wait_event 的超时是 200ms，join 会很快返回
        if let Some(thread) = self.event_thread.take() {
            let _ = thread.join();
        }
        unsafe { (self.api.terminate_destroy)(self.handle) };
    }
}

/// 事件线程：消费 mpv 事件 + 每 200ms 刷新属性快照。
fn event_loop(
    api: Arc<MpvApi>,
    handle: *mut c_void,
    inner: Arc<Mutex<Inner>>,
    snapshot: Arc<Mutex<PlayerSnapshot>>,
    stop: Arc<AtomicBool>,
    callback: Option<StateCallback>,
) {
    let mut last: Option<PlayerSnapshot> = None;
    while !stop.load(Ordering::SeqCst) {
        let event = unsafe { (api.wait_event)(handle, 0.2) };
        if !event.is_null() {
            let event = unsafe { &*event };
            match event.event_id {
                ffi::MPV_EVENT_SHUTDOWN => break,
                ffi::MPV_EVENT_START_FILE => {
                    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.opening = true;
                    guard.loaded = false;
                    guard.ended = false;
                    // 新片开始：在真正有视频帧之前不露洞，避免缓冲期看到透明窗口
                    guard.video_ready = false;
                }
                ffi::MPV_EVENT_FILE_LOADED => {
                    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.loaded = true;
                    guard.opening = false;
                    guard.ended = false;
                    guard.error.clear();
                }
                ffi::MPV_EVENT_VIDEO_RECONFIG => {
                    // 视频输出已按真实帧配置（首帧解码后触发）：此时才可安全开洞。
                    // 注意：结束/换源时 VO 拆除也会触发 RECONFIG，必须确认仍有真实帧（宽高 > 0），
                    // 否则会把「已结束」误判为有画面，洞开着露出透明窗口。
                    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                    let has_frame = api.get_i64(handle, "video-params/w").unwrap_or(0) > 0
                        && api.get_i64(handle, "video-params/h").unwrap_or(0) > 0;
                    if guard.loaded && has_frame && !guard.ended {
                        guard.video_ready = true;
                    }
                }
                ffi::MPV_EVENT_END_FILE => {
                    let reason = if event.data.is_null() {
                        ffi::MPV_END_FILE_REASON_EOF
                    } else {
                        let data = unsafe { &*(event.data as *const MpvEventEndFile) };
                        data.reason
                    };
                    let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
                    guard.loaded = false;
                    guard.opening = false;
                    guard.video_ready = false;
                    if reason == ffi::MPV_END_FILE_REASON_EOF {
                        guard.ended = true;
                    } else if reason == ffi::MPV_END_FILE_REASON_ERROR {
                        let message = api.error_text(event.error);
                        guard.error = format!("播放失败: {message}");
                        eprintln!(
                            "[vbox][player] end-file reason={reason} error={} ({message})",
                            event.error
                        );
                    } else {
                        eprintln!("[vbox][player] end-file reason={reason} error={}", event.error);
                    }
                }
                _ => {}
            }
        }

        let next = build_snapshot(&api, handle, &inner);
        let changed = last.as_ref() != Some(&next);
        if let Ok(mut slot) = snapshot.lock() {
            *slot = next.clone();
        }
        if changed {
            if let Some(callback) = &callback {
                callback(next.clone());
            }
            last = Some(next);
        }
    }
}

/// 读取属性 + 内部状态，合成快照。
fn build_snapshot(api: &MpvApi, handle: *mut c_void, inner: &Arc<Mutex<Inner>>) -> PlayerSnapshot {
    let (url, loaded, opening, ended, error, video_ready) = {
        let guard = inner.lock().unwrap_or_else(|e| e.into_inner());
        (
            guard.url.clone(),
            guard.loaded,
            guard.opening,
            guard.ended,
            guard.error.clone(),
            guard.video_ready,
        )
    };
    let paused = api.get_flag(handle, "pause").unwrap_or(false);
    let buffering = api
        .get_flag(handle, "paused-for-cache")
        .unwrap_or(false);
    let position = api.get_double(handle, "time-pos").unwrap_or(0.0).max(0.0);
    let duration = api.get_double(handle, "duration").unwrap_or(0.0).max(0.0);
    // 兜底：某些 HLS 源不会补发 FILE_LOADED / 事件线程启动晚了一拍；
    // 只要已经有时长或进度，就认为文件已加载，避免状态永远停在 opening。
    let loaded = loaded || duration > 0.0 || position > 0.0;
    let opening = opening && !loaded;
    let status = if !error.is_empty() {
        STATUS_ERROR
    } else if !loaded && !opening && !ended {
        STATUS_IDLE
    } else if opening {
        STATUS_OPENING
    } else if ended {
        STATUS_ENDED
    } else if buffering {
        STATUS_BUFFERING
    } else if paused {
        STATUS_PAUSED
    } else {
        STATUS_PLAYING
    };
    let volume = api.get_double(handle, "volume").unwrap_or(80.0);
    let rate = api.get_double(handle, "speed").unwrap_or(1.0);
    let muted = api.get_flag(handle, "mute").unwrap_or(false);
    let width = api
        .get_i64(handle, "video-params/w")
        .or_else(|| api.get_i64(handle, "dwidth"))
        .unwrap_or(0);
    let height = api
        .get_i64(handle, "video-params/h")
        .or_else(|| api.get_i64(handle, "dheight"))
        .unwrap_or(0);
    // 缓存进度：demuxer-cache-time（秒）与 cache-speed（bytes/s）。
    // 暂停/未加载/流式源不可用时属性不存在或返回 NaN/负数，统一归一为非负有限值。
    let cache_sec = non_negative(api.get_double(handle, "demuxer-cache-time"));
    let buffer_speed_bps = non_negative(api.get_double(handle, "cache-speed"));
    // 帧率 / 硬解 / 丢帧诊断（用于播放页速度行与排障；属性缺失时一律回退 0/空）
    let container_fps = non_negative(api.get_double(handle, "container-fps"));
    let estimated_fps = non_negative(api.get_double(handle, "estimated-vf-fps"));
    let display_fps = non_negative(api.get_double(handle, "display-fps"));
    let hwdec_current = api.get_string(handle, "hwdec-current").unwrap_or_default();
    let frame_drops = api.get_i64(handle, "frame-drop-count").unwrap_or(0).max(0);
    let decoder_frame_drops = api
        .get_i64(handle, "decoder-frame-drop-count")
        .unwrap_or(0)
        .max(0);
    // HDR / 杜比视界诊断：primaries/gamma 说明实际解码出的色彩空间，sig-peak 是信号峰值。
    let video_primaries = api
        .get_string(handle, "video-params/primaries")
        .unwrap_or_default();
    let video_gamma = api
        .get_string(handle, "video-params/gamma")
        .unwrap_or_default();
    let video_sig_peak = non_negative(api.get_double(handle, "video-params/sig-peak"));
    // 轨道枚举：每次快照刷新都读一次 track-list（属性读取失败一律跳过，不 panic）
    let (audio_tracks, subtitle_tracks, active_audio_id, active_subtitle_id) =
        enumerate_tracks(api, handle);
    PlayerSnapshot {
        status: status.to_string(),
        url,
        position_sec: position,
        duration_sec: duration,
        paused,
        buffering,
        volume,
        muted,
        rate,
        width,
        height,
        error,
        cache_sec,
        buffer_speed_bps,
        video_ready,
        container_fps,
        estimated_fps,
        hwdec_current,
        display_fps,
        frame_drops,
        decoder_frame_drops,
        video_primaries,
        video_gamma,
        video_sig_peak,
        audio_tracks,
        subtitle_tracks,
        active_audio_id,
        active_subtitle_id,
    }
}

/// 枚举 mpv `track-list`，返回（音轨，字幕轨，当前音轨 id，当前字幕轨 id）。
///
/// - 属性缺失 / 读取失败时跳过该轨（或该字段），任何情况下都不 panic；
/// - `label` 优先 `title`，其次 `lang`，最后回退「音轨 N / 字幕 N」；
/// - 没有选中轨道时 active id 为 `"no"`（与前端契约一致）。
fn enumerate_tracks(
    api: &MpvApi,
    handle: *mut c_void,
) -> (Vec<PlayerTrack>, Vec<PlayerTrack>, String, String) {
    let mut audio_tracks = Vec::new();
    let mut subtitle_tracks = Vec::new();
    let mut active_audio_id = "no".to_string();
    let mut active_subtitle_id = "no".to_string();

    let count = match api.get_i64(handle, "track-list/count") {
        Some(count) if count > 0 => count,
        _ => {
            return (
                audio_tracks,
                subtitle_tracks,
                active_audio_id,
                active_subtitle_id,
            )
        }
    };
    // 异常源可能返回离谱的 count，做个上限兜底避免长时间循环。
    let count = count.min(256);
    for index in 0..count {
        let prefix = format!("track-list/{index}/");
        let kind = match api.get_string(handle, &format!("{prefix}type")) {
            Some(kind) => kind,
            None => continue,
        };
        if kind != "audio" && kind != "sub" {
            continue;
        }
        let id = match api.get_i64(handle, &format!("{prefix}id")) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let title = api
            .get_string(handle, &format!("{prefix}title"))
            .unwrap_or_default();
        let lang = api
            .get_string(handle, &format!("{prefix}lang"))
            .unwrap_or_default();
        let codec = api
            .get_string(handle, &format!("{prefix}codec"))
            .unwrap_or_default();
        let external = api
            .get_flag(handle, &format!("{prefix}external"))
            .unwrap_or(false);
        let selected = api
            .get_flag(handle, &format!("{prefix}selected"))
            .unwrap_or(false);

        let fallback = if kind == "audio" {
            format!("音轨 {id}")
        } else {
            format!("字幕 {id}")
        };
        let label = if !title.trim().is_empty() {
            title.trim().to_string()
        } else if !lang.trim().is_empty() {
            lang.trim().to_string()
        } else {
            fallback
        };

        let track = PlayerTrack {
            id: id.clone(),
            label,
            lang,
            codec,
            external,
        };
        if kind == "audio" {
            if selected {
                active_audio_id = id;
            }
            audio_tracks.push(track);
        } else {
            if selected {
                active_subtitle_id = id;
            }
            subtitle_tracks.push(track);
        }
    }

    (
        audio_tracks,
        subtitle_tracks,
        active_audio_id,
        active_subtitle_id,
    )
}

/// mpv path list 选项里单个条目的转义：分隔符本身用反斜杠转义。
///
/// `glsl-shaders` 属于 path list 选项（Windows `;` / 其它 `:`），路径里
/// 恰好出现分隔符时必须转义，否则 mpv 会把它拆成两个不存在的文件。
fn escape_path_list_item(path: &str, split: char) -> String {
    if path.contains(split) {
        path.replace(split, &format!("\\{split}"))
    } else {
        path.to_string()
    }
}

/// 轨道 id 归一：空串按 `"no"`（关闭）处理。
fn normalize_track_id(id: &str) -> &str {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        "no"
    } else {
        trimmed
    }
}

/// 校验外挂字幕路径：必须非空且是已存在的普通文件。
///
/// 提前校验是为了把「文件不存在」的错误直接回给调用方（前端可提示），
/// 而不是让 mpv 在事件线程里异步失败、UI 只看到没有任何反应。
fn validate_subtitle_path(path: &str) -> Result<&str, PlayerError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(PlayerError::Api("字幕文件路径为空".into()));
    }
    if !std::path::Path::new(trimmed).is_file() {
        return Err(PlayerError::Api(format!("字幕文件不存在: {trimmed}")));
    }
    Ok(trimmed)
}

/// 把 mpv 返回的 f64 归一为非负有限值（NaN / inf / 负数 → 0）。
fn non_negative(value: Option<f64>) -> f64 {
    match value {
        Some(v) if v.is_finite() && v > 0.0 => v,
        _ => 0.0,
    }
}

/// 播放器错误。
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("libmpv 加载失败: {0}")]
    Load(String),
    #[error("libmpv 调用失败: {0}")]
    Api(String),
    #[error("播放器尚未初始化")]
    NotInitialized,
}

/// 给 UI 的播放器状态别名（保持向后兼容的导出）。
pub type PlayerState = PlayerSnapshot;

#[cfg(test)]
mod tests {
    use super::*;

    /// 无媒体（默认快照）时轨道字段为空，active id 为 "no"，且不会 panic。
    #[test]
    fn snapshot_default_has_no_tracks() {
        let snapshot = PlayerSnapshot::default();
        assert!(snapshot.audio_tracks.is_empty());
        assert!(snapshot.subtitle_tracks.is_empty());
        assert_eq!(snapshot.active_audio_id, "no");
        assert_eq!(snapshot.active_subtitle_id, "no");
    }

    /// 轨道数据结构按契约持有 id/label/lang/codec/external。
    #[test]
    fn player_track_fields_roundtrip() {
        let track = PlayerTrack {
            id: "2".to_string(),
            label: "简体中文".to_string(),
            lang: "chi".to_string(),
            codec: "aac".to_string(),
            external: false,
        };
        assert_eq!(track.id, "2");
        assert_eq!(track.label, "简体中文");
        assert_eq!(track.lang, "chi");
        assert_eq!(track.codec, "aac");
        assert!(!track.external);
    }

    /// 轨道 id 归一：空串/空白 → "no"，其余去空白后原样使用。
    #[test]
    fn track_id_normalization() {
        assert_eq!(normalize_track_id(""), "no");
        assert_eq!(normalize_track_id("   "), "no");
        assert_eq!(normalize_track_id("no"), "no");
        assert_eq!(normalize_track_id(" 3 "), "3");
        assert_eq!(normalize_track_id("auto"), "auto");
    }

    /// 非负归一（缓存/帧率字段）保持既有语义。
    #[test]
    fn non_negative_normalization() {
        assert_eq!(non_negative(None), 0.0);
        assert_eq!(non_negative(Some(-1.0)), 0.0);
        assert_eq!(non_negative(Some(f64::NAN)), 0.0);
        assert_eq!(non_negative(Some(f64::INFINITY)), 0.0);
        assert_eq!(non_negative(Some(12.5)), 12.5);
    }

    /// 初始化选项里的 hwdec 归一：空串/空白回退 auto-safe，其余保留。
    #[test]
    fn player_options_hwdec_normalization() {
        assert_eq!(normalize_hwdec(""), "auto-safe");
        assert_eq!(normalize_hwdec("   "), "auto-safe");
        assert_eq!(normalize_hwdec(" d3d11va "), "d3d11va");
        assert_eq!(normalize_hwdec("no"), "no");

        let mut options = PlayerOptions::default();
        assert_eq!(options.normalized_hwdec(), "auto-safe");
        options.hwdec = "  ".to_string();
        assert_eq!(options.normalized_hwdec(), "auto-safe");
        options.hwdec = "auto".to_string();
        assert_eq!(options.normalized_hwdec(), "auto");
        // 默认音量/倍速与播放设置默认值保持一致
        assert_eq!(PlayerOptions::default().volume, 80.0);
        assert_eq!(PlayerOptions::default().rate, 1.0);
    }

    /// 初始化选项必须在 `mpv_initialize` 之前生效：hwdec / volume / speed 都能从 mpv 读回。
    /// （没有 libmpv 的环境自动跳过；不需要窗口、网络与媒体文件）
    #[test]
    fn player_applies_initial_options_before_initialize() {
        if crate::ffi::MpvApi::load_default().is_err() {
            eprintln!("skip: 当前环境没有可用的 libmpv");
            return;
        }
        let options = PlayerOptions {
            hwdec: "no".to_string(),
            volume: 23.0,
            rate: 1.5,
        };
        let player = Player::new_with_options(
            crate::render::RenderTarget::standalone(640, 360),
            None,
            &options,
        )
        .expect("创建播放器失败");

        // 本地快照跟随初始化选项（不再硬编码 80/1.0）
        let snapshot = player.snapshot();
        assert_eq!(snapshot.volume, 23.0);
        assert_eq!(snapshot.rate, 1.5);

        // 从 mpv 属性读回，确认是 option 真正生效，而不只是本地快照
        assert_eq!(player.property_string("hwdec").as_deref(), Some("no"));
        assert_eq!(player.property_f64("volume"), Some(23.0));
        assert_eq!(player.property_f64("speed"), Some(1.5));
    }

    /// `glsl-shaders` 是 path list 选项：设置后应能读回两条路径（Windows 用 `;` 分隔），
    /// 传空切片则清空列表——这是播放器页面「画质」开关切换档位的底层路径。
    /// （没有 libmpv 的环境自动跳过；不需要窗口显示、网络与媒体文件）
    #[test]
    fn glsl_shaders_property_roundtrip() {
        if crate::ffi::MpvApi::load_default().is_err() {
            eprintln!("skip: 当前环境没有可用的 libmpv");
            return;
        }
        let options = PlayerOptions {
            hwdec: "no".to_string(),
            volume: 0.0,
            rate: 1.0,
        };
        let player = Player::new_with_options(
            crate::render::RenderTarget::standalone(320, 180),
            None,
            &options,
        )
        .expect("创建播放器失败");

        let dir = std::env::temp_dir().join(format!("vbox-glsl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let first = dir.join("first.glsl");
        let second = dir.join("second.glsl");
        for path in [&first, &second] {
            std::fs::write(path, "//!HOOK MAIN\nvec4 hook(){return HOOKED_tex(HOOKED_pos);}\n")
                .expect("写入临时着色器失败");
        }

        let paths = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        player.set_glsl_shaders(&paths).expect("设置 glsl-shaders 失败");
        let read = player.property_string("glsl-shaders").unwrap_or_default();
        assert!(read.contains("first.glsl"), "读回值应包含第一条路径: {read}");
        assert!(read.contains("second.glsl"), "读回值应包含第二条路径: {read}");

        player.set_glsl_shaders(&[]).expect("清空 glsl-shaders 失败");
        let cleared = player.property_string("glsl-shaders").unwrap_or_default();
        assert!(
            cleared.trim().is_empty() || cleared.trim().eq_ignore_ascii_case("no"),
            "清空后应为空: {cleared}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 外挂字幕路径校验：空路径 / 不存在的文件报错，存在文件返回去空白后的路径。
    #[test]
    fn subtitle_path_validation() {
        assert!(validate_subtitle_path("").is_err());
        assert!(validate_subtitle_path("   ").is_err());
        let missing = std::env::temp_dir().join("vbox-missing-subtitle-for-test.srt");
        assert!(validate_subtitle_path(&missing.to_string_lossy()).is_err());

        let dir = std::env::temp_dir().join(format!(
            "vbox-subtitle-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let file = dir.join("sample.srt");
        std::fs::write(&file, "1\n00:00:00,000 --> 00:00:01,000\nhi\n").expect("写入字幕失败");
        let padded = format!("  {}  ", file.to_string_lossy());
        assert_eq!(
            validate_subtitle_path(&padded).expect("已存在文件应通过校验"),
            file.to_string_lossy().as_ref()
        );
        // 目录不是文件，同样应被拒绝
        assert!(validate_subtitle_path(&dir.to_string_lossy()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
