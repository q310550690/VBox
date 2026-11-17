//! 解析进度事件（供桌面端「解析到哪一步」面板使用）
//!
//! 核心层不依赖 Tauri：调用方把一个回调塞进 [`ProgressSink`]，解析链（`parse`）、
//! 播放信息（`source`）和抓包（`src-tauri::capture`）在关键节点 emit 一条事件。
//! 桌面端命令层把它转成 Tauri 全局事件 `parse-progress`；CLI / 测试用
//! [`ProgressSink::none`] 即可（零开销）。
//!
//! 设计约定：
//! - `phase` 是粗粒度阶段（prepare / parser / request / super / sniff / done / error）；
//! - `status` 是这条事件的走向（start / ok / fail / info）；
//! - `elapsed_ms` 由 sink 统一计算，调用方不用关心计时；
//! - `title` 面向用户（中文短句），`detail` 是给排障看的补充（URL / 错误 / 耗时）；
//! - sink 同时携带取消令牌：解析链在重活之间检查 [`ProgressSink::cancelled`]，
//!   用户点「取消解析」后尽快收尾（在途 HTTP 无法中断，只在返回后丢弃结果）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// `detail` 最大字符数（超出截断，避免把超长 URL / 报错塞进 UI）
pub const DETAIL_MAX_CHARS: usize = 260;

/// 一条进度事件（字段与前端 `ParseProgressEvent` 对齐）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    /// prepare / parser / request / super / sniff / done / error
    pub phase: String,
    /// start / ok / fail / info
    pub status: String,
    /// 面向用户的中文短句
    pub title: String,
    /// 可选补充：URL、错误、耗时、候选数等
    pub detail: Option<String>,
    /// 自本次解析（sink 创建）开始的毫秒数
    pub elapsed_ms: u64,
}

/// 事件回调类型（必须线程安全：超解析会从工作线程 emit）
pub type ProgressFn = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

struct Inner {
    emit: ProgressFn,
    started: Instant,
    cancel: Arc<AtomicBool>,
}

/// 进度句柄：`Clone` 后可跨线程传（超解析 worker 各拿一份）。
#[derive(Clone, Default)]
pub struct ProgressSink {
    inner: Option<Arc<Inner>>,
}

impl ProgressSink {
    /// 空实现：不产生事件、永不被取消。
    pub fn none() -> Self {
        Self { inner: None }
    }

    /// 带回调的 sink（内部自带一个空闲的取消令牌）。
    pub fn new(emit: ProgressFn) -> Self {
        Self::new_with_cancel(emit, Arc::new(AtomicBool::new(false)))
    }

    /// 带回调 + 外部取消令牌的 sink。
    pub fn new_with_cancel(emit: ProgressFn, cancel: Arc<AtomicBool>) -> Self {
        Self {
            inner: Some(Arc::new(Inner {
                emit,
                started: Instant::now(),
                cancel,
            })),
        }
    }

    /// 是否有人监听（用于跳过昂贵的 detail 拼接）
    pub fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// 用户是否请求取消本次解析
    pub fn cancelled(&self) -> bool {
        self.inner
            .as_ref()
            .map(|inner| inner.cancel.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// 取消令牌（命令层可另存一份，供 `cancel_parse` 置位）
    pub fn cancel_token(&self) -> Option<Arc<AtomicBool>> {
        self.inner.as_ref().map(|inner| Arc::clone(&inner.cancel))
    }

    /// 发一条只有标题的事件。
    pub fn emit(&self, phase: &str, status: &str, title: impl Into<String>) {
        self.emit_detail(phase, status, title, None::<String>);
    }

    /// 发一条带 detail 的事件；detail 为空串时按无 detail 处理，超长自动截断。
    pub fn emit_detail(
        &self,
        phase: &str,
        status: &str,
        title: impl Into<String>,
        detail: Option<impl Into<String>>,
    ) {
        let Some(inner) = &self.inner else {
            return;
        };
        let detail = detail
            .map(Into::into)
            .map(|text| normalize_detail(&text))
            .filter(|text| !text.is_empty());
        let event = ProgressEvent {
            phase: phase.to_string(),
            status: status.to_string(),
            title: title.into(),
            detail: detail.map(|text| shorten(&text, DETAIL_MAX_CHARS)),
            elapsed_ms: inner.started.elapsed().as_millis() as u64,
        };
        (inner.emit)(event);
    }
}

/// 把多行 detail（错误堆栈 / 响应体）压成单行，便于 UI 一行展示。
fn normalize_detail(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 按字符（不是字节）截断，避免毁掉中文 / emoji 的 UTF-8 边界。
pub fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = String::with_capacity(max_chars * 3 + 3);
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn collect() -> (ProgressSink, Arc<Mutex<Vec<ProgressEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = ProgressSink::new({
            let events = Arc::clone(&events);
            Arc::new(move |event: ProgressEvent| {
                events.lock().unwrap().push(event);
            })
        });
        (sink, events)
    }

    #[test]
    fn none_sink_is_noop_and_never_cancelled() {
        let sink = ProgressSink::none();
        assert!(!sink.enabled());
        assert!(!sink.cancelled());
        sink.emit("prepare", "start", "不会出现在任何地方");
        sink.emit_detail("prepare", "ok", "同样不会", Some("detail"));
    }

    #[test]
    fn emits_phase_status_title_and_detail() {
        let (sink, events) = collect();
        assert!(sink.enabled());
        sink.emit("prepare", "start", "开始解析播放地址");
        sink.emit_detail("parser", "start", "尝试解析器 量子", Some("type=1"));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, "prepare");
        assert_eq!(events[0].status, "start");
        assert_eq!(events[0].title, "开始解析播放地址");
        assert!(events[0].detail.is_none());
        assert_eq!(events[1].detail.as_deref(), Some("type=1"));
    }

    #[test]
    fn empty_detail_is_dropped_and_lines_are_joined() {
        let (sink, events) = collect();
        sink.emit_detail("request", "fail", "请求失败", Some("  "));
        sink.emit_detail("request", "fail", "请求失败", Some("第一行\n\n第二行"));
        let events = events.lock().unwrap();
        assert!(events[0].detail.is_none());
        assert_eq!(events[1].detail.as_deref(), Some("第一行 第二行"));
    }

    #[test]
    fn detail_is_truncated_by_chars() {
        let (sink, events) = collect();
        let long = "解析".repeat(DETAIL_MAX_CHARS);
        sink.emit_detail("request", "ok", "很长", Some(long));
        let events = events.lock().unwrap();
        let detail = events[0].detail.as_deref().unwrap();
        assert_eq!(detail.chars().count(), DETAIL_MAX_CHARS + 1);
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn cancel_token_roundtrip() {
        let cancel = Arc::new(AtomicBool::new(false));
        let sink = ProgressSink::new_with_cancel(Arc::new(|_| {}), Arc::clone(&cancel));
        assert!(!sink.cancelled());
        cancel.store(true, Ordering::SeqCst);
        assert!(sink.cancelled());
        let token = sink.cancel_token().expect("token");
        token.store(false, Ordering::SeqCst);
        assert!(!sink.cancelled());
    }

    #[test]
    fn shorten_handles_multibyte_and_short_text() {
        assert_eq!(shorten("短文本", 10), "短文本");
        assert_eq!(shorten("中文测试", 2), "中文…");
        assert_eq!(shorten("abc", 3), "abc");
        assert_eq!(shorten("abcd", 3), "abc…");
    }
}
