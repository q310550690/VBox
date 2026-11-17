//! `csp_Dm84` 原生实现（动漫巴士 `dm84.tv`，HTML 站点）。
//!
//! 语义来自静态反编译：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Dm84.java`
//! （约 300 行，去混淆后可直接读）。类 `extends Spider`，不依赖宿主私有 API，可完全重写：
//!
//! 1. `init(ctx, ext)`：`ext` 只用于「代理下载」开关；基址写死 `https://dm84.tv`。
//! 2. `homeContent`：`ul.nav_row > li > a` 中 `href` 以 `/list` 开头者即分类，
//!    分类 id 取 `href` 里 `list-` 与 `.html` 之间的数字；首页同时返回 `div.item` 最新列表。
//! 3. `categoryContent(typeId, page)`：`/show-{id}--{by}-{type}--{year}-{page}.html`。
//!    原实现用 `by=time`、`type`/`year` 由筛选参数（`extend`）传入；VBox 的 `Site` 模型
//!    没有筛选承载位，这里固定 `by=time` 且过滤为空，即 `/show-{id}------{page}.html`
//!    （这是站点当前真实可用的「全部」路由，实测 200 且 36 条/页）。
//! 4. `detailContent(id)`：`/v/{id}`（id 形如 `494.html`，取自列表 `a.title[href]` 的第 3 段）。
//!    标题 `h1.v_title`、备注 `p.v_desc > span.desc`，其余元数据来自 `og:*` meta：
//!    `og:image`(封面)、`og:description`(剧情)、`og:video:area`、`og:video:class`、
//!    `og:video:actor`、`og:video:director`、`og:video:release_date`(年份)。
//!    播放线路 = `ul.tab_control > li`（线路名）与 `ul.play_list`（分集列表）按序配对，
//!    每集 `文本$href`、`#` 分集、`$$$` 分线路。
//! 5. `searchContent(word)`：`/s----------.html?wd={word}`，同样解析 `div.item`。
//! 6. `playerContent(flag, id)`：`id` 即分集相对地址（`/p/494-1-565.html`），取页面
//!    `iframe[src]` 作最终播放地址，带 UA/Referer 头。
//!
//! 相对原实现的有意偏差（站点改版所致，见模块内注释）：
//! - 分类名取完整文本（原实现 `substring(0, 2)` 截断成两字）；
//! - 列表/筛选 URL 采用站点当前真实路由（原 `%s--%s-%s--%s-%s` 模板已 404）；
//! - 不实现 `getProxyDownloadUrl`（原实现把图片/播放地址改写成代理 URL，桌面端不需要，
//!   且 `ext` 为空的站点本来也不启用）。

use std::collections::HashMap;
use std::sync::OnceLock;

use scraper::{Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Dm84";

/// 站点写死基址（`Dm84.java` 常量 `b`）。
const DEFAULT_BASE: &str = "https://dm84.tv";

/// 安卓端 UA（`Chrome/122`，见 `Dm84.b()`）。
const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7";

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let trimmed = ext.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        DEFAULT_BASE.to_string()
    }
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Accept".to_string(), ACCEPT.to_string());
    h
}

/* ============================================================
 * 选择器
 * ============================================================ */

fn nav_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.nav_row > li > a").expect("导航选择器"))
}

fn item_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.item").expect("列表行选择器"))
}

fn title_link_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a.title").expect("标题链接选择器"))
}

fn cover_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a.cover").expect("封面选择器"))
}

fn desc_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("span.desc").expect("备注选择器"))
}

fn h1_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("h1.v_title").expect("标题选择器"))
}


fn tab_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.tab_control > li").expect("线路名选择器"))
}

fn playlist_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.play_list").expect("分集列表选择器"))
}

fn anchor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a").expect("锚点选择器"))
}

fn iframe_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("iframe").expect("iframe 选择器"))
}

fn og_image_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("meta[property=\"og:image\"]").expect("og:image"))
}

fn og_desc_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("meta[property=\"og:description\"]").expect("og:description"))
}

fn meta_area_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("meta[name=\"og:video:area\"]").expect("og:video:area"))
}

fn meta_actor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("meta[name=\"og:video:actor\"]").expect("og:video:actor"))
}

fn meta_director_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("meta[name=\"og:video:director\"]").expect("og:video:director"))
}

fn meta_release_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("meta[name=\"og:video:release_date\"]").expect("release_date"))
}

/* ============================================================
 * 工具
 * ============================================================ */

fn text_of(el: &scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn attr(el: &scraper::ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or("").trim().to_string()
}

fn meta_content(doc: &Html, sel: &Selector) -> Option<String> {
    doc.select(sel)
        .next()
        .map(|el| attr(&el, "content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/* ============================================================
 * 分类 / 列表 / 搜索
 * ============================================================ */

/// 首页分类（`ul.nav_row > li > a` 里指向 `/list-{id}.html` 的项）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for a in doc.select(nav_sel()) {
        let href = attr(&a, "href");
        let Some(rest) = href.strip_prefix("/list-") else {
            continue;
        };
        let id = rest.trim_end_matches(".html").trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let name = text_of(&a);
        if name.is_empty() || out.iter().any(|c: &Category| c.type_id == id) {
            continue;
        }
        out.push(Category {
            type_id: id.to_string(),
            type_name: name,
        });
    }
    out
}

/// 分类列表页 URL（站点当前真实路由：`/show-{id}------{page}.html`）。
fn list_url(base: &str, cat: &str, page: u32) -> String {
    let suffix = if page > 1 { page.to_string() } else { String::new() };
    format!("{base}/show-{cat}------{suffix}.html")
}

/// 搜索 URL（`/s----------.html?wd=`），关键词按 query 组件编码。
fn search_url(base: &str, keyword: &str) -> String {
    let encoded =
        percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC).to_string();
    format!("{base}/s----------.html?wd={encoded}")
}

/// 解析 `div.item` 列表行。
fn parse_rows(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(item_sel()) {
        let Some(link) = row.select(title_link_sel()).next() else {
            continue;
        };
        let href = attr(&link, "href");
        if href.is_empty() {
            continue;
        }
        let id = href.split('/').nth(2).unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }
        let cover = row
            .select(cover_sel())
            .next()
            .map(|a| attr(&a, "data-bg"))
            .filter(|s| !s.is_empty())
            .map(|s| html_rule::join_url(base, &s));
        let remark = row
            .select(desc_sel())
            .next()
            .map(|el| text_of(&el))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id,
            name: text_of(&link),
            cover,
            remark,
        });
    }
    out
}

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &format!("{base}/"), &headers(), cfg.http.retry)?;
    Ok(parse_categories(&html))
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let url = list_url(&base, cat, page);
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    Ok(parse_rows(&html, &base))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let url = search_url(&base, keyword);
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    Ok(parse_rows(&html, &base))
}

/* ============================================================
 * 详情
 * ============================================================ */

fn detail_url(base: &str, id: &str) -> String {
    if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.starts_with("/v/") {
        format!("{base}{id}")
    } else {
        format!("{base}/v/{id}")
    }
}

/// 解析详情页；`id` 为列表行里的相对 id（`494.html`）。
pub fn parse_detail(site: &Site, id: &str, base: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(h1_sel())
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();

    let mut groups: Vec<EpisodeGroup> = Vec::new();
    let tabs: Vec<String> = doc.select(tab_sel()).map(|el| text_of(&el)).collect();
    for (idx, list) in doc.select(playlist_sel()).enumerate() {
        let group = tabs
            .get(idx)
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("线路{}", idx + 1));
        let mut items = Vec::new();
        for a in list.select(anchor_sel()) {
            let href = attr(&a, "href");
            if href.is_empty() {
                continue;
            }
            let title = text_of(&a);
            if title.is_empty() {
                continue;
            }
            items.push(Episode { title, url: href });
        }
        if !items.is_empty() {
            groups.push(EpisodeGroup { group, items });
        }
    }

    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少标题与播放线路（站点改版或非 Dm84 模板？）",
        )
        .into_core());
    }

    let cover = meta_content(&doc, og_image_sel()).map(|s| html_rule::join_url(base, &s));
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc: meta_content(&doc, og_desc_sel()),
        year: meta_content(&doc, meta_release_sel()),
        area: meta_content(&doc, meta_area_sel()),
        director: meta_content(&doc, meta_director_sel()),
        actor: meta_content(&doc, meta_actor_sel()),
        episodes: groups,
    })
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let url = detail_url(&base, id);
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    parse_detail(site, id, &base, &html)
}

/* ============================================================
 * 播放
 * ============================================================ */

/// 从播放页 HTML 取 `iframe[src]`（可能为空）。
pub fn parse_player_src(html: &str) -> String {
    let doc = Html::parse_document(html);
    doc.select(iframe_sel())
        .next()
        .map(|el| attr(&el, "src"))
        .unwrap_or_default()
}

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        format!("{base}{id}")
    };
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    let src = parse_player_src(&html);
    if src.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "播放页没有 iframe[src]（站点改版或需要会员/登录）",
        )
        .into_core());
    }
    let mut header = HashMap::new();
    header.insert("User-Agent".to_string(), UA.to_string());
    header.insert("Referer".to_string(), format!("{base}/"));
    Ok(json!({
        "url": src,
        "parse": 0,
        "header": header,
        "playUrl": "",
    }))
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with_ext(ext: Option<&str>) -> Site {
        Site {
            key: "dm84".to_string(),
            name: "动漫巴士".to_string(),
            api: API.to_string(),
            ext: ext.map(|e| serde_json::json!(e)),
            ..Default::default()
        }
    }

    #[test]
    fn handles_api_case_insensitive() {
        assert!(handles("csp_Dm84"));
        assert!(handles(" CSP_DM84 "));
        assert!(!handles("csp_SixV"));
    }

    #[test]
    fn urls_follow_live_site_routes() {
        let base = DEFAULT_BASE;
        assert_eq!(list_url(base, "1", 1), "https://dm84.tv/show-1------.html");
        assert_eq!(list_url(base, "2", 3), "https://dm84.tv/show-2------3.html");
        assert_eq!(search_url(base, "海贼 王"), "https://dm84.tv/s----------.html?wd=%E6%B5%B7%E8%B4%BC%20%E7%8E%8B");
        assert_eq!(detail_url(base, "494.html"), "https://dm84.tv/v/494.html");
    }

    #[test]
    fn base_uses_ext_when_http() {
        assert_eq!(base_of(&site_with_ext(Some("https://mirror.test"))), "https://mirror.test");
        assert_eq!(base_of(&site_with_ext(Some("https://mirror.test/"))), "https://mirror.test");
        assert_eq!(base_of(&site_with_ext(None)), DEFAULT_BASE);
    }

    #[test]
    fn parses_nav_categories() {
        let html = r#"
        <ul class="nav_row">
          <li class="current"><a href="/" title="首页">首页</a></li>
          <li><a href="/list-1.html" title="国产">国漫</a></li>
          <li><a href="/list-2.html" title="日本">日漫</a></li>
          <li><a href="/list-3.html" title="欧美">欧美动漫</a></li>
          <li><a href="/list-4.html" title="电影">电影</a></li>
          <li><a href="/list-1.html">重复</a></li>
        </ul>"#;
        let cats = parse_categories(html);
        assert_eq!(cats.len(), 4, "实际: {cats:?}");
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "国漫");
        assert_eq!(cats[3].type_name, "电影");
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"
        <div class="item">
          <a href="/v/494.html" class="cover lazy" data-bg="http://img.test/a.jpg" title="逆天至尊">逆天至尊</a>
          <a class="title" href="/v/494.html" title="逆天至尊">逆天至尊</a>
          <span class="desc">第565话</span>
        </div>"#;
        let rows = parse_rows(html, "https://dm84.tv");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "494.html");
        assert_eq!(rows[0].name, "逆天至尊");
        assert_eq!(rows[0].remark.as_deref(), Some("第565话"));
        assert_eq!(rows[0].cover.as_deref(), Some("http://img.test/a.jpg"));
    }

    #[test]
    fn parses_detail_meta_and_playlists() {
        let html = r#"
        <h1 class="v_title"><a href="/v/494.html">逆天至尊</a></h1>
        <p class="v_desc"><span class="desc">第565话</span><em>|</em>2021<em>|</em>大陆</p>
        <meta property="og:image" content="/upload/494.jpg"/>
        <meta property="og:description" content="剧情简介"/>
        <meta name="og:video:area" content="大陆"/>
        <meta name="og:video:actor" content="阿旦,糖醋里脊"/>
        <meta name="og:video:director" content="高峰"/>
        <meta name="og:video:release_date" content="2021"/>
        <ul class="tab_control"><li class="current">线路1</li><li>线路2</li></ul>
        <ul class="play_list current">
          <li><a href="/p/494-1-565.html">565</a></li>
          <li><a href="/p/494-1-564.html">564</a></li>
        </ul>
        <ul class="play_list">
          <li><a href="/p/494-2-1.html">1</a></li>
        </ul>"#;
        let site = site_with_ext(None);
        let d = parse_detail(&site, "494.html", "https://dm84.tv", html).expect("详情");
        assert_eq!(d.name, "逆天至尊");
        assert_eq!(d.year.as_deref(), Some("2021"));
        assert_eq!(d.area.as_deref(), Some("大陆"));
        assert_eq!(d.actor.as_deref(), Some("阿旦,糖醋里脊"));
        assert_eq!(d.director.as_deref(), Some("高峰"));
        assert_eq!(d.desc.as_deref(), Some("剧情简介"));
        assert_eq!(
            d.cover.as_deref(),
            Some("https://dm84.tv/upload/494.jpg")
        );
        assert_eq!(d.episodes.len(), 2, "实际: {:?}", d.episodes);
        assert_eq!(d.episodes[0].group, "线路1");
        assert_eq!(d.episodes[0].items.len(), 2);
        assert_eq!(d.episodes[0].items[0].title, "565");
        assert_eq!(d.episodes[0].items[0].url, "/p/494-1-565.html");
        assert_eq!(d.episodes[1].group, "线路2");
    }

    #[test]
    fn extracts_player_iframe() {
        let html = r#"<html><body><iframe src="https://hhjx.hhplayer.com/?url=ABC" width="100%"></iframe></body></html>"#;
        assert_eq!(parse_player_src(html), "https://hhjx.hhplayer.com/?url=ABC");
        assert_eq!(parse_player_src("<html></html>"), "");
    }
}
