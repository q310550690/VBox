//! `csp_Bt0`（2bt0 磁力站）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.Bt0`，站点 1 个（dc 仓 `xhztv`，key `Bt0`，
//!   站点名 `磁力 | 2bt0`，ext 为空文本）。
//! * 证据：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Bt0.java`
//!   （548 行，`extends Spider`，无 `parser.merge` 宿主依赖，仅用 jar 内 jsoup
//!   封装的语义）。同名的 `dffec63f-pg` 变体是 `extends Push` + WebView 过
//!   Cloudflare 的另一代实现（基址 `http://3bt0.com/`），本实现以干净的
//!   `0f047745` 版为准。
//! * 字符串：`merge.z.C0338a.a(byte[], byte[])` = 字节循环异或（密钥不足时
//!   首尾相接循环）后 UTF-8；本仓 `_rea/tmp/r147/dec-bt0c.mjs` 已解出全部
//!   隐藏选择器（`href`、`div[style^=background-image]`、`a>div`、
//!   `div:has(>a[href^=/tr/])>div>div`、`figcaption em`、`h2`、`url(`）。
//!
//! 协议（逐段还原）：
//! * `init`：ext 按 `$$$` 拆 —— `[0]` 基址（>7 字符）、`[1]` 代理前缀、
//!   `[2]` 弹幕开关（`"1"`）。原类把代理写进 `merge.S.e.z`，桌面端不启用代理
//!   （有意偏差 1）。
//! * 请求头：Chrome/122 UA、`Referer` = 基址、`Accept:
//!   application/json, text/plain, */*`，已有 Cookie 时附加 `Cookie`。
//! * 反爬：首次请求返回的 HTML 里带
//!   `document.cookie = "ge_js_validator_…; …"; window.location.reload();`，
//!   原类从响应头 `set-cookie` 合并 + 正则 `document.cookie *= *"([^;]*);"`
//!   提取 cookie，然后带 Cookie 重发一次。本实现按同样形式实现（单次重发，
//!   不做 JS 执行；有意偏差 2：不解析页面内脚本逻辑，只做 cookie 透传）。
//! * 首页：分类固定 5 项（`1` 电影 / `2` 电视剧 / `3` 周热门 / `4` 月热门 /
//!   `5` 年度热门）；列表行 `.masonry_item`（`h2` 标题、`a[href]` 详情、
//!   `div[style^=background-image]` 封面、`figcaption em` 备注）。
//! * 分类：`{base}tlist.php?sc={cateId}&page={page}`；行
//!   `.container:has(a[href^=magnet:])`：`a[href]` 详情、
//!   `div:has(>a[href^=/tr/])>div>div` 标题、`a>div` 封面、
//!   `div[style~=-39px]` 备注。
//! * 详情：GET 详情页；`#movie-content` 内 `img[src]` 封面、`.info-title`
//!   标题、`.tiny-title` 逐条（`\n` 连接作简介，首条作备注）；线路名来自
//!   `ul.tabs li`，每组剧集来自 `ul.tabs-content > li` 内 `.container` 的
//!   `a[href^=magnet:]`（标题 `a.torrent-title`，`标题$磁力`，`#` 连接，
//!   字符串排序后多线路 `$$$` 连接）。
//! * 搜索：`{base}search.php?sb={kw}`，行解析与首页相同（`.masonry_item`）。
//! * 播放：磁力/电驴地址直出 `parse:0`；原类在
//!   `global_magnet_systemopen` 时走 `127.0.0.1:10079` 本地代理，桌面端不启用
//!   （有意偏差 3）。
//!
//! 有意偏差汇总：
//! 1. 代理前缀（ext `[1]`）仅解析不启用；
//! 2. 反爬只做一次 cookie 提取 + 重发，不执行页面 JS（原类同样只看 cookie）；
//! 3. 弹幕/磁力系统播放（`DL+0` 前缀）不接宿主代理，直接按磁力地址返回；
//! 4. 站点当前已改版为 SPA（`www.2bt0.com` 仅返回 Vue 壳），原类所依赖的服务端
//!    渲染 HTML 不再可用 —— 本家族按原类协议完整实现并以 mock 验收。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use scraper::{Html, Selector};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::contract::CatvodError;
use crate::spider::catvod::contract::ErrorClass;

/// 家族识别名（与站点 `api` 字段一致，大小写不敏感）。
pub const API: &str = "csp_Bt0";
/// 原类硬编码基址。
pub const DEFAULT_BASE: &str = "https://2bt0.com/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const ACCEPT: &str = "application/json, text/plain, */*";
/// 原类 `init` 的固定 5 个分类。
const HOME_CATEGORIES: [(&str, &str); 5] = [
    ("1", "电影"),
    ("2", "电视剧"),
    ("3", "周热门"),
    ("4", "月热门"),
    ("5", "年度热门"),
];
/// Cookie 提取正则：`document.cookie = "name=value; ...";`
const RE_COOKIE: &str = r#"document\.cookie\s*=\s*"([^;]*);"#;

static COOKIE: OnceLock<Mutex<String>> = OnceLock::new();

fn cookie_slot() -> &'static Mutex<String> {
    COOKIE.get_or_init(|| Mutex::new(String::new()))
}

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail.into()).into_core()
}

fn re_cookie() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(RE_COOKIE).expect("cookie regex"))
}

fn sel<'a>(css: &str) -> Selector {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, Selector>> = std::cell::RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(s) = cache.get(css) {
            return s.clone();
        }
        let s = Selector::parse(css).unwrap_or_else(|_| Selector::parse("html").expect("fallback"));
        cache.insert(css.to_string(), s.clone());
        s
    })
}

fn text_of(el: scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn attr_of(el: scraper::ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or("").trim().to_string()
}

/// ext 的首个 `http(s)` 段覆盖基址（原类 `init` 即 ext[0]）。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let mut base = DEFAULT_BASE.to_string();
    let mut parts = ext.split("$$$");
    if let Some(first) = parts.next() {
        let first = first.trim();
        if first.starts_with("http") && first.len() > 7 {
            base = first.to_string();
        }
    }
    let mut b = base.clone();
    if !b.ends_with('/') {
        b.push('/');
    }
    b
}

/// 分类列表 URL。
pub fn list_url(base: &str, cate_id: &str, page: u32) -> String {
    format!("{base}tlist.php?sc={cate_id}&page={page}")
}

/// 搜索 URL。
pub fn search_url(base: &str, keyword: &str) -> String {
    format!("{base}search.php?sb={}", urlencode(keyword))
}

pub fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 从 `style` 的 `url(...)` 里取地址（原类 `trim.substring(indexOf("url(")+4, len-1)`）。
fn style_url(style: &str) -> String {
    let style = style.trim();
    let Some(pos) = style.find("url(") else {
        return String::new();
    };
    let rest = &style[pos + 4..];
    let end = rest.rfind(')').unwrap_or(rest.len());
    rest[..end].trim_matches(|c| c == '"' || c == '\'').trim().to_string()
}

fn header_map(h: &HashMap<String, String>) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in h {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            map.insert(name, val);
        }
    }
    map
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), base.to_string());
    h.insert("Accept".to_string(), ACCEPT.to_string());
    let cookie = cookie_slot().lock().map(|g| g.clone()).unwrap_or_default();
    if !cookie.is_empty() {
        h.insert("Cookie".to_string(), cookie);
    }
    h
}

/// 记录响应头里的 `set-cookie`（原类合并进 cookie 串）。
fn absorb_set_cookie(resp: &reqwest::blocking::Response) {
    let mut slot = match cookie_slot().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut merged: HashMap<String, String> = HashMap::new();
    for (name, value) in slot.split(';').filter_map(|kv| kv.split_once('=')) {
        merged.insert(name.trim().to_string(), value.trim().to_string());
    }
    for value in resp.headers().get_all("set-cookie") {
        let Ok(text) = value.to_str() else { continue };
        if let Some((name, rest)) = text.split(';').next().and_then(|kv| kv.split_once('=')) {
            merged.insert(name.trim().to_string(), rest.trim().to_string());
        }
    }
    let mut pairs: Vec<String> = merged
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    pairs.sort();
    if !pairs.is_empty() {
        *slot = pairs.join(";");
    }
}

/// 带反爬 cookie 的 GET：先按现状请求，命中 `document.cookie` 时提取并重发一次。
fn fetch(cfg: &TvConfig, url: &str, base: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let mut resp = client
        .get(url)
        .headers(header_map(&headers(base)))
        .send()
        ?;
    absorb_set_cookie(&resp);
    let mut body = resp.text()?;
    if let Some(cap) = re_cookie().captures(&body) {
        if let Some(cookie) = cap.get(1) {
            if let Ok(mut slot) = cookie_slot().lock() {
                if !cookie.as_str().is_empty() {
                    *slot = cookie.as_str().to_string();
                }
            }
        }
        resp = client
            .get(url)
            .headers(header_map(&headers(base)))
            .send()
            ?;
        body = resp.text()?;
    }
    Ok(body)
}

fn parse_cover(el: scraper::ElementRef<'_>, css: &str) -> String {
    let sel = sel(css);
    el.select(&sel)
        .next()
        .map(|n| style_url(&attr_of(n, "style")))
        .unwrap_or_default()
}

fn http_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    let base = base.trim_end_matches('/');
    if href.starts_with('/') {
        return format!("{base}{href}");
    }
    format!("{base}/{href}")
}

/// 首页推荐列表（`.masonry_item`，`h2` 标题）。
pub fn parse_home(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row = sel(".masonry_item");
    let a = sel("a[href]");
    let h2 = sel("h2");
    let mark = sel("figcaption em");
    let mut out = Vec::new();
    for item in doc.select(&row) {
        let id = item.select(&a).next().map(|n| attr_of(n, "href")).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let name = item.select(&h2).next().map(text_of).unwrap_or_default();
        let cover = parse_cover(item, "div[style^=\"background-image\"]");
        let remark = item.select(&mark).next().map(text_of).unwrap_or_default();
        out.push(VideoItem {
            id: http_url(base, &id),
            name,
            cover: if cover.is_empty() { None } else { Some(http_url(base, &cover)) },
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// 分类/搜索结果列表（`.container:has(a[href^=magnet:])`）。
/// 行标题：原类选择器 div:has(>a[href^=/tr/])>div>div（scraper 不支持 :has）。
fn list_item_name(item: scraper::ElementRef<'_>) -> String {
    let tr = sel("a[href^=\"/tr/\"]");
    let div = sel("div");
    for d in item.select(&div) {
        if d.select(&tr).next().is_some() {
            continue;
        }
        let Some(parent) = d.parent().and_then(scraper::ElementRef::wrap) else {
            continue;
        };
        if parent.select(&tr).next().is_some() {
            let text = text_of(d);
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    String::new()
}

pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row = sel(".container");
    let a = sel("a[href]");
    let magnet = sel("a[href^=\"magnet:\"]");
    let remark_sel = sel("div[style~=\"-39px\"]");
    let mut out = Vec::new();
    for item in doc.select(&row) {
        if item.select(&magnet).next().is_none() {
            continue;
        }
        let id = item.select(&a).next().map(|n| attr_of(n, "href")).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let name = list_item_name(item);
        let cover = parse_cover(item, "a>div");
        let remark = item.select(&remark_sel).next().map(text_of).unwrap_or_default();
        out.push(VideoItem {
            id: http_url(base, &id),
            name,
            cover: if cover.is_empty() { None } else { Some(http_url(base, &cover)) },
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// 搜索结果列表：原类 `c()` 用 `.masonry_item` + `h5` 标题。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row = sel(".masonry_item");
    let a = sel("a[href]");
    let h5 = sel("h5");
    let mut out = Vec::new();
    for item in doc.select(&row) {
        let id = item.select(&a).next().map(|n| attr_of(n, "href")).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let name = item.select(&h5).next().map(text_of).unwrap_or_default();
        let cover = parse_cover(item, "div[style^=\"background-image\"]");
        out.push(VideoItem {
            id: http_url(base, &id),
            name,
            cover: if cover.is_empty() { None } else { Some(http_url(base, &cover)) },
            remark: None,
        });
    }
    out
}

/// 详情页解析：`#movie-content` + 播放标签页。
pub fn parse_detail(html: &str, base: &str, id: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let content = sel("#movie-content");
    let Some(root) = doc.select(&content).next() else {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            &Site::default(),
            format!("bt0: #movie-content not found for {id}"),
        )
        .into_core());
    };
    let cover = root
        .select(&sel("img[src]"))
        .next()
        .map(|n| attr_of(n, "src"))
        .unwrap_or_default();
    let name = root
        .select(&sel(".info-title"))
        .next()
        .map(text_of)
        .unwrap_or_default();
    let tiny: Vec<String> = root.select(&sel(".tiny-title")).map(text_of).collect();
    let desc = if tiny.is_empty() { None } else { Some(tiny.join("\n")) };

    let tab_names: Vec<String> = doc.select(&sel("ul.tabs li")).map(text_of).collect();
    let groups: Vec<EpisodeGroup> = doc
        .select(&sel("ul.tabs-content > li"))
        .enumerate()
        .map(|(idx, li)| {
            let mut eps: Vec<Episode> = Vec::new();
            for container in li.select(&sel(".container")) {
                let href = container
                    .select(&sel("a[href^=\"magnet:\"]"))
                    .next()
                    .map(|n| attr_of(n, "href"))
                    .unwrap_or_default();
                if href.is_empty() {
                    continue;
                }
                let title = container
                    .select(&sel("a.torrent-title"))
                    .next()
                    .map(text_of)
                    .unwrap_or_default();
                eps.push(Episode {
                    title,
                    url: href,
                });
            }
            eps.sort_by(|a, b| a.title.cmp(&b.title).then(a.url.cmp(&b.url)));
            let group = tab_names
                .get(idx)
                .cloned()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("线路{}", idx + 1));
            EpisodeGroup { group, items: eps }
        })
        .filter(|g| !g.items.is_empty())
        .collect();

    if groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            &Site::default(),
            format!("bt0: no magnet episodes for {id}"),
        )
        .into_core());
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: if cover.is_empty() { None } else { Some(http_url(base, &cover)) },
        desc,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: groups,
    })
}

/// 五入口：分类固定 5 项（原类 `homeContent` 的分类常量）。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(HOME_CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect())
}

/// 首页推荐（TVBox `homeVideoContent` 语义）：GET 基址并按首页选择器解析。
/// 主要用途是触发原类的 cookie 反爬首跳，因此从 `fetch` 走完整流程。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let html = fetch(cfg, &base, &base)?;
    Ok(parse_home(&html, &base))
}
pub fn videos(cfg: &TvConfig, site: &Site, cate_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cate_id, page);
    let html = fetch(cfg, &url, &base)?;
    Ok(parse_list(&html, &base))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = http_url(&base, id);
    let html = fetch(cfg, &url, &base)?;
    parse_detail(&html, &base, id)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = search_url(&base, keyword);
    let html = fetch(cfg, &url, &base)?;
    Ok(parse_search(&html, &base))
}

/// 播放结果构造（同步、便于单测）：磁力/电驴直出。
pub fn player_json(base: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, &Site::default(), "bt0: empty play id").into_core());
    }
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Referer".to_string(), base.to_string());
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": id,
        "header": headers,
    }))
}

pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    player_json(&base_of(site), id).map_err(|e| {
        CatvodError::new(ErrorClass::Parse, site, e.to_string()).into_core()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        Site {
            ext: Some(Value::String(ext.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Bt0"));
        assert!(handles(" CSP_BT0 "));
        assert!(!handles("csp_Bt1"));
    }

    #[test]
    fn base_defaults_and_ext_overrides() {
        assert_eq!(base_of(&site("")), DEFAULT_BASE);
        assert_eq!(base_of(&site("http://mock.test")), "http://mock.test/");
        assert_eq!(base_of(&site("http://mock.test/$$$proxy$$$1")), "http://mock.test/");
    }

    #[test]
    fn list_and_search_urls() {
        let base = "https://2bt0.com/";
        assert_eq!(list_url(base, "1", 2), "https://2bt0.com/tlist.php?sc=1&page=2");
        assert_eq!(search_url(base, "斗罗 大陆"), "https://2bt0.com/search.php?sb=%E6%96%97%E7%BD%97+%E5%A4%A7%E9%99%86");
    }

    #[test]
    fn style_url_extracts_background() {
        assert_eq!(
            style_url("background-image: url(https://cdn.test/a.jpg)"),
            "https://cdn.test/a.jpg"
        );
        assert_eq!(style_url("background-image:url('//cdn.test/b.jpg')"), "//cdn.test/b.jpg");
        assert_eq!(style_url("color:red"), "");
    }

    #[test]
    fn http_url_joins_forms() {
        let base = "https://2bt0.com/";
        assert_eq!(http_url(base, "/tr/1.html"), "https://2bt0.com/tr/1.html");
        assert_eq!(http_url(base, "tr/2.html"), "https://2bt0.com/tr/2.html");
        assert_eq!(http_url(base, "//cdn.test/x.jpg"), "https://cdn.test/x.jpg");
        assert_eq!(http_url(base, "http://a/b"), "http://a/b");
    }

    #[test]
    fn parses_home_rows() {
        let html = r#"
        <div class="masonry_item">
          <a href="/tr/abc.html"><h2>测试电影</h2></a>
          <div style="background-image: url(//img.test/a.jpg)"></div>
          <figcaption><em>2024-01-02</em></figcaption>
        </div>"#;
        let items = parse_home(html, "https://2bt0.com/");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://2bt0.com/tr/abc.html");
        assert_eq!(items[0].name, "测试电影");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.test/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("2024-01-02"));
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"
        <div class="container">
          <a href="magnet:?xt=urn:btih:AAA"></a>
          <div><a href="/tr/1.html"></a><div>磁力标题</div></div>
          <div><a href="/tr/1.html"><div style="background-image: url(https://img.test/b.jpg)"></div></a></div>
          <div style="margin-top: -39px">10.0</div>
        </div>"#;
        let items = parse_list(html, "https://2bt0.com/");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "磁力标题");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.test/b.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("10.0"));
    }

    #[test]
    fn parses_search_rows_with_h5() {
        let html = r#"
        <div class="masonry_item">
          <a href="/tr/s.html"><h5>搜索片名</h5></a>
          <div style="background-image:url(//img.test/c.jpg)"></div>
        </div>"#;
        let items = parse_search(html, "https://2bt0.com/");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "搜索片名");
        assert_eq!(items[0].id, "https://2bt0.com/tr/s.html");
    }

    #[test]
    fn parses_detail_with_tabs_and_sorted_episodes() {
        let html = r#"
        <div id="movie-content">
          <img src="/img/cover.jpg" />
          <h1 class="info-title">示例影片</h1>
          <div class="tiny-title">2023</div>
          <div class="tiny-title">豆瓣 8.1</div>
        </div>
        <ul class="tabs"><li>1080P</li><li>4K</li></ul>
        <ul class="tabs-content">
          <li><div class="container"><a href="magnet:?xt=urn:btih:BBB"></a><a class="torrent-title">第2集</a></div>
              <div class="container"><a href="magnet:?xt=urn:btih:AAA"></a><a class="torrent-title">第1集</a></div></li>
          <li><div class="container"><a href="magnet:?xt=urn:btih:CCC"></a><a class="torrent-title">第1集</a></div></li>
        </ul>"#;
        let d = parse_detail(html, "https://2bt0.com/", "/tr/1.html").expect("detail");
        assert_eq!(d.name, "示例影片");
        assert_eq!(d.cover.as_deref(), Some("https://2bt0.com/img/cover.jpg"));
        assert_eq!(d.desc.as_deref(), Some("2023\n豆瓣 8.1"));
        assert_eq!(d.episodes.len(), 2);
        assert_eq!(d.episodes[0].group, "1080P");
        assert_eq!(d.episodes[0].items[0].title, "第1集");
        assert_eq!(d.episodes[1].group, "4K");
    }

    #[test]
    fn detail_without_episodes_errors() {
        let html = r#"<div id="movie-content"><h1 class="info-title">空</h1></div>"#;
        assert!(parse_detail(html, "https://2bt0.com/", "/tr/empty.html").is_err());
    }

    #[test]
    fn cookie_pattern_extracts_value() {
        let body = r#"<script>document.cookie = "ge_js_validator_18=abc@18@def; path=/;";window.location.reload();</script>"#;
        let cap = re_cookie().captures(body).expect("cookie");
        assert_eq!(&cap[1], "ge_js_validator_18=abc@18@def");
    }

    #[test]
    fn player_returns_magnet_directly() {
        let out = player_json("https://2bt0.com/", "magnet:?xt=urn:btih:AAA").expect("player");
        assert_eq!(out["parse"], 0);
        assert_eq!(out["url"], "magnet:?xt=urn:btih:AAA");
    }

    #[test]
    fn player_rejects_empty_id() {
        assert!(player_json("https://2bt0.com/", "  ").is_err());
    }
}
