//! 进程内搜索结果缓存（W14 §4 / §6.6）。
//!
//! 语义（与计划文档 §6.6 对齐）：
//! - 容量上限默认 50 个关键词，TTL 默认 10 分钟；
//! - 命中前先判断过期（惰性淘汰），过期条目直接丢弃；
//! - 淘汰策略为 **LRU**：命中会把条目移到队尾，超出容量淘汰最久未使用的条目；
//! - 降级结果不写缓存由内核负责（见 [`super::kernel`]）；
//! - 相同关键词的并发请求由内核做 in-flight dedupe（本模块只负责已算结果的缓存）。
//!
//! 时间统一用 [`std::time::Instant`]，测试可直接构造「过去时刻」的条目，无需 sleep。
//!
//! 另有可选的**磁盘分片缓存**（[`DiskCache`]，W14 §6.6）：按查询 hash 分片、
//! 默认 TTL 6 小时 / 容量 64 MB，超限按最旧写入时间淘汰；读写失败一律视为未命中，
//! 不影响搜索主流程。

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{SearchPage, SearchProvider, SearchQuery, SortBy};

/// 缓存键：一次查询的全部决定性参数（关键词已 trim、provider 已去重归一）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub keyword: String,
    pub page: u32,
    pub page_size: u32,
    pub providers: Vec<SearchProvider>,
    pub sort: SortBy,
}

impl CacheKey {
    /// 从查询构造缓存键；`providers` 排序后作为集合参与哈希，避免同集合不同顺序导致未命中。
    pub fn from_query(q: &SearchQuery) -> CacheKey {
        let mut providers = q.providers.clone();
        providers.sort_by_key(|provider| provider.as_str());
        providers.dedup();
        CacheKey {
            keyword: q.keyword.trim().to_string(),
            page: q.page.max(1),
            page_size: q.page_size.clamp(1, SearchQuery::MAX_PAGE_SIZE),
            providers,
            sort: q.sort,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    page: SearchPage,
    inserted_at: Instant,
}

#[derive(Debug)]
struct Inner {
    entries: HashMap<CacheKey, CacheEntry>,
    /// LRU 顺序：队头最久未使用，队尾最近使用。
    order: VecDeque<CacheKey>,
}

/// 缓存观测数据（用于设置页/测试展示；不参与淘汰）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// 命中次数（含过期条目？不含：过期按未命中计）。
    pub hits: u64,
    /// 未命中次数（键不存在或已过期）。
    pub misses: u64,
    /// LRU 淘汰次数。
    pub evictions: u64,
}

/// 线程安全的搜索结果缓存。
#[derive(Debug)]
pub struct SearchCache {
    inner: Mutex<Inner>,
    capacity: usize,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_TTL)
    }
}

/// 默认容量：50 个关键词（§6.6）。
pub const DEFAULT_CAPACITY: usize = 50;
/// 默认 TTL：10 分钟（§6.6）。
pub const DEFAULT_TTL: Duration = Duration::from_secs(600);

impl SearchCache {
    /// 指定容量与 TTL（容量至少 1，TTL 为 0 时等同于禁用缓存）。
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        SearchCache {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                order: VecDeque::new(),
            }),
            capacity: capacity.max(1),
            ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// 容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// TTL。
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// 当前条目数（含尚未被惰性清理的过期条目，仅用于观测/测试）。
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 命中/未命中/淘汰统计快照。
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// 读取；未命中或已过期返回 `None`。命中时把键移到 LRU 队尾。
    pub fn get(&self, key: &CacheKey) -> Option<SearchPage> {
        self.get_at(key, Instant::now())
    }

    /// 写入；同键覆盖并刷新为最近使用，超容量淘汰 LRU 队头。
    pub fn put(&self, key: CacheKey, page: SearchPage) {
        self.put_at(key, page, Instant::now());
    }

    /// 清空。
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.entries.clear();
        inner.order.clear();
    }

    fn get_at(&self, key: &CacheKey, now: Instant) -> Option<SearchPage> {
        let mut inner = self.lock();
        let expired = match inner.entries.get(key) {
            Some(entry) => now.saturating_duration_since(entry.inserted_at) > self.ttl,
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        if expired {
            // 惰性淘汰：过期即删（同时清理 LRU 队列里的占位）。
            inner.entries.remove(key);
            inner.order.retain(|candidate| candidate != key);
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        // 命中：移到队尾，表示最近使用。
        inner.order.retain(|candidate| candidate != key);
        inner.order.push_back(key.clone());
        self.hits.fetch_add(1, Ordering::Relaxed);
        inner.entries.get(key).map(|entry| entry.page.clone())
    }

    fn put_at(&self, key: CacheKey, page: SearchPage, now: Instant) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(&key) {
            entry.page = page;
            entry.inserted_at = now;
            // 覆盖写也视为一次使用，刷新 LRU 位置。
            inner.order.retain(|candidate| candidate != &key);
            inner.order.push_back(key);
            return;
        }

        // 先顺带清掉队头的过期条目，避免它们白占容量。
        loop {
            let expired = match inner.order.front() {
                Some(front) => inner
                    .entries
                    .get(front)
                    .map(|entry| now.saturating_duration_since(entry.inserted_at) > self.ttl)
                    .unwrap_or(true),
                None => break,
            };
            if !expired {
                break;
            }
            if let Some(front) = inner.order.pop_front() {
                inner.entries.remove(&front);
            }
        }

        while inner.entries.len() >= self.capacity {
            match inner.order.pop_front() {
                Some(oldest) => {
                    inner.entries.remove(&oldest);
                    self.evictions.fetch_add(1, Ordering::Relaxed);
                }
                None => break,
            }
        }

        inner.order.push_back(key.clone());
        inner.entries.insert(
            key,
            CacheEntry {
                page,
                inserted_at: now,
            },
        );
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 缓存内部没有会在持锁期间 panic 的逻辑；即便发生，恢复 poisoned 锁也更友好。
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 磁盘分片缓存默认容量：64 MB（W14 §6.6）。
pub const DISK_DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// 磁盘分片缓存默认 TTL：6 小时（W14 §6.6）。
pub const DISK_DEFAULT_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// 磁盘缓存条目（落盘 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskEntry {
    /// 写入时间（Unix 秒）。
    saved_at: i64,
    page: SearchPage,
}

/// 可选的磁盘分片缓存（W14 §6.6）。
///
/// 布局：`<root>/<shard_a>/<shard_b>/<hash>.json`，hash 为查询键的 FNV-1a 64 位；
/// 分片前缀用于避免单目录文件过多。淘汰按 `saved_at` 从旧到新删除直至回到容量上限内。
/// 所有 I/O 错误都静默降级（缓存是尽力而为的加速层，不影响搜索结果）。
#[derive(Debug, Clone)]
pub struct DiskCache {
    root: PathBuf,
    max_bytes: u64,
    ttl: Duration,
}

impl DiskCache {
    /// 使用默认容量/TTL 打开（目录按需创建）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_limits(root, DISK_DEFAULT_MAX_BYTES, DISK_DEFAULT_TTL)
    }

    /// 指定容量（字节）与 TTL；`max_bytes == 0` 视为禁用（所有读取都未命中、写入被忽略）。
    pub fn with_limits(root: impl Into<PathBuf>, max_bytes: u64, ttl: Duration) -> Self {
        DiskCache {
            root: root.into(),
            max_bytes: max_bytes.max(1),
            ttl,
        }
    }

    /// 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 容量上限（字节）。
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// TTL。
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// 读取；未命中、过期或读写失败都返回 `None`。
    pub fn get(&self, key: &CacheKey) -> Option<SearchPage> {
        let path = self.path_for(key);
        let raw = fs::read(&path).ok()?;
        let entry: DiskEntry = serde_json::from_slice(&raw).ok()?;
        let now = unix_secs();
        if now.saturating_sub(entry.saved_at) > self.ttl.as_secs() as i64 {
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(entry.page)
    }

    /// 写入（尽力而为；失败静默）。写临时文件后 rename，避免并发读到半截 JSON。
    pub fn put(&self, key: &CacheKey, page: &SearchPage) {
        let path = self.path_for(key);
        let Some(dir) = path.parent() else {
            return;
        };
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let entry = DiskEntry {
            saved_at: unix_secs(),
            page: page.clone(),
        };
        let Ok(raw) = serde_json::to_vec(&entry) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, &raw).is_err() {
            return;
        }
        if fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
        self.prune();
    }

    /// 当前缓存文件数（观测/测试用）。
    pub fn len(&self) -> usize {
        self.files().len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 当前总字节数（观测/测试用）。
    pub fn total_bytes(&self) -> u64 {
        self.files().iter().map(|(_, size)| *size).sum()
    }

    /// 清空缓存目录下的条目（保留根目录）。
    pub fn clear(&self) {
        for (path, _) in self.files() {
            let _ = fs::remove_file(path);
        }
    }

    /// 按容量上限淘汰最旧条目（超过时从旧到新删除）。
    pub fn prune(&self) {
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|(_, size)| *size).sum();
        if total <= self.max_bytes {
            return;
        }
        files.sort_by_key(|(path, _)| {
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
        });
        for (path, size) in files {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    /// 查询对应的缓存文件路径。
    fn path_for(&self, key: &CacheKey) -> PathBuf {
        let hash = fnv1a64(&canonical_key(key));
        let shard_a = (hash & 0xff) as u8;
        let shard_b = ((hash >> 8) & 0xff) as u8;
        self.root
            .join(format!("{shard_a:02x}"))
            .join(format!("{shard_b:02x}"))
            .join(format!("{hash:016x}.json"))
    }

    /// 递归列出缓存目录下的 `*.json` 文件及大小。
    fn files(&self) -> Vec<(PathBuf, u64)> {
        let mut files = Vec::new();
        collect_files(&self.root, &mut files);
        files
    }
}

fn collect_files(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            if let Ok(meta) = entry.metadata() {
                out.push((path, meta.len()));
            }
        }
    }
}

/// 查询键的规范化字符串（参与磁盘分片 hash）。
fn canonical_key(key: &CacheKey) -> String {
    let providers: Vec<&str> = key
        .providers
        .iter()
        .map(|provider| provider.as_str())
        .collect();
    format!(
        "{}\x1f{}\x1f{}\x1f{}\x1f{:?}",
        key.keyword,
        key.page,
        key.page_size,
        providers.join(","),
        key.sort
    )
}

/// FNV-1a 64 位（稳定、跨进程一致；不依赖标准库哈希随机种子）。
fn fnv1a64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudsearch::SearchItem;

    fn page(total: usize) -> SearchPage {
        SearchPage {
            items: (0..total)
                .map(|index| SearchItem {
                    id: format!("id{index}"),
                    title: format!("标题 {index}"),
                    provider: SearchProvider::Quark,
                    share_url: format!("https://pan.quark.cn/s/{index}"),
                    password: None,
                    size: None,
                    published_at: None,
                    sources: vec![],
                    playable: true,
                })
                .collect(),
            total,
            partial: false,
            elapsed_ms: 0,
            degraded_sources: vec![],
            deduped: 0,
            provider_counts: vec![],
        }
    }

    fn key(keyword: &str) -> CacheKey {
        CacheKey::from_query(&SearchQuery {
            keyword: keyword.to_string(),
            ..SearchQuery::default()
        })
    }

    #[test]
    fn defaults_match_plan_spec() {
        let cache = SearchCache::default();
        assert_eq!(cache.capacity(), 50, "§6.6：50 个关键词");
        assert_eq!(cache.ttl(), Duration::from_secs(600), "§6.6：10 分钟");
    }

    #[test]
    fn cache_hit_and_miss() {
        let cache = SearchCache::default();
        assert!(cache.get(&key("电影")).is_none());
        cache.put(key("电影"), page(2));
        let hit = cache.get(&key("电影")).expect("应命中");
        assert_eq!(hit.total, 2);
        assert_eq!(hit.items.len(), 2);
        assert_eq!(cache.len(), 1);
        // 关键词不同 = 未命中。
        assert!(cache.get(&key("电视剧")).is_none());
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 2);
    }

    #[test]
    fn key_normalizes_providers_order() {
        let first = CacheKey::from_query(&SearchQuery {
            keyword: " 电影 ".to_string(),
            providers: vec![SearchProvider::Quark, SearchProvider::Baidu],
            ..SearchQuery::default()
        });
        let second = CacheKey::from_query(&SearchQuery {
            keyword: "电影".to_string(),
            providers: vec![SearchProvider::Baidu, SearchProvider::Quark],
            ..SearchQuery::default()
        });
        assert_eq!(first, second);
    }

    #[test]
    fn expires_after_ttl() {
        let ttl = Duration::from_secs(300);
        let cache = SearchCache::new(8, ttl);
        let now = Instant::now();
        let expired_at = now
            .checked_sub(ttl + Duration::from_secs(1))
            .expect("Instant 可回退");
        cache.put_at(key("电影"), page(1), expired_at);

        assert!(cache.get(&key("电影")).is_none(), "过期应未命中");
        assert_eq!(cache.len(), 0, "过期条目应被惰性淘汰");
    }

    #[test]
    fn tolerates_entries_just_inside_ttl() {
        let ttl = Duration::from_secs(300);
        let cache = SearchCache::new(8, ttl);
        let now = Instant::now();
        let fresh_but_old = now.checked_sub(ttl - Duration::from_secs(5)).unwrap();
        cache.put_at(key("电影"), page(1), fresh_but_old);
        assert!(cache.get(&key("电影")).is_some(), "未过期应命中");
    }

    #[test]
    fn evicts_least_recently_used() {
        let cache = SearchCache::new(2, Duration::from_secs(300));
        cache.put(key("一"), page(1));
        cache.put(key("二"), page(2));
        cache.put(key("三"), page(3));

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key("一")).is_none(), "最久未使用的应被淘汰");
        assert!(cache.get(&key("二")).is_some());
        assert!(cache.get(&key("三")).is_some());
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn access_promotes_entry_in_lru_order() {
        let cache = SearchCache::new(2, Duration::from_secs(300));
        cache.put(key("一"), page(1));
        cache.put(key("二"), page(2));
        // 访问「一」使其成为最近使用；再插入「三」应淘汰「二」。
        assert!(cache.get(&key("一")).is_some());
        cache.put(key("三"), page(3));

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key("一")).is_some(), "被访问过的应保留");
        assert!(cache.get(&key("二")).is_none(), "LRU 应淘汰「二」");
        assert!(cache.get(&key("三")).is_some());
    }

    #[test]
    fn overwriting_existing_key_refreshes_and_does_not_evict() {
        let cache = SearchCache::new(2, Duration::from_secs(300));
        cache.put(key("一"), page(1));
        cache.put(key("二"), page(2));
        cache.put(key("一"), page(9));

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&key("一")).unwrap().total, 9);
        assert!(cache.get(&key("二")).is_some());
    }

    #[test]
    fn clear_empties_cache() {
        let cache = SearchCache::default();
        cache.put(key("电影"), page(1));
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }

    fn disk_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vbox-disk-cache-{tag}-{}-{}",
            std::process::id(),
            unix_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn key_for(word: &str, page_no: u32) -> CacheKey {
        let mut query = SearchQuery::default();
        query.keyword = word.to_string();
        query.page = page_no;
        CacheKey::from_query(&query)
    }

    #[test]
    fn disk_cache_roundtrip_and_shards() {
        let dir = disk_dir("roundtrip");
        let disk = DiskCache::new(&dir);
        assert!(disk.get(&key_for("电影", 1)).is_none());
        disk.put(&key_for("电影", 1), &page(2));
        disk.put(&key_for("电影", 2), &page(3));
        assert_eq!(disk.len(), 2);
        assert!(disk.total_bytes() > 0);
        assert_eq!(disk.get(&key_for("电影", 1)).unwrap().total, 2);
        assert_eq!(disk.get(&key_for("电影", 2)).unwrap().total, 3);
        // 分片：不同查询落在不同 shard 路径（不应共享同一文件）。
        assert_ne!(
            disk.path_for(&key_for("电影", 1)),
            disk.path_for(&key_for("电影", 2))
        );
        disk.clear();
        assert!(disk.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_cache_expired_entry_is_dropped() {
        let dir = disk_dir("expired");
        let disk = DiskCache::new(&dir);
        let key = key_for("剧集", 1);
        let path = disk.path_for(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale = DiskEntry {
            saved_at: unix_secs() - 7 * 60 * 60,
            page: page(1),
        };
        std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();

        assert!(disk.get(&key).is_none(), "超过 6 小时的条目应视为未命中");
        assert!(!path.exists(), "过期条目读取时应被删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_cache_ignores_corrupt_files() {
        let dir = disk_dir("corrupt");
        let disk = DiskCache::new(&dir);
        let key = key_for("坏数据", 1);
        let path = disk.path_for(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json").unwrap();
        assert!(disk.get(&key).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_cache_prunes_to_max_bytes() {
        let dir = disk_dir("prune");
        let disk = DiskCache::with_limits(&dir, u64::MAX, DISK_DEFAULT_TTL);
        for index in 0..5u32 {
            disk.put(&key_for("关键词", index + 1), &page(20));
        }
        let total = disk.total_bytes();
        assert!(total > 0);
        let tight = DiskCache::with_limits(&dir, total / 2, DISK_DEFAULT_TTL);
        tight.prune();
        assert!(tight.total_bytes() <= total / 2, "prune 后应低于容量上限");
        assert!(tight.len() < 5, "应删除部分条目");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
