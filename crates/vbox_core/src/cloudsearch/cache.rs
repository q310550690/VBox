//! 进程内搜索结果缓存（W14 §4）。
//!
//! 语义：
//! - 容量上限默认 64 条，TTL 默认 300s；
//! - 命中前先判断过期（惰性淘汰），过期条目直接丢弃；
//! - 超出容量按**插入顺序**淘汰最旧（FIFO，不做 LRU，实现足够简单且可预期）。
//!
//! 时间统一用 [`std::time::Instant`]，测试可直接构造「过去时刻」的条目，无需 sleep。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    /// 插入顺序（FIFO 淘汰用）；理论上与 `entries` 的键集合一致。
    order: VecDeque<CacheKey>,
}

/// 线程安全的搜索结果缓存。
#[derive(Debug)]
pub struct SearchCache {
    inner: Mutex<Inner>,
    capacity: usize,
    ttl: Duration,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_TTL)
    }
}

/// 默认容量。
pub const DEFAULT_CAPACITY: usize = 64;
/// 默认 TTL。
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

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

    /// 读取；未命中或已过期返回 `None`。
    pub fn get(&self, key: &CacheKey) -> Option<SearchPage> {
        self.get_at(key, Instant::now())
    }

    /// 写入；同键覆盖（不改变原有 FIFO 位置），超容量淘汰最旧。
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
            None => return None,
        };
        if expired {
            // 惰性淘汰：过期即删（同时清理 FIFO 队列里的占位）。
            inner.entries.remove(key);
            inner.order.retain(|candidate| candidate != key);
            return None;
        }
        inner.entries.get(key).map(|entry| entry.page.clone())
    }

    fn put_at(&self, key: CacheKey, page: SearchPage, now: Instant) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(&key) {
            entry.page = page;
            entry.inserted_at = now;
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
            let Some(front) = inner.order.pop_front() else {
                break;
            };
            inner.entries.remove(&front);
        }

        while inner.entries.len() >= self.capacity {
            match inner.order.pop_front() {
                Some(oldest) => {
                    inner.entries.remove(&oldest);
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
        }
    }

    fn key(keyword: &str) -> CacheKey {
        CacheKey::from_query(&SearchQuery {
            keyword: keyword.to_string(),
            ..SearchQuery::default()
        })
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
    fn evicts_oldest_by_insertion_order() {
        let cache = SearchCache::new(2, Duration::from_secs(300));
        cache.put(key("一"), page(1));
        cache.put(key("二"), page(2));
        cache.put(key("三"), page(3));

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key("一")).is_none(), "最旧的应被淘汰");
        assert!(cache.get(&key("二")).is_some());
        assert!(cache.get(&key("三")).is_some());
    }

    #[test]
    fn overwriting_existing_key_does_not_evict() {
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
}
