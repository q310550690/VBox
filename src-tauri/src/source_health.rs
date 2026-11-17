//! 源健康度：记录站点 / 线路的连续失败次数，供前端变灰、禁用与自动切源。
//!
//! - 存储：`<app_data_dir>/source_health.db`（SQLite，打不开时降级为内存库）；
//! - 键：`kind = "site" | "line"`，`key` = 站点 key / 线路 URL；
//! - 成功一次即清零「连续失败」，但保留「累计失败」用于排查；
//! - 连续失败 ≥ [`DISABLE_AFTER`] 时前端视为禁用（变灰、跳过后台自动切换）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 连续失败多少次后视为不可用（前端据此变灰 / 自动切源）。
pub const DISABLE_AFTER: i64 = 3;

const DB_FILE: &str = "source_health.db";

/// 一条源健康记录（camelCase 给前端）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceHealth {
    /// site = 站点；line = 线路
    pub kind: String,
    pub key: String,
    /// 最近一次记录时的展示名（站点名 / 线路名）
    #[serde(default)]
    pub label: String,
    /// 连续失败次数（成功一次即清零）
    pub failures: i64,
    /// 累计失败次数
    pub total_failures: i64,
    pub last_error: Option<String>,
    /// 最近一次失败里识别出的 catvod 稳定码（如 `catvod.net.timeout`）
    #[serde(default)]
    pub last_code: Option<String>,
    pub updated_at: i64,
    /// 是否已视为不可用（failures >= DISABLE_AFTER）
    pub disabled: bool,
}

const MIGRATION_SQL: &str = "
CREATE TABLE IF NOT EXISTS source_health (
    kind           TEXT NOT NULL,
    key            TEXT NOT NULL,
    label          TEXT NOT NULL DEFAULT '',
    failures       INTEGER NOT NULL DEFAULT 0,
    total_failures INTEGER NOT NULL DEFAULT 0,
    last_error     TEXT,
    last_code      TEXT,
    updated_at     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, key)
);

CREATE TABLE IF NOT EXISTS source_health_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT NOT NULL,
    key        TEXT NOT NULL,
    code       TEXT,
    message    TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_source_health_events_created
    ON source_health_events (created_at DESC);

CREATE TABLE IF NOT EXISTS source_health_daily (
    day  TEXT PRIMARY KEY,
    ok   INTEGER NOT NULL DEFAULT 0,
    fail INTEGER NOT NULL DEFAULT 0
);
";

/// 源健康存储（Tauri managed state）。
pub struct SourceHealthStore {
    conn: Mutex<Option<Connection>>,
    /// 数据库路径（仅测试/排障读取；正式代码不直接使用）
    #[cfg_attr(not(test), allow(dead_code))]
    path: PathBuf,
}

impl SourceHealthStore {
    /// 打开/创建 `<dir>/source_health.db`；失败时降级为内存库（仅本次会话有效）。
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(DB_FILE);
        let conn = Connection::open(&path)
            .or_else(|_| Connection::open_in_memory())
            .ok();
        let store = Self {
            conn: Mutex::new(conn),
            path,
        };
        store.init();
        store
    }

    fn init(&self) {
        let Ok(mut guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_mut() else {
            return;
        };
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=2000;");
        let _ = conn.execute_batch(MIGRATION_SQL);
        migrate_last_code(conn);
        migrate_event_message(conn);
    }

    fn with_conn<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = guard
            .as_mut()
            .ok_or_else(|| "源健康数据库不可用".to_string())?;
        action(conn)
    }

    /// 记录一次结果：成功清零连续失败；失败 +1 并保存错误信息。
    pub fn record(
        &self,
        kind: &str,
        key: &str,
        label: &str,
        ok: bool,
        error: Option<&str>,
    ) -> Result<SourceHealth, String> {
        let kind = normalize_kind(kind);
        let key = key.trim();
        if key.is_empty() {
            return Err("源标识为空".to_string());
        }
        let label = label.trim();
        let now = now_ms();
        let code = error.and_then(detect_code);
        self.with_conn(|conn| {
            if ok {
                conn.execute(
                    "INSERT INTO source_health (kind, key, label, failures, total_failures, last_error, last_code, updated_at)
                     VALUES (?1, ?2, ?3, 0, 0, NULL, NULL, ?4)
                     ON CONFLICT(kind, key) DO UPDATE SET
                         label = excluded.label,
                         failures = 0,
                         last_error = NULL,
                         last_code = NULL,
                         updated_at = excluded.updated_at",
                    params![kind, key, label, now],
                )
                .map_err(err_str)?;
            } else {
                conn.execute(
                    "INSERT INTO source_health (kind, key, label, failures, total_failures, last_error, last_code, updated_at)
                     VALUES (?1, ?2, ?3, 1, 1, ?4, ?5, ?6)
                     ON CONFLICT(kind, key) DO UPDATE SET
                         label = CASE WHEN excluded.label <> '' THEN excluded.label ELSE source_health.label END,
                         failures = source_health.failures + 1,
                         total_failures = source_health.total_failures + 1,
                         last_error = excluded.last_error,
                         last_code = excluded.last_code,
                         updated_at = excluded.updated_at",
                    params![kind, key, label, error, code, now],
                )
                .map_err(err_str)?;
            }
            // 只有失败才写事件表；成功记录只清空连续失败并累计 daily 统计
            if !ok {
                let _ = conn.execute(
                    "INSERT INTO source_health_events (kind, key, code, message, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![kind, key, code.as_deref(), error, now],
                );
                let _ = conn.execute(
                    "DELETE FROM source_health_events
                     WHERE id <= (SELECT COALESCE(MAX(id), 0) - ?1 FROM source_health_events)",
                    params![MAX_EVENTS as i64],
                );
            }
            // 按天累计成败（失败率趋势；trim 到 90 天）
            let day = day_key(now);
            let (ok_inc, fail_inc) = if ok { (1_i64, 0_i64) } else { (0, 1) };
            let _ = conn.execute(
                "INSERT INTO source_health_daily (day, ok, fail) VALUES (?1, ?2, ?3)
                 ON CONFLICT(day) DO UPDATE SET ok = ok + excluded.ok, fail = fail + excluded.fail",
                params![day, ok_inc, fail_inc],
            );
            if let Some(cutoff) = day_key_checked(now - DAILY_RETENTION_DAYS * 86_400_000) {
                let _ = conn.execute(
                    "DELETE FROM source_health_daily WHERE day < ?1",
                    params![cutoff],
                );
            }
            query_one(conn, &kind, key).map(|row| row.unwrap_or_else(|| SourceHealth {
                kind: kind.clone(),
                key: key.to_string(),
                label: label.to_string(),
                failures: if ok { 0 } else { 1 },
                total_failures: if ok { 0 } else { 1 },
                last_error: error.map(str::to_string),
                last_code: code.clone(),
                updated_at: now,
                disabled: false,
            }))
        })
    }

    /// 全部记录（失败次数多的排前面）。
    pub fn list(&self) -> Result<Vec<SourceHealth>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT kind, key, label, failures, total_failures, last_error, last_code, updated_at
                     FROM source_health
                     ORDER BY failures DESC, updated_at DESC",
                )
                .map_err(err_str)?;
            let rows = stmt
                .query_map([], row_to_health)
                .map_err(err_str)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err_str)
                .map(|rows| rows.into_iter().map(with_disabled).collect())
        })
    }

    /// 按最近一次的 catvod 稳定码聚合（数量 + 累计失败）。
    pub fn code_stats(&self) -> Result<Vec<ErrorCodeStat>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT last_code, COUNT(*) AS sources, COALESCE(SUM(failures), 0) AS failures
                     FROM source_health
                     WHERE last_code IS NOT NULL
                     GROUP BY last_code
                     ORDER BY failures DESC, sources DESC, last_code ASC",
                )
                .map_err(err_str)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(ErrorCodeStat {
                        code: row.get(0)?,
                        sources: row.get(1)?,
                        failures: row.get(2)?,
                    })
                })
                .map_err(err_str)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
        })
    }

    /// 最近失败事件（倒序，带源名与稳定码），供设置页「最近失败事件」。
    pub fn recent_events(&self, limit: usize) -> Result<Vec<ErrorEvent>, String> {
        let limit = limit.clamp(1, MAX_EVENTS) as i64;
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT e.kind, e.key, COALESCE(h.label, ''), e.code, e.created_at, e.message
                     FROM source_health_events e
                     LEFT JOIN source_health h ON h.kind = e.kind AND h.key = e.key
                     ORDER BY e.id DESC
                     LIMIT ?1",
                )
                .map_err(err_str)?;
            let rows = stmt
                .query_map(params![limit], |row| {
                    Ok(ErrorEvent {
                        kind: row.get(0)?,
                        key: row.get(1)?,
                        label: row.get(2)?,
                        code: row.get(3)?,
                        message: row.get(5)?,
                        created_at: row.get(4)?,
                    })
                })
                .map_err(err_str)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
        })
    }

    /// 最近 N 天成败计数（升序、缺失日补零），供设置页「近 7 天」趋势。
    pub fn recent_daily(&self, days: usize) -> Result<Vec<DailyStat>, String> {
        let days = days.clamp(1, DAILY_RETENTION_DAYS as usize);
        let now = now_ms();
        let today = now / 86_400_000;
        let start = today - (days as i64 - 1);
        let start_key = day_key_from_days(start);
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT day, ok, fail FROM source_health_daily WHERE day >= ?1")
                .map_err(err_str)?;
            let rows = stmt
                .query_map(params![start_key], |row| {
                    Ok(DailyStat {
                        day: row.get(0)?,
                        ok: row.get(1)?,
                        fail: row.get(2)?,
                    })
                })
                .map_err(err_str)?;
            let mut by_day = std::collections::HashMap::new();
            for row in rows {
                let stat = row.map_err(err_str)?;
                by_day.insert(stat.day.clone(), stat);
            }
            let mut out = Vec::with_capacity(days);
            for d in start..=today {
                let key = day_key_from_days(d);
                out.push(by_day.remove(&key).unwrap_or(DailyStat {
                    day: key,
                    ok: 0,
                    fail: 0,
                }));
            }
            Ok(out)
        })
    }

    /// 重置单个源（清零连续失败）。
    pub fn reset(&self, kind: &str, key: &str) -> Result<(), String> {
        let kind = normalize_kind(kind);
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE source_health SET failures = 0, last_error = NULL, last_code = NULL, updated_at = ?3
                 WHERE kind = ?1 AND key = ?2",
                params![kind, key.trim(), now_ms()],
            )
            .map_err(err_str)
            .map(|_| ())
        })
    }

    /// 全部重置（清空连续失败与错误信息）。
    pub fn reset_all(&self) -> Result<(), String> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE source_health SET failures = 0, last_error = NULL, last_code = NULL, updated_at = ?1",
                params![now_ms()],
            )
            .map_err(err_str)
            .map(|_| ())
        })
    }

    /// 供排障/测试读取数据库路径。
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 从错误文本里识别 catvod 稳定码（20 类契约，见 `vbox_core::spider::catvod::contract`）。
fn detect_code(message: &str) -> Option<String> {
    use vbox_core::spider::catvod::contract::ErrorClass;
    ErrorClass::detect(message).map(|class| class.code().to_string())
}

/// 旧库补列：SQLite 不支持 `ADD COLUMN IF NOT EXISTS`，用 PRAGMA 判断。
fn migrate_event_message(conn: &Connection) {
    let has = conn
        .prepare("PRAGMA table_info(source_health_events)")
        .and_then(|mut stmt| {
            let cols = stmt.query_map([], |row| row.get::<_, String>(1))?;
            let mut has = false;
            for col in cols {
                if col?.eq_ignore_ascii_case("message") {
                    has = true;
                }
            }
            Ok(has)
        })
        .unwrap_or(true);
    if !has {
        let _ = conn.execute("ALTER TABLE source_health_events ADD COLUMN message TEXT", []);
    }
}

fn migrate_last_code(conn: &Connection) {
    let has = conn
        .prepare("PRAGMA table_info(source_health)")
        .and_then(|mut stmt| {
            let cols = stmt.query_map([], |row| row.get::<_, String>(1))?;
            let mut has = false;
            for col in cols {
                if col?.eq_ignore_ascii_case("last_code") {
                    has = true;
                }
            }
            Ok(has)
        })
        .unwrap_or(true);
    if !has {
        let _ = conn.execute("ALTER TABLE source_health ADD COLUMN last_code TEXT", []);
    }
}

/// 错误码聚合：设置页「错误码分布」用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorCodeStat {
    pub code: String,
    /// 当前仍带该码的源数量
    pub sources: i64,
    /// 该码对应的累计失败次数
    pub failures: i64,
}

/// 一次失败事件（设置页「最近失败事件」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorEvent {
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub label: String,
    /// 识别出的 catvod 稳定码；未识别为 NULL/None
    pub code: Option<String>,
    /// 原始错误文本（含站点/jar 身份）；schema 迁移前的历史事件为 None
    #[serde(default)]
    pub message: Option<String>,
    pub created_at: i64,
}

/// 一次失败事件（设置页「最近失败事件」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyStat {
    /// 本地日历日 `YYYY-MM-DD`
    pub day: String,
    pub ok: i64,
    pub fail: i64,
}

/// 事件表保留上限（超过后删最旧的）。
const MAX_EVENTS: usize = 2000;
/// 按天统计保留天数。
const DAILY_RETENTION_DAYS: i64 = 90;

/// 当前毫秒时间戳对应的本地日期 `YYYY-MM-DD`（不依赖 chrono，按 UTC 日界）。
fn day_key(now_ms: i64) -> String {
    day_key_from_days(now_ms.max(0) / 86_400_000)
}

fn day_key_checked(ms: i64) -> Option<String> {
    if ms < 0 {
        return None;
    }
    Some(day_key(ms))
}

/// 公历 civil date（Howard Hinnant 算法），`days` 为 1970-01-01 起的天数。
fn day_key_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn normalize_kind(kind: &str) -> String {
    match kind.trim().to_ascii_lowercase().as_str() {
        "line" | "线路" => "line".to_string(),
        _ => "site".to_string(),
    }
}

fn query_one(
    conn: &Connection,
    kind: &str,
    key: &str,
) -> Result<Option<SourceHealth>, String> {
    conn.query_row(
        "SELECT kind, key, label, failures, total_failures, last_error, last_code, updated_at
         FROM source_health WHERE kind = ?1 AND key = ?2",
        params![kind, key],
        row_to_health,
    )
    .optional()
    .map_err(err_str)
    .map(|row| row.map(with_disabled))
}

fn row_to_health(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceHealth> {
    Ok(SourceHealth {
        kind: row.get(0)?,
        key: row.get(1)?,
        label: row.get(2)?,
        failures: row.get(3)?,
        total_failures: row.get(4)?,
        last_error: row.get(5)?,
        last_code: row.get(6)?,
        updated_at: row.get(7)?,
        disabled: false,
    })
}

fn with_disabled(mut health: SourceHealth) -> SourceHealth {
    health.disabled = health.failures >= DISABLE_AFTER;
    health
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn err_str(err: rusqlite::Error) -> String {
    err.to_string()
}

/* ---------------- Tauri 命令 ---------------- */

/// 全部源健康记录（前端用于变灰/禁用/自动切源）。
#[tauri::command]
pub fn source_health_list(
    store: tauri::State<'_, SourceHealthStore>,
) -> Result<Vec<SourceHealth>, String> {
    store.list()
}

/// 记录一次源请求结果（成功清零连续失败；失败 +1）。
///
/// 正常由后端读取命令内部调用；也开放给前端记录搜索/解析等场景。
#[tauri::command]
pub fn source_health_record(
    store: tauri::State<'_, SourceHealthStore>,
    kind: String,
    key: String,
    label: Option<String>,
    ok: bool,
    error: Option<String>,
) -> Result<SourceHealth, String> {
    store.record(
        &kind,
        &key,
        label.as_deref().unwrap_or(""),
        ok,
        error.as_deref(),
    )
}

/// 重置单个源（清零连续失败，重新启用）。
#[tauri::command]
pub fn source_health_reset(
    store: tauri::State<'_, SourceHealthStore>,
    kind: String,
    key: String,
) -> Result<(), String> {
    store.reset(&kind, &key)
}

/// 全部重置。
#[tauri::command]
pub fn source_health_reset_all(store: tauri::State<'_, SourceHealthStore>) -> Result<(), String> {
    store.reset_all()
}

/// 按 catvod 稳定码聚合失败（前端「错误码分布」）。
#[tauri::command]
pub fn source_health_code_stats(
    store: tauri::State<'_, SourceHealthStore>,
) -> Result<Vec<ErrorCodeStat>, String> {
    store.code_stats()
}

/// 最近失败事件（倒序，前端「最近失败事件」用；limit 1..=2000）。
#[tauri::command]
pub fn source_health_recent_events(
    store: tauri::State<'_, SourceHealthStore>,
    limit: Option<usize>,
) -> Result<Vec<ErrorEvent>, String> {
    store.recent_events(limit.unwrap_or(20))
}

/// 最近 N 天成败趋势（前端「近 7 天」；days 1..=90，默认 7）。
#[tauri::command]
pub fn source_health_daily(
    store: tauri::State<'_, SourceHealthStore>,
    days: Option<usize>,
) -> Result<Vec<DailyStat>, String> {
    store.recent_daily(days.unwrap_or(7))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_for_test(tag: &str) -> SourceHealthStore {
        let dir = std::env::temp_dir().join(format!(
            "vbox-health-{}-{}-{tag}",
            std::process::id(),
            now_ms()
        ));
        SourceHealthStore::new(&dir)
    }

    #[test]
    fn health_failures_accumulate_and_reset_on_success() {
        let store = store_for_test("record");
        let first = store.record("site", "site-a", "站点 A", false, Some("HTTP 502")).unwrap();
        assert_eq!(first.failures, 1);
        assert!(!first.disabled);
        let second = store.record("site", "site-a", "", false, Some("HTTP 502")).unwrap();
        assert_eq!(second.failures, 2);
        assert_eq!(second.label, "站点 A", "后续空 label 不应覆盖既有名称");
        let third = store.record("site", "site-a", "站点 A", false, None).unwrap();
        assert_eq!(third.failures, 3);
        assert!(third.disabled);
        assert_eq!(third.total_failures, 3);

        let recovered = store.record("site", "site-a", "站点 A", true, None).unwrap();
        assert_eq!(recovered.failures, 0);
        assert!(!recovered.disabled);
        assert!(recovered.last_error.is_none());
        assert_eq!(recovered.total_failures, 3, "累计失败保留用于排查");
    }

    #[test]
    fn health_list_order_reset_and_kind_normalization() {
        let store = store_for_test("list");
        store.record("site", "ok-site", "正常", true, None).unwrap();
        store.record("line", "https://x/a", "线路 A", false, Some("超时")).unwrap();
        store.record("LINE", "https://x/b", "线路 B", false, Some("超时")).unwrap();
        store.record("line", "https://x/b", "线路 B", false, Some("超时")).unwrap();
        store.record("line", "https://x/b", "线路 B", false, Some("超时")).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].key, "https://x/b", "失败次数多的排前面");
        assert!(list[0].disabled);

        store.reset("line", "https://x/b").unwrap();
        let list = store.list().unwrap();
        let line = list.iter().find(|h| h.key == "https://x/b").unwrap();
        assert_eq!(line.failures, 0);
        assert!(!line.disabled);

        store.reset_all().unwrap();
        assert!(store.list().unwrap().iter().all(|h| h.failures == 0));
    }

    #[test]
    fn records_catvod_error_code_and_aggregates() {
        let store = store_for_test("codes");
        store
            .record(
                "site",
                "a",
                "站点 A",
                false,
                Some("catvod.net.timeout：请求超时（2s）"),
            )
            .unwrap();
        store
            .record(
                "site",
                "b",
                "站点 B",
                false,
                Some("catvod.result.parse：详情页缺少标题"),
            )
            .unwrap();
        store
            .record("site", "c", "站点 C", false, Some("HTTP 502"))
            .unwrap();

        let list = store.list().unwrap();
        let a = list.iter().find(|h| h.key == "a").unwrap();
        assert_eq!(a.last_code.as_deref(), Some("catvod.net.timeout"));
        let c = list.iter().find(|h| h.key == "c").unwrap();
        assert!(c.last_code.is_none(), "未识别出稳定码时保持为空");

        let stats = store.code_stats().unwrap();
        assert_eq!(stats.len(), 2);
        let timeout = stats.iter().find(|s| s.code == "catvod.net.timeout").unwrap();
        assert_eq!(timeout.sources, 1);
        assert_eq!(timeout.failures, 1);

        // 恢复后清空码
        store.record("site", "a", "站点 A", true, None).unwrap();
        let stats = store.code_stats().unwrap();
        assert!(stats.iter().all(|s| s.code != "catvod.net.timeout"));
    }

    #[test]
    fn failure_events_are_recorded_newest_first() {
        let store = store_for_test("events");
        store
            .record("site", "a", "站点 A", false, Some("catvod.net.timeout：请求超时"))
            .unwrap();
        store
            .record("site", "b", "站点 B", false, Some("catvod.result.parse：缺少标题"))
            .unwrap();
        store.record("site", "a", "站点 A", false, Some("HTTP 502")).unwrap();
        // 成功不产生事件
        store.record("site", "a", "站点 A", true, None).unwrap();

        let events = store.recent_events(10).unwrap();
        assert_eq!(events.len(), 3, "成功不应产生失败事件");
        assert_eq!(events[0].code, None, "最新事件未识别码时为空");
        assert_eq!(events[0].key, "a");
        assert!(events.iter().any(|e| e.code.as_deref() == Some("catvod.net.timeout")));
        assert_eq!(events[0].message.as_deref(), Some("HTTP 502"), "事件应保留原始错误文本");
        assert!(events
            .iter()
            .any(|e| e.message.as_deref().is_some_and(|m| m.contains("请求超时"))));
        assert!(events
            .iter()
            .any(|e| e.code.as_deref() == Some("catvod.result.parse")));

        // limit 生效且按倒序
        let one = store.recent_events(1).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].key, "a");
    }

    #[test]
    fn daily_stats_accumulate_and_fill_gaps() {
        let store = store_for_test("daily");
        store.record("site", "a", "A", true, None).unwrap();
        store.record("site", "a", "A", true, None).unwrap();
        store.record("site", "a", "A", false, Some("HTTP 502")).unwrap();

        let daily = store.recent_daily(7).unwrap();
        assert_eq!(daily.len(), 7, "缺失日应补零");
        let today = daily.last().unwrap();
        assert_eq!(today.ok, 2);
        assert_eq!(today.fail, 1);
        assert!(daily[0].ok == 0 && daily[0].fail == 0);
        assert!(today.day.len() == 10 && today.day.as_bytes()[4] == b'-');
    }

    #[test]
    fn health_persists_across_store_instances() {
        let dir = std::env::temp_dir().join(format!(
            "vbox-health-persist-{}-{}",
            std::process::id(),
            now_ms()
        ));
        {
            let store = SourceHealthStore::new(&dir);
            store.record("site", "s1", "站点 1", false, Some("error")).unwrap();
            assert!(store.path().exists());
        }
        let store = SourceHealthStore::new(&dir);
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].failures, 1);
    }
}
