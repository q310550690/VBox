//! 搜索编排：缓存 → 多源并行 + 超时降级 → 后处理 → 回写缓存（W14 §4 / §7.2）。
//!
//! 并发模型：
//! - 每个 source 一个 detached OS 线程（`CloudSearchSource: Send + Sync`），结果经 `mpsc` 回传；
//! - 主线程按 [`SearchKernel::per_source_timeout`] 收结果，超时未回的 source 记入
//!   `degraded_sources` 并把 `partial` 置 `true`，**不阻塞**整体返回；
//! - 慢线程自然结束，其结果被丢弃（channel 已关闭时 send 失败即忽略）；
//! - 降级结果是「部分可用」而不是错误：只有全部 source 都没数据时才返回空页。
//!
//! 分页语义：本地分页（[`super::rank`]）为准，向 source 请求时固定取 `page=1`、
//! `page_size=MAX_PAGE_SIZE` 作为候选池；远端分页（如 PanSou 的 `page` 参数）与本地
//! 分页叠加会造成候选偏差，当前骨架按「远端第一页 + 本地分页」处理。

use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::cache::{CacheKey, SearchCache};
use super::rank;
use super::{CloudSearchSource, SearchItem, SearchPage, SearchQuery};
use crate::error::CoreError;

/// 默认单源等待预算（并行执行，因此整体等待也接近这个值）。
pub const DEFAULT_SOURCE_TIMEOUT: Duration = Duration::from_millis(1_200);

/// 聚合搜索内核。
pub struct SearchKernel {
    sources: Vec<Arc<dyn CloudSearchSource>>,
    cache: SearchCache,
    per_source_timeout: Duration,
}

impl Default for SearchKernel {
    fn default() -> Self {
        SearchKernel {
            sources: Vec::new(),
            cache: SearchCache::default(),
            per_source_timeout: DEFAULT_SOURCE_TIMEOUT,
        }
    }
}

impl SearchKernel {
    /// 用给定 source 列表构造（缓存与超时用默认值）。
    pub fn new(sources: Vec<Arc<dyn CloudSearchSource>>) -> Self {
        SearchKernel {
            sources,
            ..SearchKernel::default()
        }
    }

    /// 追加一个 source。
    pub fn add_source(&mut self, source: Arc<dyn CloudSearchSource>) -> &mut Self {
        self.sources.push(source);
        self
    }

    /// 覆盖单源等待预算。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.per_source_timeout = timeout;
        self
    }

    /// 覆盖缓存（容量 / TTL 由调用方决定）。
    pub fn with_cache(mut self, cache: SearchCache) -> Self {
        self.cache = cache;
        self
    }

    /// 单源等待预算。
    pub fn per_source_timeout(&self) -> Duration {
        self.per_source_timeout
    }

    /// 已注册 source 数量。
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// 缓存当前条目数（观测 / 测试用）。
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// 搜索引擎入口（§7.2 `cloud_search` 的薄封装调用点）。
    ///
    /// 空关键词 / 空 source 都返回空页（`partial = false`）；需要区分错误时用
    /// [`SearchKernel::search_checked`]。
    pub fn search(&self, q: &SearchQuery) -> SearchPage {
        self.search_checked(q).unwrap_or_else(|_error| {
            tracing::warn!("云盘聚合搜索失败，返回空页");
            let mut page = SearchPage::empty();
            page.elapsed_ms = 0;
            page
        })
    }

    /// 同 [`SearchKernel::search`]，但非法查询（如空关键词）返回错误。
    pub fn search_checked(&self, q: &SearchQuery) -> Result<SearchPage, CoreError> {
        let started = Instant::now();
        let query = q.normalized()?;
        if self.sources.is_empty() {
            let mut page = SearchPage::empty();
            page.elapsed_ms = started.elapsed().as_millis() as u64;
            return Ok(page);
        }

        let key = CacheKey::from_query(&query);
        if let Some(mut cached) = self.cache.get(&key) {
            cached.elapsed_ms = started.elapsed().as_millis() as u64;
            return Ok(cached);
        }

        let candidates_query = SearchQuery {
            page: 1,
            page_size: SearchQuery::MAX_PAGE_SIZE,
            ..query.clone()
        };

        let mut items: Vec<SearchItem> = Vec::new();
        let mut degraded_sources: Vec<String> = Vec::new();
        let mut partial = false;

        let names: Vec<String> = self.sources.iter().map(|source| source.name()).collect();
        let total_sources = self.sources.len();
        let mut received = 0usize;

        let (sender, receiver) = mpsc::channel::<(usize, Result<SearchPage, CoreError>)>();
        let mut spawn_failures: Vec<usize> = Vec::new();

        for (index, source) in self.sources.iter().cloned().enumerate() {
            let sender = sender.clone();
            let query = candidates_query.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("cloudsearch-source-{index}"))
                .spawn(move || {
                    // 单个 source panic 不应拖垮整个搜索。
                    let outcome =
                        std::panic::catch_unwind(AssertUnwindSafe(|| source.search(&query)))
                            .unwrap_or_else(|_| {
                                Err(CoreError::Config(format!("云盘搜索源线程 panic: {index}")))
                            });
                    // 主线程已超时返回时 send 失败，属预期。
                    let _ = sender.send((index, outcome));
                });
            if spawned.is_err() {
                spawn_failures.push(index);
                received += 1;
            }
        }
        drop(sender);

        for index in &spawn_failures {
            degraded_sources.push(names[*index].clone());
        }
        if !spawn_failures.is_empty() {
            partial = true;
        }

        // 按 source 注册顺序整理结果，保证聚合顺序确定（与线程完成顺序无关）。
        let mut outcomes: Vec<Option<Result<SearchPage, CoreError>>> =
            (0..total_sources).map(|_| None).collect();
        let deadline = started + self.per_source_timeout;
        while received < total_sources {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline.saturating_duration_since(now);
            match receiver.recv_timeout(remaining) {
                Ok((index, outcome)) => {
                    if index < outcomes.len() && outcomes[index].is_none() {
                        received += 1;
                    }
                    outcomes[index] = Some(outcome);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        for (index, outcome) in outcomes.into_iter().enumerate() {
            match outcome {
                None => {
                    // 超时未返回。
                    degraded_sources.push(names[index].clone());
                    partial = true;
                }
                Some(Ok(page)) => {
                    items.extend(page.items);
                    if page.partial {
                        partial = true;
                        if page.degraded_sources.is_empty() {
                            degraded_sources.push(names[index].clone());
                        } else {
                            degraded_sources.extend(page.degraded_sources);
                        }
                    }
                }
                Some(Err(_error)) => {
                    tracing::warn!(source = %names[index], "云盘搜索源失败，降级");
                    degraded_sources.push(names[index].clone());
                    partial = true;
                }
            }
        }

        // 去重（同名降级项只保留一次，保持出现顺序）。
        dedupe_in_place(&mut degraded_sources);

        let mut page = rank::postprocess(items, &query);
        page.partial = partial;
        page.degraded_sources = degraded_sources;
        page.elapsed_ms = started.elapsed().as_millis() as u64;

        // 只有完整结果才写缓存；降级结果缓存 5 分钟会把短时故障放大成持续问题。
        if !partial {
            self.cache.put(key, page.clone());
        }
        Ok(page)
    }
}

fn dedupe_in_place(values: &mut Vec<String>) {
    let mut seen = Vec::with_capacity(values.len());
    values.retain(|value| {
        if seen.contains(value) {
            false
        } else {
            seen.push(value.clone());
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudsearch::{SearchItem, SearchProvider, SourceKind, SourceStatus};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn item(id: &str, title: &str) -> SearchItem {
        SearchItem {
            id: id.to_string(),
            title: title.to_string(),
            provider: SearchProvider::Quark,
            share_url: format!("https://pan.quark.cn/s/{id}"),
            password: None,
            size: None,
            published_at: None,
            sources: vec!["seed".to_string()],
            playable: true,
        }
    }

    struct FakeSource {
        name: String,
        items: Vec<SearchItem>,
        error: bool,
        delay: Duration,
        calls: Arc<AtomicUsize>,
    }

    impl FakeSource {
        fn new(name: &str, items: Vec<SearchItem>) -> Self {
            FakeSource {
                name: name.to_string(),
                items,
                error: false,
                delay: Duration::ZERO,
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn failing(name: &str) -> Self {
            FakeSource {
                error: true,
                ..FakeSource::new(name, Vec::new())
            }
        }

        fn slow(name: &str, delay: Duration, items: Vec<SearchItem>) -> Self {
            FakeSource {
                delay,
                ..FakeSource::new(name, items)
            }
        }
    }

    impl CloudSearchSource for FakeSource {
        fn search(&self, _q: &SearchQuery) -> Result<SearchPage, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            if self.error {
                return Err(CoreError::Config(format!("{} 失败", self.name)));
            }
            let total = self.items.len();
            Ok(SearchPage {
                items: self.items.clone(),
                total,
                partial: false,
                elapsed_ms: 0,
                degraded_sources: Vec::new(),
            })
        }

        fn health(&self) -> SourceStatus {
            SourceStatus {
                kind: SourceKind::BuiltinKernel,
                ready: true,
                version: "test".to_string(),
                detail: self.name.clone(),
            }
        }

        fn name(&self) -> String {
            self.name.clone()
        }
    }

    fn query(keyword: &str, page: u32, page_size: u32) -> SearchQuery {
        SearchQuery {
            keyword: keyword.to_string(),
            page,
            page_size,
            ..SearchQuery::default()
        }
    }

    #[test]
    fn empty_kernel_returns_empty_page() {
        let kernel = SearchKernel::default();
        let page = kernel.search(&query("电影", 1, 20));
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
        assert!(!page.partial);
        assert!(page.degraded_sources.is_empty());
    }

    #[test]
    fn empty_keyword_returns_empty_page() {
        let kernel = SearchKernel::new(vec![Arc::new(FakeSource::new(
            "a",
            vec![item("a", "电影")],
        ))]);
        let page = kernel.search(&query("  ", 1, 20));
        assert_eq!(page.total, 0);
        assert!(!page.partial);
        assert!(kernel.search_checked(&query("", 1, 20)).is_err());
    }

    #[test]
    fn aggregates_and_dedupes_sources() {
        let first: Arc<dyn CloudSearchSource> = Arc::new(FakeSource::new(
            "tg",
            vec![item("quark:a", "电影 A"), item("quark:b", "电影 B")],
        ));
        let second: Arc<dyn CloudSearchSource> = Arc::new(FakeSource::new(
            "pansou",
            vec![item("quark:a", "电影 A 高清"), item("quark:c", "电影 C")],
        ));
        let kernel = SearchKernel::new(vec![first, second]);

        let page = kernel.search(&query("电影", 1, 20));
        assert!(!page.partial);
        assert_eq!(page.total, 3, "重复 id 应合并");
        assert_eq!(page.items.len(), 3);
        let merged = page
            .items
            .iter()
            .find(|item| item.id == "quark:a")
            .expect("合并项");
        assert_eq!(merged.title, "电影 A 高清", "应保留更长标题");
    }

    #[test]
    fn marks_failed_source_as_degraded() {
        let good: Arc<dyn CloudSearchSource> =
            Arc::new(FakeSource::new("tg", vec![item("quark:a", "电影 A")]));
        let bad: Arc<dyn CloudSearchSource> = Arc::new(FakeSource::failing("pansou"));
        let kernel = SearchKernel::new(vec![good, bad]);

        let page = kernel.search(&query("电影", 1, 20));
        assert!(page.partial, "有源失败应标记 partial");
        assert_eq!(page.degraded_sources, vec!["pansou".to_string()]);
        assert_eq!(page.total, 1);
    }

    #[test]
    fn marks_slow_source_as_degraded_without_blocking() {
        let fast: Arc<dyn CloudSearchSource> =
            Arc::new(FakeSource::new("tg", vec![item("quark:a", "电影 A")]));
        let slow: Arc<dyn CloudSearchSource> = Arc::new(FakeSource::slow(
            "slow",
            Duration::from_millis(600),
            vec![item("quark:b", "电影 B")],
        ));
        let kernel = SearchKernel::new(vec![fast, slow]).with_timeout(Duration::from_millis(60));

        let started = Instant::now();
        let page = kernel.search(&query("电影", 1, 20));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "不应被慢源阻塞: {elapsed:?}"
        );
        assert!(page.partial);
        assert_eq!(page.degraded_sources, vec!["slow".to_string()]);
        assert_eq!(page.total, 1, "慢源结果被超时丢弃");
    }

    #[test]
    fn caches_complete_pages() {
        let source = Arc::new(FakeSource::new("tg", vec![item("quark:a", "电影 A")]));
        let calls = source.calls.clone();
        let kernel = SearchKernel::new(vec![source as Arc<dyn CloudSearchSource>]);

        let first = kernel.search(&query("电影", 1, 20));
        assert_eq!(first.total, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let second = kernel.search(&query("电影", 1, 20));
        assert_eq!(second.total, 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "第二次应命中缓存，不再调用 source"
        );

        // 不同查询不该命中同一缓存。
        let _ = kernel.search(&query("电视剧", 1, 20));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(kernel.cache_len(), 2);
    }

    #[test]
    fn paginates_locally() {
        let items: Vec<SearchItem> = (0..5)
            .map(|index| item(&format!("quark:{index}"), &format!("电影 {index}")))
            .collect();
        let kernel = SearchKernel::new(vec![Arc::new(FakeSource::new("tg", items))]);

        let page = kernel.search(&query("电影", 2, 2));
        assert_eq!(page.total, 5, "total 是过滤后的全量");
        assert_eq!(page.items.len(), 2, "items 是当前页切片");
    }

    #[test]
    fn degraded_page_is_not_cached() {
        let bad: Arc<dyn CloudSearchSource> = Arc::new(FakeSource::failing("bad"));
        let kernel = SearchKernel::new(vec![bad]);
        let page = kernel.search(&query("电影", 1, 20));
        assert!(page.partial);
        assert_eq!(kernel.cache_len(), 0, "降级结果不应写缓存");
    }
}
