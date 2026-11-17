//! `csp_SeedHub`（🧲 SeedHub 磁力站）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.SeedHub`，站点 1 个（4k(xhztv)）。
//! * 证据：`_rea/decompiled/5fc50e17-5199d8f8bcbe3c6d6938ded33cb90596/deobf/
//!   com/github/catvod/spider/SeedHub.java`（320 行、0 个 `Method dump skipped`，
//!   字符串全为明文，`playerContent` 完整可读）。
//!   另有 dffec63f/fc213119 等新版变体（662/355 行）字符串经 `short[]`/XOR 混淆，
//!   且 `playerContent` 1781 条指令被 jadx 跳过；本实现取「完整可读」的旧版语义，
//!   偏差记入文档与下文。
//! * 画像：`profiles.json` 中 `csp_SeedHub` 原为 `unported` / `anti_bot`。
//!
//! 协议（原类逐段还原）：
//! * 基址 `https://www.seedhub.cc`；`init` 时若 ext 非空则**整体替换**基址
//!   （原类 `a = str`，不做任何归一化，这里保持一致；ext 为空用默认值）。
//! * 请求头：`User-Agent: Chrome/94`、`Referer: {base}/`（原类 `c()`）。
//! * `homeContent`：硬编码 3 分类 电影/1、电视剧/3、动漫/2，`type_flag=2`，
//!   `filters={}`；不发请求。
//! * `categoryContent`：
//!   - 分类 id **不含** `movies` 时：`{base}/categories/{cateId}/movies/`，
//!     第 2 页起追加 `?page={page}`；随后依次替换筛选占位符 `{key}` 与
//!     `{cateId}`；行 `div.content div.cover`：`a[title]` 标题、`a[href]` id、
//!     `img[src]` 封面（补全后附 `@Headers={"User-Agent":…}`）、
//!     打分 `li` 中含「评分: 」的 `a` 文本作为备注。
//!   - 分类 id **含** `movies` 时（详情页被当分类）：仅第 1 页；抓 `{base}{id}`，
//!     封面 `div.content>div.cover>img` 的 `src`/`alt`，种子 `ul.seeds > li`
//!     （**整组倒序**）逐条 `a[title]`/`a[href]`，id = `alt$$$封面$$$href`。
//! * `detailContent`：id `名称$$$封面$$$详情页路径`；抓详情页取
//!   `div.content>p.link-start>a[href]`：磁力/ed2k → 线路 `magnet`；
//!   其它 → 线路 `SeedHub` 且播放地址取详情页 URL。封面附 `@Headers=`。
//! * `searchContent`：`https://cn.bing.com/search?q=site:www.seedhub.cc/movies/{kw}`
//!   （原类**不做 URL 编码**，保留）；命中 `ol#b_results li` 的 `a[href]`，
//!   逐条抓取目标页，按 `div.content>div.cover>img` 取 `alt`/封面，
//!   `ul.seeds > li` 倒序取种子；id = `alt$$$封面$$$href`，
//!   名称 = `alt 标题`（空格连接），备注固定 `SeedHub`。
//! * `playerContent`：`url` 原样返回；以 `tvbox`/`ed2k` 开头 → `parse:0`，
//!   否则带移动端 UA 头且 `parse:1`；`playUrl` 恒空串（原类行为）。
//!
//! 有意偏差（桌面端能力边界 / 原类自身行为）：
//! 1. 上游 `.cc` 已挂 Cloudflare「Just a moment…」（403），`.info` 返回约 1.6KB
//!    的 JS 挑战页（200），无头环境无法通过 → 本家族只做离线 fixture + mock 验收，
//!    不提供 `#[ignore]` 真站用例（审计矩阵记 mock-only）。
//! 2. 搜索原类用 4 线程池并发抓取每个 Bing 命中页，这里顺序抓取（语义相同、
//!    避免引入线程池；结果顺序与并发版一致）。
//! 3. 搜索的 `site:` 域名写死 `www.seedhub.cc`（原类如此，ext 覆盖基址后也不变）。
//! 4. `Collections.reverse` 的「整组倒序」按原类保留（先倒序 list 再正序输出）。
//! 5. 原类 `categoryContent` 输出 `pagecount/total = Integer.MAX_VALUE`、
//!    `limit = list.len`；桌面模型只消费列表，故不下发这些字段。

use std::collections::HashMap;
use std::sync::OnceLock;

use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_SeedHub";

/// 原类默认基址（`public static String a`）。
const DEFAULT_BASE: &str = "https://www.seedhub.cc";

/// 原类 `c()` / `b()` 的桌面 UA（Chrome/94）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36";

/// 原类 `playerContent` 下发的移动端 UA。
const UA_MOBILE: &str = "Mozilla/5.0 (Linux; Android 13; Xiaomi 13 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/113.0.5672.131 Mobile Safari/537.36";

/// 原类搜索用的 Bing 基址。
const BING: &str = "https://cn.bing.com";

/// 搜索 `site:` 前缀（原类写死）。
const SEARCH_SITE: &str = "site:www.seedhub.cc/movies/";

/// 首页硬编码分类（`电影&电视剧&动漫` / `1&3&2`）。
const HOME_CATEGORIES: [(&str, &str); 3] = [("电影", "1"), ("电视剧", "3"), ("动漫", "2")];

/* ============================================================
 * 选择器（OnceLock 缓存）
 * ============================================================ */

fn sel(cache: &'static OnceLock<Selector>, css: &str) -> &'static Selector {
    cache.get_or_init(|| Selector::parse(css).expect("静态选择器必须合法"))
}

fn folder_row_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.content div.cover")
}

fn folder_link_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "a")
}

fn folder_img_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "img")
}

fn li_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "li")
}

fn detail_cover_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.content > div.cover > img")
}

fn seeds_list_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "ul.seeds > li")
}

fn seed_link_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "a")
}

fn detail_link_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.content > p.link-start > a")
}

fn bing_row_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "ol#b_results li")
}

/* ============================================================
 * 基础工具
 * ============================================================ */

pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn attr_of(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(|s| s.trim().to_string())
}

/// 基址：ext 非空时整体替换（原类 `init` 语义：`a = str`，原样使用）。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let ext = ext.trim();
    if ext.is_empty() {
        DEFAULT_BASE.to_string()
    } else {
        ext.to_string()
    }
}

/// 请求头：Chrome/94 UA + `Referer: {base}/`。
fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), format!("{}/", base.trim_end_matches('/')));
    h
}

/// 原类 `b(String)`：封面 URL 追加 `@Headers={"User-Agent":…}`。
pub fn cover_with_headers(url: &str) -> String {
    let ua = serde_json::to_string(UA).unwrap_or_else(|_| "\"\"".to_string());
    format!("{url}@Headers={{\"User-Agent\":{ua}}}")
}

/// 链接补全（对应 `l.f(base, href)`）。
pub fn join_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    let base = base.trim_end_matches('/');
    if href.is_empty() {
        base.to_string()
    } else if href.starts_with('/') {
        format!("{base}{href}")
    } else {
        format!("{base}/{href}")
    }
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn fetch(cfg: &TvConfig, url: &str, base: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(base), 2)
}

/// 替换筛选占位符（原类只替换值非空的项）。
fn apply_filters(mut url: String, filter: &HashMap<String, String>) -> String {
    for (k, v) in filter {
        if v.is_empty() {
            continue;
        }
        url = url.replace(&format!("{{{k}}}"), v);
    }
    url
}

/* ============================================================
 * 解析（纯函数，便于单测）
 * ============================================================ */

/// 首页分类（原类硬编码，不发请求）。
pub fn parse_categories() -> Vec<Category> {
    HOME_CATEGORIES
        .iter()
        .map(|(name, id)| Category {
            type_id: id.to_string(),
            type_name: name.to_string(),
        })
        .collect()
}

/// 分类页 URL（原类两段拼接逻辑）。
pub fn category_url(base: &str, type_id: &str, page: u32, filter: &HashMap<String, String>) -> String {
    let base = base.trim_end_matches('/');
    let mut url = if page == 1 {
        format!("{base}/categories/{{cateId}}/movies/")
    } else {
        format!("{base}/categories/{{cateId}}/movies/?page={page}")
    };
    url = apply_filters(url, filter);
    url.replace("{cateId}", type_id)
}

/// 列表页行解析：`div.content div.cover`。
pub fn parse_folder_list(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(folder_row_sel()) {
        let Some(link) = row.select(folder_link_sel()).next() else { continue };
        let name = attr_of(&link, "title").unwrap_or_default();
        let Some(href) = attr_of(&link, "href") else { continue };
        if href.is_empty() {
            continue;
        }
        let cover = row
            .select(folder_img_sel())
            .next()
            .and_then(|img| attr_of(&img, "src"))
            .map(|src| cover_with_headers(&join_url(base, &src)));
        let remark = row
            .select(li_sel())
            .find(|li| text_of(li).contains("评分:"))
            .and_then(|li| li.select(folder_link_sel()).next().map(|a| text_of(&a)));
        out.push(VideoItem {
            id: href,
            name,
            cover,
            remark,
        });
    }
    out
}

/// 详情页封面（`div.content > div.cover > img` 的 alt 与 src）。
pub fn parse_movie_cover(html: &str) -> Option<(String, String)> {
    let doc = Html::parse_document(html);
    let img = doc.select(detail_cover_sel()).next()?;
    Some((attr_of(&img, "alt").unwrap_or_default(), attr_of(&img, "src").unwrap_or_default()))
}

/// 种子列表 `ul.seeds > li`（原类整组倒序后逐条输出）。
///
/// * `name_prefix`：`Some(alt)` 时名称为 `"{alt} {title}"`（搜索路径），
///   `None` 时名称直接取 `a[title]`（分类路径）。
pub fn parse_seed_items(
    html: &str,
    cover: &str,
    id_alt: &str,
    name_prefix: Option<&str>,
) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut rows: Vec<VideoItem> = Vec::new();
    for li in doc.select(seeds_list_sel()) {
        let Some(a) = li.select(seed_link_sel()).next() else { continue };
        let title = attr_of(&a, "title").unwrap_or_default();
        let Some(href) = attr_of(&a, "href") else { continue };
        if href.is_empty() {
            continue;
        }
        let name = match name_prefix {
            Some(prefix) => format!("{prefix} {title}"),
            None => title,
        };
        rows.push(VideoItem {
            id: format!("{id_alt}$$${cover}$$${href}"),
            name,
            cover: None,
            remark: None,
        });
    }
    rows.reverse();
    rows
}

/// 详情页外链 `div.content > p.link-start > a[href]`。
pub fn parse_detail_link(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    doc.select(detail_link_sel()).next().and_then(|a| attr_of(&a, "href"))
}

/// Bing 命中页的链接（`ol#b_results li` 下第一个 `a[href]`）。
pub fn parse_bing_hits(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(bing_row_sel()) {
        if let Some(a) = li.select(seed_link_sel()).next() {
            if let Some(href) = attr_of(&a, "href") {
                if href.starts_with("http://") || href.starts_with("https://") {
                    out.push(href);
                }
            }
        }
    }
    out
}

/// 搜索命中的目标页 → `(alt, 封面绝对地址, 种子行)`。
pub fn parse_search_page(html: &str, base: &str) -> Option<(String, String, Vec<VideoItem>)> {
    let (alt, src) = parse_movie_cover(html)?;
    let cover = join_url(base, &src);
    let rows = parse_seed_items(html, &cover, &alt, Some(&alt));
    Some((alt, cover, rows))
}

/// 播放计划（原类 `playerContent`：仅看 url 前缀）。
pub fn player_json(url: &str) -> Value {
    let mut out = json!({ "url": url, "playUrl": "" });
    if url.starts_with("tvbox") || url.starts_with("ed2k") {
        out["parse"] = json!(0);
    } else {
        let header = json!({ "User-Agent": UA_MOBILE }).to_string();
        out["header"] = json!(header);
        out["parse"] = json!(1);
    }
    out
}

/* ============================================================
 * 入口
 * ============================================================ */

pub fn categories(_cfg: &TvConfig, _site: &Site) -> Vec<Category> {
    parse_categories()
}

/// 分类：id 含 `movies` 走「详情当分类」，否则走文件夹列表。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
    filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    if type_id.contains("movies") {
        if page != 1 {
            return Ok(Vec::new());
        }
        let url = join_url(&base, type_id);
        let html = fetch(cfg, &url, &base)?;
        let (alt, cover) = parse_movie_cover(&html).unwrap_or_default();
        let cover = join_url(&base, &cover);
        return Ok(parse_seed_items(&html, &cover, &alt, None));
    }
    let url = category_url(&base, type_id, page, filter);
    let html = fetch(cfg, &url, &base)?;
    Ok(parse_folder_list(&html, &base))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let parts: Vec<&str> = id.trim().split("$$$").collect();
    let (Some(name), Some(pic), Some(href)) = (parts.first(), parts.get(1), parts.get(2)) else {
        return Err(parse_err(site, format!("详情 id 需为「名称$$$封面$$$路径」，实际：{id}")));
    };
    let url = join_url(&base, href);
    let html = fetch(cfg, &url, &base)?;
    let link = parse_detail_link(&html)
        .ok_or_else(|| parse_err(site, format!("详情页未找到 p.link-start > a：{url}")))?;
    let is_magnet = link.starts_with("magnet:") || link.starts_with("ed2k:");
    let (group, play_url) = if is_magnet {
        ("magnet".to_string(), link.clone())
    } else {
        ("SeedHub".to_string(), url.clone())
    };
    Ok(VideoDetail {
        id: id.to_string(),
        name: (*name).to_string(),
        cover: Some(cover_with_headers(pic)),
        episodes: vec![EpisodeGroup {
            group,
            items: vec![Episode {
                title: "立即播放".to_string(),
                url: play_url,
            }],
        }],
        ..Default::default()
    })
}

/// 搜索：Bing `site:` 命中 → 逐条抓目标页 → 种子行。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = format!("{BING}/search?q={SEARCH_SITE}{keyword}");
    let html = fetch(cfg, &url, BING)?;
    let mut out = Vec::new();
    for hit in parse_bing_hits(&html) {
        let Ok(page) = fetch(cfg, &hit, &base) else { continue };
        let Some((alt, cover, rows)) = parse_search_page(&page, &base) else { continue };
        for mut row in rows {
            row.remark = Some("SeedHub".to_string());
            if !alt.is_empty() && row.name.is_empty() {
                row.name = alt.clone();
            }
            let _ = &cover;
            out.push(row);
        }
    }
    Ok(out)
}

pub fn player_content(_cfg: &TvConfig, _site: &Site, _flag: &str, id: &str) -> Result<Value> {
    Ok(player_json(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;

    fn site_with_ext(ext: Option<&str>) -> Site {
        Site {
            key: "s".into(),
            name: "SeedHub".into(),
            api: API.into(),
            ext: ext.map(|s| Json::String(s.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_SeedHub"));
        assert!(handles("CSP_SEEDHUB"));
        assert!(!handles("csp_Seedhub2"));
    }

    #[test]
    fn base_uses_default_or_ext_verbatim() {
        assert_eq!(base_of(&site_with_ext(None)), DEFAULT_BASE);
        assert_eq!(base_of(&site_with_ext(Some("https://mirror.example/base"))), "https://mirror.example/base");
        assert_eq!(base_of(&site_with_ext(Some("   "))), DEFAULT_BASE);
    }

    #[test]
    fn headers_carry_referer() {
        let h = headers("https://www.seedhub.cc");
        assert_eq!(h.get("User-Agent").map(String::as_str), Some(UA));
        assert_eq!(h.get("Referer").map(String::as_str), Some("https://www.seedhub.cc/"));
    }

    #[test]
    fn home_categories_are_hardcoded() {
        let cats = parse_categories();
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "3");
        assert_eq!(cats[2].type_id, "2");
    }

    #[test]
    fn cover_suffix_matches_original() {
        let s = cover_with_headers("https://img/x.jpg");
        assert!(s.starts_with("https://img/x.jpg@Headers={\"User-Agent\":\""));
        assert!(s.ends_with("\"}"));
    }

    #[test]
    fn join_url_shapes() {
        assert_eq!(join_url("https://a.com", "/x/y"), "https://a.com/x/y");
        assert_eq!(join_url("https://a.com/", "x"), "https://a.com/x");
        assert_eq!(join_url("https://a.com", "https://b.com/x"), "https://b.com/x");
        assert_eq!(join_url("https://a.com", "//cdn/x"), "https://cdn/x");
    }

    #[test]
    fn category_url_first_and_later_pages() {
        let empty = HashMap::new();
        assert_eq!(
            category_url("https://s.cc", "1", 1, &empty),
            "https://s.cc/categories/1/movies/"
        );
        assert_eq!(
            category_url("https://s.cc/", "1", 2, &empty),
            "https://s.cc/categories/1/movies/?page=2"
        );
        let mut f = HashMap::new();
        f.insert("area".to_string(), "us".to_string());
        f.insert("empty".to_string(), String::new());
        // 原类只在模板里存在 `{key}` 时才替换；本模板只有 `{cateId}`，故筛选不改变 URL。
        assert_eq!(
            category_url("https://s.cc", "3", 1, &f),
            "https://s.cc/categories/3/movies/"
        );
        assert_eq!(apply_filters("{area}|{empty}|{other}".to_string(), &f), "us|{empty}|{other}");
    }

    #[test]
    fn folder_rows_parse_title_cover_and_rating() {
        let html = r#"
        <div class="content">
          <div class="cover">
            <a href="/movies/1/" title="沙丘2"><img src="/img/dune.jpg"></a>
            <li>评分: 8.7<a href="/detail">详情</a></li>
          </div>
        </div>"#;
        let rows = parse_folder_list(html, "https://s.cc");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "/movies/1/");
        assert_eq!(rows[0].name, "沙丘2");
        assert!(rows[0].cover.as_deref().unwrap_or("").contains("/img/dune.jpg@Headers="));
        assert_eq!(rows[0].remark.as_deref(), Some("详情"));
    }

    #[test]
    fn seed_items_are_reversed_and_compose_id() {
        let html = r#"
        <ul class="seeds">
          <li><a href="magnet:?xt=a" title="4K">A</a></li>
          <li><a href="magnet:?xt=b" title="1080P">B</a></li>
        </ul>"#;
        let rows = parse_seed_items(html, "https://img/c.jpg", "沙丘2", Some("沙丘2"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "沙丘2 1080P");
        assert_eq!(rows[0].id, "沙丘2$$$https://img/c.jpg$$$magnet:?xt=b");
        assert_eq!(rows[1].id, "沙丘2$$$https://img/c.jpg$$$magnet:?xt=a");
        let plain = parse_seed_items(html, "c", "a", None);
        assert_eq!(plain[0].name, "1080P");
        assert_eq!(plain[0].id, "a$$$c$$$magnet:?xt=b");
    }

    #[test]
    fn movie_cover_and_detail_link() {
        let html = r#"
        <div class="content">
          <div class="cover"><img src="/c.jpg" alt="沙丘2"></div>
          <p class="link-start"><a href="magnet:?xt=urn:btih:abc">下载</a></p>
        </div>"#;
        assert_eq!(parse_movie_cover(html), Some(("沙丘2".to_string(), "/c.jpg".to_string())));
        assert_eq!(parse_detail_link(html).as_deref(), Some("magnet:?xt=urn:btih:abc"));
    }

    #[test]
    fn bing_hits_keep_absolute_links_only() {
        let html = r#"
        <ol id="b_results">
          <li><h2><a href="https://www.seedhub.cc/movies/9/">x</a></h2></li>
          <li><a href="/relative">y</a></li>
          <li>no link</li>
        </ol>"#;
        assert_eq!(parse_bing_hits(html), vec!["https://www.seedhub.cc/movies/9/".to_string()]);
    }

    #[test]
    fn search_page_combines_cover_and_seeds() {
        let html = r#"
        <div class="content">
          <div class="cover"><img src="/c.jpg" alt="沙丘2"></div>
          <ul class="seeds">
            <li><a href="magnet:?xt=a" title="4K"></a></li>
          </ul>
        </div>"#;
        let (alt, cover, rows) = parse_search_page(html, "https://s.cc").expect("应解析");
        assert_eq!(alt, "沙丘2");
        assert_eq!(cover, "https://s.cc/c.jpg");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "沙丘2$$$https://s.cc/c.jpg$$$magnet:?xt=a");
    }

    #[test]
    fn player_plan_follows_prefix() {
        let ed2k = player_json("ed2k://|file|a.mkv|1|H|/");
        assert_eq!(ed2k["parse"], json!(0));
        assert_eq!(ed2k["playUrl"], json!(""));
        let tvbox = player_json("tvbox-x");
        assert_eq!(tvbox["parse"], json!(0));
        let http = player_json("https://a/x.m3u8");
        assert_eq!(http["parse"], json!(1));
        assert_eq!(http["url"], json!("https://a/x.m3u8"));
        assert!(http["header"].as_str().unwrap_or("").contains("Android 13"));
    }
}
