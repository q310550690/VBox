//! 运行日志：内存环形缓冲 + 导出
//!
//! 不是完整的 tracing 日志系统，只记录 VBox 自身关键路径（播放 / 保存 / 代理 / 下载 / 解析等），
//! 供「设置 → 运行日志」实时查看与导出排查。最多保留 500 条，重启后清空。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

const MAX_LOGS: usize = 500;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Unix 毫秒时间戳
    pub at: u64,
    /// info / warn / error
    pub level: String,
    pub text: String,
}

static LOGS: OnceLock<Mutex<VecDeque<LogEntry>>> = OnceLock::new();
/// 限频状态：key → 上次记录时间
static THROTTLE: OnceLock<Mutex<HashMap<&'static str, Instant>>> = OnceLock::new();

fn store() -> &'static Mutex<VecDeque<LogEntry>> {
    LOGS.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_LOGS)))
}

fn throttle_store() -> &'static Mutex<HashMap<&'static str, Instant>> {
    THROTTLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 追加一条日志（同时输出到 stderr，保留原有控制台排查习惯）。
pub fn push(level: &str, text: impl Into<String>) {
    let text = text.into();
    eprintln!("[vbox][{level}] {text}");
    let entry = LogEntry {
        at: now_ms(),
        level: level.to_string(),
        text,
    };
    if let Ok(mut guard) = store().lock() {
        if guard.len() >= MAX_LOGS {
            guard.pop_front();
        }
        guard.push_back(entry);
    }
}

/// 限频写入：同一个 `key` 在 `interval` 内只记录一次。
///
/// 给轮询 / 重试这类**会连续失败**的路径用（下载工作线程、HLS 分片重试…），
/// 否则一条持续失败会在几秒内刷满 500 条环形缓冲，把有用的历史挤掉。
pub fn push_throttled(
    key: &'static str,
    level: &str,
    text: impl Into<String>,
    interval: Duration,
) {
    {
        let Ok(mut guard) = throttle_store().lock() else {
            return;
        };
        let now = Instant::now();
        match guard.get(key) {
            Some(last) if now.duration_since(*last) < interval => return,
            _ => {
                guard.insert(key, now);
            }
        }
    }
    push(level, text);
}

/// 打开独立的运行日志窗口（已存在则聚焦）。
#[tauri::command]
pub fn open_log_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("vbox-logs") {
        let _ = window.show();
        let _ = window.set_focus();
        push("info", "聚焦运行日志窗口");
        return Ok(());
    }
    match tauri::WebviewWindowBuilder::new(
        &app,
        "vbox-logs",
        tauri::WebviewUrl::App("index.html#/log-window".into()),
    )
    .title("VBox 运行日志")
    .inner_size(900.0, 560.0)
    .min_inner_size(520.0, 320.0)
    .build()
    {
        Ok(_) => {
            push("info", "打开运行日志窗口");
            Ok(())
        }
        Err(e) => Err(format!("打开日志窗口失败: {e}")),
    }
}

/// 最近 N 条日志（旧 → 新）。
#[tauri::command]
pub fn logs_recent(limit: Option<usize>) -> Vec<LogEntry> {
    let limit = limit.unwrap_or(200).min(MAX_LOGS);
    match store().lock() {
        Ok(guard) => guard
            .iter()
            .skip(guard.len().saturating_sub(limit))
            .cloned()
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 清空运行日志。
#[tauri::command]
pub fn logs_clear() {
    if let Ok(mut guard) = store().lock() {
        guard.clear();
    }
}

/// 导出运行日志到文本文件（返回保存路径；用户取消返回 None）。
#[tauri::command]
pub async fn logs_export(limit: Option<usize>) -> Result<Option<String>, String> {
    let path = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("导出运行日志")
            .set_file_name("vbox-logs.txt")
            .add_filter("日志文件", &["txt", "log"])
            .save_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = path else {
        push("info", "取消导出运行日志");
        return Ok(None);
    };

    let limit = limit.unwrap_or(MAX_LOGS).min(MAX_LOGS);
    let mut lines: Vec<String> = match store().lock() {
        Ok(guard) => guard
            .iter()
            .skip(guard.len().saturating_sub(limit))
            .map(|entry| {
                // 本地可读格式：YYYY-MM-DD HH:mm:ss.SSS [level] text
                format!("{} [{}] {}", format_time(entry.at), entry.level, entry.text)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    // 追加一行「导出」本身，导出的文件里能看到这次导出动作
    let exported = lines.len();
    lines.push(format!(
        "{} [info] 导出运行日志: {} 条 -> {}",
        format_time(now_ms()),
        exported,
        path.display()
    ));
    let body = if exported == 0 {
        "（暂无日志）\n".to_string()
    } else {
        format!("VBox 运行日志（{exported} 条）\n\n{}\n", lines.join("\n"))
    };
    std::fs::write(&path, body).map_err(|e| format!("写入日志文件失败: {e}"))?;
    push(
        "info",
        format!("导出运行日志: {exported} 条 -> {}", path.display()),
    );
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// 毫秒时间戳 → 本地时间字符串（不引入 chrono：用简单换算，够日志排查用）。
fn format_time(ms: u64) -> String {
    // 以 UTC+8 粗略展示；时区差异只影响可读性，不影响排查（文件里同时可看原始条目顺序）
    const OFFSET_MS: u64 = 8 * 3600 * 1000;
    let total = ms + OFFSET_MS;
    let secs = total / 1000;
    let millis = total % 1000;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    // 1970-01-01 起的天数 → 年月日（简化版公历换算）
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}")
}

/// Howard Hinnant 的 days → (year, month, day) 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这两个用例共用全局环形缓冲，串行执行避免互相清空/挤掉
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn keeps_only_latest_entries() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 用一个独立容量验证环形裁剪逻辑（通过全局 store 逐条写入）
        logs_clear();
        for i in 0..(MAX_LOGS + 10) {
            push("info", format!("line {i}"));
        }
        let recent = logs_recent(Some(MAX_LOGS));
        assert_eq!(recent.len(), MAX_LOGS);
        assert!(recent.last().unwrap().text.ends_with(&format!("line {}", MAX_LOGS + 9)));
        logs_clear();
    }

    #[test]
    fn formats_time_readably() {
        let text = format_time(0);
        assert!(text.starts_with("1970-01-01"), "{text}");
        assert!(text.ends_with(".000"), "{text}");
    }

    #[test]
    fn throttles_repeated_events() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        logs_clear();
        for _ in 0..5 {
            push_throttled(
                "test-throttle",
                "warn",
                "同一个错误",
                Duration::from_secs(60),
            );
        }
        let hits = logs_recent(Some(MAX_LOGS))
            .into_iter()
            .filter(|entry| entry.text == "同一个错误")
            .count();
        assert_eq!(hits, 1, "限频后同一 key 只应记录一次");
        logs_clear();
    }
}
