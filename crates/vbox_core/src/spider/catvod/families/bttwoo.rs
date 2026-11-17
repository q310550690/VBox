//! `csp_Bttwoo` 原生实现（BT之家影视，独立 `extends Spider` 类）。
//!
//! 语义来自反编译：`_rea/tmp/xinsj/out-Bttwoo/Bttwoo.java`（dex2jar + Vineflower，
//! 2817 行、0 失败）与字符串解码后的 `Bttwoo.decoded.java`（119/119 字符串）。
//!
//! 协议：
//! 1. `homeContent`：GET `https://www.bttwo.net/` → `ul.navlist > li.menu-item-type-custom > a`
//!    取分类（href、锚文本）；首页推荐列表 = `div.leibox > ul > li`；`filters` 恒 `{}`。
//! 2. `categoryContent`：`{base}{type_id}/page/{page}`；页数 = `div.pagenavi_txt > a.extend`
//!    的 href 末段数字；行 = `div.bt_img > ul > li`（id 取自 `h3.dytit > a` 的 href
//!    中 `movie/(\d+).html`，标题同元素文本，封面 `a > img` 的 `data-original`，
//!    备注 `div.jidi > span`）；输出 `page/pageCount/limit/total/list`。
//! 3. `detailContent`：`{base}movie/{id}.html`；标题 `div.moviedteail_tt > h1`、
//!    封面 `div.dyimg > img` 的 src、简介 `meta[name=description]` 的 content、
//!    信息 `ul.moviedteail_list > li`（「类型：/年份：/地区：/上映：/导演：/主演：」）、
//!    线路名 `div.mi_paly_box > div > div.ypxingq_t`、剧集 `div.paly_list_btn a`；
//!    `vod_play_from` 用 `$$$`、`vod_play_url` 用 `#`、单集 `名称$链接`。
//! 4. `searchContent`：`{base}xssearch?q={kw}&f=_all&p=1`；行 = `div.mi_ne_kd > ul > li`。
//! 5. `playerContent`：抓 `{base}v_play/{id}.html`，依次取
//!    ① `="(.*?)";var` 密文 base64、② `parse\("(.*?)"\);var iv` 的 key、
//!    ③ `iv=md5\.enc\.Utf8\.parse\((.*?)\);var decrypted` 的 iv，
//!    再做 AES/CBC/PKCS5 解密（`yq.N`），从明文里取 `url: "(.*?)"` 作为直链。
//!
//! 与 Android 实现的有意偏差（均在文档 §129.4 记录）：
//! - 上游 `https://www.bttwo.net/` 已不可达（`fetch failed`），因此只能 mock 验收；
//! - key/iv 捕获串做「去引号 + hex/base64/md5」归一化（原类直接 `getBytes()`，
//!   但真实页面模板可能给出带引号或 32 位 hex 的 iv，这里做防御式兼容）；
//! - 详情里「上映：」映射到 `vod_remarks`（原类把它写进备注位）；
//! - `palyUrl` 保留原类拼写且恒为空串（原类该字段永远为空）。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};
use super::super::appto::aes_cbc_decrypt;

pub const API: &str = "csp_Bttwoo";

const BASE: &str = "https://www.bttwo.net/";
const UA: &str = "Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/100.0.4896.127 Safari/537.36";
const UA_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.114 Safari/537.36";
/// 原类播放输出里的 UA 带一个前导空格，这里保持原样。
const UA_PLAY: &str = " Mozilla/5.0 (Windows NT 6.1; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/100.0.4896.127 Safari/537.36";

const PAGE_LIMIT: u32 = 25;

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 选择器
 * ============================================================ */

fn sel(expr: &'static str) -> &'static Selector {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static Selector>>> =
        OnceLock::new();
    let map = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = map.lock().expect("选择器缓存");
    if let Some(found) = guard.get(expr) {
        return found;
    }
    let parsed: &'static Selector = Box::leak(Box::new(
        Selector::parse(expr).unwrap_or_else(|_| panic!("非法选择器: {expr}")),
    ));
    guard.insert(expr, parsed);
    parsed
}

fn nav_sel() -> &'static Selector {
    sel("ul.navlist > li.menu-item-type-custom > a")
}
fn home_row_sel() -> &'static Selector {
    sel("div.leibox > ul > li")
}
fn row_sel() -> &'static Selector {
    sel("div.bt_img > ul > li")
}
fn item_anchor_sel() -> &'static Selector {
    sel("h3.dytit > a")
}
fn item_cover_sel() -> &'static Selector {
    sel("a > img")
}
fn item_remark_sel() -> &'static Selector {
    sel("div.jidi > span")
}
fn pager_sel() -> &'static Selector {
    sel("div.pagenavi_txt > a.extend")
}
fn title_sel() -> &'static Selector {
    sel("div.moviedteail_tt > h1")
}
fn detail_cover_sel() -> &'static Selector {
    sel("div.dyimg > img")
}
fn desc_meta_sel() -> &'static Selector {
    sel("meta[name=description]")
}
fn info_li_sel() -> &'static Selector {
    sel("ul.moviedteail_list > li")
}
fn play_name_sel() -> &'static Selector {
    sel("div.mi_paly_box > div > div.ypxingq_t")
}
fn play_box_sel() -> &'static Selector {
    sel("div.paly_list_btn")
}
fn play_link_sel() -> &'static Selector {
    sel("a")
}
fn search_row_sel() -> &'static Selector {
    sel("div.mi_ne_kd > ul > li")
}
fn search_img_sel() -> &'static Selector {
    sel("img")
}
fn search_anchor_sel() -> &'static Selector {
    sel("a")
}

/* ============================================================
 * 正则
 * ============================================================ */

fn movie_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"movie/(\d+)\.html").expect("详情 id 正则"))
}

fn play_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"v_play/(\S+)\.html").expect("播放 id 正则"))
}

fn page_num_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d+)").expect("页码正则"))
}

fn cipher_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"="(.*?)";var"#).expect("密文正则"))
}
fn key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"parse\("(.*?)"\);var iv"#).expect("key 正则"))
}
fn iv_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"iv=md5\.enc\.Utf8\.parse\((.*?)\);var decrypted").expect("iv 正则"))
}
fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"url: "(.*?)""#).expect("真实 url 正则"))
}

/* ============================================================
 * 工具
 * ============================================================ */

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn attr_of(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(|s| s.trim().to_string())
}

fn headers(referer: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), referer.to_string());
    h.insert("Accept".to_string(), "*/*".to_string());
    h
}

fn search_headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA_MAC.to_string());
    h.insert("Referer".to_string(), BASE.to_string());
    h
}

fn absolute(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("https:{}", href)
    } else if href.starts_with('/') {
        html_rule::join_url(base, href)
    } else {
        html_rule::join_url(&format!("{}/", base.trim_end_matches('/')), href)
    }
}

fn fetch_html(cfg: &TvConfig, url: &str, hdrs: &HashMap<String, String>) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, hdrs, cfg.http.retry)
}

/// 基址：优先取 ext 里的 http URL（为可测试性新增，原类无 ext 覆盖），否则用内置域名。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    for seg in ext.split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return format!("{}/", seg.trim_end_matches('/'));
        }
    }
    BASE.to_string()
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/* ============================================================
 * 首页 / 分类 / 列表
 * ============================================================ */

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base_full = base_of(site);
    let base = base_full.trim_end_matches('/');
    let html = fetch_html(cfg, &base_full, &headers(&base_full))?;
    let cats = parse_home_categories(&html, base);
    if cats.is_empty() {
        return Err(parse_err(site, "BT之家首页未解析出分类（站点改版或不可达）"));
    }
    Ok(cats)
}

pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base_full = base_of(site);
    let base = base_full.trim_end_matches('/');
    let html = fetch_html(cfg, &base_full, &headers(&base_full))?;
    Ok(parse_home_rows(&html, base))
}

/// 首页分类 + 首页推荐一起解析（`homeContent(boolean)` 的等价物）。
pub fn home_content(cfg: &TvConfig, site: &Site) -> Result<Value> {
    let base_full = base_of(site);
    let base = base_full.trim_end_matches('/');
    let html = fetch_html(cfg, &base_full, &headers(&base_full))?;
    let cats = parse_home_categories(&html, base);
    let items = parse_home_rows(&html, base);
    if cats.is_empty() && items.is_empty() {
        return Err(parse_err(
            site,
            "BT之家首页未解析出分类或推荐列表（站点改版或不可达）",
        ));
    }
    Ok(json!({
        "class": cats.iter().map(|c| json!({"type_id": c.type_id, "type_name": c.type_name})).collect::<Vec<_>>(),
        "filters": {},
        "list": items.iter().map(item_json).collect::<Vec<_>>(),
    }))
}

fn item_json(item: &VideoItem) -> Value {
    json!({
        "vod_id": item.id,
        "vod_name": item.name,
        "vod_pic": item.cover,
        "vod_remarks": item.remark,
    })
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base_full = base_of(site);
    let url = list_url(&base_full, cat, page);
    let html = fetch_html(cfg, &url, &headers(&base_full))?;
    let base = base_full.trim_end_matches('/');
    let (items, count) = parse_list_page(&html, base);
    if items.is_empty() && count == 0 {
        return Err(parse_err(
            site,
            format!("BT之家分类页未解析出条目：{url}"),
        ));
    }
    Ok(items)
}

pub fn videos_page(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Value> {
    let base_full = base_of(site);
    let url = list_url(&base_full, cat, page);
    let html = fetch_html(cfg, &url, &headers(&base_full))?;
    let base = base_full.trim_end_matches('/');
    let (items, count) = parse_list_page(&html, base);
    if items.is_empty() && count == 0 {
        return Err(parse_err(
            site,
            format!("BT之家分类页未解析出条目：{url}"),
        ));
    }
    let total = if items.is_empty() {
        0
    } else if items.len() > 1 {
        items.len() * PAGE_LIMIT as usize
    } else {
        items.len()
    };
    Ok(json!({
        "page": page,
        "pageCount": count.max(1),
        "limit": PAGE_LIMIT,
        "total": total,
        "list": items.iter().map(item_json).collect::<Vec<_>>(),
    }))
}

fn list_url(base: &str, cat: &str, page: u32) -> String {
    let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
    format!("{}{}/page/{}", base, cat.trim_matches('/'), page)
}

pub fn parse_home_categories(html: &str, base: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for a in doc.select(nav_sel()) {
        let name = text_of(&a);
        if name.is_empty() {
            continue;
        }
        let Some(href) = attr_of(&a, "href") else {
            continue;
        };
        let href = absolute(base, &href);
        let mut id = href.trim_end_matches('/').to_string();
        if let Some(stripped) = id.strip_prefix(base) {
            id = stripped.trim_matches('/').to_string();
        }
        if id.is_empty() || id.contains("://") {
            continue;
        }
        out.push(Category {
            type_id: id,
            type_name: name,
        });
    }
    out
}

pub fn parse_home_rows(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(home_row_sel()) {
        if let Some(item) = row_item(&row, base) {
            out.push(item);
        }
    }
    out
}

pub fn parse_list_page(html: &str, base: &str) -> (Vec<VideoItem>, u32) {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(row_sel()) {
        if let Some(item) = row_item(&row, base) {
            out.push(item);
        }
    }
    let count = doc
        .select(pager_sel())
        .next()
        .and_then(|a| attr_of(&a, "href"))
        .map(|href| absolute(base, &href))
        .and_then(|url| page_num_re().captures_iter(&url).last().map(|c| c[1].to_string()))
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0);
    (out, count)
}

fn row_item(row: &ElementRef<'_>, base: &str) -> Option<VideoItem> {
    let a = row.select(item_anchor_sel()).next()?;
    let href = attr_of(&a, "href")?;
    let url = absolute(base, &href);
    let id = movie_id_re()
        .captures(&url)
        .map(|c| c[1].to_string())
        .or_else(|| play_id_re().captures(&url).map(|c| c[1].to_string()))?;
    let name = text_of(&a);
    let cover = row
        .select(item_cover_sel())
        .next()
        .and_then(|img| attr_of(&img, "data-original").or_else(|| attr_of(&img, "src")))
        .filter(|s| !s.is_empty())
        .map(|s| absolute(base, &s));
    let remark = row
        .select(item_remark_sel())
        .next()
        .map(|el| text_of(&el))
        .filter(|s| !s.is_empty());
    Some(VideoItem {
        id,
        name,
        cover,
        remark,
    })
}

/* ============================================================
 * 详情
 * ============================================================ */

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = detail_url(&base, id);
    let html = fetch_html(cfg, &url, &headers(&url))?;
    parse_detail(site, id, &base, &html)
}

fn detail_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        return id.to_string();
    }
    let cleaned = id
        .trim_start_matches('/')
        .trim_end_matches("/")
        .trim_end_matches(".html")
        .trim_end_matches('/')
        .to_string();
    let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
    format!("{}movie/{cleaned}.html", base)
}

pub fn parse_detail(site: &Site, id: &str, base: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(title_sel())
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    let cover = doc
        .select(detail_cover_sel())
        .next()
        .and_then(|el| attr_of(&el, "src").or_else(|| attr_of(&el, "data-original")))
        .filter(|s| !s.is_empty())
        .map(|s| absolute(base, &s));
    let desc = doc
        .select(desc_meta_sel())
        .next()
        .and_then(|el| attr_of(&el, "content"))
        .or_else(|| {
            doc.select(sel("div.moviedteail_list")).next().map(|el| text_of(&el))
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut meta = DetailMeta::default();
    for li in doc.select(info_li_sel()) {
        meta.apply(&text_of(&li));
    }

    let groups = parse_play_groups(&doc, base);

    if name.is_empty() && groups.is_empty() {
        return Err(parse_err(
            site,
            format!("BT之家详情页缺少标题与播放线路（{base}movie/{id}.html 可能已改版）"),
        ));
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year: meta.year,
        area: meta.area,
        director: meta.director,
        actor: meta.actor,
        episodes: groups,
    })
}

#[derive(Default)]
struct DetailMeta {
    year: Option<String>,
    area: Option<String>,
    director: Option<String>,
    actor: Option<String>,
}

impl DetailMeta {
    fn apply(&mut self, raw: &str) {
        let flat = raw
            .replace('\u{a0}', " ")
            .replace('\u{3000}', " ")
            .replace('<', " ")
            .replace('>', " ");
        for part in flat.split(['\n', '\r']) {
            let text = part.trim();
            if text.is_empty() {
                continue;
            }
            if let Some((label, value)) = split_label(text) {
                let value = value.trim().to_string();
                if value.is_empty() {
                    continue;
                }
                match label.as_str() {
                    "年份" | "上映" if self.year.is_none() => self.year = Some(value),
                    "地区" => self.area = Some(value),
                    "导演" => self.director = Some(value),
                    "主演" => self.actor = Some(value),
                    _ => {}
                }
            }
        }
    }
}

fn split_label(text: &str) -> Option<(String, String)> {
    for sep in ['：', ':'] {
        if let Some((label, value)) = text.split_once(sep) {
            let label = label.trim().trim_start_matches("类型").trim_start_matches(' ').trim_start_matches('\u{a0}').to_string();
            // 多标签挤在一个 li 里（"类型：剧情 地区：大陆"）时取第一个标签的值。
            let value = value.split(['　', '\u{a0}']).next().unwrap_or(value).to_string();
            return Some((label, value));
        }
    }
    None
}

fn parse_play_groups(doc: &Html, base: &str) -> Vec<EpisodeGroup> {
    let names: Vec<String> = doc
        .select(play_name_sel())
        .map(|el| text_of(&el))
        .filter(|s| !s.is_empty())
        .collect();
    let boxes: Vec<_> = doc.select(play_box_sel()).collect();
    let mut groups = Vec::new();
    for (idx, name) in names.iter().enumerate() {
        let Some(box_el) = boxes.get(idx).or_else(|| boxes.last()) else {
            break;
        };
        let mut items = Vec::new();
        for a in box_el.select(play_link_sel()) {
            let Some(href) = attr_of(&a, "href") else {
                continue;
            };
            let url = absolute(base, &href);
            let id = play_id_re()
                .captures(&url)
                .map(|c| c[1].to_string())
                .unwrap_or_else(|| url.clone());
            let title = text_of(&a);
            let title = if title.is_empty() {
                format!("第{}集", items.len() + 1)
            } else {
                title
            };
            items.push(Episode { title, url: id });
            if items.len() >= MAX_EPISODES {
                break;
            }
        }
        if !items.is_empty() {
            groups.push(EpisodeGroup {
                group: name.clone(),
                items,
            });
        }
    }
    groups
}

const MAX_EPISODES: usize = 500;

/* ============================================================
 * 搜索
 * ============================================================ */

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let encoded = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let base = base_of(site);
    let url = format!("{}xssearch?q={encoded}&f=_all&p=1", base);
    let html = fetch_html(cfg, &url, &search_headers())?;
    if html.contains("没有找到您想要的结果") {
        return Ok(Vec::new());
    }
    Ok(parse_search(&html, base.trim_end_matches('/')))
}

pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(search_row_sel()) {
        let Some(a) = row.select(search_anchor_sel()).next() else {
            continue;
        };
        let Some(href) = attr_of(&a, "href") else {
            continue;
        };
        let url = absolute(base, &href);
        let Some(id) = movie_id_re()
            .captures(&url)
            .map(|c| c[1].to_string())
            .or_else(|| play_id_re().captures(&url).map(|c| c[1].to_string()))
        else {
            continue;
        };
        let img = row.select(search_img_sel()).next();
        let name = img
            .as_ref()
            .and_then(|el| attr_of(el, "alt"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| text_of(&a));
        let cover = img
            .as_ref()
            .and_then(|el| attr_of(el, "data-original").or_else(|| attr_of(el, "src")))
            .filter(|s| !s.is_empty())
            .map(|s| absolute(base, &s));
        let remark = row
            .select(item_remark_sel())
            .next()
            .map(|el| text_of(&el))
            .filter(|s| !s.is_empty());
        if name.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

/* ============================================================
 * 播放
 * ============================================================ */

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let url = play_page_url(&base, id);
    let html = fetch_html(cfg, &url, &headers(&url))?;
    let direct = decrypt_player(&html).ok_or_else(|| {
        host_err(
            site,
            format!(
                "BT之家播放页未找到 dncry 加密串或解密失败（{url}）；该站点播放依赖 AES-CBC 三段脚本"
            ),
        )
    })?;
    Ok(json!({
        "url": direct,
        "header": { "User-Agent": UA_PLAY },
        "parse": "0",
        "palyUrl": "",
    }))
}

fn play_page_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        return id.to_string();
    }
    let cleaned = id
        .trim_start_matches('/')
        .trim_end_matches("/")
        .trim_end_matches(".html")
        .trim_end_matches('/')
        .to_string();
    let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
    format!("{base}v_play/{cleaned}.html")
}

/// 解析播放页里的 dncry 三段并解密出真实地址；失败返回 `None`。
pub fn decrypt_player(html: &str) -> Option<String> {
    let cipher_b64 = cipher_re().captures(html)?.get(1)?.as_str().trim().to_string();
    let key_raw = key_re().captures(html)?.get(1)?.as_str().to_string();
    let iv_raw = iv_re().captures(html)?.get(1)?.as_str().to_string();

    let ciphertext = base64_decode(&cipher_b64)?;
    let key = normalize_bytes(&key_raw)?;
    let iv = normalize_bytes(&iv_raw)?;
    if key.len() != 16 || iv.len() != 16 {
        return None;
    }
    let plain = aes_cbc_decrypt(&key, &iv, &ciphertext).ok()?;
    let text = String::from_utf8_lossy(&plain).to_string();
    let direct = url_re().captures(&text)?.get(1)?.as_str().trim().to_string();
    if direct.is_empty() {
        None
    } else {
        Some(direct)
    }
}

/// base64 解码：兼容标准与 URL-safe 两种字母表、自动补 `=`。
fn base64_decode(raw: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let trimmed: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
        .collect();
    if trimmed.is_empty() {
        return None;
    }
    let mut padded = trimmed.clone();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE
                .decode(padded.as_bytes())
                .ok()
        })
}

/// key/iv 归一化：去引号后按 16/24/32 字节、32 位 hex、base64、md5 依次尝试。
fn normalize_bytes(raw: &str) -> Option<Vec<u8>> {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    // 32 位 hex 优先按 hex 解（16 字节）；其余长度 16/24/32 直接当原始 key。
    if trimmed.len() == 32 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Ok(decoded) = hex::decode(trimmed) {
            if decoded.len() == 16 {
                return Some(decoded);
            }
        }
    }
    match bytes.len() {
        16 | 24 | 32 => return Some(bytes.to_vec()),
        _ => {}
    }
    if let Some(decoded) = base64_decode(trimmed) {
        if matches!(decoded.len(), 16 | 24 | 32) {
            return Some(decoded);
        }
    }
    // md5(raw) 兜底：某些模板把 iv 写成 `md5.enc.Utf8.parse(<32位hex>)`。
    Some(md5::compute(bytes).0.to_vec())
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Bttwoo"));
        assert!(handles(" CSP_BTTWOO "));
        assert!(!handles("csp_BttwooGuard"));
    }

    #[test]
    fn builds_detail_and_play_urls() {
        assert_eq!(
            detail_url("https://www.bttwo.net/", "123"),
            "https://www.bttwo.net/movie/123.html"
        );
        assert_eq!(
            detail_url("https://x.test/", "https://x.test/movie/9.html"),
            "https://x.test/movie/9.html"
        );
        assert_eq!(
            play_page_url("https://www.bttwo.net/", "1-2-3"),
            "https://www.bttwo.net/v_play/1-2-3.html"
        );
    }

    #[test]
    fn parses_home_categories_from_nav() {
        let html = r#"
        <ul class="navlist">
          <li class="menu-item-type-custom"><a href="/zuixin/">最新电影</a></li>
          <li class="menu-item-type-custom"><a href="https://www.bttwo.net/guochanju/">国产剧</a></li>
        </ul>"#;
        let cats = parse_home_categories(html, "https://www.bttwo.net");
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "zuixin");
        assert_eq!(cats[1].type_name, "国产剧");
    }

    #[test]
    fn parses_home_rows() {
        let html = r#"
        <div class="leibox"><ul>
          <li><h3 class="dytit"><a href="/movie/12.html">示例一</a></h3>
              <a href="/movie/12.html"><img data-original="/p/12.jpg"></a>
              <div class="jidi"><span> HD </span></div></li>
        </ul></div>"#;
        let rows = parse_home_rows(html, "https://www.bttwo.net");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "12");
        assert_eq!(rows[0].name, "示例一");
        assert_eq!(rows[0].cover.as_deref(), Some("https://www.bttwo.net/p/12.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("HD"));
    }

    #[test]
    fn parses_category_rows_and_page_count() {
        let html = r#"
        <div class="bt_img"><ul>
          <li><h3 class="dytit"><a href="/movie/7.html">A</a></h3>
              <a href="/movie/7.html"><img data-original="https://img/a.jpg"></a>
              <div class="jidi"><span>更新至3集</span></div></li>
        </ul></div>
        <div class="pagenavi_txt"><a class="extend" href="https://www.bttwo.net/zuixin/page/9">下一页</a></div>"#;
        let (rows, count) = parse_list_page(html, "https://www.bttwo.net");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "7");
        assert_eq!(rows[0].remark.as_deref(), Some("更新至3集"));
        assert_eq!(count, 9);
    }

    #[test]
    fn parses_detail_fields_and_groups() {
        let html = r#"
        <div class="moviedteail_tt"><h1>示例电影</h1></div>
        <div class="dyimg"><img src="/p/1.jpg"></div>
        <meta name="description" content="一段简介">
        <ul class="moviedteail_list">
          <li>类型：剧情</li><li>年份：2020</li><li>地区：大陆</li>
          <li>导演：张三</li><li>主演：李四</li>
        </ul>
        <div class="mi_paly_box"><div><div class="ypxingq_t">线路一</div></div></div>
        <div class="paly_list_btn">
          <a href="/v_play/1-1-1.html">第1集</a>
          <a href="/v_play/1-1-2.html">第2集</a>
        </div>"#;
        let d = parse_detail(&site(), "1", "https://www.bttwo.net", html).expect("详情");
        assert_eq!(d.name, "示例电影");
        assert_eq!(d.year.as_deref(), Some("2020"));
        assert_eq!(d.area.as_deref(), Some("大陆"));
        assert_eq!(d.director.as_deref(), Some("张三"));
        assert_eq!(d.actor.as_deref(), Some("李四"));
        assert_eq!(d.desc.as_deref(), Some("一段简介"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "线路一");
        assert_eq!(d.episodes[0].items[1].url, "1-1-2");
    }

    #[test]
    fn detail_without_title_or_groups_reports_error() {
        let err = parse_detail(&site(), "9", "https://www.bttwo.net", "<html></html>")
            .expect_err("应报错");
        assert!(format!("{err:?}").contains("catvod.result.parse"));
    }

    #[test]
    fn parses_search_rows() {
        let html = r#"
        <div class="mi_ne_kd"><ul>
          <li><a href="/movie/31.html"><img data-original="/p/31.jpg" alt="搜索片名"></a>
              <div class="jidi"><span>HD</span></div></li>
        </ul></div>"#;
        let rows = parse_search(html, "https://www.bttwo.net");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "31");
        assert_eq!(rows[0].name, "搜索片名");
        assert_eq!(rows[0].cover.as_deref(), Some("https://www.bttwo.net/p/31.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("HD"));
    }

    #[test]
    fn normalizes_key_and_iv_forms() {
        assert_eq!(normalize_bytes("0123456789abcdef").unwrap().len(), 16);
        assert_eq!(
            normalize_bytes("\"00112233445566778899aabbccddeeff\"").unwrap(),
            hex::decode("00112233445566778899aabbccddeeff").unwrap()
        );
        assert_eq!(normalize_bytes("not-16-bytes!!").unwrap().len(), 16);
        assert!(normalize_bytes("   ").is_none());
    }

    #[test]
    fn base64_decodes_padded_and_url_safe() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
    }

    #[test]
    fn decrypts_player_script() {
        // 用与线上同构的三段脚本：密文=hex 串的 base64，key/iv 为 16 字符。
        let key = b"1234567890abcdef";
        let iv = b"abcdef1234567890";
        let plain = r#"var url: "https://cdn.test/movie.m3u8";"#;
        let cipher = {
            use aes::Aes128;
            use cbc::Encryptor;
            use cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
            let mut buf = vec![0u8; plain.len() + 16];
            buf[..plain.len()].copy_from_slice(plain.as_bytes());
            let encrypted = Encryptor::<Aes128>::new_from_slices(key, iv)
                .expect("key/iv")
                .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
                .expect("加密")
                .to_vec();
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(encrypted)
        };
        let html = format!(
            r#"<script>var dncry="{}";var key=parse("{}");var iv=md5.enc.Utf8.parse("{}");var decrypted=1;</script>"#,
            cipher,
            std::str::from_utf8(key).unwrap(),
            std::str::from_utf8(iv).unwrap()
        );
        assert_eq!(
            decrypt_player(&html).as_deref(),
            Some("https://cdn.test/movie.m3u8")
        );
        assert!(decrypt_player("<html></html>").is_none());
    }

    #[test]
    fn list_url_joins_type_and_page() {
        assert_eq!(
            list_url("https://www.bttwo.net/", "zuixin", 3),
            "https://www.bttwo.net/zuixin/page/3"
        );
        assert_eq!(
            list_url("https://www.bttwo.net", "/zuixin/", 1),
            "https://www.bttwo.net/zuixin/page/1"
        );
    }
}
