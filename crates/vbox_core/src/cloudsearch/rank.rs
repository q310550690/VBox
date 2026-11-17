//! 结果后处理：去重合并、provider 过滤、排序、分页（W14 §4）。
//!
//! `postprocess` 只做「纯函数」变换，不关心结果来自哪些源；`partial` /
//! `degraded_sources` / `elapsed_ms` 由 [`super::kernel`] 在调用后填充。

use std::cmp::Ordering;
use std::collections::HashMap;

use super::{SearchItem, SearchPage, SearchQuery, SortBy};

/// 去重 → 过滤 → 排序 → 分页。
///
/// - `id` 相同视为同一资源，合并 `sources` 并保留字段里「信息量更大」的值；
/// - 标题为空（trim 后）的条目直接剔除；
/// - `providers` 非空时按 provider 过滤（空 = 全部）；
/// - `SortBy::Time` 按 `published_at` 降序（`None` 置后）；`SortBy::Relevance`
///   按关键词命中评分降序，再按 `published_at` 降序、`title` 升序保证稳定；
/// - `total` 是「去重 + 过滤后」的总数，`items` 是当前页切片。
pub fn postprocess(items: Vec<SearchItem>, q: &SearchQuery) -> SearchPage {
    let raw_len = items.len();
    let mut merged = merge_duplicates(items);
    let deduped = raw_len.saturating_sub(merged.len());
    merged.retain(|item| !item.title.trim().is_empty());

    // 过滤标签的计数在 provider 筛选前统计，代表该关键词下各网盘的真实条数。
    let provider_counts = count_by_provider(&merged);

    if !q.providers.is_empty() {
        merged.retain(|item| q.providers.contains(&item.provider));
    }

    sort_items(&mut merged, q);

    let total = merged.len();
    // 容忍调用方传入未归一化的 query：这里再收敛一次边界。
    let page = q.page.max(1) as usize;
    let page_size = q.page_size.clamp(1, SearchQuery::MAX_PAGE_SIZE) as usize;
    let start = (page - 1).saturating_mul(page_size).min(total);
    let end = start.saturating_add(page_size).min(total);
    let page_items = merged[start..end].to_vec();

    SearchPage {
        items: page_items,
        total,
        // kernel 会按实际聚合情况覆盖这两个字段。
        partial: false,
        elapsed_ms: 0,
        degraded_sources: Vec::new(),
        deduped,
        provider_counts,
    }
}

/// 按 provider 统计条数，按数量降序、同数量按 [`SearchProvider::ALL`] 顺序。
fn count_by_provider(items: &[SearchItem]) -> Vec<super::ProviderCount> {
    use std::collections::HashMap;
    let mut counts: HashMap<super::SearchProvider, usize> = HashMap::new();
    for item in items {
        *counts.entry(item.provider).or_insert(0) += 1;
    }
    let mut rows: Vec<super::ProviderCount> = counts
        .into_iter()
        .map(|(provider, count)| super::ProviderCount { provider, count })
        .collect();
    let order = |provider: super::SearchProvider| {
        super::SearchProvider::ALL
            .iter()
            .position(|candidate| *candidate == provider)
            .unwrap_or(usize::MAX)
    };
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| order(a.provider).cmp(&order(b.provider)))
    });
    rows
}

/// 按 `id` 合并重复项；保持首次出现的相对顺序（后续排序再决定最终顺序）。
fn merge_duplicates(items: Vec<SearchItem>) -> Vec<SearchItem> {
    let mut order: Vec<String> = Vec::with_capacity(items.len());
    let mut merged: HashMap<String, SearchItem> = HashMap::with_capacity(items.len());

    for item in items {
        match merged.get_mut(&item.id) {
            Some(existing) => merge_item(existing, item),
            None => {
                order.push(item.id.clone());
                merged.insert(item.id.clone(), item);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|id| merged.remove(&id))
        .collect()
}

/// 把 `incoming` 的信息合并进 `target`（target 为首次出现的条目）。
fn merge_item(target: &mut SearchItem, incoming: SearchItem) {
    // 标题：保留更长的（通常信息更全），空串让位。
    let target_title = target.title.trim();
    let incoming_title = incoming.title.trim();
    if !incoming_title.is_empty()
        && (target_title.is_empty()
            || incoming_title.chars().count() > target_title.chars().count())
    {
        target.title = incoming_title.to_string();
    }

    if target.share_url.trim().is_empty() && !incoming.share_url.trim().is_empty() {
        target.share_url = incoming.share_url;
    }

    // 提取码：保留第一个非空值。
    if target
        .password
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        if let Some(password) = incoming.password.filter(|value| !value.trim().is_empty()) {
            target.password = Some(password);
        }
    }

    // 体积：取较大值。
    target.size = match (target.size, incoming.size) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };

    // 时间：取较新值（Unix 秒）。
    target.published_at = match (target.published_at, incoming.published_at) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };

    // `playable`：任一副本可播即可播。
    target.playable = target.playable || incoming.playable;

    // sources：并集（去重、保持出现顺序）。
    for source in incoming.sources {
        if !source.is_empty() && !target.sources.contains(&source) {
            target.sources.push(source);
        }
    }
}

/// 排序；排序键相同则保持原有（首次出现）顺序，保证确定性。
fn sort_items(items: &mut [SearchItem], q: &SearchQuery) {
    match q.sort {
        SortBy::Time => {
            items.sort_by(|a, b| {
                compare_time_desc(a.published_at, b.published_at)
                    .then_with(|| a.title.cmp(&b.title))
            });
        }
        SortBy::Relevance => {
            let keyword = q.keyword.trim().to_lowercase();
            items.sort_by(|a, b| {
                relevance_score(&keyword, &b.title)
                    .cmp(&relevance_score(&keyword, &a.title))
                    .then_with(|| b.published_at.cmp(&a.published_at))
                    .then_with(|| a.title.cmp(&b.title))
            });
        }
    }
}

/// 关键词命中评分：完整短语命中权重最高，其次逐词命中，最后按出现次数累加。
fn relevance_score(keyword_lower: &str, title: &str) -> u32 {
    if keyword_lower.is_empty() {
        return 0;
    }
    let title_lower = title.to_lowercase();
    let mut score = 0u32;

    if title_lower == keyword_lower {
        score += 1000;
    } else if title_lower.starts_with(keyword_lower) {
        score += 500;
    } else if title_lower.contains(keyword_lower) {
        score += 300;
    }

    score += (count_occurrences(&title_lower, keyword_lower) as u32) * 20;

    // 多词关键词：每个词单独出现也给分（避免「合集 4K 合集」这类标题被整串匹配埋没）。
    for token in keyword_lower.split_whitespace() {
        if token != keyword_lower && title_lower.contains(token) {
            score += 40;
        }
    }
    score
}

/// 统计 `needle` 在 `haystack` 中不重叠出现的次数。
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut rest = haystack;
    while let Some(position) = rest.find(needle) {
        count += 1;
        rest = &rest[position + needle.len()..];
    }
    count
}

/// 稳定的「时间降序、`None` 置后」比较器（供其它模块复用）。
pub(crate) fn compare_time_desc(a: Option<i64>, b: Option<i64>) -> Ordering {
    b.cmp(&a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudsearch::SearchProvider;

    fn item(id: &str, title: &str, provider: SearchProvider) -> SearchItem {
        SearchItem {
            id: id.to_string(),
            title: title.to_string(),
            provider,
            share_url: format!("https://example.com/s/{id}"),
            password: None,
            size: None,
            published_at: None,
            sources: vec!["seed".to_string()],
            playable: true,
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
    fn dedupes_and_merges_fields() {
        let mut first = item("quark:abc", "电影合集", SearchProvider::Quark);
        first.size = Some(100);
        first.published_at = Some(1000);
        first.sources = vec!["tg:a".to_string()];

        let mut second = item("quark:abc", "电影合集 4K", SearchProvider::Quark);
        second.size = Some(2048);
        second.published_at = Some(2000);
        second.password = Some("xy9z".to_string());
        second.sources = vec!["tg:b".to_string(), "tg:a".to_string()];
        second.playable = false;

        let page = postprocess(vec![first, second], &query("电影", 1, 20));
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        let merged = &page.items[0];
        assert_eq!(merged.title, "电影合集 4K", "应保留更长标题");
        assert_eq!(merged.size, Some(2048), "应保留较大体积");
        assert_eq!(merged.published_at, Some(2000), "应保留较新时间");
        assert_eq!(merged.password.as_deref(), Some("xy9z"));
        assert_eq!(merged.sources, vec!["tg:a", "tg:b"], "sources 取并集且去重");
        assert!(merged.playable, "任一副本可播即可播");
    }

    #[test]
    fn drops_items_with_blank_title() {
        let page = postprocess(
            vec![
                item("a", "   ", SearchProvider::Quark),
                item("b", "", SearchProvider::Baidu),
                item("c", "正常标题", SearchProvider::Baidu),
            ],
            &query("正常", 1, 20),
        );
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, "c");
    }

    #[test]
    fn filters_by_provider() {
        let items = vec![
            item("a", "电影 A", SearchProvider::Quark),
            item("b", "电影 B", SearchProvider::Baidu),
            item("c", "电影 C", SearchProvider::Aliyun),
        ];
        let mut q = query("电影", 1, 20);
        q.providers = vec![SearchProvider::Baidu, SearchProvider::Aliyun];
        let page = postprocess(items.clone(), &q);
        assert_eq!(page.total, 2);
        assert_eq!(
            page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c"]
        );

        // 空 providers = 全部。
        let all = postprocess(items, &query("电影", 1, 20));
        assert_eq!(all.total, 3);
    }

    #[test]
    fn sorts_by_time_desc_with_none_last() {
        let mut old = item("a", "电影 旧", SearchProvider::Quark);
        old.published_at = Some(100);
        let mut new = item("b", "电影 新", SearchProvider::Quark);
        new.published_at = Some(300);
        let unknown = item("c", "电影 未知", SearchProvider::Quark);

        let mut q = query("电影", 1, 20);
        q.sort = SortBy::Time;
        let page = postprocess(vec![old, new, unknown], &q);
        assert_eq!(
            page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a", "c"],
            "时间降序，None 置后"
        );
    }

    #[test]
    fn sorts_by_relevance() {
        let weak = item("a", "周末看电影", SearchProvider::Quark);
        let exact = item("b", "电影", SearchProvider::Quark);
        let prefix = item("c", "电影合集 4K", SearchProvider::Quark);
        let none = item("d", "纪录片", SearchProvider::Quark);

        let mut q = query("电影", 1, 20);
        q.sort = SortBy::Relevance;
        let page = postprocess(vec![weak, exact, prefix, none], &q);
        assert_eq!(
            page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c", "a", "d"],
            "完全匹配 > 前缀 > 包含 > 未命中"
        );
    }

    #[test]
    fn relevance_ties_break_by_time_then_title() {
        let mut older = item("a", "电影 甲", SearchProvider::Quark);
        older.published_at = Some(100);
        let mut newer = item("b", "电影 乙", SearchProvider::Quark);
        newer.published_at = Some(200);

        let mut q = query("电影", 1, 20);
        q.sort = SortBy::Relevance;
        let page = postprocess(vec![older, newer], &q);
        assert_eq!(
            page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    #[test]
    fn paginates_and_reports_total() {
        let items: Vec<SearchItem> = (0..7)
            .map(|index| {
                item(
                    &format!("id{index}"),
                    &format!("电影 {index}"),
                    SearchProvider::Quark,
                )
            })
            .collect();
        let first = postprocess(items.clone(), &query("电影", 1, 3));
        assert_eq!(first.total, 7);
        assert_eq!(first.items.len(), 3);
        assert_eq!(first.items[0].id, "id0");

        let last = postprocess(items.clone(), &query("电影", 3, 3));
        assert_eq!(last.total, 7);
        assert_eq!(last.items.len(), 1);
        assert_eq!(last.items[0].id, "id6");

        // 越界页返回空切片，但 total 仍是全量。
        let beyond = postprocess(items, &query("电影", 9, 3));
        assert_eq!(beyond.total, 7);
        assert!(beyond.items.is_empty());

        // 未归一化的 page=0 / page_size=999 也要能安全处理。
        let raw = postprocess(
            (0..60)
                .map(|index| {
                    item(
                        &format!("p{index}"),
                        &format!("电影 {index}"),
                        SearchProvider::Quark,
                    )
                })
                .collect(),
            &query("电影", 0, 999),
        );
        assert_eq!(raw.items.len(), 50, "page_size 上限 50");
        assert_eq!(raw.total, 60);
    }

    #[test]
    fn occurrences_counting_is_non_overlapping() {
        assert_eq!(count_occurrences("aaaa", "aa"), 2);
        assert_eq!(count_occurrences("abc", "z"), 0);
        assert_eq!(count_occurrences("abc", ""), 0);
    }

    #[test]
    fn compare_time_desc_ordering() {
        assert_eq!(compare_time_desc(Some(2), Some(1)), Ordering::Less);
        assert_eq!(compare_time_desc(None, Some(1)), Ordering::Greater);
    }

    #[test]
    fn reports_deduped_count_and_provider_counts() {
        let page = postprocess(
            vec![
                item("quark:a", "甲", SearchProvider::Quark),
                item("quark:a", "甲 更长标题", SearchProvider::Quark),
                item("p115:b", "乙", SearchProvider::P115),
                item("other:c", "丙", SearchProvider::Other),
            ],
            &query("电影", 1, 20),
        );
        assert_eq!(page.deduped, 1, "跨频道同 id 合并计 1 条去重");
        assert_eq!(page.total, 3);
        let counts: Vec<(SearchProvider, usize)> = page
            .provider_counts
            .iter()
            .map(|row| (row.provider, row.count))
            .collect();
        assert_eq!(
            counts,
            vec![
                (SearchProvider::Quark, 1),
                (SearchProvider::P115, 1),
                (SearchProvider::Other, 1)
            ],
            "同数量按枚举顺序，且计数参与去重但不含重复项"
        );
    }

    #[test]
    fn provider_counts_are_before_provider_filter() {
        let mut q = query("电影", 1, 20);
        q.providers = vec![SearchProvider::Quark];
        let page = postprocess(
            vec![
                item("quark:a", "甲", SearchProvider::Quark),
                item("p115:b", "乙", SearchProvider::P115),
                item("pikpak:c", "   ", SearchProvider::PikPak),
            ],
            &q,
        );
        assert_eq!(page.total, 1, "筛选后仅剩夸克");
        assert_eq!(page.deduped, 0);
        let counts: Vec<(SearchProvider, usize)> = page
            .provider_counts
            .iter()
            .map(|row| (row.provider, row.count))
            .collect();
        assert_eq!(
            counts,
            vec![(SearchProvider::Quark, 1), (SearchProvider::P115, 1)],
            "计数在筛选前统计，空标题已剔除"
        );
    }
}
