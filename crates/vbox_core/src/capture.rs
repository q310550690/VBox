//! 网络请求观测（抓包面板数据源）
//!
//! 说明：这里记录的是 **VBox 自身发出的 HTTP 请求**（配置拉取、源请求、解析接口、spider
//! 请求），用于排障与"抓包"面板展示；它不是对系统网卡的全量抓包，也不做 HTTPS 中间人解密。
//! 真正的媒体地址嗅探由桌面端 WebView 抓包模块完成（src-tauri/src/capture.rs）。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 单条网络日志
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEvent {
    pub id: u64,
    pub at_ms: u128,
    /// config / source / parse / spider / image / other
    pub kind: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: Option<u128>,
    pub size: Option<usize>,
    pub error: Option<String>,
}

static LOG: Mutex<VecDeque<NetworkEvent>> = Mutex::new(VecDeque::new());
static SEQ: Mutex<u64> = Mutex::new(0);
const MAX_EVENTS: usize = 500;

fn next_id() -> u64 {
    let mut guard = SEQ.lock().unwrap_or_else(|e| e.into_inner());
    *guard += 1;
    *guard
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 记录一条请求（请求开始）
pub fn begin(kind: &str, method: &str, url: &str) -> u64 {
    let id = next_id();
    let event = NetworkEvent {
        id,
        at_ms: now_ms(),
        kind: kind.to_string(),
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        duration_ms: None,
        size: None,
        error: None,
    };
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    guard.push_back(event);
    while guard.len() > MAX_EVENTS {
        guard.pop_front();
    }
    id
}

/// 更新请求结束状态
pub fn finish(
    id: u64,
    status: Option<u16>,
    duration_ms: u128,
    size: Option<usize>,
    error: Option<String>,
) {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ev) = guard.iter_mut().find(|e| e.id == id) {
        ev.status = status;
        ev.duration_ms = Some(duration_ms);
        ev.size = size;
        ev.error = error;
    }
}

/// 读取最近 N 条网络日志（倒序，新的在前）
pub fn recent(limit: usize) -> Vec<NetworkEvent> {
    let guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    guard.iter().rev().take(limit).cloned().collect()
}

/// 清空
pub fn clear() {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    guard.clear();
}
