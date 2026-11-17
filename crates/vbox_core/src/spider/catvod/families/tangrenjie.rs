//! `csp_Tangrenjie`（唐人街影院）原生实现。
//!
//! 来源：`_rea/decompiled/ddaf13d7-0206/deobf/com/github/catvod/spider/Tangrenjie.java`
//! （479 行，`categoryContent` 被 jadx 跳过 496 指令）。用
//! `jadx --show-bad-code --single-class com.github.catvod.spider.Tangrenjie`
//! 对 `ddaf13d7-0206.jpg` 重跑恢复出 572 行，`categoryContent` 可读。
//!
//! 原实现（老站 `https://www.tangrenjie.tv`，现 ECONNREFUSED）：
//! * 基址写死 `.tv`；请求头 UA Chrome/91(macOS) + Accept + Accept-Language。
//! * `homeContent`：`ul.top_nav li a`，只收「电影/电视剧/动漫/综艺」，`type_id`
//!   取 `/vod/type/id/(\d+).html` 的数字；`filters` 写死 4 组（电影/剧集/纪录片/动漫）。
//! * `categoryContent`：`{base}/vod/show/id/{type}` + filter 键值 + `/page/{page}.html`；
//!   行 `ul.vodlist li.vodlist_item`：`a.vodlist_thumb[title]`、`img[data-original]`、
//!   `span.pic_text b`、`/vod/detail/id/(\d+).html`；分页 `ul.page li a`。
//! * `detailContent`：`/vod/detail/id/{id}.html`；封面/标题 `div.content_thumb a`、
//!   简介 `div.content > section > font`、备注 `span.data_style b`、元数据 `li.data`、
//!   播放线路 `div.play_source_tab a`（alt 中文名）对行表 `Bf`（kdm3u8=全球二、
//!   haiwaikan=蓝光二）映射，`div.playlist_full` 逐个 `ul>li>a` 的
//!   `/vod/play/id/{id}/sid/{sid}/nid/{nid}.html` 拼成 `{id}/sid/{sid}/nid/{nid}={type}`。
//! * `playerContent`：`{base}/vod/play/id/{path}.html`，页面 `var player_aaaa={...url...}`
//!   的 `url` 为 Base64(URL 编码) 的直链，`parse:0`；失败回退 `parse:1` 播放页。
//! * `searchContent`：`/vod/search.html?wd=`，行 `li.searchlist_item`。
//!
//! 现站（2026-11，`https://www.tangrenjie.net`，实测全链可访问）已换成
//! MacCMS 模板（`MAC`），路由与选择器全变：
//! * 导航 `#nav li a`，`/type/{id}.html`（电影 20/连续剧 28/动漫 35/综艺 36/
//!   体育赛事 37/短剧 39/影视解说 52）；筛选 `/show/{id}---{class}--------.html`。
//! * 列表第 1 页 `/type/{id}.html`，第 N 页 `/show/{id}--------{N}---.html`；
//!   行 `ul.img-list li > a[href^=/vod/]`：`title`、`img[src]`、`h2`、`p`、`i`。
//! * 详情 `/vod/{slug}.html`：`div.detail-pic img`、`div.detail-title h1`、
//!   `div.detail-info dl`（dt/dd 元数据）、`div.vod-intro p`（简介）、
//!   播放区 `div.box[id^=playlist_]`（标题 `h2`，`div.video_list a` 指向
//!   `/play/{slug}-{sid}-{nid}.html`）。
//! * 播放页 `/play/{slug}-{sid}-{nid}.html`：标准 MacCMS `var player_aaaa={...}`，
//!   `url` 为转义的 m3u8 直链（`encrypt:0`）。
//! * 搜索 `/search/{URL编码关键词}----------{N}---.html`，行 `ul.show-list li`。
//!
//! 有意偏差：
//! 1. 基址默认改为现役 `https://www.tangrenjie.net`（原类写死 `.tv`，已死）；
//!    `ext` 首段 http(s) 可覆盖基址（原类不读 ext）。
//! 2. 分类改为现站导航（原类只收 4 个固定分类）；筛选 URL 生成暂不实现
//!    （分派层不传筛选值，现站筛选值编码规则独立于查询参数）。
//! 3. 列表备注取 `i`（状态，如「正片」），原类取 `span.pic_text b`；
//!    `p`（主演）不入字段。
//! 4. 详情线路名取播放区标题（去 emoji 与「-在线播放」后缀），原类用
//!    `Bf` 行表映射（老站行表对新站线路名无意义）。
//! 5. 搜索改为现站 `/search/{wd}----------1---.html`（原 `/vod/search.html?wd=` 404）。
//! 6. 播放取 `player_aaaa.url` 直链（等价老站 base64 解码后的结果）；
//!    `encrypt!=0` 或其他加密标记视为不支持，报明确错误。

use std::collections::HashMap;
use std::sync::OnceLock;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Tangrenjie";

/// 现役站点（原实现写死 `https://www.tangrenjie.tv`，已 ECONNREFUSED）。
const DEFAULT_BASE: &str = "https://www.tangrenjie.net";

/// 原实现与现站通用的 PC UA。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 导航里不是分类的入口。
const NON_CATEGORY_NAV: [&str; 3] = ["网站首页", "专题", "会员"];

/// 是否本家族可处理的 api（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 基址：默认现役站；`ext` 首段为 http(s) 时覆盖（原类不读 ext，记偏差 1）。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let first = ext.split('$').next().unwrap_or("").trim();
    if first.starts_with("http://") || first.starts_with("https://") {
        first.trim_end_matches('/').to_string()
    } else {
        DEFAULT_BASE.to_string()
    }
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert(
        "Accept".to_string(),
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
    );
    h.insert(
        "Accept-Language".to_string(),
        "zh-CN,zh;q=0.8,zh-TW;q=0.7,zh-HK;q=0.5,en-US;q=0.3,en;q=0.2".to_string(),
    );
    h
}

fn fetch_text(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    Ok(get_text(&client, url, &headers(), 2)?)
}

/* ============================================================
 * 选择器与工具
 * ============================================================ */

fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("tangrenjie: 非法 CSS 选择器")
}

fn nav_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| selector("#nav li a"))
}

fn list_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| selector("ul.img-list li"))
}

fn search_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| selector("ul.show-list li"))
}

fn play_box_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| selector("div.box[id^=\"playlist_\"]"))
}

fn type_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/type/(\d+)\.html").expect("tangrenjie: type_id 正则"))
}

fn player_json_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?s)var\s+player_aaaa\s*=\s*(\{.*?\})\s*;").expect("tangrenjie: player 正则")
    })
}

fn text_of(el: &ElementRef<'_>) -> String {
    collapse(&el.text().collect::<String>())
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value()
        .attr(name)
        .map(collapse)
        .filter(|s| !s.is_empty())
}

/// 详情 id：现站为 `/vod/{slug}.html`（相对路径），也兼容绝对 URL。
fn normalize_id(id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.starts_with('/') {
        id.to_string()
    } else {
        format!("/{id}")
    }
}

fn absolute(base: &str, id: &str) -> String {
    join_url(base, &normalize_id(id))
}

/* ============================================================
 * URL 生成
 * ============================================================ */

/// 分类列表 URL：第 1 页 `/type/{cat}.html`，第 N 页 `/show/{cat}--------{N}---.html`。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let base = base.trim_end_matches('/');
    let cat = cat.trim();
    if page <= 1 {
        format!("{base}/type/{cat}.html")
    } else {
        format!("{base}/show/{cat}--------{page}---.html")
    }
}

pub fn detail_url(base: &str, id: &str) -> String {
    absolute(base, id)
}

/// 搜索 URL：现站路由 `/search/{关键词}----------{N}---.html`。
pub fn search_url(base: &str, keyword: &str) -> String {
    let base = base.trim_end_matches('/');
    let kw = utf8_percent_encode(keyword.trim(), NON_ALPHANUMERIC).to_string();
    format!("{base}/search/{kw}----------1---.html")
}

/// 播放页 URL（id 形如 `/play/{slug}-{sid}-{nid}.html`）。
pub fn play_url(base: &str, id: &str) -> String {
    absolute(base, id)
}

/* ============================================================
 * 分类与列表
 * ============================================================ */

/// 解析首页导航分类（`#nav li a`，只收 `/type/{id}.html`）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out: Vec<Category> = Vec::new();
    for a in doc.select(nav_sel()) {
        let name = collapse(&a.text().collect::<String>());
        if name.is_empty() || NON_CATEGORY_NAV.contains(&name.as_str()) {
            continue;
        }
        let href = a.value().attr("href").unwrap_or_default();
        let id = match type_id_re().captures(href) {
            Some(cap) => cap[1].to_string(),
            None => continue,
        };
        if out.iter().any(|c| c.type_id == id) {
            continue;
        }
        out.push(Category {
            type_id: id,
            type_name: name,
        });
    }
    out
}

/// 解析列表页行：`ul.img-list li > a[href^=/vod/]`。
pub fn parse_list(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(list_row_sel()) {
        let a = match li.select(&selector("a[href]")).next() {
            Some(a) => a,
            None => continue,
        };
        let href = a.value().attr("href").unwrap_or_default();
        if !href.contains("/vod/") {
            continue;
        }
        let mut name = attr(&a, "title").unwrap_or_default();
        if name.is_empty() {
            if let Some(h2) = a.select(&selector("h2")).next() {
                name = text_of(&h2);
            }
        }
        if name.is_empty() {
            continue;
        }
        let cover = a
            .select(&selector("img"))
            .next()
            .and_then(|img| attr(&img, "src").or_else(|| attr(&img, "data-original")));
        let remark = {
            let i = a.select(&selector("i")).next().map(|e| text_of(&e)).unwrap_or_default();
            if i.is_empty() {
                None
            } else {
                Some(i)
            }
        };
        out.push(VideoItem {
            id: normalize_id(href),
            name,
            cover,
            remark,
        });
    }
    out
}

/// 分类视频列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let cat = cat.trim();
    if cat.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "分类 id 为空").into_core());
    }
    let url = list_url(&base, cat, page.max(1));
    let html = fetch_text(cfg, &url)?;
    Ok(parse_list(&html))
}

/* ============================================================
 * 详情
 * ============================================================ */

/// 详情页解析结果（纯 HTML 输入，便于单测）。
fn clean_group_name(raw: &str) -> String {
    // 去掉 emoji/符号，再去掉模板后缀「-在线播放」。
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .collect();
    let cleaned = cleaned.trim_start_matches('▶').trim();
    let cut = ["在线播放", "播放列表", "播放"];
    for suffix in cut {
        if let Some(idx) = cleaned.find(suffix) {
            if idx > 0 {
                return cleaned[..idx].to_string();
            }
        }
    }
    cleaned.to_string()
}

fn dl_pairs(doc: &Html) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dl in doc.select(&selector("div.detail-info dl")) {
        let dt = dl
            .select(&selector("dt"))
            .next()
            .map(|e| text_of(&e))
            .unwrap_or_default();
        let dd = dl
            .select(&selector("dd"))
            .next()
            .map(|e| text_of(&e))
            .unwrap_or_default();
        if !dt.is_empty() && !dd.is_empty() {
            out.push((dt, dd));
        }
    }
    out
}

/// 从 `dt/dd` 里按关键词取元数据。
fn meta_value(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(dt, _)| dt.contains(key))
        .map(|(_, dd)| dd.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_episode_groups(doc: &Html) -> Vec<EpisodeGroup> {
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for box_el in doc.select(play_box_sel()) {
        let raw_title = box_el
            .select(&selector("div.box-title h2"))
            .next()
            .map(|e| text_of(&e))
            .unwrap_or_default();
        let group = {
            let g = clean_group_name(&raw_title);
            if g.is_empty() {
                format!("线路{}", groups.len() + 1)
            } else {
                g
            }
        };
        let mut items: Vec<Episode> = Vec::new();
        for a in box_el.select(&selector("div.video_list a[href]")) {
            let href = a.value().attr("href").unwrap_or_default().trim().to_string();
            if !href.contains("/play/") {
                continue;
            }
            let title = text_of(&a);
            if title.is_empty() {
                continue;
            }
            items.push(Episode {
                title,
                url: normalize_id(&href),
            });
        }
        if !items.is_empty() {
            groups.push(EpisodeGroup { group, items });
        }
    }
    groups
}

/// 解析详情页；缺少标题视为非详情页。
pub fn parse_detail(html: &str, id: &str) -> Option<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(&selector("div.detail-title h1"))
        .next()
        .map(|e| text_of(&e))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            doc.select(&selector("h1"))
                .next()
                .map(|e| text_of(&e))
                .filter(|s| !s.is_empty())
        })?;
    let cover = doc
        .select(&selector("div.detail-pic img"))
        .next()
        .and_then(|img| attr(&img, "src").or_else(|| attr(&img, "data-original")));
    let desc = {
        let parts: Vec<String> = doc
            .select(&selector("div.vod-intro p"))
            .map(|p| text_of(&p))
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    };
    let pairs = dl_pairs(&doc);
    Some(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year: meta_value(&pairs, "年份"),
        area: meta_value(&pairs, "地区"),
        director: meta_value(&pairs, "导演"),
        actor: meta_value(&pairs, "主演").or_else(|| meta_value(&pairs, "演员")),
        episodes: parse_episode_groups(&doc),
    })
}

/// 详情入口。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    if id.trim().is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "详情 id 为空").into_core());
    }
    let base = base_of(site);
    let url = detail_url(&base, id);
    let html = fetch_text(cfg, &url)?;
    parse_detail(&html, id).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少 `div.detail-title h1`（站点改版或非 Tangrenjie 模板？）",
        )
        .into_core()
    })
}

/* ============================================================
 * 分类入口 + 搜索 + 播放
 * ============================================================ */

/// 首页分类入口。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = fetch_text(cfg, &format!("{base}/"))?;
    let cats = parse_categories(&html);
    if cats.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "首页导航解析为空（站点改版或反爬？）",
        )
        .into_core());
    }
    Ok(cats)
}

/// 解析搜索结果行（`ul.show-list li`）。
pub fn parse_search(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(search_row_sel()) {
        let title_a = match li.select(&selector("h2 a[href]")).next() {
            Some(a) => a,
            None => continue,
        };
        let href = title_a.value().attr("href").unwrap_or_default();
        if !href.contains("/vod/") {
            continue;
        }
        let name = text_of(&title_a);
        if name.is_empty() {
            continue;
        }
        let cover = li
            .select(&selector("a.play-img img"))
            .next()
            .and_then(|img| attr(&img, "src").or_else(|| attr(&img, "data-original")));
        let mut remark: Option<String> = None;
        for dl in li.select(&selector("dl")) {
            let dt = dl
                .select(&selector("dt"))
                .next()
                .map(|e| text_of(&e))
                .unwrap_or_default();
            let dd = dl
                .select(&selector("dd"))
                .next()
                .map(|e| text_of(&e))
                .unwrap_or_default();
            if dd.is_empty() {
                continue;
            }
            if dt.contains("状态") {
                remark = Some(dd);
                break;
            }
            if remark.is_none() && dt.contains("主演") {
                remark = Some(dd);
            }
        }
        out.push(VideoItem {
            id: normalize_id(href),
            name,
            cover,
            remark,
        });
    }
    out
}

/// 搜索入口。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let kw = keyword.trim();
    if kw.is_empty() {
        return Ok(Vec::new());
    }
    let base = base_of(site);
    let url = search_url(&base, kw);
    let html = fetch_text(cfg, &url)?;
    Ok(parse_search(&html))
}

/// 从播放页提取 `player_aaaa` 的 JSON 对象。
pub fn parse_player_json(html: &str) -> Option<Value> {
    let cap = player_json_re().captures(html)?;
    serde_json::from_str::<Value>(&cap[1]).ok()
}

/// 从播放页解析直链（MacCMS `player_aaaa.url`，`encrypt:0` 时即 m3u8）。
pub fn parse_play_url(html: &str) -> Option<String> {
    let player = parse_player_json(html)?;
    let encrypt = player.get("encrypt").and_then(Value::as_i64).unwrap_or(0);
    if encrypt != 0 {
        return None;
    }
    let url = player.get("url").and_then(Value::as_str)?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("//") {
        return Some(format!("https:{url}"));
    }
    Some(url)
}

/// 播放入口：取播放页 `player_aaaa.url`，直链 `parse:0`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    if id.trim().is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "播放 id 为空").into_core());
    }
    let base = base_of(site);
    let url = play_url(&base, id);
    let html = fetch_text(cfg, &url)?;
    let resolved = parse_play_url(&html).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "播放页未找到可用的 `player_aaaa.url`（加密标记或站点改版？）",
        )
        .into_core()
    })?;
    Ok(json!({
        "url": resolved,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * 单测（纯解析，不发请求）
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Tangrenjie"));
        assert!(handles("CSP_TANGRENJIE"));
        assert!(!handles("csp_Ddrk"));
    }

    fn site_with_ext(ext: Value) -> Site {
        let mut site = Site::default();
        site.key = "tangrenjie".to_string();
        site.name = "唐人街影院".to_string();
        site.api = API.to_string();
        site.ext = Some(ext);
        site
    }

    #[test]
    fn base_defaults_to_live_site_and_ext_can_override() {
        assert_eq!(base_of(&site_with_ext(json!(null))), DEFAULT_BASE);
        assert_eq!(
            base_of(&site_with_ext(json!("https://mirror.example/tj$$$x/$$$2"))),
            "https://mirror.example/tj"
        );
    }

    #[test]
    fn list_url_uses_type_then_show_pages() {
        assert_eq!(list_url(DEFAULT_BASE, "20", 1), "https://www.tangrenjie.net/type/20.html");
        assert_eq!(
            list_url(DEFAULT_BASE, "20", 3),
            "https://www.tangrenjie.net/show/20--------3---.html"
        );
    }

    #[test]
    fn search_url_escapes_keyword() {
        assert_eq!(
            search_url(DEFAULT_BASE, "吸血鬼"),
            "https://www.tangrenjie.net/search/%E5%90%B8%E8%A1%80%E9%AC%BC----------1---.html"
        );
    }

    #[test]
    fn parses_nav_categories() {
        let html = r#"<div id="navbar"><ul id="nav" class="ui-nav">
            <li class="nav-item current"><a class="nav-link" href="/">网站首页</a></li>
            <li class="nav-item 20=0"><a class="nav-link" href="/type/20.html">电影</a></li>
            <li class="nav-item 28=0"><a class="nav-link" href="/type/28.html">连续剧</a></li>
            <li class="nav-item "><a class="nav-link" href="/topic.html">专题</a></li>
            <li class="nav-item mac_user"><a class="nav-link" href="javascript:;">会员</a></li>
        </ul></div>"#;
        let cats = parse_categories(html);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "20");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "28");
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"<ul class="img-list">
            <li><a href="/vod/SCCCCKw8.html" title="梁祝传说"><img src="https://img.tangrenjie.net/a.jpg" alt="梁祝传说"/><h2>梁祝传说</h2><p>祝英台,梁山伯</p><i>正片</i><em></em></a></li>
            <li><a href="/vod/SCCCCKwA.html"><img data-original="https://img.tangrenjie.net/b.jpg"/><h2>寄生狂潮</h2><i>更新至10集</i></a></li>
        </ul>"#;
        let items = parse_list(html);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "/vod/SCCCCKw8.html");
        assert_eq!(items[0].name, "梁祝传说");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.tangrenjie.net/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("正片"));
        assert_eq!(items[1].name, "寄生狂潮");
        assert_eq!(items[1].cover.as_deref(), Some("https://img.tangrenjie.net/b.jpg"));
    }

    #[test]
    fn parses_detail_with_play_groups() {
        let html = r#"<div class="detail-cols fn-clear">
            <div class="detail-pic fn-left"><img src="https://img.tangrenjie.net/p.jpg" alt="海报"/></div>
            <div class="detail-info fn-left">
              <div class="detail-title fn-clear"><h1>吸血新一代</h1></div>
              <div class="info"><dl><dt>主演：</dt><dd>斯派斯,凯利</dd></dl>
                <dl><dt>导演：</dt><dd>弗兰克</dd></dl>
                <dl><dt>地区：</dt><dd>美国</dd></dl>
                <dl><dt>年份：</dt><dd>2026</dd></dl>
                <dl><dt>状态：</dt><dd>正片</dd></dl></div>
            </div></div>
            <div class="vod-intro"><p>剧情梗概一。</p><p>剧情梗概二。</p></div>
            <div class="box" id="playlist_1"><div class="box-title"><h2><i class="title-icon">▶️</i>主线路-在线播放</h2></div>
              <div class="video_list fn-clear"><a href="/play/SCCCCKyi-1-1.html">正片</a></div></div>
            <div class="box" id="playlist_2"><div class="box-title"><h2>其他线路-在线播放</h2></div>
              <div class="video_list fn-clear"><a href="/play/SCCCCKyi-2-1.html">正片</a></div></div>"#;
        let detail = parse_detail(html, "/vod/SCCCCKyi.html").expect("detail");
        assert_eq!(detail.name, "吸血新一代");
        assert_eq!(detail.cover.as_deref(), Some("https://img.tangrenjie.net/p.jpg"));
        assert_eq!(detail.actor.as_deref(), Some("斯派斯,凯利"));
        assert_eq!(detail.director.as_deref(), Some("弗兰克"));
        assert_eq!(detail.area.as_deref(), Some("美国"));
        assert_eq!(detail.year.as_deref(), Some("2026"));
        assert!(detail.desc.as_deref().unwrap_or("").contains("剧情梗概二"));
        assert_eq!(detail.episodes.len(), 2);
        assert_eq!(detail.episodes[0].group, "主线路");
        assert_eq!(detail.episodes[0].items[0].title, "正片");
        assert_eq!(detail.episodes[0].items[0].url, "/play/SCCCCKyi-1-1.html");
        assert_eq!(detail.episodes[1].group, "其他线路");
    }

    #[test]
    fn parses_search_rows() {
        let html = r#"<ul class="show-list">
            <li><a class="play-img" href="/vod/SCCCCKix.html"><img src="https://img.tangrenjie.net/s.jpg"/></a>
              <div class="play-txt"><h2><a href="/vod/SCCCCKix.html">吸血鬼嘉年华</a></h2>
                <dl><dt>主演：</dt><dd>内尔,基隆</dd></dl>
                <dl><dt>状态：</dt><dd><span class="color">正片</span></dd></dl></div></li>
        </ul>"#;
        let items = parse_search(html);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "吸血鬼嘉年华");
        assert_eq!(items[0].id, "/vod/SCCCCKix.html");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.tangrenjie.net/s.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("正片"));
    }

    #[test]
    fn parses_player_aaaa_url() {
        let html = r#"<script>var player_aaaa={"flag":"play","encrypt":0,"trysee":0,"url":"https:\/\/hn.bfvvs.com\/play\/bqxVWjDa\/index.m3u8","link":"\/play\/SCCCCKyi-1-1.html"};</script>"#;
        assert_eq!(
            parse_play_url(html).as_deref(),
            Some("https://hn.bfvvs.com/play/bqxVWjDa/index.m3u8")
        );
    }

    #[test]
    fn player_requires_url_and_plain_encrypt() {
        assert!(parse_play_url("<script>var player_aaaa={\"encrypt\":0,\"url\":\"\"};</script>").is_none());
        assert!(parse_play_url("<script>var player_aaaa={\"encrypt\":1,\"url\":\"abc\"};</script>").is_none());
        assert!(parse_play_url("<html></html>").is_none());
    }

    #[test]
    fn empty_markup_yields_no_items() {
        assert!(parse_categories("<html></html>").is_empty());
        assert!(parse_list("<html></html>").is_empty());
        assert!(parse_search("<html></html>").is_empty());
    }
}
