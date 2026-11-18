//! Telegram 公开频道检索（W14 §4）。
//!
//! - [`parse_channel_html`]：纯函数，解析 `t.me/s/{handle}` 页面里的消息块，
//!   用 [`super::detect`] 提取分享链接，一条消息里的每个链接生成一个 [`SearchItem`]；
//! - [`TgKernelSource`]：`CloudSearchSource` 实现，按频道 GET `https://t.me/s/{handle}?q=<keyword>`，
//!   单频道失败只降级（`partial = true` + `degraded_sources = ["tg:<handle>"]`），不拖垮整体。
//!
//! 说明：Telegram 网页没有稳定公开 API，这里只做「HTML 结构容错解析」，
//! 选择器一旦失效只会导致结果变少，不会 panic（找不到文本块就跳过）。

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use scraper::{ElementRef, Html, Selector};

use super::cache::SearchCache;
use super::detect::{detect_share, extract_share_links};
use super::{CloudSearchSource, SearchItem, SearchPage, SearchQuery, SourceKind, SourceStatus};
use crate::error::CoreError;

/// 默认 UA（构造内部 client 时用；外部传入 client 则尊重调用方设置）。
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 单次搜索里并行抓取频道的总预算（毫秒）。
///
/// 内核还有一层全局预算（默认 8s）；这里先收敛，保证慢频道不拖住快频道返回。
const CHANNEL_BUDGET: Duration = Duration::from_millis(4_000);

/// Telegram 公开频道（仅保存 handle，真正的抓取在 [`TgKernelSource`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgChannel {
    handle: String,
}

impl TgChannel {
    /// 归一化 handle（去 `@` / 去 `https://t.me/` 前缀 / 去首尾空白）。
    pub fn new(handle: impl Into<String>) -> Self {
        TgChannel {
            handle: normalize_handle(&handle.into()),
        }
    }

    /// 归一化后的 handle。
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// 公开预览页 URL（不带关键词）。
    pub fn page_url(&self) -> String {
        format!("https://t.me/s/{}", self.handle)
    }

    /// 搜索页 URL。
    pub fn search_url(&self, keyword: &str) -> String {
        format!(
            "https://t.me/s/{}?q={}",
            self.handle,
            url_encode_query(keyword)
        )
    }

    /// 解析这个频道的网页 HTML。
    pub fn parse(&self, html: &str) -> Vec<SearchItem> {
        parse_channel_html(html, &self.handle)
    }
}

/// 解析 Telegram 频道预览页 HTML，返回可识别的分享项。
///
/// 纯函数：不联网、不读取全局状态，便于单测与回放。
pub fn parse_channel_html(html: &str, channel: &str) -> Vec<SearchItem> {
    let channel = normalize_handle(channel);
    let prepared = normalize_breaks(html);
    let document = Html::parse_document(&prepared);

    let messages = collect_messages(&document);
    let mut items = Vec::new();
    for message in messages {
        let links = extract_share_links(&message.text);
        if links.is_empty() {
            continue;
        }
        let title = message_title(&message.text);
        for raw_link in links {
            let Some(share) = detect_share(&raw_link) else {
                continue;
            };
            items.push(SearchItem {
                id: format!("{}:{}", share.provider.as_str(), share.share_id),
                title: title.clone(),
                provider: share.provider,
                share_url: share.share_url,
                password: share.password,
                size: None,
                published_at: message.published_at,
                sources: vec![channel.clone()],
                playable: share.provider.is_playable(),
            });
        }
    }
    items
}

/// 解析后的一条消息（内部结构）。
struct TgMessage {
    text: String,
    published_at: Option<i64>,
}

/// 先取 `.tgme_widget_message` 消息块；结构异常时回退为「整页文本块 + 首个时间」。
fn collect_messages(document: &Html) -> Vec<TgMessage> {
    let wrapper_selector = selector_or_none(".tgme_widget_message");
    let text_selector = selector_or_none(".tgme_widget_message_text");
    let time_selector = selector_or_none("time[datetime]");

    let mut messages = Vec::new();
    if let (Some(wrapper_selector), Some(text_selector), Some(time_selector)) =
        (&wrapper_selector, &text_selector, &time_selector)
    {
        for wrapper in document.select(wrapper_selector) {
            let Some(text_element) = wrapper.select(text_selector).next() else {
                continue;
            };
            let text = element_text(text_element);
            if text.trim().is_empty() {
                continue;
            }
            let published_at = wrapper
                .select(time_selector)
                .next()
                .and_then(|time| time.value().attr("datetime"))
                .and_then(parse_rfc3339_seconds);
            messages.push(TgMessage { text, published_at });
        }
    }

    if messages.is_empty() {
        // 回退：没有标准消息块时，把页面上所有文本块当独立消息，时间取页面第一个 datetime。
        let published_at = time_selector.as_ref().and_then(|time_selector| {
            document
                .select(time_selector)
                .next()
                .and_then(|time| time.value().attr("datetime"))
                .and_then(parse_rfc3339_seconds)
        });
        if let Some(text_selector) = &text_selector {
            for text_element in document.select(text_selector) {
                let text = element_text(text_element);
                if text.trim().is_empty() {
                    continue;
                }
                messages.push(TgMessage { text, published_at });
            }
        }
    }
    messages
}

fn selector_or_none(css: &str) -> Option<Selector> {
    Selector::parse(css).ok()
}

fn element_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>()
}

/// 把 `<br>` / `<br/>` / `<br />` 归一成 `\n`，让文本节点自带换行（用于取「首行标题」）。
fn normalize_breaks(html: &str) -> String {
    let bytes = html.as_bytes();
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut copied_upto = 0usize;
    let mut search_from = 0usize;
    while let Some(relative) = lower[search_from..].find("<br") {
        let start = search_from + relative;
        let Some(close_relative) = lower[start..].find('>') else {
            break;
        };
        let close = start + close_relative;
        let is_break_tag = lower[start + 3..close]
            .chars()
            .all(|ch| matches!(ch, '/' | ' ' | '\t' | '\r' | '\n'));
        if is_break_tag {
            out.push_str(&html[copied_upto..start]);
            out.push('\n');
            copied_upto = close + 1;
        }
        search_from = close + 1;
        if search_from >= bytes.len() {
            break;
        }
    }
    out.push_str(&html[copied_upto..]);
    out
}

/// 从消息文本提取标题：删掉分享链接与提取码短语，取第一行并收敛空白。
fn message_title(text: &str) -> String {
    let mut cleaned = remove_links(text);
    cleaned = strip_password_phrases(&cleaned);

    for line in cleaned.lines() {
        let collapsed = collapse_whitespace(line);
        let trimmed = collapsed.trim_matches(is_title_punctuation).trim();
        if !trimmed.is_empty() {
            return truncate_chars(trimmed, 120);
        }
    }
    "未命名资源".to_string()
}

fn remove_links(text: &str) -> String {
    let mut out = text.to_string();
    for link in extract_share_links(text) {
        let needle = link.to_ascii_lowercase();
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(position) = lower.find(&needle) else {
                break;
            };
            out.replace_range(position..position + needle.len(), " ");
        }
    }
    out
}

/// 删除「提取码：abcd」「password: abcd」这类片段（按行处理，避免误删整段）。
fn strip_password_phrases(text: &str) -> String {
    const KEYS: [&str; 6] = ["提取码", "访问码", "密码", "passcode", "password", "pwd"];
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut line = line.to_string();
        for key in KEYS {
            let key_lower = key.to_ascii_lowercase();
            loop {
                let lower = line.to_ascii_lowercase();
                let Some(position) = lower.find(&key_lower) else {
                    break;
                };
                let mut end = position + key_lower.len();
                let rest = &line[end..];
                let skipped = rest
                    .char_indices()
                    .take_while(|(_, ch)| {
                        ch.is_whitespace() || matches!(ch, ':' | '：' | '=' | ' ')
                    })
                    .last()
                    .map(|(offset, ch)| offset + ch.len_utf8())
                    .unwrap_or(0);
                end += skipped;
                // 提取码本身：直到下一个空白为止。
                let value_end = line[end..]
                    .char_indices()
                    .find(|(_, ch)| ch.is_whitespace())
                    .map(|(offset, _)| end + offset)
                    .unwrap_or(line.len());
                line.replace_range(position..value_end, " ");
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out
}

fn is_title_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '-' | '—' | '|' | '·' | ':' | '：' | '、' | '，' | '。' | '!' | '！' | '?' | '？' | ' '
    )
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

fn normalize_handle(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let without_host = without_scheme
        .strip_prefix("t.me/")
        .or_else(|| without_scheme.strip_prefix("www.t.me/"))
        .unwrap_or(without_scheme);
    let without_s = without_host.strip_prefix("s/").unwrap_or(without_host);
    without_s
        .trim_start_matches('@')
        .trim_end_matches('/')
        .to_string()
}

/// 最小 URL 查询串编码（只处理关键词里的少量特殊字符，避免额外依赖）。
fn url_encode_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 解析 RFC3339（`2026-11-01T12:34:56Z` / `...+08:00`）为 Unix 秒；时区后缀必填。
pub fn parse_rfc3339_seconds(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let (date_part, time_part) = raw.split_once('T').or_else(|| raw.split_once(' '))?;
    let mut date = date_part.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (time_part, offset_seconds) = if let Some(rest) = time_part.strip_suffix('Z') {
        (rest, 0i64)
    } else {
        // 找时区符号：`+` / `-`，且必须出现在时分秒之后。
        let sign_index = time_part
            .char_indices()
            .skip(1)
            .find(|(_, ch)| *ch == '+' || *ch == '-')
            .map(|(index, _)| index)?;
        let (time, offset) = time_part.split_at(sign_index);
        (time, parse_offset(offset)?)
    };

    let mut clock = time_part.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second_token = clock.next()?;
    let second: i64 = second_token.split('.').next()?.parse().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds)
}

/// `+08:00` / `-0530` → 秒（东区为正）。
fn parse_offset(offset: &str) -> Option<i64> {
    let sign = match offset.chars().next()? {
        '+' => 1i64,
        '-' => -1i64,
        _ => return None,
    };
    let rest = &offset[1..];
    let (hour, minute) = if let Some((hour, minute)) = rest.split_once(':') {
        (hour, minute)
    } else if rest.len() == 4 {
        (&rest[..2], &rest[2..])
    } else {
        return None;
    };
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    Some(sign * (hour * 3_600 + minute * 60))
}

/// Howard Hinnant 的 `days_from_civil`：公历日期 → 1970-01-01 起的天数。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Telegram 频道源：聚合多个公开频道，共享一个 HTTP client 与频道级缓存。
pub struct TgKernelSource {
    client: reqwest::blocking::Client,
    channels: Vec<String>,
    cache: Arc<SearchCache>,
}

impl TgKernelSource {
    /// 用调用方提供的 blocking client 构造。
    pub fn new(client: reqwest::blocking::Client, channels: Vec<String>) -> Self {
        TgKernelSource {
            client,
            channels: channels
                .into_iter()
                .map(|handle| normalize_handle(&handle))
                .collect(),
            cache: Arc::new(SearchCache::default()),
        }
    }

    /// 按默认 UA / 超时构造 client。
    pub fn with_default_client(channels: Vec<String>) -> Result<Self, CoreError> {
        let builder = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(DEFAULT_UA);
        let client = crate::net::apply_system_proxy(builder)
            .build()
            .map_err(CoreError::Http)?;
        Ok(Self::new(client, channels))
    }

    /// 注入共享缓存（`Arc` 便于 kernel / 多处复用同一份缓存）。
    pub fn with_cache(mut self, cache: Arc<SearchCache>) -> Self {
        self.cache = cache;
        self
    }

    /// 频道列表。
    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    /// 频道级缓存键：关键词带上 `tg:<handle>:` 前缀，避免和主搜索缓存串味。
    fn channel_cache_key(handle: &str, keyword: &str) -> SearchQuery {
        SearchQuery {
            keyword: format!("tg:{handle}:{keyword}"),
            page: 1,
            page_size: SearchQuery::MAX_PAGE_SIZE,
            providers: Vec::new(),
            sort: Default::default(),
        }
    }
}

/// 抓取单个频道并解析；HTTP / 解析失败都返回 `CoreError`（由调用方决定降级）。
///
/// 独立成自由函数是为了能在并行抓取时把 `Client`（内部 `Arc`，clone 廉价）搬进线程。
fn fetch_channel_with(
    client: &reqwest::blocking::Client,
    handle: &str,
    keyword: &str,
) -> Result<Vec<SearchItem>, CoreError> {
    let url = format!("https://t.me/s/{handle}");
    let response = client.get(&url).query(&[("q", keyword)]).send()?;
    let response = response.error_for_status()?;
    let html = response.text()?;
    Ok(parse_channel_html(&html, handle))
}

impl CloudSearchSource for TgKernelSource {
    fn search(&self, q: &SearchQuery) -> Result<SearchPage, CoreError> {
        let started = Instant::now();
        let keyword = q.keyword.trim();
        // 空关键词不联网：直接返回空页（normalized 已在上层拦截，这里兜底直接调用 trait 的情况）。
        if keyword.is_empty() || self.channels.is_empty() {
            let mut page = SearchPage::empty();
            page.elapsed_ms = started.elapsed().as_millis() as u64;
            return Ok(page);
        }

        let mut items = Vec::new();
        let mut degraded_sources = Vec::new();
        let mut first_error: Option<CoreError> = None;

        // 1) 频道级缓存命中的直接复用。
        let mut pending: Vec<String> = Vec::new();
        for handle in &self.channels {
            let key = super::cache::CacheKey::from_query(&Self::channel_cache_key(handle, keyword));
            if let Some(cached) = self.cache.get(&key) {
                items.extend(cached.items);
            } else {
                pending.push(handle.clone());
            }
        }

        // 2) 未命中的频道并行抓取：每频道独立线程 + 总预算，慢频道不拖住快频道。
        if !pending.is_empty() {
            let (tx, rx) = mpsc::channel::<(String, Result<Vec<SearchItem>, CoreError>)>();
            for handle in &pending {
                let tx = tx.clone();
                let client = self.client.clone();
                let handle = handle.clone();
                let keyword = keyword.to_string();
                std::thread::spawn(move || {
                    let result = fetch_channel_with(&client, &handle, &keyword);
                    let _ = tx.send((handle, result));
                });
            }
            drop(tx);

            let deadline = Instant::now() + CHANNEL_BUDGET;
            let mut received: Vec<String> = Vec::new();
            while received.len() < pending.len() {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok((handle, Ok(mut channel_items))) => {
                        let key = super::cache::CacheKey::from_query(&Self::channel_cache_key(
                            &handle, keyword,
                        ));
                        self.cache.put(
                            key,
                            SearchPage {
                                total: channel_items.len(),
                                items: channel_items.clone(),
                                partial: false,
                                elapsed_ms: 0,
                                degraded_sources: Vec::new(),
                                deduped: 0,
                                provider_counts: Vec::new(),
                            },
                        );
                        items.append(&mut channel_items);
                        received.push(handle);
                    }
                    Ok((handle, Err(error))) => {
                        // 关键词可能出现在 URL 里，日志只带频道名，不带错误详情（§11.2 隐私）。
                        tracing::warn!(channel = %handle, "Telegram 频道抓取失败，降级");
                        degraded_sources.push(format!("tg:{handle}"));
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                        received.push(handle);
                    }
                    Err(_) => break,
                }
            }
            // 预算内未返回的频道按降级处理（线程自然结束后结果丢弃）。
            for handle in pending {
                if !received.iter().any(|done| done == &handle) {
                    degraded_sources.push(format!("tg:{handle}"));
                }
            }
        }

        // 全部频道失败：向上返回错误，让 kernel 统一按 degraded 处理。
        if degraded_sources.len() == self.channels.len() && items.is_empty() {
            if let Some(error) = first_error {
                return Err(error);
            }
            return Err(CoreError::Config(
                "Telegram 频道均不可达（可能需要配置网络代理）".to_string(),
            ));
        }

        Ok(SearchPage {
            total: items.len(),
            items,
            partial: !degraded_sources.is_empty(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            degraded_sources,
            deduped: 0,
            provider_counts: Vec::new(),
        })
    }

    fn health(&self) -> SourceStatus {
        SourceStatus {
            kind: SourceKind::BuiltinKernel,
            ready: !self.channels.is_empty(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            detail: format!("telegram public channels: {}", self.channels.len()),
        }
    }

    fn name(&self) -> String {
        "tg".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
<div class="tgme_channel_info">公开频道</div>
<div class="tgme_widget_message_wrap js-widget_message_wrap">
  <div class="tgme_widget_message text_not_supported_wrap" data-post="yunpan_channel/101">
    <div class="tgme_widget_message_text js-message_text" dir="auto">
      阿里云盘 4K 电影合集【持续更新】<br/>
      https://pan.quark.cn/s/24f565ff8b77?pwd=xy9z<br/>
      提取码：xy9z
    </div>
    <div class="tgme_widget_message_footer">
      <a class="tgme_widget_message_date" href="https://t.me/yunpan_channel/101">
        <time datetime="2026-11-01T12:34:56+00:00">12:34</time>
      </a>
    </div>
  </div>
</div>
<div class="tgme_widget_message_wrap">
  <div class="tgme_widget_message" data-post="yunpan_channel/102">
    <div class="tgme_widget_message_text js-message_text">今天天气不错，随便聊聊人生</div>
    <div class="tgme_widget_message_footer">
      <time datetime="2026-11-02T08:00:00+00:00">08:00</time>
    </div>
  </div>
</div>
<div class="tgme_widget_message_wrap">
  <div class="tgme_widget_message" data-post="yunpan_channel/103">
    <div class="tgme_widget_message_text js-message_text">
      百度网盘 学习资料合集<br/>
      https://pan.baidu.com/s/1abc?password=1234
    </div>
    <div class="tgme_widget_message_footer">
      <time datetime="2026-11-03T00:00:00+08:00">00:00</time>
    </div>
  </div>
</div>
"#;

    #[test]
    fn parses_messages_with_share_links() {
        let items = parse_channel_html(FIXTURE, "yunpan_channel");
        assert_eq!(items.len(), 2, "非分享消息应被跳过: {items:?}");

        let quark = items
            .iter()
            .find(|item| item.provider == crate::cloudsearch::SearchProvider::Quark)
            .expect("应解析出 Quark 分享");
        assert_eq!(quark.id, "quark:24f565ff8b77");
        assert_eq!(quark.title, "阿里云盘 4K 电影合集【持续更新】");
        assert_eq!(quark.password.as_deref(), Some("xy9z"));
        assert_eq!(
            quark.share_url,
            "https://pan.quark.cn/s/24f565ff8b77?pwd=xy9z"
        );
        assert!(quark.playable);
        assert_eq!(quark.sources, vec!["yunpan_channel".to_string()]);
        assert_eq!(quark.published_at, Some(1_793_536_496));

        let baidu = items
            .iter()
            .find(|item| item.provider == crate::cloudsearch::SearchProvider::Baidu)
            .expect("应解析出 Baidu 分享");
        assert_eq!(baidu.id, "baidu:1abc");
        assert_eq!(baidu.title, "百度网盘 学习资料合集");
        assert_eq!(baidu.password.as_deref(), Some("1234"));
        // 2026-11-03T00:00:00+08:00 == 2026-11-02T16:00:00Z
        assert_eq!(baidu.published_at, Some(1_793_635_200));
    }

    #[test]
    fn skips_messages_without_share_links() {
        let html = r#"
<div class="tgme_widget_message" data-post="c/1">
  <div class="tgme_widget_message_text">纯聊天，没有链接</div>
  <time datetime="2026-11-01T00:00:00Z">x</time>
</div>
"#;
        assert!(parse_channel_html(html, "c").is_empty());
    }

    #[test]
    fn skips_empty_message_text() {
        let html = r#"
<div class="tgme_widget_message" data-post="c/1">
  <div class="tgme_widget_message_text">   </div>
  <time datetime="2026-11-01T00:00:00Z">x</time>
</div>
<div class="tgme_widget_message" data-post="c/2">
  <div class="tgme_widget_message_text">资源<br/>magnet:?xt=urn:btih:aaaabbbbccccddddeeeeffff0000111122223333</div>
  <time datetime="2026-11-01T00:00:00Z">x</time>
</div>
"#;
        let items = parse_channel_html(html, "c");
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].provider,
            crate::cloudsearch::SearchProvider::Magnet
        );
        assert!(!items[0].playable);
        assert_eq!(items[0].title, "资源");
    }

    #[test]
    fn multiple_links_in_one_message_become_multiple_items() {
        let html = r#"
<div class="tgme_widget_message" data-post="c/9">
  <div class="tgme_widget_message_text">
    双盘合集<br/>
    https://pan.quark.cn/s/aaaa1111<br/>
    https://pan.baidu.com/s/bbbb2222?pwd=ww11
  </div>
  <time datetime="2026-11-01T00:00:00Z">x</time>
</div>
"#;
        let items = parse_channel_html(html, "c");
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.title == "双盘合集"));
        assert!(items
            .iter()
            .all(|item| item.sources == vec!["c".to_string()]));
    }

    #[test]
    fn fallback_without_message_wrappers() {
        let html = r#"
<div class="tgme_widget_message_text">只有文本块<br/>https://drive.uc.cn/s/uc123456</div>
<time datetime="2026-11-01T00:00:00Z">x</time>
"#;
        let items = parse_channel_html(html, "c");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].provider, crate::cloudsearch::SearchProvider::Uc);
        assert_eq!(items[0].published_at, Some(1_793_491_200));
    }

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(parse_rfc3339_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_seconds("2021-01-01T00:00:00Z"),
            Some(1_609_459_200)
        );
        assert_eq!(
            parse_rfc3339_seconds("2021-01-01T08:00:00+08:00"),
            Some(1_609_459_200)
        );
        assert_eq!(
            parse_rfc3339_seconds("2021-01-01T00:00:00.500Z"),
            Some(1_609_459_200)
        );
        assert_eq!(
            parse_rfc3339_seconds("2021-01-01T00:00:00"),
            None,
            "缺时区应拒绝"
        );
        assert_eq!(parse_rfc3339_seconds("not-a-date"), None);
    }

    #[test]
    fn channel_handle_normalization() {
        assert_eq!(TgChannel::new("@yunpan_channel").handle(), "yunpan_channel");
        assert_eq!(
            TgChannel::new("https://t.me/s/yunpan_channel").handle(),
            "yunpan_channel"
        );
        assert_eq!(
            TgChannel::new("t.me/yunpan_channel/").search_url("电影"),
            "https://t.me/s/yunpan_channel?q=%E7%94%B5%E5%BD%B1"
        );
    }

    #[test]
    #[ignore = "live: 需要可访问 t.me 的网络（桌面端可用系统代理）"]
    fn live_builtin_channels_return_results() {
        let source = TgKernelSource::with_default_client(crate::cloudsearch::builtin_channels())
            .expect("client");
        let query = SearchQuery {
            keyword: "可能".to_string(),
            page: 1,
            page_size: 20,
            ..SearchQuery::default()
        };
        let page = source.search(&query).expect("search");
        assert!(
            !page.items.is_empty(),
            "内置频道没有返回结果：检查网络/代理与频道清单是否失效"
        );
    }

    #[test]
    fn empty_keyword_does_not_need_network() {
        let source = TgKernelSource::new(
            reqwest::blocking::Client::builder()
                .build()
                .expect("client"),
            vec!["yunpan_channel".to_string()],
        );
        let page = source
            .search(&SearchQuery {
                keyword: "   ".to_string(),
                ..SearchQuery::default()
            })
            .expect("空关键词应直接返回空页");
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
        assert!(!page.partial);
    }

    #[test]
    fn empty_channels_returns_empty_page() {
        let source = TgKernelSource::new(
            reqwest::blocking::Client::builder()
                .build()
                .expect("client"),
            Vec::new(),
        );
        let page = source
            .search(&SearchQuery {
                keyword: "电影".to_string(),
                ..SearchQuery::default()
            })
            .expect("无频道应返回空页");
        assert_eq!(page.total, 0);
        assert!(!page.partial);
        assert!(!source.health().ready);
        assert_eq!(source.name(), "tg");
    }
}
