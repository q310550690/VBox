//! 观看历史存储（SQLite）
//!
//! 设计要点：
//! - 数据库文件：`<app_data_dir>/history.db`，连接用 `Mutex<Option<Connection>>`
//!   托管（Tauri 命令是同步的，查询都很小，直接在主线程执行）；
//! - `history` 表以 `(site_key, vod_id)` 为唯一键：同一部影片看新集时覆盖旧行，
//!   不新增记录；
//! - `history_settings` 只有一行（id=1）：`retention_days`（0=永久）与
//!   `max_items`（0=不限）；
//! - 所有可能改变数据的入口（list / upsert / set_settings）都会先执行一次
//!   `prune`，保证前端拿到的列表就是裁剪后的结果。
//! - 打开数据库时会做一次幂等清理 `dedupe_legacy`：旧版本在缺少站点 key 时写出的
//!   `site_key = ''` 记录会并入同 `vod_id` + 片名的真实站点记录，避免同一部片出现
//!   两条一模一样的记录（其中空站点那条点击后无法回到详情页）。
//!
//! 注意：命令名与参数名必须与前端契约保持一致（`history_list`、
//! `history_upsert(item)`、`history_remove(siteKey, id)`……），不要随意重命名。

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 数据库文件名（位于 app_data_dir 下）
const DB_FILE: &str = "history.db";

/// 一天的毫秒数
const MS_PER_DAY: i64 = 86_400_000;

/// 数据库不可用时的统一错误文案
const UNAVAILABLE: &str = "观看历史数据库不可用（初始化失败）";

/// 建表/迁移脚本：可反复执行。
const MIGRATION_SQL: &str = "
CREATE TABLE IF NOT EXISTS history (
    site_key     TEXT    NOT NULL,
    vod_id       TEXT    NOT NULL,
    site_name    TEXT    NOT NULL DEFAULT '',
    name         TEXT    NOT NULL DEFAULT '',
    cover        TEXT,
    ep_index     INTEGER NOT NULL DEFAULT 0,
    ep_title     TEXT    NOT NULL DEFAULT '',
    position_sec INTEGER NOT NULL DEFAULT 0,
    duration_sec INTEGER NOT NULL DEFAULT 0,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (site_key, vod_id)
);
CREATE INDEX IF NOT EXISTS idx_history_updated_at ON history (updated_at DESC);
CREATE TABLE IF NOT EXISTS history_settings (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    retention_days  INTEGER NOT NULL DEFAULT 0,
    max_items       INTEGER NOT NULL DEFAULT 200
);
INSERT OR IGNORE INTO history_settings (id, retention_days, max_items) VALUES (1, 0, 200);
";

/// 一条观看历史（每个「站点 + 影片」唯一，`id` 对应数据库列 `vod_id`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub site_key: String,
    #[serde(default)]
    pub site_name: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub ep_index: i64,
    #[serde(default)]
    pub ep_title: String,
    #[serde(default)]
    pub position_sec: i64,
    #[serde(default)]
    pub duration_sec: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// 历史保留策略设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySettings {
    /// 保留天数，0 = 永久保留
    pub retention_days: i64,
    /// 最多保留条数，0 = 不限
    pub max_items: i64,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            retention_days: 0,
            max_items: 200,
        }
    }
}

impl HistorySettings {
    /// 负数视为 0（非法参数按「永久/不限」处理）。
    fn normalized(self) -> Self {
        Self {
            retention_days: self.retention_days.max(0),
            max_items: self.max_items.max(0),
        }
    }
}

/// SQLite 历史存储（Tauri managed state）。
pub struct HistoryStore {
    /// `None` = 数据库打开失败，所有命令返回错误而不是 panic。
    conn: Mutex<Option<Connection>>,
}

impl HistoryStore {
    /// 打开（必要时创建）`<dir>/history.db`；失败时降级为「不可用」状态。
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(DB_FILE);
        let conn = match open_connection(&path) {
            Ok(conn) => Some(conn),
            Err(err) => {
                eprintln!(
                    "[vbox] 观看历史数据库打开失败({}): {err}",
                    path.display()
                );
                None
            }
        };
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// 读取列表（先 prune，再按 updated_at DESC, rowid DESC 排序）。
    pub fn list(&self) -> Result<Vec<HistoryItem>, String> {
        self.with_conn(|conn| {
            prune(conn)?;
            query_list(conn)
        })
    }

    /// UPSERT 一条记录（按 site_key + vod_id），随后裁剪并返回最新列表。
    ///
    /// 写入前做两处兜底（历史记录出现「两条一模一样、其中一条点了没反应」的根因）：
    /// - `site_key` 为空：先并入同 `vod_id` + 片名的已知站点记录；找不到就忽略，
    ///   绝不写入无法回跳详情页的「死记录」；
    /// - `vod_id` 为空：按 `(site_key, name)` 兜底，沿用同名记录已有的 id，
    ///   避免同一部片因为 id 缺失再新增一条。
    pub fn upsert(&self, item: &HistoryItem) -> Result<Vec<HistoryItem>, String> {
        let item = normalize_item(item);
        self.with_conn(|conn| {
            let site_key = if item.site_key.is_empty() {
                let found: Option<String> = conn
                    .query_row(
                        "SELECT site_key FROM history
                         WHERE site_key <> '' AND vod_id = ?1 AND name = ?2
                         ORDER BY updated_at DESC, rowid DESC LIMIT 1",
                        params![item.id, item.name],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(err_str)?;
                match found {
                    Some(known) => known,
                    None => {
                        // 没有站点 key 的记录点了也无法回到详情页，写入只会制造死记录。
                        eprintln!(
                            "[vbox] 忽略缺少站点标识的历史记录: {}（vod_id={}）",
                            item.name, item.id
                        );
                        prune(conn)?;
                        return query_list(conn);
                    }
                }
            } else {
                item.site_key.clone()
            };

            let vod_id = if item.id.is_empty() {
                let found: Option<String> = conn
                    .query_row(
                        "SELECT vod_id FROM history
                         WHERE site_key = ?1 AND name = ?2
                         ORDER BY updated_at DESC, rowid DESC LIMIT 1",
                        params![site_key, item.name],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(err_str)?;
                found.unwrap_or_default()
            } else {
                item.id.clone()
            };

            let updated_at = if item.updated_at > 0 {
                item.updated_at
            } else {
                now_ms()
            };
            conn.execute(
                "INSERT INTO history (
                     site_key, vod_id, site_name, name, cover,
                     ep_index, ep_title, position_sec, duration_sec, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(site_key, vod_id) DO UPDATE SET
                     site_name     = excluded.site_name,
                     name          = excluded.name,
                     cover         = excluded.cover,
                     ep_index      = excluded.ep_index,
                     ep_title      = excluded.ep_title,
                     position_sec  = excluded.position_sec,
                     duration_sec  = excluded.duration_sec,
                     updated_at    = excluded.updated_at",
                params![
                    site_key,
                    vod_id,
                    item.site_name,
                    item.name,
                    item.cover,
                    item.ep_index,
                    item.ep_title,
                    item.position_sec,
                    item.duration_sec,
                    updated_at,
                ],
            )
            .map_err(err_str)?;
            prune(conn)?;
            query_list(conn)
        })
    }

    /// 删除某个「站点 + 影片」，返回最新列表。
    pub fn remove(&self, site_key: &str, id: &str) -> Result<Vec<HistoryItem>, String> {
        let site_key = site_key.trim();
        let id = id.trim();
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM history WHERE site_key = ?1 AND vod_id = ?2",
                params![site_key, id],
            )
            .map_err(err_str)?;
            prune(conn)?;
            query_list(conn)
        })
    }

    /// 清空全部历史，返回空列表。
    pub fn clear(&self) -> Result<Vec<HistoryItem>, String> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM history", [])
                .map_err(err_str)?;
            prune(conn)?;
            query_list(conn)
        })
    }

    /// 读取保留策略设置。
    pub fn settings(&self) -> Result<HistorySettings, String> {
        self.with_conn(|conn| read_settings(conn))
    }

    /// 保存保留策略设置（负数按 0 处理），保存后立即裁剪。
    pub fn set_settings(
        &self,
        retention_days: i64,
        max_items: i64,
    ) -> Result<HistorySettings, String> {
        self.with_conn(|conn| {
            let settings = HistorySettings {
                retention_days,
                max_items,
            }
            .normalized();
            conn.execute(
                "INSERT INTO history_settings (id, retention_days, max_items)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET
                     retention_days = excluded.retention_days,
                     max_items      = excluded.max_items",
                params![settings.retention_days, settings.max_items],
            )
            .map_err(err_str)?;
            prune(conn)?;
            read_settings(conn)
        })
    }

    /// 借用内部连接执行闭包（锁中毒时继续用内部值，不让一次 panic 拖垮整个功能）。
    fn with_conn<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let conn = guard.as_mut().ok_or_else(|| UNAVAILABLE.to_string())?;
        action(conn)
    }
}

/* ---------------- Tauri 命令 ---------------- */

/// 列出观看历史（先按当前设置裁剪），按最近观看排序。
#[tauri::command]
pub fn history_list(store: tauri::State<'_, HistoryStore>) -> Result<Vec<HistoryItem>, String> {
    store.list()
}

/// 写入/更新一条观看历史，返回裁剪后的完整列表。
#[tauri::command]
pub fn history_upsert(
    item: HistoryItem,
    store: tauri::State<'_, HistoryStore>,
) -> Result<Vec<HistoryItem>, String> {
    store.upsert(&item)
}

/// 删除某站点下的一部影片（不看 ep_index，整片删除）。
#[tauri::command]
pub fn history_remove(
    site_key: String,
    id: String,
    store: tauri::State<'_, HistoryStore>,
) -> Result<Vec<HistoryItem>, String> {
    store.remove(&site_key, &id)
}

/// 清空全部观看历史。
#[tauri::command]
pub fn history_clear(store: tauri::State<'_, HistoryStore>) -> Result<Vec<HistoryItem>, String> {
    store.clear()
}

/// 读取历史保留策略。
#[tauri::command]
pub fn history_settings(
    store: tauri::State<'_, HistoryStore>,
) -> Result<HistorySettings, String> {
    store.settings()
}

/// 保存历史保留策略（保存后立即裁剪）。
#[tauri::command]
pub fn history_set_settings(
    retention_days: i64,
    max_items: i64,
    store: tauri::State<'_, HistoryStore>,
) -> Result<HistorySettings, String> {
    store.set_settings(retention_days, max_items)
}

/* ---------------- 内部工具 ---------------- */

/// 打开连接并完成 PRAGMA 设置与建表。
fn open_connection(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(err_str)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// 连接级 PRAGMA：WAL + 3s busy timeout（多线程/多连接时避免 SQLITE_BUSY）。
fn configure(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=3000;")
        .map_err(err_str)
}

/// 建表迁移（幂等）：建表 + 清理旧版本留下的重复记录。
fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(MIGRATION_SQL).map_err(err_str)?;
    dedupe_legacy(conn)
}

/// 旧数据幂等清理：合并「同一影片被写成两条，其中一条 `site_key` 为空」的重复记录。
///
/// 根因：旧版本在拿不到站点 key 时把 `site_key` 写成空串。主键是 `(site_key, vod_id)`，
/// 空站点行与真实站点行互不冲突，于是同一部片会出现两条展示完全一样的记录；其中空站点
/// 那条点进 `/detail//<id>` 时路由参数为空，会落到兜底路由跳回首页（或详情页报参数错误）。
/// 这里把空站点行并入「同 `vod_id` + 同片名、`site_key` 非空」的记录：保留较新的更新时间，
/// 同一集时进度取较大值（避免续播位置倒退），随后删除空站点行。
///
/// 没有任何可打开记录可以合并时保留原行（前端会标注「来源已失效」并允许删除），
/// 不会静默丢用户数据。函数可重复执行。
fn dedupe_legacy(conn: &Connection) -> Result<(), String> {
    let orphans: Vec<(i64, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT rowid, vod_id, name FROM history
                 WHERE trim(site_key) = ''
                 ORDER BY updated_at DESC, rowid DESC",
            )
            .map_err(err_str)?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(err_str)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err_str)?
    };
    if orphans.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction().map_err(err_str)?;
    for (rowid, vod_id, name) in orphans {
        let target: Option<i64> = tx
            .query_row(
                "SELECT rowid FROM history
                 WHERE rowid <> ?1 AND trim(site_key) <> '' AND vod_id = ?2 AND name = ?3
                 ORDER BY updated_at DESC, rowid DESC LIMIT 1",
                params![rowid, vod_id, name],
                |row| row.get(0),
            )
            .optional()
            .map_err(err_str)?;
        if let Some(target_rowid) = target {
            merge_rows(&tx, target_rowid, rowid)?;
        }
    }
    tx.commit().map_err(err_str)
}

/// 把 `source_rowid` 合并进 `target_rowid` 并删除 source（target 必须保留可打开的站点 key）。
fn merge_rows(conn: &Connection, target_rowid: i64, source_rowid: i64) -> Result<(), String> {
    let target = read_row(conn, target_rowid)?;
    let source = read_row(conn, source_rowid)?;
    // 更新时间较新的那条决定站点名 / 集数 / 封面等元信息；
    // 同一集时进度取两者较大值，避免「旧记录进度更靠后」被新记录覆盖。
    let (base, other) = if source.updated_at > target.updated_at {
        (&source, &target)
    } else {
        (&target, &source)
    };
    let (position_sec, duration_sec) = if base.ep_index == other.ep_index {
        (
            base.position_sec.max(other.position_sec),
            base.duration_sec.max(other.duration_sec),
        )
    } else {
        (base.position_sec, base.duration_sec)
    };
    conn.execute(
        "UPDATE history SET
             site_name = ?2, name = ?3, cover = ?4, ep_index = ?5, ep_title = ?6,
             position_sec = ?7, duration_sec = ?8, updated_at = ?9
         WHERE rowid = ?1",
        params![
            target_rowid,
            base.site_name,
            base.name,
            base.cover,
            base.ep_index,
            base.ep_title,
            position_sec,
            duration_sec,
            base.updated_at,
        ],
    )
    .map_err(err_str)?;
    conn.execute("DELETE FROM history WHERE rowid = ?1", params![source_rowid])
        .map_err(err_str)?;
    Ok(())
}

/// 按 rowid 读取一条历史记录。
fn read_row(conn: &Connection, rowid: i64) -> Result<HistoryItem, String> {
    conn.query_row(
        "SELECT site_key, vod_id, site_name, name, cover,
                ep_index, ep_title, position_sec, duration_sec, updated_at
         FROM history WHERE rowid = ?1",
        params![rowid],
        row_to_item,
    )
    .map_err(err_str)
}

/// 写入前规范化文本字段（去掉首尾空白；封面空串按 None 处理）。
fn normalize_item(item: &HistoryItem) -> HistoryItem {
    let cover = item
        .cover
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    HistoryItem {
        site_key: item.site_key.trim().to_string(),
        site_name: item.site_name.trim().to_string(),
        id: item.id.trim().to_string(),
        name: item.name.trim().to_string(),
        cover,
        ep_index: item.ep_index,
        ep_title: item.ep_title.trim().to_string(),
        position_sec: item.position_sec,
        duration_sec: item.duration_sec,
        updated_at: item.updated_at,
    }
}

/// 读取 settings 单行；缺失时回退默认值。
fn read_settings(conn: &Connection) -> Result<HistorySettings, String> {
    let found: Option<HistorySettings> = conn
        .query_row(
            "SELECT retention_days, max_items FROM history_settings WHERE id = 1",
            [],
            |row| {
                Ok(HistorySettings {
                    retention_days: row.get(0)?,
                    max_items: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(err_str)?;
    Ok(found.map(HistorySettings::normalized).unwrap_or_default())
}

/// 按当前设置裁剪：
/// - retention_days > 0：删除 `updated_at < now - days*86400000` 的记录；
/// - max_items > 0：按 `updated_at DESC, rowid DESC` 排序后删除第 max_items 条之后的记录。
fn prune(conn: &mut Connection) -> Result<(), String> {
    let HistorySettings {
        retention_days,
        max_items,
    } = read_settings(conn)?;
    if retention_days <= 0 && max_items <= 0 {
        return Ok(());
    }

    let tx = conn.transaction().map_err(err_str)?;
    if retention_days > 0 {
        let cutoff = now_ms().saturating_sub(retention_days.saturating_mul(MS_PER_DAY));
        tx.execute(
            "DELETE FROM history WHERE updated_at < ?1",
            params![cutoff],
        )
        .map_err(err_str)?;
    }
    if max_items > 0 {
        tx.execute(
            "DELETE FROM history WHERE rowid IN (
                 SELECT rowid FROM history
                 ORDER BY updated_at DESC, rowid DESC
                 LIMIT -1 OFFSET ?1
             )",
            params![max_items],
        )
        .map_err(err_str)?;
    }
    tx.commit().map_err(err_str)
}

/// 查询列表（不带裁剪，调用方负责先 prune）。
fn query_list(conn: &Connection) -> Result<Vec<HistoryItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT site_key, vod_id, site_name, name, cover,
                    ep_index, ep_title, position_sec, duration_sec, updated_at
             FROM history
             ORDER BY updated_at DESC, rowid DESC",
        )
        .map_err(err_str)?;
    let rows = stmt
        .query_map([], row_to_item)
        .map_err(err_str)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
}

/// 行 → HistoryItem（`vod_id` 映射为结构的 `id` 字段）。
fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryItem> {
    Ok(HistoryItem {
        site_key: row.get("site_key")?,
        site_name: row.get("site_name")?,
        id: row.get("vod_id")?,
        name: row.get("name")?,
        cover: row.get("cover")?,
        ep_index: row.get("ep_index")?,
        ep_title: row.get("ep_title")?,
        position_sec: row.get("position_sec")?,
        duration_sec: row.get("duration_sec")?,
        updated_at: row.get("updated_at")?,
    })
}

/// 统一错误转换。
fn err_str(err: rusqlite::Error) -> String {
    format!("SQLite 错误: {err}")
}

/// 当前 Unix 毫秒时间戳。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/* ---------------- 单元测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存库（与正式库同一套 PRAGMA/建表逻辑）。
    fn test_store() -> HistoryStore {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        configure(&conn).expect("PRAGMA 失败");
        migrate(&conn).expect("建表失败");
        HistoryStore {
            conn: Mutex::new(Some(conn)),
        }
    }

    fn item(id: &str, ep_index: i64, updated_at: i64) -> HistoryItem {
        HistoryItem {
            site_key: "site-a".to_string(),
            site_name: "站点 A".to_string(),
            id: id.to_string(),
            name: format!("影片 {id}"),
            cover: Some(format!("https://example.com/{id}.jpg")),
            ep_index,
            ep_title: format!("第 {} 集", ep_index + 1),
            position_sec: 30,
            duration_sec: 3600,
            updated_at,
        }
    }

    /// 同一影片看新集：只更新原行，不新增记录。
    #[test]
    fn history_upsert_same_video_overwrites_row() {
        let store = test_store();
        store.upsert(&item("vod-1", 0, 1_000)).unwrap();
        store.upsert(&item("vod-1", 5, 2_000)).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1, "同一影片不应新增行");
        assert_eq!(list[0].id, "vod-1");
        assert_eq!(list[0].ep_index, 5);
        assert_eq!(list[0].ep_title, "第 6 集");
        assert_eq!(list[0].updated_at, 2_000);

        // 不同站点同 vod_id 视为另一条记录。
        let mut other = item("vod-1", 0, 3_000);
        other.site_key = "site-b".to_string();
        store.upsert(&other).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
    }

    /// 列表按最近观看时间倒序（同一时间按写入顺序 rowid 倒序）。
    #[test]
    fn history_list_sorted_by_updated_at_desc() {
        let store = test_store();
        store.upsert(&item("a", 0, 100)).unwrap();
        store.upsert(&item("b", 0, 300)).unwrap();
        store.upsert(&item("c", 0, 200)).unwrap();

        let ids: Vec<String> = store.list().unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string(), "a".to_string()]);
    }

    /// 按条数裁剪：只保留最近 max_items 条。
    #[test]
    fn history_prune_by_max_items() {
        let store = test_store();
        store.set_settings(0, 2).unwrap();
        store.upsert(&item("a", 0, 100)).unwrap();
        store.upsert(&item("b", 0, 200)).unwrap();
        store.upsert(&item("c", 0, 300)).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        let ids: Vec<String> = list.into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["c".to_string(), "b".to_string()]);
    }

    /// 按天数裁剪：设置后立即删除过期记录，后续写入/list 也会继续裁剪。
    #[test]
    fn history_prune_by_retention_days() {
        let store = test_store();
        let now = now_ms();
        let ten_days_ago = now - 10 * MS_PER_DAY;

        // retention 默认 0（永久）：旧记录保留。
        store.upsert(&item("old", 0, ten_days_ago)).unwrap();
        store.upsert(&item("new", 0, now)).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);

        // 保存 7 天策略 → 立即裁剪掉 10 天前的记录。
        store.set_settings(7, 0).unwrap();
        let ids: Vec<String> = store.list().unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["new".to_string()]);

        // list 自身也会裁剪：再写入一条过期记录后 list 只会剩一条。
        store.upsert(&item("stale", 0, ten_days_ago)).unwrap();
        let ids: Vec<String> = store.list().unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["new".to_string()]);
    }

    /// 设置持久化：保存后可读回；负数按 0 处理。
    #[test]
    fn history_settings_persist_and_normalize() {
        let store = test_store();
        assert_eq!(
            store.settings().unwrap(),
            HistorySettings {
                retention_days: 0,
                max_items: 200
            }
        );

        let saved = store.set_settings(30, 500).unwrap();
        assert_eq!(
            saved,
            HistorySettings {
                retention_days: 30,
                max_items: 500
            }
        );
        assert_eq!(store.settings().unwrap(), saved);

        let normalized = store.set_settings(-5, -1).unwrap();
        assert_eq!(
            normalized,
            HistorySettings {
                retention_days: 0,
                max_items: 0
            }
        );
        assert_eq!(store.settings().unwrap(), normalized);
    }

    /// 删除单条与清空。
    #[test]
    fn history_remove_and_clear() {
        let store = test_store();
        store.upsert(&item("a", 0, 100)).unwrap();
        store.upsert(&item("b", 0, 200)).unwrap();

        let list = store.remove("site-a", "a").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "b");

        assert!(store.clear().unwrap().is_empty());
        assert!(store.list().unwrap().is_empty());
    }

    /// 旧数据迁移：空 site_key 的重复行并入真实站点行（保留最新更新时间、同集进度取大值），
    /// 迁移可重复执行。
    #[test]
    fn migrate_merges_legacy_empty_site_duplicates() {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        configure(&conn).expect("PRAGMA 失败");
        conn.execute_batch(MIGRATION_SQL).expect("建表失败");
        // 复现用户库里的真实数据：空站点行（较旧、已看 6 秒）+ 真实站点行（较新、0 秒）
        conn.execute(
            "INSERT INTO history (
                 site_key, vod_id, site_name, name, cover,
                 ep_index, ep_title, position_sec, duration_sec, updated_at
             ) VALUES ('', '181194', '', '杯雪一战成名', NULL, 0, '', 6, 0, 1000)",
            [],
        )
        .expect("写入空站点行失败");
        conn.execute(
            "INSERT INTO history (
                 site_key, vod_id, site_name, name, cover,
                 ep_index, ep_title, position_sec, duration_sec, updated_at
             ) VALUES ('豪华资源', '181194', '🎥┃豪华┃资源', '杯雪一战成名', NULL, 0, '', 0, 0, 2000)",
            [],
        )
        .expect("写入真实站点行失败");

        migrate(&conn).expect("迁移失败");
        let list = query_list(&conn).expect("查询失败");
        assert_eq!(list.len(), 1, "空站点重复行应被合并: {list:?}");
        assert_eq!(list[0].site_key, "豪华资源");
        assert_eq!(list[0].updated_at, 2000, "应保留较新的更新时间");
        assert_eq!(list[0].position_sec, 6, "同一集应保留较大进度");
        assert_eq!(list[0].site_name, "🎥┃豪华┃资源");

        // 幂等：再迁移一次仍是同一条。
        migrate(&conn).expect("第二次迁移失败");
        assert_eq!(query_list(&conn).unwrap().len(), 1);
    }

    /// 无站点标识的写入：同影片已有可打开记录时并入它，而不是新增一条死记录。
    #[test]
    fn upsert_empty_site_key_adopts_known_site() {
        let store = test_store();
        store.upsert(&item("vod-1", 0, 1_000)).unwrap();

        let mut broken = item("vod-1", 3, 2_000);
        broken.site_key = "  ".to_string();
        store.upsert(&broken).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1, "空站点写入不应新增记录: {list:?}");
        assert_eq!(list[0].site_key, "site-a");
        assert_eq!(list[0].ep_index, 3);
        assert_eq!(list[0].updated_at, 2_000);
    }

    /// 无站点标识、也没有可合并记录：直接忽略，避免生成点击无反应的记录。
    #[test]
    fn upsert_empty_site_key_without_twin_is_ignored() {
        let store = test_store();
        let mut broken = item("vod-x", 0, 1_000);
        broken.site_key = String::new();
        let list = store.upsert(&broken).unwrap();
        assert!(list.is_empty(), "不应写入缺少站点标识的记录: {list:?}");
        assert!(store.list().unwrap().is_empty());
    }

    /// vod_id 为空时按 (site_key, name) 兜底：沿用同名记录已有的 id，不新增重复行。
    #[test]
    fn upsert_empty_vod_id_uses_name_fallback() {
        let store = test_store();
        store.upsert(&item("vod-1", 0, 1_000)).unwrap();

        let mut no_id = item("", 5, 2_000);
        no_id.name = "影片 vod-1".to_string();
        store.upsert(&no_id).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1, "id 缺失时不应新增记录: {list:?}");
        assert_eq!(list[0].id, "vod-1");
        assert_eq!(list[0].ep_index, 5);
    }
}
