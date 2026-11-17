//! `csp_Xinsj`（✨新视觉）原生实现：MacCMS v10 模板站点。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.Xinsj`，站点 1 个（`https://www.6080yy3.com/`）。
//! * 证据：`_rea/tmp/xinsj/out-Xinsj/Xinsj.decoded.java`（dex2jar + Vineflower，
//!   约 3.5k 行、方法 27 个；`short[]` 字符数组 941 个已整体解码）；
//!   画像 `crates/vbox_core/src/spider/catvod/profiles.json` 中 `csp_Xinsj`
//!   原为 `unported` / `site_dead`（上游 ENOTFOUND），本轮转 native。
//!
//! 协议（原类逐段还原）：
//! * 基址硬编码 `https://www.6080yy3.com/`（原类 `a = "https://www.6080yy3.com/"`），
//!   列表 URL 直接拼接，因此真实地址是 `https://www.6080yy3.com//vodshow/…`
//!   （双斜杠是原类行为，保留）。
//! * `homeContent`：`ul.nav-menu-items > li.nav-menu-item > a`；标题含
//!   电影/电视剧/动漫/综艺 的菜单项收为分类（id 取 `/vodtype/(\d+).html`，
//!   退化为 `/vodshow/(\S+).html` 的第一段）；首页推荐 `div.module-items div.module-item`。
//! * `categoryContent`：`/vodshow/{type_id}----------{page}---.html`；
//!   列表行 `div.module-item-cover > div.module-item-pic > a`（链接/标题）+
//!   `img[data-src]`（封面）+ `div.module-item-text`（备注）；页数取 `div[id=page]`。
//! * `detailContent`：`/video/{id}.html`；标题 `div.video-info-header > h1.page-title`，
//!   封面 `div.module-item-pic > img[data-src]`，元数据 `div.video-info-main
//!   span.video-info-itemtitle` 按 `分类：/上映：/地区：/更新：/主演：/导演：` 切。
//!   线路名 `div.module-tab-content div span`，剧集 `div.module-play-list` 中 `a`；
//!   **含「夸克」的线路原类整条跳过**（这里保持一致）。
//! * `searchContent`：`/vod-s/{URLEncoder.encode(kw)}-------------.html`；
//!   行 `div.module-search-item`：标题 `img.lazyload[alt]`、封面 `img.lazyload[data-src]`、
//!   备注 `div.video-info-header a` 文本、id 取该 `a[href]` 的 `/video/(\d+).html`。
//! * `playerContent`：`/vplay/{id}.html`，取 `script` 中 `var player_` 的 JSON；
//!   `url = MacPlayerConfig.server_list[from].parse + player.url`（原类拼接语义），
//!   返回 `{"parse":0,"playUrl":id,"url":…}`。
//!
//! 有意偏差（桌面端能力边界 / 原类自身 bug）：
//! 1. 站点已 ENOTFOUND，本家族只做离线 fixture + mock 验收；`ext` 首个 http(s) 段可
//!    覆盖基址（仅测试/镜像钩子，原类没有该行为）。
//! 2. `playerconfig.js` 不可达或 `server_list[from].parse` 缺失时按空前缀处理，
//!    最终直链取 `player_aaaa.url`；`encrypt=1`（MacCMS 加密播放串）不还原，返回
//!    `catvod.play.parse` 错误并记录在文档。
//! 3. 原类在 `init` 里注册了 `r0` 时间戳/`Referer` 等请求头，桌面端统一由
//!    `crate::http` 构造（UA/Referer 一致，其余头省略）。
//! 4. 剧集列表按「线路名下标」与 `div.module-play-list` 一一配对；若站点模板把
//!    播放列表拆成别的结构，按行回退为全部 `div.module-play-list a` 平铺（记为偏差）。
//! 5. 线路名含「夸克」不展示（原类行为），因此该线路的夸克分享无法通过本家族播放。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_Xinsj";

/// 原类硬编码基址（带尾斜杠，列表 URL 会拼出双斜杠，保留原行为）。
const DEFAULT_BASE: &str = "https://www.6080yy3.com/";

/// 原类 `t0` 的 UA（Chrome/102）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.62 Safari/537.36";

/// 播放器配置脚本路径（原类 `N()` 里请求的地址）。
const PLAYER_CFG_PATH: &str = "/static/js/playerconfig.js";

/// 原类跳过的线路关键字。
const SKIP_LINE: &str = "夸克";

/// 首页导航里当作分类的菜单文本。
const HOME_CATEGORIES: [&str; 4] = ["电影", "电视剧", "动漫", "综艺"];

/* ============================================================
 * 选择器 / 正则（OnceLock 缓存）
 * ============================================================ */

fn sel(cache: &'static OnceLock<Selector>, css: &str) -> &'static Selector {
    cache.get_or_init(|| Selector::parse(css).expect("静态选择器必须合法"))
}

fn nav_link_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "ul.nav-menu-items > li.nav-menu-item > a")
}

fn home_item_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-items div.module-item")
}

fn home_title_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-item-titlebox a")
}

fn list_item_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-item-cover")
}

fn list_pic_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-item-pic > a")
}

fn list_desc_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-item-text")
}

fn page_box_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div[id=page]")
}

fn detail_cover_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-item-pic > img")
}

fn detail_title_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.video-info-header > h1.page-title")
}

fn detail_meta_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.video-info-main span.video-info-itemtitle")
}

fn detail_content_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.video-info-content span")
}

fn tab_name_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-tab-content div span")
}

fn play_list_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-play-list")
}

fn play_link_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "a")
}

fn search_item_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.module-search-item")
}

fn search_img_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "img.lazyload")
}

fn search_link_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "div.video-info-header a")
}

fn script_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "script")
}

fn type_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"/vodtype/(\d+)\.html").expect("正则合法"))
}

fn video_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"/video/(\d+)\.html").expect("正则合法"))
}

/// 播放页 id 形如 `123-1-5`（`/vplay/123-1-5.html`）。
fn play_id_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"/vplay/(\d+-\d+-\d+)\.html").expect("正则合法"))
}

fn page_num_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"-+(\d+)---\.html").expect("正则合法"))
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

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

/// 基址：默认硬编码常量；`ext` 首个 http(s) 段可覆盖（仅测试/镜像钩子，见偏差 1）。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    for seg in ext.split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            let mut base = seg.trim_end_matches('/').to_string();
            base.push('/');
            return base;
        }
    }
    DEFAULT_BASE.to_string()
}

/// 相对/绝对链接补全。
fn join_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    let base = base.trim_end_matches('/');
    if href.starts_with('/') {
        format!("{base}{href}")
    } else {
        format!("{base}/{href}")
    }
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn fetch(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(), 2)
}

/// 分类 id 归一：优先 `/vodtype/{数字}.html`，否则 `/vodshow/{段}.html` 的第一段。
fn cat_id_of(href: &str) -> Option<String> {
    if let Some(c) = type_re().captures(href) {
        return c.get(1).map(|m| m.as_str().to_string());
    }
    if let Some(pos) = href.find("/vodshow/") {
        let rest = &href[pos + "/vodshow/".len()..];
        let id = rest.split(['-', '.', '/', '?']).next().unwrap_or("").trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

/// 列表页 URL（原类拼 `https://www.6080yy3.com//vodshow/…` 的双斜杠格式）。
pub fn list_url(base: &str, type_id: &str, page: u32) -> String {
    format!("{base}/vodshow/{type_id}----------{page}---.html")
}

/// 搜索 URL（原类 13 个 `-`）。
fn search_url(base: &str, keyword: &str) -> String {
    let enc = urlencode(keyword);
    format!("{base}/vod-s/{enc}-------------.html")
}

fn detail_url(base: &str, id: &str) -> String {
    format!("{base}/video/{id}.html")
}

fn play_url(base: &str, id: &str) -> String {
    format!("{base}/vplay/{id}.html")
}

fn img_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    sel(&S, "img")
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/* ============================================================
 * 解析（纯函数，便于单测）
 * ============================================================ */

/// 首页导航 → 分类。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for a in doc.select(nav_link_sel()) {
        let name = text_of(&a);
        if !HOME_CATEGORIES.iter().any(|k| name.contains(k)) {
            continue;
        }
        let Some(href) = attr_of(&a, "href") else { continue };
        let Some(id) = cat_id_of(&href) else { continue };
        if !seen.insert(id.clone()) {
            continue;
        }
        out.push(Category {
            type_id: id,
            type_name: name,
        });
    }
    out
}

/// 首页推荐位。
pub fn parse_home_items(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for item in doc.select(home_item_sel()) {
        let Some(a) = item.select(home_title_sel()).next() else { continue };
        let name = text_of(&a);
        if name.is_empty() {
            continue;
        }
        let Some(href) = attr_of(&a, "href") else { continue };
        let Some(id) = video_id_of(&href) else { continue };
        let cover = item
            .select(img_sel())
            .next()
            .and_then(|img| attr_of(&img, "data-src").or_else(|| attr_of(&img, "src")))
            .map(|src| join_url(base, &src))
            .filter(|src| !src.is_empty());
        let remark = item
            .select(list_desc_sel())
            .next()
            .map(|el| text_of(&el))
            .filter(|t| !t.is_empty());
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

fn video_id_of(href: &str) -> Option<String> {
    video_re()
        .captures(href)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// 分类列表页：返回条目与页数。
pub fn parse_list(html: &str, base: &str) -> (Vec<VideoItem>, u32) {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for cover in doc.select(list_item_sel()) {
        let Some(a) = cover.select(list_pic_sel()).next() else { continue };
        let name = text_of(&a);
        if name.is_empty() {
            continue;
        }
        let Some(href) = attr_of(&a, "href").or_else(|| attr_of(&a, "data-original")) else {
            continue;
        };
        let Some(id) = video_id_of(&href) else { continue };
        let pic = cover
            .select(img_sel())
            .next()
            .and_then(|img| attr_of(&img, "data-src").or_else(|| attr_of(&img, "src")))
            .map(|src| join_url(base, &src))
            .filter(|src| !src.is_empty());
        let remark = cover
            .select(list_desc_sel())
            .next()
            .map(|el| text_of(&el))
            .filter(|t| !t.is_empty());
        out.push(VideoItem {
            id,
            name,
            cover: pic,
            remark,
        });
    }
    (out, page_count_of(&doc))
}

/// 页数：`div[id=page]` 里所有 `a[href]` 的 `{n}.html` 最大值。
fn page_count_of(doc: &Html) -> u32 {
    let mut max = 1u32;
    for a in doc.select(page_box_sel()).flat_map(|box_| box_.select(play_link_sel())) {
        if let Some(href) = attr_of(&a, "href") {
            for c in page_num_re().captures_iter(&href) {
                if let Some(n) = c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
                    max = max.max(n);
                }
            }
        }
    }
    max
}

/// 详情页。
pub fn parse_detail(html: &str, id: &str, base: &str, site: &Site) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(detail_title_sel())
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();
    let cover = doc
        .select(detail_cover_sel())
        .next()
        .and_then(|img| attr_of(&img, "data-src").or_else(|| attr_of(&img, "src")))
        .map(|src| join_url(base, &src))
        .filter(|src| !src.is_empty());

    // 元数据：`分类：/上映：/地区：/更新：/主演：/导演：` 前缀切分。
    let mut type_name = String::new();
    let mut year = String::new();
    let mut area = String::new();
    let mut remark = String::new();
    let mut actor = String::new();
    let mut director = String::new();
    for span in doc.select(detail_meta_sel()) {
        let text = text_of(&span);
        let pick = |prefix: &str| text.strip_prefix(prefix).map(|s| s.trim().to_string());
        if let Some(v) = pick("分类：") {
            type_name = v;
        } else if let Some(v) = pick("上映：") {
            year = v;
        } else if let Some(v) = pick("地区：") {
            area = v;
        } else if let Some(v) = pick("更新：") {
            remark = v;
        } else if let Some(v) = pick("主演：") {
            actor = v;
        } else if let Some(v) = pick("导演：") {
            director = v;
        }
    }

    let desc = doc
        .select(detail_content_sel())
        .next()
        .map(|el| text_of(&el))
        .filter(|t| !t.is_empty());

    // 线路与剧集：线路名按顺序与 `div.module-play-list` 配对；含「夸克」的线路跳过。
    let tab_names: Vec<String> = doc.select(tab_name_sel()).map(|el| text_of(&el)).collect();
    let lists: Vec<_> = doc.select(play_list_sel()).collect();
    let mut groups = Vec::new();
    for (idx, tab) in tab_names.iter().enumerate() {
        if tab.is_empty() || tab.contains(SKIP_LINE) {
            continue;
        }
        let mut items = Vec::new();
        if let Some(list) = lists.get(idx) {
            for a in list.select(play_link_sel()) {
                let href = attr_of(&a, "href").unwrap_or_default();
                let Some(pid) = play_id_of(&href) else { continue };
                let title = text_of(&a);
                items.push(Episode {
                    title: if title.is_empty() { pid.clone() } else { title },
                    url: pid,
                });
            }
        }
        if items.is_empty() {
            continue;
        }
        groups.push(EpisodeGroup {
            group: tab.clone(),
            items,
        });
    }
    if groups.is_empty() {
        return Err(parse_err(site, format!("详情页未解析到播放线路（{id}）")));
    }

    // 分类/更新时间不是 VideoDetail 的独立字段，写进 desc 前缀便于前端展示。
    let mut prefix = String::new();
    if !type_name.is_empty() {
        prefix.push_str(&format!("分类：{type_name}\n"));
    }
    if !remark.is_empty() {
        prefix.push_str(&format!("更新：{remark}\n"));
    }
    let desc = if prefix.is_empty() {
        desc
    } else {
        Some(match desc {
            Some(d) => format!("{prefix}{d}"),
            None => prefix,
        })
    };

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year: Some(year).filter(|s| !s.is_empty()),
        area: Some(area).filter(|s| !s.is_empty()),
        director: Some(director).filter(|s| !s.is_empty()),
        actor: Some(actor).filter(|s| !s.is_empty()),
        episodes: groups,
    })

}

fn play_id_of(href: &str) -> Option<String> {
    play_id_re()
        .captures(href)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// 搜索页。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for item in doc.select(search_item_sel()) {
        let img = item.select(search_img_sel()).next();
        let name = img
            .as_ref()
            .and_then(|i| attr_of(i, "alt"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let cover = img
            .as_ref()
            .and_then(|i| attr_of(i, "data-src").or_else(|| attr_of(i, "src")))
            .map(|src| join_url(base, &src))
            .filter(|src| !src.is_empty());
        let a = item.select(search_link_sel()).next();
        let remark = a
            .as_ref()
            .map(|el| text_of(el))
            .filter(|t| !t.is_empty());
        let id = a
            .as_ref()
            .and_then(|el| attr_of(el, "href"))
            .and_then(|href| video_id_of(&href))
            .or_else(|| {
                item.select(play_link_sel())
                    .filter_map(|el| attr_of(&el, "href"))
                    .find_map(|href| video_id_of(&href))
            });
        let Some(id) = id else { continue };
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
 * 播放页 / 播放器配置
 * ============================================================ */

/// 从播放页 `script` 里取 `var player_…` 的 JSON（原类同款：首个 `{` 到最后一个 `}`）。
pub fn parse_player_page(html: &str) -> Option<Value> {
    let doc = Html::parse_document(html);
    for script in doc.select(script_sel()) {
        let text: String = script.text().collect::<String>();
        if !text.contains("var player_") {
            continue;
        }
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end <= start {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&text[start..=end]) {
            return Some(value);
        }
    }
    None
}

/// 从 `playerconfig.js` 里取 `MacPlayerConfig.server_list`（原类 `N()` 的等价物）。
pub fn parse_player_config(js: &str) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    let Some(pos) = js.find("MacPlayerConfig.server_list") else {
        return out;
    };
    let Some(eq) = js[pos..].find('=') else { return out };
    let Some(lbrace) = js[pos + eq..].find('{') else { return out };
    let start = pos + eq + lbrace;
    let bytes = js.as_bytes();
    let mut depth = 0i32;
    let mut end = None;
    let mut in_str = false;
    let mut escaped = false;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        let ch = *b;
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == b'\\' {
                escaped = true;
            } else if ch == b'"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = end else { return out };
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&js[start..end]) {
        out = map.into_iter().collect();
    }
    out
}

/// 播放器配置里 `server_list[from].parse` 的前缀（原类 `var31.getString("parse")`）。
fn parse_prefix(config: &HashMap<String, Value>, from: &str) -> String {
    config
        .get(from)
        .and_then(|v| v.get("parse"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn player_config(cfg: &TvConfig, base: &str, from: &str) -> String {
    let url = format!("{base}{PLAYER_CFG_PATH}");
    match fetch(cfg, &url) {
        Ok(js) => parse_prefix(&parse_player_config(&js), from),
        Err(_) => String::new(),
    }
}

/* ============================================================
 * TVBox 五入口
 * ============================================================ */

/// 首页分类。站点不可达时返回空（上游已 ENOTFOUND，见模块头）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Vec<Category> {
    let base = base_of(site);
    match fetch(cfg, &base) {
        Ok(html) => parse_categories(&html),
        Err(_) => Vec::new(),
    }
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let html = fetch(cfg, &list_url(&base, type_id, page))?;
    let (items, _page_count) = parse_list(&html, &base);
    Ok(items)
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let html = fetch(cfg, &detail_url(&base, id))?;
    parse_detail(&html, id, &base, site)
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let html = fetch(cfg, &search_url(&base, keyword))?;
    Ok(parse_search(&html, &base))
}

/// 播放。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let html = fetch(cfg, &play_url(&base, id))?;
    let data = parse_player_page(&html).ok_or_else(|| {
        parse_err(site, format!("播放页未找到 var player_ 数据（{id} 可能已改版）"))
    })?;
    let url = data
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if url.is_empty() {
        return Err(parse_err(site, format!("播放页 url 为空（{id}）")));
    }
    if data.get("encrypt").and_then(|v| v.as_i64()).unwrap_or(0) == 1 {
        return Err(parse_err(
            site,
            "MacCMS 加密播放串（encrypt=1）未还原，见模块头偏差 2",
        ));
    }
    let from = data.get("from").and_then(|v| v.as_str()).unwrap_or_default();
    let prefix = player_config(cfg, &base, from);
    // 原类语义是 `prefix + url`；只有 prefix 是 http 解析前缀时才拼接（偏差 2）。
    let final_url = if prefix.starts_with("http") && !url.starts_with("http") {
        format!("{prefix}{url}")
    } else {
        url
    };
    Ok(json!({
        "parse": 0,
        "playUrl": id,
        "url": final_url,
        "header": { "User-Agent": UA },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<&str>) -> Site {
        Site {
            key: "xinsj-test".to_string(),
            name: "新视觉".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: ext.map(|e| json!(e)),
            ..Default::default()
        }
    }

    const HOME_HTML: &str = r#"
<ul class="nav-menu-items">
  <li class="nav-menu-item"><a href="/">首页</a></li>
  <li class="nav-menu-item"><a href="/vodtype/1.html">电影</a></li>
  <li class="nav-menu-item"><a href="/vodtype/2.html">电视剧</a></li>
  <li class="nav-menu-item"><a href="/vodtype/3.html">动漫</a></li>
  <li class="nav-menu-item"><a href="/vodtype/4.html">综艺</a></li>
  <li class="nav-menu-item"><a href="/topic.html">专题</a></li>
</ul>
<div class="module-items">
  <div class="module-item">
    <div class="module-item-titlebox"><a href="/video/100.html">测试电影</a></div>
    <div class="module-item-pic"><a href="/video/100.html"><img data-src="/upload/100.jpg"></a></div>
    <div class="module-item-text">更新至01集</div>
  </div>
</div>
"#;

    const LIST_HTML: &str = r#"
<div class="module-items">
  <div class="module-item-cover">
    <div class="module-item-pic"><a href="/video/200.html">列表电影</a><img data-src="/upload/200.jpg"></div>
    <div class="module-item-text">HD</div>
  </div>
</div>
<div id="page">
  <a href="https://www.6080yy3.com//vodshow/1----------2---.html">2</a>
  <a href="https://www.6080yy3.com//vodshow/1----------3---.html">3</a>
</div>
"#;

    const DETAIL_HTML: &str = r#"
<div class="module-item-pic"><img data-src="/upload/d.jpg"></div>
<div class="video-info-header"><h1 class="page-title">测试剧集</h1></div>
<div class="video-info-main">
  <span class="video-info-itemtitle">分类：国产剧</span>
  <span class="video-info-itemtitle">上映：2024</span>
  <span class="video-info-itemtitle">地区：大陆</span>
  <span class="video-info-itemtitle">更新：更新至10集</span>
  <span class="video-info-itemtitle">主演：张三</span>
  <span class="video-info-itemtitle">导演：李四</span>
</div>
<div class="video-info-content"><span>简介文本</span></div>
<div class="module-tab-content"><div><span>线路一</span></div><div><span>夸克</span></div></div>
<div class="module-play-list"><a href="/vplay/300-1-1.html">第01集</a></div>
<div class="module-play-list"><a href="/vplay/300-2-1.html">夸克1</a></div>
"#;

    const SEARCH_HTML: &str = r#"
<div class="module-items">
  <div class="module-search-item">
    <div class="module-item-pic"><a href="/video/400.html"><img class="lazyload" data-src="/upload/400.jpg" alt="搜索电影"></a></div>
    <div class="video-info-header"><a href="/video/400.html">HD</a></div>
  </div>
</div>
"#;

    const PLAYER_HTML: &str = r#"
<script>var player_aaaa={"flag":"lzm3u8","encrypt":0,"url":"https://cdn.example.com/1/index.m3u8","from":"dplayer"};</script>
"#;

    const PLAYER_CFG_JS: &str = r#"
var MacPlayerConfig = {};MacPlayerConfig.server_list={"dplayer":{"parse":"https://jx.example.com/?url=","server":"","path":""},"qq":{"parse":"","server":"https://v.qq.com/"}};MacPlayerConfig.player_list={};
"#;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles(API));
        assert!(handles("csp_xinsj"));
        assert!(!handles("csp_FourKHDR"));
    }

    #[test]
    fn base_or_ext_override_always_has_trailing_slash() {
        assert_eq!(base_of(&site(None)), DEFAULT_BASE);
        assert_eq!(base_of(&site(Some("https://mirror.example/$$$x"))), "https://mirror.example/");
        assert_eq!(base_of(&site(Some("not-a-url"))), DEFAULT_BASE);
    }

    #[test]
    fn list_and_search_url_keep_original_shape() {
        assert_eq!(
            list_url(DEFAULT_BASE, "1", 2),
            "https://www.6080yy3.com//vodshow/1----------2---.html"
        );
        let s = search_url(DEFAULT_BASE, "测试");
        assert!(s.starts_with("https://www.6080yy3.com//vod-s/%E6%B5%8B%E8%AF%95"));
        assert!(s.ends_with("-------------.html"));
    }

    #[test]
    fn categories_from_nav_keep_four() {
        let cats = parse_categories(HOME_HTML);
        let ids: Vec<&str> = cats.iter().map(|c| c.type_id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3", "4"]);
        let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
        assert_eq!(names, vec!["电影", "电视剧", "动漫", "综艺"]);
    }

    #[test]
    fn home_items_parse_title_cover_and_remark() {
        let items = parse_home_items(HOME_HTML, DEFAULT_BASE);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "100");
        assert_eq!(items[0].name, "测试电影");
        assert_eq!(items[0].cover.as_deref(), Some("https://www.6080yy3.com/upload/100.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新至01集"));
    }

    #[test]
    fn list_items_and_pagecount() {
        let (items, pages) = parse_list(LIST_HTML, DEFAULT_BASE);
        assert_eq!(pages, 3);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "200");
        assert_eq!(items[0].name, "列表电影");
        assert_eq!(items[0].remark.as_deref(), Some("HD"));
    }

    #[test]
    fn detail_meta_groups_and_skip_quark_line() {
        let detail = parse_detail(DETAIL_HTML, "300", DEFAULT_BASE, &site(None)).expect("解析成功");
        assert_eq!(detail.name, "测试剧集");
        assert_eq!(detail.year.as_deref(), Some("2024"));
        assert_eq!(detail.area.as_deref(), Some("大陆"));
        assert_eq!(detail.actor.as_deref(), Some("张三"));
        assert_eq!(detail.director.as_deref(), Some("李四"));
        let desc = detail.desc.clone().unwrap_or_default();
        assert!(desc.contains("分类：国产剧"), "desc={desc}");
        assert!(desc.contains("更新：更新至10集"), "desc={desc}");
        assert!(desc.contains("简介文本"), "desc={desc}");
        // 含「夸克」的线路被原类跳过，只剩「线路一」。
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, "线路一");
        assert_eq!(detail.episodes[0].items.len(), 1);
        assert_eq!(detail.episodes[0].items[0].url, "300-1-1");
        assert_eq!(detail.episodes[0].items[0].title, "第01集");
    }

    #[test]
    fn detail_without_playlists_is_parse_error() {
        let err = parse_detail("<html><body>空</body></html>", "1", DEFAULT_BASE, &site(None))
            .expect_err("没有线路必须报错");
        let text = format!("{err}");
        assert!(text.contains("播放线路"), "err={text}");
    }

    #[test]
    fn search_items_parse() {
        let items = parse_search(SEARCH_HTML, DEFAULT_BASE);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "400");
        assert_eq!(items[0].name, "搜索电影");
        assert_eq!(items[0].cover.as_deref(), Some("https://www.6080yy3.com/upload/400.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("HD"));
    }

    #[test]
    fn player_page_json_parse() {
        let value = parse_player_page(PLAYER_HTML).expect("找到 player_ 数据");
        assert_eq!(value.get("from").and_then(|v| v.as_str()), Some("dplayer"));
        assert_eq!(
            value.get("url").and_then(|v| v.as_str()),
            Some("https://cdn.example.com/1/index.m3u8")
        );
        assert_eq!(value.get("encrypt").and_then(|v| v.as_i64()), Some(0));
    }

    #[test]
    fn player_config_prefix_parse() {
        let cfg = parse_player_config(PLAYER_CFG_JS);
        assert_eq!(parse_prefix(&cfg, "dplayer"), "https://jx.example.com/?url=");
        assert_eq!(parse_prefix(&cfg, "qq"), "");
        assert_eq!(parse_prefix(&cfg, "missing"), "");
    }

    #[test]
    fn join_url_handles_absolute_relative_and_protocol_relative() {
        assert_eq!(join_url(DEFAULT_BASE, "https://a/b.jpg"), "https://a/b.jpg");
        assert_eq!(join_url(DEFAULT_BASE, "//cdn/a.jpg"), "https://cdn/a.jpg");
        assert_eq!(join_url(DEFAULT_BASE, "/upload/a.jpg"), "https://www.6080yy3.com/upload/a.jpg");
        assert_eq!(join_url(DEFAULT_BASE, "upload/a.jpg"), "https://www.6080yy3.com/upload/a.jpg");
    }
}
