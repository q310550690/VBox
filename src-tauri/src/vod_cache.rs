//! 点播分类标签（tab）磁盘缓存。
//!
//! 站点分类标签每次启动都重新请求，冷启动/切站点时首页会先空一下。这里把
//! 「线路 + 站点」维度的分类结果持久化到 `<app_data_dir>/vod-cache.json`：
//! - `categories`：首屏直接渲染（任意时效，但丢弃超过 30 天的条目）；
//! - `categories_fresh`：仅 30 分钟内，命令层命中时直接跳过网络（不记源健康）；
//! - `save_categories`：每次网络抓取成功后读-改-写，顺手清理旧条目。
//!
//! 缓存只是「加速首屏」，任何读取失败都按无缓存处理，不影响正常网络请求。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use vbox_core::model::Category;

/// 缓存文件名（位于 app_data_dir 下）。
const FILE_NAME: &str = "vod-cache.json";
/// 磁盘缓存版本；版本不符按「无缓存」处理。
const CACHE_VERSION: u32 = 1;
/// 文件内最多保留的条目数（超出按 savedAt 从旧到新丢弃）。
const MAX_ENTRIES: usize = 300;
/// 首屏缓存最长保留：30 天；超过即视为不可用。
const MAX_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// 新鲜 TTL：30 分钟内命中时命令层直接跳过网络。
const FRESH_TTL_MS: u64 = 30 * 60 * 1000;

/// 单个「线路 + 站点」的分类缓存条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CategoryEntry {
    line: String,
    site: String,
    saved_at: u64,
    items: Vec<Category>,
}

/// 缓存文件结构。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct VodCacheFile {
    version: u32,
    entries: Vec<CategoryEntry>,
}

/// 分类缓存存储（Tauri managed state）。
pub struct VodCacheStore {
    path: PathBuf,
    /// 串行化读-改-写，避免并发保存互相覆盖。
    lock: Mutex<()>,
}

impl VodCacheStore {
    /// 绑定到 `<dir>/vod-cache.json`。
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        Self {
            path: dir.join(FILE_NAME),
            lock: Mutex::new(()),
        }
    }

    /// 缓存文件路径（仅测试/排障使用）。
    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    /// 任意时效读取（首屏渲染用）：超过 30 天的条目视为不可用。
    pub fn categories(&self, line: &str, site: &str) -> Option<Vec<Category>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let file = self.read_locked().ok()?;
        let now = now_millis();
        file.entries
            .into_iter()
            .find(|entry| {
                entry.line == line
                    && entry.site == site
                    && now.saturating_sub(entry.saved_at) <= MAX_AGE_MS
            })
            .map(|entry| entry.items)
    }

    /// 仅 30 分钟内的缓存（命令层据此跳过网络）。
    pub fn categories_fresh(&self, line: &str, site: &str) -> Option<Vec<Category>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let file = self.read_locked().ok()?;
        let now = now_millis();
        file.entries
            .into_iter()
            .find(|entry| {
                entry.line == line
                    && entry.site == site
                    && now.saturating_sub(entry.saved_at) <= FRESH_TTL_MS
            })
            .map(|entry| entry.items)
    }

    /// 保存分类标签（读-改-写）：清理 30 天前的条目，最多保留 300 条（按 savedAt 新→旧）。
    pub fn save_categories(
        &self,
        line: &str,
        site: &str,
        items: &[Category],
    ) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read_locked().unwrap_or_default();
        let now = now_millis();
        file.version = CACHE_VERSION;
        // 先清理过期条目，再替换同 line+site 的旧条目。
        file.entries
            .retain(|entry| now.saturating_sub(entry.saved_at) <= MAX_AGE_MS);
        file.entries
            .retain(|entry| !(entry.line == line && entry.site == site));
        file.entries.push(CategoryEntry {
            line: line.to_string(),
            site: site.to_string(),
            saved_at: now,
            items: items.to_vec(),
        });
        // 最多保留 MAX_ENTRIES 条：按 savedAt 新→旧排序后截断。
        file.entries.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
        if file.entries.len() > MAX_ENTRIES {
            file.entries.truncate(MAX_ENTRIES);
        }
        let text = serde_json::to_string_pretty(&file)
            .map_err(|err| format!("序列化分类缓存失败: {err}"))?;
        write_atomic(&self.path, &text)
    }

    /// 读取缓存文件；缺失 / 损坏 / 版本不符都返回错误（调用方按无缓存处理）。
    fn read_locked(&self) -> Result<VodCacheFile, String> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|err| format!("读取分类缓存失败({}): {err}", self.path.display()))?;
        let file: VodCacheFile = serde_json::from_str(&text)
            .map_err(|err| format!("解析分类缓存失败({}): {err}", self.path.display()))?;
        if file.version != CACHE_VERSION {
            return Err(format!("分类缓存版本不符（{}）", file.version));
        }
        Ok(file)
    }

    /// 测试专用：直接写入一条指定 `saved_at` 的缓存条目（用于构造过期场景）。
    #[cfg(test)]
    fn save_entry_at(
        &self,
        line: &str,
        site: &str,
        saved_at: u64,
        items: &[Category],
    ) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read_locked().unwrap_or_default();
        file.version = CACHE_VERSION;
        file.entries
            .retain(|entry| !(entry.line == line && entry.site == site));
        file.entries.push(CategoryEntry {
            line: line.to_string(),
            site: site.to_string(),
            saved_at,
            items: items.to_vec(),
        });
        let text = serde_json::to_string_pretty(&file)
            .map_err(|err| format!("序列化分类缓存失败: {err}"))?;
        write_atomic(&self.path, &text)
    }
}

/// 当前毫秒时间戳。
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 原子写：先写临时文件再 rename；Windows 上 rename 失败时删目标重试一次。
fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("创建分类缓存目录失败({}): {err}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)
        .map_err(|err| format!("写入分类缓存临时文件失败({}): {err}", tmp.display()))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Windows 上目标被短暂占用时 rename 可能失败：删掉目标再试一次
            if std::fs::remove_file(path).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                return Ok(());
            }
            let _ = std::fs::remove_file(&tmp);
            Err(format!("保存分类缓存失败({}): {first}", path.display()))
        }
    }
}

/* ---------------- 单元测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 唯一临时目录（测试结束由调用方清理）。
    fn temp_dir(tag: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!(
                "vbox-vod-cache-{}-{}-{seq}",
                std::process::id(),
                now_millis()
            ))
            .join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn category_list(tags: &[&str]) -> Vec<Category> {
        tags.iter()
            .map(|tag| Category {
                type_id: tag.to_string(),
                type_name: tag.to_string(),
            })
            .collect()
    }

    /// `Category` 没有实现 `PartialEq`：比较「标签名列表」。
    fn cached_names(cache: Option<Vec<Category>>) -> Option<Vec<String>> {
        cache.map(|items| items.iter().map(|item| item.type_name.clone()).collect())
    }

    fn names(tags: &[&str]) -> Option<Vec<String>> {
        Some(tags.iter().map(|tag| tag.to_string()).collect())
    }

    /// 保存后可读回；字段为 camelCase；临时文件不残留。
    #[test]
    fn save_and_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        let store = VodCacheStore::new(&dir);
        let items = category_list(&["电影", "电视剧"]);

        store.save_categories("line-a", "site-1", &items).unwrap();

        assert_eq!(
            cached_names(store.categories("line-a", "site-1")),
            names(&["电影", "电视剧"])
        );
        assert_eq!(
            cached_names(store.categories_fresh("line-a", "site-1")),
            names(&["电影", "电视剧"])
        );

        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("\"savedAt\""), "应为 camelCase 键");
        assert!(text.contains("\"entries\""));
        assert!(!store.path().with_extension("json.tmp").exists());

        // 覆盖保存：同 line+site 只留最新一条。
        store
            .save_categories("line-a", "site-1", &category_list(&["综艺"]))
            .unwrap();
        assert_eq!(
            cached_names(store.categories("line-a", "site-1")),
            names(&["综艺"])
        );
        let loaded: VodCacheFile =
            serde_json::from_str(&std::fs::read_to_string(store.path()).unwrap()).unwrap();
        assert_eq!(loaded.entries.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// fresh 只认 30 分钟内；超过 30 分钟仍是首屏可用缓存。
    #[test]
    fn fresh_ttl_expiry() {
        let dir = temp_dir("fresh");
        let store = VodCacheStore::new(&dir);
        let now = now_millis();
        let items = category_list(&["电影"]);

        store
            .save_entry_at("line-a", "site-1", now - FRESH_TTL_MS - 1, &items)
            .unwrap();
        assert_eq!(
            cached_names(store.categories("line-a", "site-1")),
            names(&["电影"])
        );
        assert!(
            store.categories_fresh("line-a", "site-1").is_none(),
            "超过 30 分钟不应算 fresh"
        );

        store
            .save_entry_at("line-a", "site-1", now - FRESH_TTL_MS + 60_000, &items)
            .unwrap();
        assert_eq!(
            cached_names(store.categories_fresh("line-a", "site-1")),
            names(&["电影"]),
            "30 分钟内应算 fresh"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超过 30 天的条目：首屏也不再返回。
    #[test]
    fn old_entries_are_dropped() {
        let dir = temp_dir("old");
        let store = VodCacheStore::new(&dir);
        let items = category_list(&["电影"]);

        store
            .save_entry_at(
                "line-a",
                "site-1",
                now_millis() - MAX_AGE_MS - 1_000,
                &items,
            )
            .unwrap();
        assert!(store.categories("line-a", "site-1").is_none());
        assert!(store.categories_fresh("line-a", "site-1").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 损坏 JSON / 版本不符 → None；后续保存能自愈。
    #[test]
    fn corrupt_json_returns_none() {
        let dir = temp_dir("corrupt");
        let store = VodCacheStore::new(&dir);

        std::fs::write(store.path(), "{ not json").unwrap();
        assert!(store.categories("line-a", "site-1").is_none());
        assert!(store.categories_fresh("line-a", "site-1").is_none());

        std::fs::write(store.path(), r#"{"version":0,"entries":[]}"#).unwrap();
        assert!(store.categories("line-a", "site-1").is_none());

        // 损坏文件不应阻断保存：写入时会重建。
        let items = category_list(&["电影"]);
        store.save_categories("line-a", "site-1", &items).unwrap();
        assert_eq!(
            cached_names(store.categories("line-a", "site-1")),
            names(&["电影"])
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 保存时清理 30 天前的条目，并把总数截到 300 条。
    #[test]
    fn save_prunes_old_and_caps_entries() {
        let dir = temp_dir("prune");
        let store = VodCacheStore::new(&dir);
        let items = category_list(&["电影"]);

        // 超龄条目：保存时应被清掉。
        store
            .save_entry_at(
                "old-line",
                "old-site",
                now_millis() - MAX_AGE_MS - 1_000,
                &items,
            )
            .unwrap();
        // 填满到超过上限（都在 30 天内，只有「条数上限」能砍掉它们）。
        for index in 0..(MAX_ENTRIES + 5) {
            store
                .save_entry_at(
                    "line",
                    &format!("site-{index}"),
                    now_millis() - index as u64,
                    &items,
                )
                .unwrap();
        }
        store.save_categories("line-a", "site-1", &items).unwrap();

        let loaded: VodCacheFile =
            serde_json::from_str(&std::fs::read_to_string(store.path()).unwrap()).unwrap();
        assert_eq!(loaded.entries.len(), MAX_ENTRIES, "最多保留 300 条");
        assert!(
            loaded.entries.iter().all(|entry| entry.line != "old-line"),
            "30 天前的条目应被清理"
        );
        assert_eq!(
            cached_names(store.categories("line-a", "site-1")),
            names(&["电影"]),
            "最新写入的条目必须保留"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 缓存按 line + site 隔离，互不串用。
    #[test]
    fn line_and_site_are_isolated() {
        let dir = temp_dir("isolate");
        let store = VodCacheStore::new(&dir);
        let tv = category_list(&["电视剧"]);
        let movie = category_list(&["电影"]);

        store.save_categories("line-a", "site-1", &tv).unwrap();
        store.save_categories("line-b", "site-1", &movie).unwrap();

        assert_eq!(
            cached_names(store.categories("line-a", "site-1")),
            names(&["电视剧"])
        );
        assert_eq!(
            cached_names(store.categories("line-b", "site-1")),
            names(&["电影"])
        );
        assert!(store.categories("line-a", "site-2").is_none());
        assert!(store.categories("line-b", "site-2").is_none());
        assert!(store.categories("line-c", "site-1").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
