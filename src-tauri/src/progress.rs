//! 解析进度 → Tauri 全局事件
//!
//! 核心层（`vbox_core::progress`）只在关键节点调用回调；这里把回调转成
//! `parse-progress` 事件发给前端，并补上前端需要的 `requestId` / `atMs`。
//!
//! 线程模型：事件可能来自解析链的任意线程（超解析工作线程、spawn_blocking 线程），
//! `AppHandle::emit` 本身是线程安全的（内部转发到主线程），因此这里不需要加锁。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Emitter;
use vbox_core::progress::{ProgressEvent, ProgressSink};

/// 前端监听的事件名（与 `src/types/index.ts` / `useParseProgress` 保持一致）
pub const EVENT_NAME: &str = "parse-progress";

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 构造一个把事件转发到前端的 [`ProgressSink`]。
///
/// - `request_id`：前端生成、随 invoke 传入；原样回传，前端据此过滤掉别的请求；
/// - `cancel`：命令层登记的取消令牌，前端点「取消解析」后由 `cancel_parse` 置位。
pub fn sink(
    app: &tauri::AppHandle,
    request_id: Option<String>,
    cancel: Arc<AtomicBool>,
) -> ProgressSink {
    let app = app.clone();
    let request_id = request_id.unwrap_or_default();
    ProgressSink::new_with_cancel(
        Arc::new(move |event: ProgressEvent| {
            let payload = serde_json::json!({
                "requestId": request_id,
                "phase": event.phase,
                "status": event.status,
                "title": event.title,
                "detail": event.detail,
                "elapsedMs": event.elapsed_ms,
                "atMs": now_ms(),
            });
            // 前端没有监听时 emit 返回错误，忽略即可（不打断解析）
            let _ = app.emit(EVENT_NAME, payload);
        }),
        cancel,
    )
}
