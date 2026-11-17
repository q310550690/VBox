//! `csp_Xb6v` 原生实现（第六个 HTML/磁力型垂直切片，MacCMS 变体）。
//!
//! 语义来自静态反编译：`_rea/decompiled/41c87635-custom_spider/deobf/com/github/catvod/spider/Xb6v.java`
//! （270 行、0 处 `Method dump skipped`）。该类 `extends Spider`，不依赖宿主私有 API，
//! 因此可以 100% 原生重写。与 `csp_SixV` 同族（同一套 `#post_container .post_hover` /
//! `[class=zoom]` / `#post_content` 磁力表 / 帝国 CMS 搜索模板），差异如下：
//!
//! 1. 基址 `https://www.xb6v.com`，分类来自首页 `#menus` 导航（不是写死数组）。
//! 2. 列表 `{base}{分类路径}`，第 2 页起 `{base}{分类路径}index_{page}.html`（每页 18 条）。
//! 3. 详情元数据在 `div.context` 的纯文本行里，形如
//!    `◎年　　代:　2026`（标签内含全角空格、ASCII 冒号、值前一个全角空格）：
//!    主演的续行没有 `◎` 前缀，简介在 `◎简　　介` 之后直到文末。
//! 4. 播放页链接本身就是 `magnet:`，`playerContent` 直接回放（`parse: 0`）。
//! 5. 搜索按原实现 `POST {base}/e/search/index.php`；站点已下线该接口
//!    （实测 404），非 2xx 映射为明确错误码而不是黑箱。
//!
//! 与 Android 原实现的差异（均为注释中标注的有意偏差）：
//! - `ext` 首段 http(s) 视为基址覆盖（原实现不支持；仅用于 mock/镜像）；
//! - 元数据用「标签 + 值」逐行解析而不是单一巨型正则（原实现用一个正则匹配整个
//!   `context` 的 HTML，对续行/换行更脆弱）；
//! - 分页上限不再根据尾页推断（VBox 侧不需要 pagecount），按请求页直接构造 URL；
//! - 播放头保留 UA/Referer（原实现 `header` 为空串），供磁力直链之外的场景使用。

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Xb6v";

/// 站点默认基址（原实现 `T.g("https://www.xb6v.com", …)`）。
const DEFAULT_BASE: &str = "https://www.xb6v.com";

/// 安卓端 UA（`Chrome/122`）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 导航里需要跳过的两个非分类入口。
const SKIP_NAV_NAMES: [&str; 2] = ["首页", "最新50部"];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 选择器
 * ============================================================ */

fn menus_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#menus a[href]").expect("导航选择器"))
}

fn list_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#post_container .post_hover").expect("列表行选择器"))
}

fn zoom_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("[class=zoom]").expect("zoom 选择器"))
}

fn img_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("img").expect("img 选择器"))
}

fn rel_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("[rel]").expect("rel 选择器"))
}

fn content_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#post_content").expect("#post_content 选择器"))
}

fn table_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("table").expect("table 选择器"))
}

fn anchor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a").expect("a 选择器"))
}

fn context_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse(".context").expect(".context 选择器"))
}

fn title_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse(".article_container > h1").expect("标题选择器"))
}

fn content_img_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("#post_content img").expect("封面选择器"))
}

fn label_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^◎\s*(.*?)\s*[:：]\s*(.*)$").expect("元数据标签正则"))
}

/* ============================================================
 * 对外入口
 * ============================================================ */

/// 分类来自首页 `#menus` 导航（原实现同样在 `homeContent` 里现抓）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site)?;
    let html = fetch_text(cfg, &base, &format!("{base}/"))?;
    let cats = parse_categories(&html, &base);
    if cats.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "首页导航解析为空（站点改版或非 Xb6v 模板？）",
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

/// 原实现 `POST /e/search/index.php`；本站已下线该接口（实测 404）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = format!("{base}/e/search/index.php");
    let mut headers = headers(&base);
    headers.insert("Origin".to_string(), base.clone());
    headers.insert(
        "Content-Type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    );
    let mut form: HashMap<String, String> = HashMap::new();
    form.insert("show".to_string(), "title".to_string());
    form.insert("tempid".to_string(), "1".to_string());
    form.insert("tbname".to_string(), "article".to_string());
    form.insert("mid".to_string(), "1".to_string());
    form.insert("dopost".to_string(), "search".to_string());
    form.insert("submit".to_string(), String::new());
    form.insert("keyboard".to_string(), keyword.to_string());

    let client = crate::http::client_for(&cfg.http)?;
    let body = match crate::http::post_form_text(&client, &url, &headers, &form) {
        Ok(body) => body,
        Err(_) => {
            return Err(CatvodError::new(
                ErrorClass::Parse,
                site,
                "xb6v 搜索接口不可用（站点已下线 /e/search/index.php，实测 HTTP 404）",
            )
            .into_core());
        }
    };
    Ok(parse_list(&body, &base))
}

/// 磁力直出（原实现 `playerContent` 原样返回 `id`）。
pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site)?;
    let mut header = serde_json::Map::new();
    header.insert("User-Agent".to_string(), json!(UA));
    header.insert("Referer".to_string(), json!(format!("{base}/")));
    Ok(json!({
        "parse": 0,
        "jx": 0,
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
            format!("csp_Xb6v 的 ext 首段不是 http(s) 基址：{first}"),
        )
        .into_core());
    }
    Ok(first.trim_end_matches('/').to_string())
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Referer".to_string(), format!("{base}/"));
    headers.insert(
        "Accept".to_string(),
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
    );
    headers.insert("Accept-Language".to_string(), "zh-CN,zh;q=0.9".to_string());
    headers
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

/// 第 1 页 `{base}{cat}`；第 2 页起 `{base}{cat}index_{page}.html`。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let mut path = cat.trim().to_string();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    if !path.ends_with('/') {
        path.push('/');
    }
    if page <= 1 {
        format!("{base}{path}")
    } else {
        format!("{base}{path}index_{page}.html")
    }
}

fn detail_url(base: &str, id: &str) -> String {
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
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<Category> = Vec::new();
    for a in doc.select(menus_sel()) {
        let href = attr(&a, "href");
        let name = text_of(&a);
        if href.is_empty() || name.is_empty() {
            continue;
        }
        if SKIP_NAV_NAMES.iter().any(|skip| name.as_str() == *skip) {
            continue;
        }
        if href == "/" || href.starts_with("http://") || href.starts_with("https://") {
            continue;
        }
        let mut path = href.split('#').next().unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }
        if let Some(stripped) = path.strip_prefix(base) {
            path = stripped.to_string();
        }
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        out.push(Category {
            type_id: path,
            type_name: name,
        });
    }
    out
}

pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(list_row_sel()) {
        let Some(link) = row.select(zoom_sel()).next() else {
            continue;
        };
        let href = attr(&link, "href");
        if href.is_empty() {
            continue;
        }
        let name = {
            let title = attr(&link, "title");
            if title.is_empty() {
                text_of(&link)
            } else {
                title
            }
        };
        let cover = link
            .select(img_sel())
            .next()
            .map(|img| absolutize(&attr(&img, "src"), base))
            .filter(|src| !src.is_empty());
        let remark = row
            .select(rel_sel())
            .find(|el| attr(el, "rel").to_lowercase().contains("category"))
            .map(|el| text_of(&el));
        out.push(VideoItem {
            id: href,
            name: clean_text(&name),
            cover,
            remark: remark.map(|text| clean_text(&text)).filter(|s| !s.is_empty()),
        });
    }
    out
}

pub fn parse_detail(site: &Site, id: &str, html: &str, base: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);

    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for content in doc.select(content_sel()) {
        for table in content.select(table_sel()) {
            let mut items: Vec<Episode> = Vec::new();
            for a in table.select(anchor_sel()) {
                let href = attr(&a, "href");
                if !href.starts_with("magnet:") {
                    continue;
                }
                items.push(Episode {
                    title: clean_text(&text_of(&a)),
                    url: href,
                });
            }
            if !items.is_empty() {
                groups.push(EpisodeGroup {
                    group: format!("磁力线路{}", groups.len() + 1),
                    items,
                });
            }
        }
    }

    let lines: Vec<String> = doc
        .select(context_sel())
        .flat_map(|el| {
            el.text()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
        })
        .collect();
    let meta = Meta::from_lines(&lines);

    let mut name = doc
        .select(title_sel())
        .next()
        .map(|el| clean_text(&text_of(&el)))
        .unwrap_or_default();
    if name.is_empty() {
        name = meta
            .get("中文名")
            .or_else(|| meta.get("片名"))
            .cloned()
            .unwrap_or_default();
    }
    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少标题与磁力线路（站点改版或非 Xb6v 模板？）",
        )
        .into_core());
    }

    let cover = doc
        .select(content_img_sel())
        .next()
        .map(|img| absolutize(&attr(&img, "src"), base))
        .filter(|src| !src.is_empty());

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc: meta.get("简介").cloned().filter(|s| !s.is_empty()),
        year: meta.get("年代").cloned(),
        area: meta.get("产地").cloned(),
        director: meta.get("导演").cloned(),
        actor: meta.get("主演").cloned(),
        episodes: groups,
    })
}

/// `div.context` 的纯文本行 → 元数据（标签内全角空格、续行、简介块）。
#[derive(Debug, Default)]
pub struct Meta(HashMap<String, String>);

impl Meta {
    fn from_lines(lines: &[String]) -> Self {
        let mut map: HashMap<String, String> = HashMap::new();
        let mut i = 0usize;
        while i < lines.len() {
            let line = lines[i].trim();
            let Some(rest) = line.strip_prefix('◎') else {
                i += 1;
                continue;
            };
            let (label, value) = split_label(rest);
            let key = normalize_label(&label);
            if key.is_empty() {
                i += 1;
                continue;
            }
            if key == "主演" || key == "简介" {
                let mut parts: Vec<String> = Vec::new();
                if !value.is_empty() {
                    parts.push(value);
                }
                i += 1;
                while i < lines.len() {
                    let next = lines[i].trim();
                    if next.starts_with('◎') || next.is_empty() {
                        break;
                    }
                    parts.push(next.to_string());
                    i += 1;
                }
                let joined = if key == "主演" {
                    parts.join(" ")
                } else {
                    parts.join("\n")
                };
                map.entry(key).or_insert(joined);
                continue;
            }
            if !value.is_empty() {
                map.entry(key).or_insert(value);
            }
            i += 1;
        }
        Meta(map)
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.0.get(key)
    }
}

fn split_label(rest: &str) -> (String, String) {
    if let Some(caps) = label_re().captures(&format!("◎{rest}")) {
        let label = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        let value = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
        return (label, clean_text(&value));
    }
    (rest.to_string(), String::new())
}

fn normalize_label(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{3000}')
        .collect()
}

/* ============================================================
 * 小工具
 * ============================================================ */

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

fn clean_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_space = false;
    for c in raw.chars() {
        if c.is_whitespace() || c == '\u{3000}' {
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        last_space = false;
        out.push(c);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Xb6v"));
        assert!(handles(" CSP_XB6V "));
        assert!(!handles("csp_SixV"));
    }

    #[test]
    fn parse_categories_skips_home_and_absolute_links() {
        let html = r#"
            <div id="menus">
              <li><a href="/">首页</a></li>
              <li><a href="https://www.xb6v.com/qian50m.html">最新50部</a></li>
              <li><a href="/dianshiju/">电视剧</a></li>
              <li><a href="/dianshiju/guoju/">国剧</a></li>
            </div>"#;
        let cats = parse_categories(html, "https://www.xb6v.com");
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "/dianshiju/");
        assert_eq!(cats[0].type_name, "电视剧");
        assert_eq!(cats[1].type_id, "/dianshiju/guoju/");
        assert_eq!(cats[1].type_name, "国剧");
    }

    #[test]
    fn list_url_appends_index_for_later_pages() {
        let base = "https://www.xb6v.com";
        assert_eq!(list_url(base, "/dianshiju/guoju/", 1), "https://www.xb6v.com/dianshiju/guoju/");
        assert_eq!(list_url(base, "dianshiju/guoju", 2), "https://www.xb6v.com/dianshiju/guoju/index_2.html");
    }

    #[test]
    fn parse_list_reads_row_fields() {
        let html = r#"
          <div id="post_container">
            <li class="post">
              <div class="post_hover">
                <div class="thumbnail">
                  <a class="zoom" href="/dianshiju/guoju/29956.html" title="玫瑰战争">
                    <img src="/upload/vod/1.webp">
                  </a>
                </div>
                <div class="article">
                  <h2><a href="/dianshiju/guoju/29956.html">玫瑰战争</a></h2>
                  <p><span rel="category tag">国剧</span></p>
                </div>
              </div>
            </li>
          </div>"#;
        let items = parse_list(html, "https://www.xb6v.com");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "/dianshiju/guoju/29956.html");
        assert_eq!(items[0].name, "玫瑰战争");
        assert_eq!(items[0].cover.as_deref(), Some("https://www.xb6v.com/upload/vod/1.webp"));
        assert_eq!(items[0].remark.as_deref(), Some("国剧"));
    }

    #[test]
    fn parse_detail_reads_metadata_and_magnet_groups() {
        let html = r#"
          <div class="article_container"><h1>玫瑰战争</h1></div>
          <div id="post_content">
            <img src="https://img.dytt-tupian.com/upload/vod/1.webp">
            <table><tr><td>磁力：<a href="magnet:?xt=urn:btih:abc">01-06.2160p</a></td></tr></table>
            <table><tr><td><a href="https://pan.xunlei.com/s/xyz">迅雷云盘</a></td></tr></table>
          </div>
          <div class="context">
            ◎片　　名:　玫瑰戰爭<br>
            ◎年　　代:　2026<br>
            ◎产　　地:　中国香港 / 中国大陆<br>
            ◎类　　别:　剧情<br>
            ◎导　　演:　钟澍佳 Chung Shu Kai<br>
            ◎主　　演:　杨茜尧 Tavia Yeung<br>
            陈豪 Moses Chan<br>
            林夏薇 Rosina Lin<br>
            ◎简　　介<br>
            一宗骇人的碎尸案。
          </div>"#;
        let detail = parse_detail(&Site::default(), "/dianshiju/guoju/29956.html", html, "https://www.xb6v.com")
            .expect("detail");
        assert_eq!(detail.name, "玫瑰战争");
        assert_eq!(detail.year.as_deref(), Some("2026"));
        assert_eq!(detail.area.as_deref(), Some("中国香港 / 中国大陆"));
        assert_eq!(detail.director.as_deref(), Some("钟澍佳 Chung Shu Kai"));
        let actor = detail.actor.unwrap_or_default();
        assert!(actor.contains("杨茜尧"));
        assert!(actor.contains("陈豪 Moses Chan"));
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, "磁力线路1");
        assert_eq!(detail.episodes[0].items[0].url, "magnet:?xt=urn:btih:abc");
        assert!(detail.desc.unwrap_or_default().contains("碎尸案"));
    }

    #[test]
    fn parse_detail_without_data_errors() {
        let err = parse_detail(&Site::default(), "x", "<html><body></body></html>", "https://www.xb6v.com")
            .expect_err("must fail");
        assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
    }

    #[test]
    fn player_returns_magnet_directly() {
        let site = Site::default();
        let value = player_content(&TvConfig::default(), &site, "", "magnet:?xt=urn:btih:abc").expect("player");
        assert_eq!(value["parse"], json!(0));
        assert_eq!(value["url"], json!("magnet:?xt=urn:btih:abc"));
        assert!(value["header"]["User-Agent"].as_str().unwrap_or("").contains("Chrome/122"));
    }
}
