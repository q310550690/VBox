//! `csp_Meijumi` 原生实现（WordPress 影视站 + 磁力/ed2k 直链型）。
//!
//! 语义来自静态反编译：`_rea/decompiled/5fc50e17-5199d8f8bcbe3c6d6938ded33cb90596/deobf/com/github/catvod/spider/Meijumi.java`
//! （216 行、0 处 `Method dump skipped`、无字符串混淆）。类名 `MeijuMi`，`extends Spider`，
//! 不依赖宿主私有 API，因此可 100% 原生重写。
//!
//! 原实现协议：
//! 1. `a = "https://www.meijumi.net"`；`init(context, ext)` 时 ext 非空则整体覆盖基址。
//! 2. `homeContent`：GET 首页 → `ul.nav-menu a`，过滤文本含「更新/推荐/首页/韩剧/即将」的入口；
//!    `type_id` 直接用导航的绝对 `href`，`type_name` 用链接文本。
//! 3. `categoryContent`：第 1 页 URL 就是 `type_id`；第 2 页起 `{type_id}page/{page}/`
//!    （`type_id` 以 `/` 结尾）。行 `div#post_list_box article`：标题取首个 `a` 的 `title`
//!    （去「迅雷下载」）、封面 `img[src]`（经 `b()` 追加 `@Headers={UA}`）、备注 `.gxts`、
//!    id 取首个 `a` 的 `href`。
//! 4. `detailContent`：id 即详情页 URL。封面 `div.single-content img` 首图；
//!    标题 `.entry-title` 去「迅雷下载」；链接 `div.single-content a[href^=magnet],a[href^=ed2k]`，
//!    名称先取锚文本，再按协议覆盖——ed2k 用 URL 解码后的 `|file|(.*?)\|`，
//!    magnet 用 URL 解码后的 `(^|&)dn=([^&]*)(&|$)`；`名称$链接` 以 `#` 连接，
//!    `vod_play_from="美剧迷"`。
//! 5. `playerContent`：`url = id`；id 以 `tvbox`/`ed2k` 开头 → `parse: 0`，
//!    否则 `header={User-Agent}` + `parse: 1`（原实现对 magnet 也走解析器，保持等价）。
//! 6. `searchContent`：GET `{base}/?s={URLEncoder.encode(keyword)}`，
//!    行 `ul.search-page article`，字段与列表页相同。
//!
//! 与 Android 原实现的差异（均有意，注释内标注）：
//! - `ext` 首段为 http(s) 时作为基址覆盖（原实现是整串覆盖，VBox 侧 ext 可能带 `$$$` 附加段）；
//! - 相对 `href`/`src` 统一用 `html_rule::join_url` 绝对化（原实现依赖站点输出绝对地址）；
//! - 搜索关键词用 `NON_ALPHANUMERIC` 百分号编码（Java `URLEncoder` 对空格用 `+`，等价可被服务端接受）；
//! - 原实现的 `danmaku`（`Proxy.getUrl()?do=diydanmu…`）属宿主面板路由，桌面端不实现。

use std::collections::HashMap;
use std::sync::OnceLock;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Meijumi";

/// 站点默认基址（原实现 `MeijuMi.a`）。
const DEFAULT_BASE: &str = "https://www.meijumi.net";

/// 安卓端 UA（`Chrome/94.0.4606.54`）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36";

/// 播放线路名（原实现 `vod_play_from="美剧迷"`）。
const PLAY_FROM: &str = "美剧迷";

/// 首页导航里需要跳过的入口（原实现按文本包含判断）。
const SKIP_NAV_KEYWORDS: [&str; 5] = ["更新", "推荐", "首页", "韩剧", "即将"];

/// 是否处理该 api（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 选择器与正则
 * ============================================================ */

fn nav_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.nav-menu a[href]").expect("导航选择器"))
}

fn list_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div#post_list_box article").expect("列表行选择器"))
}

fn search_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.search-page article").expect("搜索行选择器"))
}

fn anchor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a").expect("a 选择器"))
}

fn img_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("img").expect("img 选择器"))
}

fn gxts_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse(".gxts").expect(".gxts 选择器"))
}

fn content_img_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.single-content img").expect("正文封面选择器"))
}

fn content_anchor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.single-content a[href]").expect("正文链接选择器"))
}

fn title_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse(".entry-title").expect("标题选择器"))
}

fn ed2k_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\|file\|(.*?)\|").expect("ed2k 文件名正则"))
}

fn magnet_dn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|&)dn=([^&]*)(?:&|$)").expect("magnet dn 正则"))
}

/* ============================================================
 * 对外入口
 * ============================================================ */

/// 分类来自首页 `ul.nav-menu a`（原实现每次 `homeContent` 现抓）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site)?;
    let html = fetch_text(cfg, &base, &format!("{base}/"))?;
    let cats = parse_categories(&html, &base);
    if cats.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "首页导航解析为空（站点改版或非美剧迷模板？）",
        )
        .into_core());
    }
    Ok(cats)
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = list_url(&base, cat, page);
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_list(&html, &base))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site)?;
    let url = detail_url(&base, id);
    let html = fetch_text(cfg, &base, &url)?;
    parse_detail(site, id, &html, &base)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let encoded = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/?s={encoded}");
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_search(&html, &base))
}

/// 原实现：id 以 `tvbox`/`ed2k` 开头走直链，其余（含 magnet）交给解析器并带 UA 头。
pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "csp_Meijumi 播放地址为空").into_core());
    }
    if id.starts_with("tvbox") || id.starts_with("ed2k") {
        return Ok(json!({
            "parse": 0,
            "jx": 0,
            "playUrl": "",
            "url": id,
            "header": {},
        }));
    }
    let mut header = serde_json::Map::new();
    header.insert("User-Agent".to_string(), json!(UA));
    Ok(json!({
        "parse": 1,
        "jx": 1,
        "playUrl": "",
        "url": id,
        "header": Value::Object(header),
    }))
}

/* ============================================================
 * URL 与请求
 * ============================================================ */

/// `ext` 首段为 http(s) 时覆盖基址（mock/镜像用），否则用默认基址。
pub fn base_of(site: &Site) -> Result<String> {
    let raw = site.ext_string();
    if raw.trim().is_empty() {
        return Ok(DEFAULT_BASE.to_string());
    }
    let first = raw.split("$$$").next().unwrap_or("").trim();
    if first.is_empty() {
        return Ok(DEFAULT_BASE.to_string());
    }
    if !(first.starts_with("http://") || first.starts_with("https://")) {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Meijumi 的 ext 首段不是 http(s) 基址：{first}"),
        )
        .into_core());
    }
    Ok(first.trim_end_matches('/').to_string())
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Referer".to_string(), format!("{base}/"));
    headers
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

/// 第 1 页为 `type_id` 本身；第 2 页起为 `{type_id}/page/{page}/`。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let raw = cat.trim();
    let url = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        html_rule::join_url(base, raw)
    };
    if page <= 1 {
        url
    } else {
        format!("{}/page/{page}/", url.trim_end_matches('/'))
    }
}

fn detail_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        html_rule::join_url(base, id)
    }
}

/* ============================================================
 * 解析
 * ============================================================ */

pub fn parse_categories(html: &str, base: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out: Vec<Category> = Vec::new();
    for a in doc.select(nav_sel()) {
        let name = clean_text(&text_of(&a));
        if name.is_empty() {
            continue;
        }
        if SKIP_NAV_KEYWORDS.iter().any(|kw| name.contains(kw)) {
            continue;
        }
        let href = attr(&a, "href");
        if href.is_empty() {
            continue;
        }
        if href.starts_with("javascript:") || href.starts_with('#') || href.starts_with("mailto:") {
            continue;
        }
        let url = absolutize(&href, base);
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        out.push(Category {
            type_id: url,
            type_name: name,
        });
    }
    out
}

pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(list_row_sel()) {
        if let Some(item) = parse_row(&row, base) {
            out.push(item);
        }
    }
    out
}

pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(search_row_sel()) {
        if let Some(item) = parse_row(&row, base) {
            out.push(item);
        }
    }
    out
}

fn parse_row(row: &scraper::ElementRef<'_>, base: &str) -> Option<VideoItem> {
    let link = row.select(anchor_sel()).next()?;
    let href = attr(&link, "href");
    if href.is_empty() {
        return None;
    }
    let title = clean_title(&attr(&link, "title"));
    let name = if title.is_empty() {
        clean_title(&text_of(&link))
    } else {
        title
    };
    if name.is_empty() {
        return None;
    }
    let cover = row
        .select(img_sel())
        .next()
        .map(|img| poster(&absolutize(&attr(&img, "src"), base)))
        .filter(|src| !src.is_empty());
    let remark = row
        .select(gxts_sel())
        .next()
        .map(|el| clean_text(&text_of(&el)))
        .filter(|text| !text.is_empty());
    Some(VideoItem {
        id: absolutize(&href, base),
        name,
        cover,
        remark,
    })
}

pub fn parse_detail(site: &Site, id: &str, html: &str, base: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);

    let name = doc
        .select(title_sel())
        .next()
        .map(|el| clean_title(&text_of(&el)))
        .unwrap_or_default();
    if name.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少 .entry-title（站点改版或非美剧迷模板？）",
        )
        .into_core());
    }

    let cover = doc
        .select(content_img_sel())
        .next()
        .map(|img| poster(&absolutize(&attr(&img, "src"), base)))
        .filter(|src| !src.is_empty());

    let mut items: Vec<Episode> = Vec::new();
    for a in doc.select(content_anchor_sel()) {
        let href = attr(&a, "href");
        if !(href.starts_with("magnet:") || href.starts_with("ed2k:")) {
            continue;
        }
        let display = magnet_or_ed2k_name(&href).unwrap_or_else(|| clean_text(&text_of(&a)));
        let display = if display.is_empty() {
            href.clone()
        } else {
            display
        };
        items.push(Episode {
            title: display,
            url: href,
        });
    }

    if items.is_empty() {
        // 无磁力/ed2k 的详情页在站点上确实存在（纯文章页），此时给出空线路而不是报错。
        return Ok(VideoDetail {
            id: id.to_string(),
            name,
            cover,
            desc: None,
            year: None,
            area: None,
            director: None,
            actor: None,
            episodes: Vec::new(),
        });
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items,
        }],
    })
}

/// 从 ed2k/magnet 链接里取「文件名」作为剧集标题（原实现 `detailContent`）。
pub fn magnet_or_ed2k_name(href: &str) -> Option<String> {
    let decoded = url_decode_plus(href);
    if href.starts_with("ed2k:") {
        return ed2k_name_re()
            .captures(&decoded)
            .and_then(|caps| caps.get(1))
            .map(|m| clean_text(m.as_str()))
            .filter(|s| !s.is_empty());
    }
    if href.starts_with("magnet:") {
        return magnet_dn_re()
            .captures(&decoded)
            .and_then(|caps| caps.get(1))
            .map(|m| clean_text(m.as_str()))
            .filter(|s| !s.is_empty());
    }
    None
}

/* ============================================================
 * 小工具
 * ============================================================ */

/// 原实现 `b(String)`：封面地址追加 `@Headers={User-Agent: ...}`，供宿主图片代理带 UA。
pub fn poster(src: &str) -> String {
    let src = src.trim();
    if src.is_empty() {
        return String::new();
    }
    if src.contains("@Headers=") {
        return src.to_string();
    }
    format!("{src}@Headers={{\"User-Agent\":\"{UA}\"}}")
}

fn url_decode_plus(raw: &str) -> String {
    let replaced = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&replaced)
        .decode_utf8_lossy()
        .to_string()
}

fn attr(el: &scraper::ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or("").trim().to_string()
}

fn text_of(el: &scraper::ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join("")
}

fn absolutize(src: &str, base: &str) -> String {
    let src = src.trim();
    if src.is_empty() {
        return String::new();
    }
    if let Some(rest) = src.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return src.to_string();
    }
    html_rule::join_url(base, src)
}

/// 标题清洗：去「迅雷下载」后压缩空白（原实现 `.replace("迅雷下载","").trim()`）。
fn clean_title(raw: &str) -> String {
    clean_text(&raw.replace("迅雷下载", ""))
}

fn clean_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim().to_string()
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<&str>) -> Site {
        let mut s = Site::default();
        s.key = "meijumi".to_string();
        s.name = "美剧迷".to_string();
        s.api = API.to_string();
        if let Some(ext) = ext {
            s.ext = Some(json!(ext));
        }
        s
    }

    fn cfg() -> TvConfig {
        TvConfig::default()
    }

    const NAV_HTML: &str = r#"
        <html><body>
        <ul class="nav-menu">
          <li><a href="/">首页</a></li>
          <li><a href="https://www.meijumi.net/usa/">美剧</a></li>
          <li><a href="https://www.meijumi.net/usa/xuanyi/">灵异/惊悚</a></li>
          <li><a href="https://www.meijumi.net/hanju/">韩剧</a></li>
          <li><a href="https://www.meijumi.net/gengxin/">美剧更新时间表</a></li>
          <li><a href="https://www.meijumi.net/zixun/haoju/">好剧推荐</a></li>
          <li><a href="javascript:void(0);">收藏本站</a></li>
        </ul>
        </body></html>"#;

    const LIST_HTML: &str = r#"
        <html><body><div id="post_list_box">
          <article>
            <a href="https://www.meijumi.net/45785.html" title="《幻视任务第一季》VisionQuest 网盘/迅雷下载">
              <img src="https://tututu.wujinyuzhou.top/2026/10/huanshizhengtu_compressed.jpg" />
              <span class="gxts">更新至：全8集</span>
            </a>
          </article>
          <article>
            <a href="/12345.html"><img src="/wp/1.jpg" /><span class="gxts">完结</span></a>
          </article>
        </div></body></html>"#;

    const DETAIL_HTML: &str = r#"
        <html><body>
        <h1 class="entry-title">《幻视任务第一季》VisionQuest 网盘/迅雷下载</h1>
        <div class="single-content">
          <p><img src="https://tututu.wujinyuzhou.top/2026/10/huanshizhengtu_compressed.jpg" /></p>
          <p><a href="magnet:?xt=urn:btih:d7292586b0fef8a3d514c478fac2694c58eae90d&amp;dn=VisionQuest%20S01E01%2EPilot">E01</a></p>
          <p><a href="ed2k://|file|VisionQuest.S01E02.1080p.mkv|1234|HASH|/">E02</a></p>
        </div>
        </body></html>"#;

    const SEARCH_HTML: &str = r#"
        <html><body><ul class="search-page">
          <article><a href="https://www.meijumi.net/45785.html" title="《幻视任务第一季》迅雷下载">
            <img src="/wp/a.jpg" /><span class="gxts">全8集</span></a></article>
        </ul></body></html>"#;

    #[test]
    fn handles_api_case_insensitively() {
        assert!(handles("csp_Meijumi"));
        assert!(handles("  CSP_MEIJUMI "));
        assert!(!handles("csp_Xb6v"));
        assert!(!handles("csp_Douban"));
    }

    #[test]
    fn base_override_and_default() {
        assert_eq!(base_of(&site(None)).unwrap(), DEFAULT_BASE);
        assert_eq!(base_of(&site(Some("http://127.0.0.1:9/$$$extra"))).unwrap(), "http://127.0.0.1:9");
        assert!(base_of(&site(Some("not-a-url"))).is_err());
    }

    #[test]
    fn categories_skip_non_category_entries() {
        let cats = parse_categories(NAV_HTML, DEFAULT_BASE);
        let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
        assert_eq!(names, vec!["美剧", "灵异/惊悚"]);
        assert_eq!(cats[0].type_id, "https://www.meijumi.net/usa/");
        assert_eq!(cats[1].type_id, "https://www.meijumi.net/usa/xuanyi/");
    }

    #[test]
    fn list_url_builds_pages() {
        assert_eq!(list_url(DEFAULT_BASE, "https://www.meijumi.net/usa/", 1), "https://www.meijumi.net/usa/");
        assert_eq!(list_url(DEFAULT_BASE, "https://www.meijumi.net/usa", 1), "https://www.meijumi.net/usa");
        assert_eq!(
            list_url(DEFAULT_BASE, "https://www.meijumi.net/usa/", 2),
            "https://www.meijumi.net/usa/page/2/"
        );
        assert_eq!(list_url(DEFAULT_BASE, "/usa/", 3), "https://www.meijumi.net/usa/page/3/");
    }

    #[test]
    fn list_rows_clean_title_and_cover() {
        let rows = parse_list(LIST_HTML, DEFAULT_BASE);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "《幻视任务第一季》VisionQuest 网盘/");
        assert_eq!(rows[0].id, "https://www.meijumi.net/45785.html");
        assert_eq!(rows[0].remark.as_deref(), Some("更新至：全8集"));
        assert!(rows[0].cover.as_deref().unwrap().starts_with("https://tututu.wujinyuzhou.top/2026/10/huanshizhengtu_compressed.jpg@Headers={\"User-Agent\":\"Mozilla/5.0"));
        assert_eq!(rows[1].id, "https://www.meijumi.net/12345.html");
        assert_eq!(rows[1].cover.as_deref(), Some("https://www.meijumi.net/wp/1.jpg@Headers={\"User-Agent\":\"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36\"}"));
    }

    #[test]
    fn detail_extracts_magnet_and_ed2k_names() {
        let det = parse_detail(&site(None), "https://www.meijumi.net/45785.html", DETAIL_HTML, DEFAULT_BASE).unwrap();
        assert_eq!(det.name, "《幻视任务第一季》VisionQuest 网盘/");
        assert!(det.cover.as_deref().unwrap().contains("@Headers="));
        assert_eq!(det.episodes.len(), 1);
        assert_eq!(det.episodes[0].group, PLAY_FROM);
        let items = &det.episodes[0].items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "VisionQuest S01E01.Pilot");
        assert!(items[0].url.starts_with("magnet:?xt=urn:btih:d7292586"));
        assert_eq!(items[1].title, "VisionQuest.S01E02.1080p.mkv");
        assert!(items[1].url.starts_with("ed2k://|file|"));
    }

    #[test]
    fn search_rows_parse() {
        let rows = parse_search(SEARCH_HTML, DEFAULT_BASE);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "《幻视任务第一季》");
        assert_eq!(rows[0].remark.as_deref(), Some("全8集"));
    }

    #[test]
    fn player_splits_direct_and_parsed() {
        let s = site(None);
        let direct = player_content(&cfg(), &s, "", "ed2k://|file|a.mkv|1|x|/").unwrap();
        assert_eq!(direct["parse"], 0);
        assert_eq!(direct["url"], "ed2k://|file|a.mkv|1|x|/");
        let parsed = player_content(&cfg(), &s, "", "magnet:?xt=urn:btih:abc&dn=x").unwrap();
        assert_eq!(parsed["parse"], 1);
        assert_eq!(parsed["header"]["User-Agent"], UA);
        assert!(player_content(&cfg(), &s, "", "  ").is_err());
    }

    #[test]
    fn detail_without_links_is_empty_but_ok() {
        let html = r#"<html><body><h1 class="entry-title">普通文章</h1><div class="single-content"><p>无资源</p></div></body></html>"#;
        let det = parse_detail(&site(None), "https://www.meijumi.net/a.html", html, DEFAULT_BASE).unwrap();
        assert_eq!(det.name, "普通文章");
        assert!(det.episodes.is_empty());
    }
}
