//! 收藏存储（SQLite）
//!
//! 设计要点：
//! - 数据库文件：`<app_data_dir>/favorites.db`，连接用 `Mutex<Option<Connection>>`
//!   托管（Tauri 命令是同步的，查询都很小，直接在主线程执行）；
//! - `favorites` 表以 `(site_key, vod_id)` 为唯一键：重复收藏同一部影片只更新展示字段，
//!   不新增记录；
//! - 列表按 `created_at DESC, rowid DESC` 排序（最近收藏的在前）；
//! - `favorite_add` 的 `created_at <= 0` 时用当前毫秒时间戳兜底。
//!
//! 注意：命令名与参数名必须与前端契约保持一致（`favorite_list`、
//! `favorite_add(item)`、`favorite_remove(siteKey, vodId)`、`favorite_check(siteKey, vodId)`），
//! 不要随意重命名。

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 数据库文件名（位于 app_data_dir 下）
const DB_FILE: &str = "favorites.db";

/// 数据库不可用时的统一错误文案
const UNAVAILABLE: &str = "收藏数据库不可用（初始化失败）";

/// 建表脚本：可反复执行。
const MIGRATION_SQL: &str = "
CREATE TABLE IF NOT EXISTS favorites (
    site_key   TEXT    NOT NULL,
    vod_id     TEXT    NOT NULL,
    site_name  TEXT,
    name       TEXT    NOT NULL DEFAULT '',
    cover      TEXT,
    year       TEXT,
    area       TEXT,
    remark     TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (site_key, vod_id)
);
CREATE INDEX IF NOT EXISTS idx_favorites_created_at ON favorites (created_at DESC);
";

/// 一条收藏（每个「站点 + 影片」唯一）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteItem {
    pub site_key: String,
    #[serde(default)]
    pub site_name: Option<String>,
    pub vod_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub remark: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

/// SQLite 收藏存储（Tauri managed state）。
pub struct FavoriteStore {
    /// `None` = 数据库打开失败，所有命令返回错误而不是 panic。
    conn: Mutex<Option<Connection>>,
}

impl FavoriteStore {
    /// 打开（必要时创建）`<dir>/favorites.db`；失败时降级为「不可用」状态。
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(DB_FILE);
        let conn = match open_connection(&path) {
            Ok(conn) => Some(conn),
            Err(err) => {
                eprintln!("[vbox] 收藏数据库打开失败({}): {err}", path.display());
                None
            }
        };
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// 读取全部收藏（最近收藏的在前）。
    pub fn list(&self) -> Result<Vec<FavoriteItem>, String> {
        self.with_conn(|conn| query_list(conn))
    }

    /// UPSERT 一条收藏（按 site_key + vod_id），返回更新后的完整列表。
    pub fn add(&self, item: &FavoriteItem) -> Result<Vec<FavoriteItem>, String> {
        self.with_conn(|conn| {
            let created_at = if item.created_at > 0 {
                item.created_at
            } else {
                now_ms()
            };
            conn.execute(
                "INSERT INTO favorites (
                     site_key, vod_id, site_name, name, cover, year, area, remark, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(site_key, vod_id) DO UPDATE SET
                     site_name  = excluded.site_name,
                     name       = excluded.name,
                     cover      = excluded.cover,
                     year       = excluded.year,
                     area       = excluded.area,
                     remark     = excluded.remark,
                     created_at = excluded.created_at",
                params![
                    item.site_key,
                    item.vod_id,
                    item.site_name,
                    item.name,
                    item.cover,
                    item.year,
                    item.area,
                    item.remark,
                    created_at,
                ],
            )
            .map_err(err_str)?;
            query_list(conn)
        })
    }

    /// 取消收藏（按 site_key + vod_id），返回更新后的完整列表。
    pub fn remove(&self, site_key: &str, vod_id: &str) -> Result<Vec<FavoriteItem>, String> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM favorites WHERE site_key = ?1 AND vod_id = ?2",
                params![site_key, vod_id],
            )
            .map_err(err_str)?;
            query_list(conn)
        })
    }

    /// 是否已收藏。
    pub fn check(&self, site_key: &str, vod_id: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM favorites WHERE site_key = ?1 AND vod_id = ?2 LIMIT 1",
                    params![site_key, vod_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(err_str)?;
            Ok(found.is_some())
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

/// 列出全部收藏（最近收藏的在前）。
#[tauri::command]
pub fn favorite_list(store: tauri::State<'_, FavoriteStore>) -> Result<Vec<FavoriteItem>, String> {
    store.list()
}

/// 新增/更新一条收藏，返回更新后的完整列表。
#[tauri::command]
pub fn favorite_add(
    item: FavoriteItem,
    store: tauri::State<'_, FavoriteStore>,
) -> Result<Vec<FavoriteItem>, String> {
    store.add(&item)
}

/// 取消收藏，返回更新后的完整列表。
#[tauri::command]
pub fn favorite_remove(
    site_key: String,
    vod_id: String,
    store: tauri::State<'_, FavoriteStore>,
) -> Result<Vec<FavoriteItem>, String> {
    store.remove(&site_key, &vod_id)
}

/// 查询某部影片是否已收藏。
#[tauri::command]
pub fn favorite_check(
    site_key: String,
    vod_id: String,
    store: tauri::State<'_, FavoriteStore>,
) -> Result<bool, String> {
    store.check(&site_key, &vod_id)
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

/// 建表迁移（幂等）。
fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(MIGRATION_SQL).map_err(err_str)
}

/// 查询收藏列表（按收藏时间倒序，同一毫秒按写入顺序倒序）。
fn query_list(conn: &Connection) -> Result<Vec<FavoriteItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT site_key, vod_id, site_name, name, cover, year, area, remark, created_at
             FROM favorites
             ORDER BY created_at DESC, rowid DESC",
        )
        .map_err(err_str)?;
    let rows = stmt.query_map([], row_to_item).map_err(err_str)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err_str)
}

/// 行 → FavoriteItem。
fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<FavoriteItem> {
    Ok(FavoriteItem {
        site_key: row.get("site_key")?,
        site_name: row.get("site_name")?,
        vod_id: row.get("vod_id")?,
        name: row.get("name")?,
        cover: row.get("cover")?,
        year: row.get("year")?,
        area: row.get("area")?,
        remark: row.get("remark")?,
        created_at: row.get("created_at")?,
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
    fn test_store() -> FavoriteStore {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        configure(&conn).expect("PRAGMA 失败");
        migrate(&conn).expect("建表失败");
        FavoriteStore {
            conn: Mutex::new(Some(conn)),
        }
    }

    fn item(vod_id: &str, created_at: i64) -> FavoriteItem {
        FavoriteItem {
            site_key: "site-a".to_string(),
            site_name: Some("站点 A".to_string()),
            vod_id: vod_id.to_string(),
            name: format!("影片 {vod_id}"),
            cover: Some(format!("https://example.com/{vod_id}.jpg")),
            year: Some("2026".to_string()),
            area: Some("大陆".to_string()),
            remark: Some("HD".to_string()),
            created_at,
        }
    }

    /// 新增 / 列表 / 删除 / check 的基本闭环。
    #[test]
    fn favorites_crud_and_check() {
        let store = test_store();
        assert!(store.list().unwrap().is_empty());
        assert!(!store.check("site-a", "vod-1").unwrap());

        store.add(&item("vod-1", 1_000)).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.check("site-a", "vod-1").unwrap());
        assert!(!store.check("site-b", "vod-1").unwrap());
        assert!(!store.check("site-a", "vod-2").unwrap());

        let list = store.remove("site-a", "vod-1").unwrap();
        assert!(list.is_empty());
        assert!(!store.check("site-a", "vod-1").unwrap());
    }

    /// 同一「站点 + 影片」重复收藏只更新原行，不新增记录。
    #[test]
    fn favorites_upsert_does_not_duplicate() {
        let store = test_store();
        store.add(&item("vod-1", 1_000)).unwrap();

        let mut updated = item("vod-1", 2_000);
        updated.name = "影片（改名）".to_string();
        updated.remark = Some("更新后的备注".to_string());
        let list = store.add(&updated).unwrap();

        assert_eq!(list.len(), 1, "同一影片不应新增行");
        assert_eq!(list[0].vod_id, "vod-1");
        assert_eq!(list[0].name, "影片（改名）");
        assert_eq!(list[0].remark.as_deref(), Some("更新后的备注"));
        assert_eq!(list[0].created_at, 2_000);

        // 不同站点同 vod_id 视为另一条收藏。
        let mut other = item("vod-1", 3_000);
        other.site_key = "site-b".to_string();
        store.add(&other).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
    }

    /// createdAt <= 0 时用当前毫秒时间戳兜底。
    #[test]
    fn favorites_add_defaults_created_at() {
        let store = test_store();
        let before = now_ms();
        let list = store.add(&item("vod-1", 0)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].created_at >= before,
            "空 createdAt 应使用当前时间戳"
        );
        assert!(list[0].created_at > 0);
    }

    /// 列表按收藏时间倒序（同一时间按写入顺序 rowid 倒序）。
    #[test]
    fn favorites_list_sorted_by_created_at_desc() {
        let store = test_store();
        store.add(&item("a", 100)).unwrap();
        store.add(&item("b", 300)).unwrap();
        store.add(&item("c", 200)).unwrap();

        let ids: Vec<String> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|i| i.vod_id)
            .collect();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string(), "a".to_string()]);
    }

    /// 可选字段为 NULL 时也能正确读回。
    #[test]
    fn favorites_optional_fields_roundtrip() {
        let store = test_store();
        let mut minimal = item("vod-1", 100);
        minimal.site_name = None;
        minimal.cover = None;
        minimal.year = None;
        minimal.area = None;
        minimal.remark = None;
        store.add(&minimal).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].site_name, None);
        assert_eq!(list[0].cover, None);
        assert_eq!(list[0].year, None);
        assert_eq!(list[0].area, None);
        assert_eq!(list[0].remark, None);
    }
}
