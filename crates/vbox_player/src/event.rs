//! 播放事件模型
//!
//! 播放器向外汇报的状态变化（进度、播放/暂停、缓冲、错误等）。
//! 上层据此更新 UI（进度条、状态提示）。

/// 播放器事件
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// 进度更新（当前秒，总秒）
    Progress { current: f64, duration: f64 },
    /// 播放状态变化
    StateChanged { playing: bool },
    /// 缓冲中
    Buffering { percent: u8 },
    /// 播放结束
    Ended,
    /// 发生错误
    Error(String),
}

/// 事件回调类型别名
pub type EventCallback = Box<dyn Fn(PlayerEvent) + Send + Sync>;
