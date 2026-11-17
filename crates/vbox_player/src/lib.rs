//! vbox_player：VBox 播放内核
//!
//! 对系统 libmpv 的薄封装，提供跨平台的播放控制（open/play/pause/seek/音量/字幕/音轨）。
//! 真正的 libmpv FFI 在 Stage 8 接入；框架阶段只定义接口与事件模型，
//! 保证上层（Tauri 命令层 / CLI）现在就能按接口编写调用代码，且整个 workspace 可编译。

pub mod event; // 播放事件模型
pub mod ffi; // libmpv 动态加载的 C API 子集
pub mod mpv; // 播放器实例与命令
pub mod render; // 渲染目标抽象

// 顶层再导出常用类型，方便上层直接 `use vbox_player::Player;`
pub use event::{EventCallback, PlayerEvent};
pub use mpv::{
    Player, PlayerError, PlayerOptions, PlayerSnapshot, PlayerState, PlayerTrack, StateCallback,
    STATUS_BUFFERING, STATUS_ENDED, STATUS_ERROR, STATUS_IDLE, STATUS_OPENING, STATUS_PAUSED,
    STATUS_PLAYING,
};
pub use render::RenderTarget;
