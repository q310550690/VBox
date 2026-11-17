//! `csp_Duanju`（短剧站 `duanju.one`）原生实现。
//!
//! 来源：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Duanju.java`
//! （166 行、0 处 jadx skip；`dffec63f-pg` 版 162 行同协议）。
//!
//! 原实现是「MacCMS module 模板」短剧站，协议很短：
//! * `init`：`ext` 直接作为基址（缺省 `https://duanju.one`）。
//! * `homeContent`：8 个写死分类（`1` 抖剧、`2` 快剧、`3` 都市、`26` 穿越、`25` 逆袭、
//!   `27` 虐恋、`28` 重生、`32` 其他）；首页第一组 `div.module-items` 的 `.module-item` 作为推荐。
//! * `categoryContent`：`{base}/vodshow/{cat}--------{page}---.html`（12 段 `-` 拼接，
//!   `[0]=分类`、`[8]=页`），行 `.module-items .module-item`。
//! * `detailContent`：id 是绝对 URL；标题 `h1.page-title`，标签 `a.tag-link` / `div.tag-link a`，
//!   更新信息 `div.title-info span`；播放线路来自 `.module-tab-item.tab-item` 的
//!   `span`（线路名）+ `small`（集数），分集容器 `[class=scroll-content]` 的 `a`，
//!   线路名拼接成 `名称(共N集)`；分集链接 = `{base}{href}`。
//! * `searchContent`：`{base}/vodsearch/{keyword}-------------.html`，行 `.module-search-item`。
//! * `playerContent`：抓播放页里的 `player_aaaa={...}` JSON，取 `url` 直出 + UA 头。
//!
//! 有意偏差：
//! 1. 上游 `duanju.one` 实测 DNS 失效（`fetch failed`），本轮只能 mock 验收；
//! 2. 原实现把 `vod_actor` 写成固定串 `FongMi`、`vod_director` 写成 `Qile`（模板占位），
//!    本实现不写占位值（保持字段为空），避免把「线路名/占位串」误当演员/导演；
//! 3. 原实现给 `vod_content` 写固定提示语，本实现改为抓取页面简介（若存在）；
//! 4. `vod_play_from` 的集数后缀保留原实现格式（`名称(共N集)`），便于与原站对照。

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Duanju";
pub const DEFAULT_BASE: &str = "https://duanju.one";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const CATEGORIES: [(&str, &str); 8] = [
    ("1", "抖剧"),
    ("2", "快剧"),
    ("3", "都市"),
    ("26", "穿越"),
    ("25", "逆袭"),
    ("27", "虐恋"),
    ("28", "重生"),
    ("32", "其他"),
];

/// 是否本家族可处理的 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let first = ext.split("$$$").next().unwrap_or("").trim();
    let base = if first.starts_with("http://") || first.starts_with("https://") {
        first.to_string()
    } else {
        DEFAULT_BASE.to_string()
    };
    base.trim_end_matches('/').to_string()
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".into(), UA.into());
    h
}

fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("duanju: 非法 CSS 选择器")
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join("").trim().to_string()
}

fn collapse(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn abs_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("https:{href}")
    } else {
        join_url(&format!("{base}/"), href)
    }
}

/// 12 段分类 URL：`{base}/vodshow/{cat}--------{page}---.html`。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let mut segs = [""; 12];
    if !cat.is_empty() {
        segs[0] = cat;
    }
    let page_s = if page > 1 { page.to_string() } else { String::new() };
    let mut parts: Vec<String> = Vec::with_capacity(12);
    for (idx, seg) in segs.iter().enumerate() {
        if idx == 8 {
            parts.push(page_s.clone());
        } else {
            parts.push((*seg).to_string());
        }
    }
    format!("{base}/vodshow/{}.html", parts.join("-"))
}

fn parse_rows(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = selector(".module-items .module-item");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let a = match row.select(&selector(".module-item-pic a")).next() {
            Some(a) => a,
            None => continue,
        };
        let href = a.value().attr("href").unwrap_or_default().trim();
        if href.is_empty() {
            continue;
        }
        let name = a
            .value()
            .attr("title")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| text_of(&a));
        let cover = row
            .select(&selector(".module-item-pic img"))
            .next()
            .and_then(|img| {
                img.value()
                    .attr("data-src")
                    .or_else(|| img.value().attr("src"))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| abs_url(base, s))
            });
        let remark = row
            .select(&selector(".module-item-text"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: abs_url(base, href),
            name,
            cover,
            remark,
        });
    }
    out
}

fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = selector(".module-search-item");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let a = match row.select(&selector(".module-item-pic a")).next() {
            Some(a) => a,
            None => continue,
        };
        let href = a.value().attr("href").unwrap_or_default().trim();
        if href.is_empty() {
            continue;
        }
        let img = row.select(&selector(".module-item-pic img")).next();
        let name = img
            .and_then(|i| i.value().attr("alt"))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| text_of(&a));
        let cover = img.and_then(|i| {
            i.value()
                .attr("data-src")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| abs_url(base, s))
        });
        let remark = row
            .select(&selector(".video-info-header a.video-serial"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: abs_url(base, href),
            name,
            cover,
            remark,
        });
    }
    out
}

/// 详情：标题 / 标签 / 线路（tab 名 + 集数）× 分集容器。
pub fn parse_detail(base: &str, html: &str) -> Option<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(&selector("h1.page-title"))
        .next()
        .map(|e| collapse(&text_of(&e)))
        .filter(|s| !s.is_empty())?;

    // 标签：`div.tag-link a` 与 `a.tag-link` 合并去重（原实现分别取第 2、3 项当地区/年份）。
    let mut tags: Vec<String> = Vec::new();
    for el in doc.select(&selector("div.tag-link a, a.tag-link")) {
        let t = collapse(&text_of(&el));
        if !t.is_empty() && !tags.contains(&t) {
            tags.push(t);
        }
    }
    let year = tags
        .iter()
        .find(|t| t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()))
        .cloned();
    // 原实现取 `a.tag-link` 的第 2 项作为地区（第 1 项是类型、第 3 项常是年份）。
    let non_numeric: Vec<&String> = tags
        .iter()
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let area = non_numeric
        .get(1)
        .or_else(|| non_numeric.first())
        .map(|s| (*s).clone());

    let cover = doc
        .select(&selector(".module-info-pic img, .module-item-pic img"))
        .next()
        .and_then(|img| {
            img.value()
                .attr("data-src")
                .or_else(|| img.value().attr("src"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| abs_url(base, s))
        });

    let tab_names: Vec<(String, String)> = doc
        .select(&selector(".module-tab-item.tab-item"))
        .map(|el| {
            let line = el
                .select(&selector("span"))
                .next()
                .map(|e| collapse(&text_of(&e)))
                .unwrap_or_default();
            let count = el
                .select(&selector("small"))
                .next()
                .map(|e| collapse(&text_of(&e)))
                .unwrap_or_default();
            (line, count)
        })
        .collect();
    let containers: Vec<ElementRef<'_>> = doc.select(&selector("[class=scroll-content]")).collect();

    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    for (idx, container) in containers.iter().enumerate() {
        let mut items: Vec<Episode> = Vec::new();
        for (n, a) in container.select(&selector("a")).enumerate() {
            let href = a.value().attr("href").unwrap_or_default().trim();
            if href.is_empty() {
                continue;
            }
            let mut title = collapse(&text_of(&a));
            if title.is_empty() {
                title = format!("第{}集", n + 1);
            }
            items.push(Episode {
                title,
                url: abs_url(base, href),
            });
        }
        if items.is_empty() {
            continue;
        }
        let (line, count) = tab_names
            .get(idx)
            .cloned()
            .unwrap_or_else(|| (format!("线路{}", idx + 1), String::new()));
        let group = if count.is_empty() {
            line
        } else {
            // 原实现把 tab 的 small 文本拼成「(共N集)」后缀。
            format!("{line}(共{count}集)")
        };
        episodes.push(EpisodeGroup { group, items });
    }
    if episodes.is_empty() {
        return None;
    }

    let mut desc = String::new();
    for el in doc.select(&selector(".module-info-introduction .module-info-content, .module-info-introduction")) {
        let t = collapse(&text_of(&el));
        if !t.is_empty() {
            desc = t;
            break;
        }
    }

    Some(VideoDetail {
        id: String::new(),
        name,
        cover,
        desc: if desc.is_empty() { None } else { Some(desc) },
        year,
        area,
        director: None,
        actor: None,
        episodes,
    })
}

/// 播放页：`player_aaaa={...}` JSON 里的 `url`。
pub fn parse_player(html: &str) -> Option<String> {
    let marker = "player_aaaa=";
    let start = html.find(marker)?;
    let rest = &html[start + marker.len()..];
    let end = rest.find("</script>").unwrap_or(rest.len().min(8000));
    let raw = rest[..end].trim().trim_end_matches(';').trim();
    let value: Value = serde_json::from_str(raw).ok()?;
    let url = value.get("url").and_then(Value::as_str)?.trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

fn site_error(site: &Site, detail: impl Into<String>, class: ErrorClass) -> crate::error::CoreError {
    CatvodError::new(class, site, detail.into()).into_core()
}

/// 分类（写死 8 个）。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cat.trim(), page);
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(), 2)?;
    Ok(parse_rows(&body, &base))
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let target = id.trim();
    if target.is_empty() {
        return Err(site_error(site, "详情 id 为空", ErrorClass::Parse));
    }
    let url = abs_url(&base, target);
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(), 2)?;
    parse_detail(&base, &body).ok_or_else(|| {
        site_error(
            site,
            "详情页没有可播放线路（站点改版或非 Duanju 模板？）",
            ErrorClass::Parse,
        )
    })
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let enc = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let url = format!("{base}/vodsearch/{enc}-------------.html");
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(), 2)?;
    Ok(parse_search(&body, &base))
}

/// 播放。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
    _vip_flags: &[String],
) -> Result<Value> {
    let base = base_of(site);
    let target = id.trim();
    if target.is_empty() {
        return Err(site_error(site, "播放 id 为空", ErrorClass::Parse));
    }
    let url = abs_url(&base, target);
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(), 2)?;
    match parse_player(&body) {
        Some(play_url) => Ok(json!({
            "url": play_url,
            "parse": 0,
            "jx": 0,
            "playUrl": "",
            "header": { "User-Agent": UA, "Referer": format!("{base}/") },
        })),
        None => Err(site_error(
            site,
            "播放页未找到 player_aaaa.url（站点改版？）",
            ErrorClass::HostApi,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Duanju"));
        assert!(handles("CSP_DUANJU"));
        assert!(!handles("csp_Zxzj"));
    }

    #[test]
    fn list_url_has_page_at_index_eight() {
        // 12 段用 '-' 拼接：第 1 页只留分类、共 11 个连字符。
        assert_eq!(
            list_url("https://duanju.one", "1", 1),
            format!("https://duanju.one/vodshow/1{}.html", "-".repeat(11))
        );
        assert_eq!(
            list_url("https://duanju.one", "26", 3),
            "https://duanju.one/vodshow/26--------3---.html"
        );
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"<div class="module-items"><div class="module-item">
          <div class="module-item-pic"><a href="/voddetail/123.html" title="逆袭人生">
            <img data-src="//img.test/a.jpg"></a></div>
          <div class="module-item-text">更新至12集</div></div></div>"#;
        let rows = parse_rows(html, "https://duanju.one");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "https://duanju.one/voddetail/123.html");
        assert_eq!(rows[0].name, "逆袭人生");
        assert_eq!(rows[0].cover.as_deref(), Some("https://img.test/a.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("更新至12集"));
    }

    #[test]
    fn parses_detail_tabs_and_episodes() {
        let html = r#"<h1 class="page-title">逆袭人生</h1>
          <div class="tag-link"><a>抖剧</a></div>
          <a class="tag-link">都市</a><a class="tag-link">2026</a>
          <div class="title-info"><span>已完结</span></div>
          <div class="module-tab-item tab-item"><span>线路1</span><small>12</small></div>
          <div class="module-tab-item tab-item"><span>线路2</span><small>3</small></div>
          <div class="scroll-content"><a href="/vodplay/123-1-1.html">第01集</a>
            <a href="/vodplay/123-1-2.html">第02集</a></div>
          <div class="scroll-content"><a href="/vodplay/123-2-1.html">第01集</a></div>"#;
        let d = parse_detail("https://duanju.one", html).expect("detail");
        assert_eq!(d.name, "逆袭人生");
        assert_eq!(d.year.as_deref(), Some("2026"));
        assert_eq!(d.area.as_deref(), Some("都市"));
        assert_eq!(d.episodes.len(), 2);
        assert_eq!(d.episodes[0].group, "线路1(共12集)");
        assert_eq!(d.episodes[1].items[0].url, "https://duanju.one/vodplay/123-2-1.html");
    }

    #[test]
    fn parses_search_rows() {
        let html = r#"<div class="module-search-item">
          <div class="module-item-pic"><a href="/voddetail/9.html">
            <img alt="重生之我在都市" data-src="/u/9.jpg"></a></div>
          <div class="video-info-header"><a class="video-serial">全20集</a></div></div>"#;
        let rows = parse_search(html, "https://duanju.one");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "https://duanju.one/voddetail/9.html");
        assert_eq!(rows[0].name, "重生之我在都市");
        assert_eq!(rows[0].remark.as_deref(), Some("全20集"));
    }

    #[test]
    fn parses_player_json() {
        let html = r#"<script>var player_aaaa={"flag":"play","url":"https:\/\/cdn.test\/a.m3u8"};</script>"#;
        assert_eq!(parse_player(html).as_deref(), Some("https://cdn.test/a.m3u8"));
    }

    #[test]
    fn detail_without_episodes_returns_none() {
        let html = r#"<h1 class="page-title">无线路</h1>"#;
        assert!(parse_detail("https://duanju.one", html).is_none());
    }
}
