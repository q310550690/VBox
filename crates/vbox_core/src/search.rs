//! 搜索：单源搜索 + 跨源并发搜索
//!
//! 单源搜索由各源适配器实现；跨源搜索用固定大小的线程池并发调用所有
//! 可搜索站点，带总超时（超时后返回已完成结果），单源失败不影响其它源。
//!
//! 线程数由桌面端「设置 → 搜索 → 搜索线程」控制（`SearchPrefs`），
//! CLI 等不传时使用 [`DEFAULT_SEARCH_THREADS`]。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::model::{SearchHit, Site, TvConfig, VideoItem};

/// 默认跨源搜索线程数（与设置页默认值保持一致）
pub const DEFAULT_SEARCH_THREADS: usize = 8;
/// 线程数上限（防止误配置成上千个并发请求）
pub const MAX_SEARCH_THREADS: usize = 32;
/// 跨源搜索结果总量上限（前端分页展示，不会一次性渲染）
const MAX_RESULTS: usize = 2000;

/// 单源搜索（供搜索结果页/快速搜索使用）
pub fn search_site(
    cfg: &TvConfig,
    site: &Site,
    keyword: &str,
    quick: bool,
) -> Result<Vec<VideoItem>> {
    crate::sources::search_site(cfg, site, keyword, quick)
}

/// 跨源并发搜索（默认线程数）：按站点顺序合并结果，单源失败/超时不影响其它源
pub fn search_all(
    cfg: &TvConfig,
    keyword: &str,
    limit_sites: usize,
    per_site_timeout_ms: u64,
) -> Vec<SearchHit> {
    search_all_with_threads(
        cfg,
        keyword,
        limit_sites,
        per_site_timeout_ms,
        DEFAULT_SEARCH_THREADS,
    )
}

/// 跨源并发搜索（可配置线程数）。
///
/// - `threads`：同时进行的站点请求数（1..=32，越界自动归一化）；
/// - `per_site_timeout_ms`：**整次跨源搜索**的等待上限，超时返回已完成的结果；
/// - 单源失败/超时不会让整次搜索失败，也不会阻塞其它源。
///
/// 结果按站点在配置里的原始顺序合并；之所以不按完成顺序返回，是为了让前端
/// 站点列表与配置文件顺序一致、多次搜索结果稳定。
pub fn search_all_with_threads(
    cfg: &TvConfig,
    keyword: &str,
    limit_sites: usize,
    per_site_timeout_ms: u64,
    threads: usize,
) -> Vec<SearchHit> {
    let kw = keyword.trim();
    if kw.is_empty() {
        return Vec::new();
    }
    let sites: Vec<Site> = cfg
        .sites
        .iter()
        .filter(|s| s.searchable() && s.kind != -1)
        .take(limit_sites.max(1))
        .cloned()
        .collect();
    if sites.is_empty() {
        return Vec::new();
    }

    let total = sites.len();
    let workers = threads.clamp(1, MAX_SEARCH_THREADS).min(total);
    // 任务队列 + 结果通道：固定 worker 数从队列取任务，避免一次打满全部站点。
    let queue: Arc<Mutex<VecDeque<(usize, Site)>>> =
        Arc::new(Mutex::new(sites.into_iter().enumerate().collect()));
    // 总超时后置位：worker 不再领取新任务，避免已返回结果后还在后台继续请求站点。
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(usize, Vec<SearchHit>)>();
    for _ in 0..workers {
        let cfg = cfg.clone();
        let kw = kw.to_string();
        let tx = tx.clone();
        let queue = Arc::clone(&queue);
        let cancelled = Arc::clone(&cancelled);
        std::thread::spawn(move || loop {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let job = {
                let mut guard = queue.lock().unwrap_or_else(|e| e.into_inner());
                guard.pop_front()
            };
            let Some((idx, site)) = job else {
                break;
            };
            let site_name = site.name.clone();
            let site_key = site.key.clone();
            // 失败站点也回传空结果：让接收端能按“已完成站点数”结束等待，
            // 而不是任何单站失败都要干等到总超时。
            let hits = match search_site(&cfg, &site, &kw, false) {
                Ok(items) => items
                    .into_iter()
                    .map(|item| SearchHit {
                        site_key: site_key.clone(),
                        site_name: site_name.clone(),
                        item,
                    })
                    .collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            };
            let _ = tx.send((idx, hits));
        });
    }
    drop(tx);

    let deadline = Instant::now() + Duration::from_millis(per_site_timeout_ms.clamp(1_000, 60_000));
    let mut collected: Vec<(usize, Vec<SearchHit>)> = Vec::new();
    let mut done = 0usize;
    while done < total {
        let now = Instant::now();
        if now >= deadline {
            // 停止派发后续站点；已在请求中的 worker 会在当前请求返回后退出。
            cancelled.store(true, Ordering::Relaxed);
            queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
            break;
        }
        match rx.recv_timeout(deadline.saturating_duration_since(now)) {
            Ok((idx, hits)) => {
                done += 1;
                collected.push((idx, hits));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::Relaxed);
                queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let mut all: Vec<(usize, SearchHit)> = collected
        .into_iter()
        .flat_map(|(idx, hits)| hits.into_iter().map(move |hit| (idx, hit)))
        .collect();
    all.sort_by_key(|(idx, _)| *idx);
    all.into_iter()
        .map(|(_, hit)| hit)
        .take(MAX_RESULTS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Site;

    fn site(key: &str, kind: i32, searchable: bool) -> Site {
        Site {
            key: key.to_string(),
            name: key.to_string(),
            kind,
            api: "http://127.0.0.1:1/api".to_string(),
            searchable,
            ..Default::default()
        }
    }

    /// 空关键词 / 无搜索源时直接返回空，不启动线程。
    #[test]
    fn empty_keyword_or_sites_returns_empty() {
        let cfg = TvConfig::empty();
        assert!(search_all_with_threads(&cfg, "  ", 60, 8000, 8).is_empty());
        let mut cfg = TvConfig::empty();
        cfg.sites = vec![site("a", 1, false), site("b", -1, true)];
        assert!(search_all_with_threads(&cfg, "x", 60, 8000, 8).is_empty());
    }

    /// 线程数归一化不会 panic：0 按 1 处理，超大值按上限处理。
    #[test]
    fn thread_bounds_are_normalized() {
        let mut cfg = TvConfig::empty();
        cfg.sites = vec![site("a", 1, true)];
        // 站点地址不可达，搜索会失败/超时，但函数必须安全返回（不 panic）。
        let hits = search_all_with_threads(&cfg, "x", 60, 1, 0);
        assert!(hits.is_empty());
        let hits = search_all_with_threads(&cfg, "x", 60, 1, 10_000);
        assert!(hits.is_empty());
    }
}
