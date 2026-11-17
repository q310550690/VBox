//! `csp_Libvio` 原生实现（stui MacCMS 模板 + SN 播放源解析表）。
//!
//! 语义来自静态反编译：`_rea/decompiled/88c55af0-fan/deobf/com/github/catvod/spider/Libvio.java`
//! （432 行，0 处 jadx `Method dump skipped`）。该类 `extends Spider`，只依赖
//! 宿主公开契约，因此可以 100% 原生重写：
//!
//! 1. `init(ctx, ext)`：`ext` 即站点基址（例如 `https://www.libvio.pw/`）；
//!    同时把 SN 播放源表与 N 过滤表内嵌为常量（见 `SN_TABLE`）。
//! 2. `homeContent`：抓首页，从 `ul.stui-header__menu li a` 里筛
//!    电影/剧集/动漫/日韩剧/欧美剧（`/type/(\d+).html`）。
//! 3. `categoryContent`：12 段 `-` 连接参数（`[0]=typeId`、`[8]=page`），
//!    URL = `{base}/show/{segments}.html`；行 = `div.stui-vodlist__box`。
//! 4. `detailContent`：`{base}/detail/{id}.html`；元数据从 `p.data` 用
//!    「类型：/地区：/年份：/主演：/导演：/更新：」标签抽取；播放线路 =
//!    `div.stui-vodlist__head h3` 与 `ul.stui-content__playlist` 按下标配对，
//!    仅保留 SN 表里收录的 show 名；分集 id = `{sid}-{nid}-{pid}`（来自
//!    `/play/(\d+)-(\d+)-(\d+).html`）。
//! 5. `searchContent`：`{base}/index.php/ajax/suggest?mid=1&wd=…&limit=10&timestamp=…`。
//! 6. `playerContent`：`{base}/play/{id}.html` 取 `var player_={…}` 的
//!    `from`/`url`；按 SN 表拼解析前缀 → 抓解析页 → 取 `urls`（或
//!    `aliyunline2` 的 `vid`）变量值作为直链。
//!
//! 与 Android 实现的有意偏差（均不影响对外语义）：
//! - 元数据改成「标签正则」抽取，不再依赖 `p.data` 的下标顺序；
//! - `urls`/`vid` 使用捕获组替代 Java lookbehind（Rust `regex` 不支持 `(?<=)`）；
//! - 播放线路组保持页面顺序，不按 SN `or` 排序（表里所有 `or` 均为 999）；
//! - `ext` 允许 URL / `$$$` 文本 / JSON 对象三种形态，缺省回退内置基址。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Libvio";

/// 无 ext 时的回退基址（原实现必须由 ext 指定；这里给一个当前已知域名）。
const DEFAULT_BASE: &str = "https://www.libvio.pw";

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.114 Safari/537.36";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8";
const ACCEPT_LANGUAGE: &str = "zh-CN,zh;q=0.8,zh-TW;q=0.7,zh-HK;q=0.5,en-US;q=0.3,en;q=0.2";
const UA_PLAY: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.62 Safari/537.36";

/// 首页导航里当作分类的菜单文本。
const HOME_CATEGORIES: [&str; 5] = ["电影", "剧集", "动漫", "日韩剧", "欧美剧"];

/// SN 播放源表：`(from, 显示名, 解析前缀)`，与原 `init` 里的 JSON 常量一致。
const SN_TABLE: [(&str, &str, &str); 17] = [
    ("ali", "AI播放", "https://p2.cfnode1.xyz/ai.php?url="),
    ("aliyunline", "BD播放", "https://p2.cfnode1.xyz/ty.php?url="),
    ("aliyunline2", "BD2播放", "https://p2.cfnode1.xyz/ty2.php?url="),
    ("tianyi_625", "BD3播放", "https://p2.cfnode1.xyz/ty625.php?url="),
    ("yr", "LIBVIO", "https://p2.cfnode1.xyz/ai.php?url="),
    ("app", "LINEAPP", "https://p2.cfnode1.xyz/ai.php?url="),
    ("duoduozy", "LINE100", "https://p2.cfnode1.xyz/lb.php?url="),
    ("LINE1080", "LINE1080", "https://p2.cfnode1.xyz/lb.php?url="),
    ("LINE400", "LINE400", "https://p2.cfnode1.xyz/lb.php?url="),
    ("line401", "LINE401", ""),
    ("line402-日语", "LINE402", "https://p2.cfnode1.xyz/lb.php?url="),
    ("LINE405", "HD播放", "https://p2.cfnode1.xyz/lb.php?url="),
    ("LINE406", "LINE406", "https://p2.cfnode1.xyz/zm.php?url="),
    ("LINE407", "LINE407", "https://p2.cfnode1.xyz/lb.php?url="),
    ("LINE408", "LINE408", ""),
    ("p300", "LINE300", ""),
    ("p301", "LINE301", ""),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 选择器
 * ============================================================ */

fn header_menu_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.stui-header__menu li a").expect("导航选择器"))
}

fn list_row_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.stui-vodlist__box").expect("列表行选择器"))
}

fn anchor_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a").expect("a 选择器"))
}

fn pic_text_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("a .pic-text").expect("备注选择器"))
}

fn cover_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.stui-content__thumb a img").expect("详情封面选择器"))
}

fn title_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.stui-content__detail h1").expect("详情标题选择器"))
}

fn data_p_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("p.data").expect("元数据选择器"))
}

fn desc_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("span.detail-content").expect("简介选择器"))
}

fn group_h3_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("div.stui-vodlist__head h3").expect("播放线路标题选择器"))
}

fn playlist_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("ul.stui-content__playlist").expect("分集列表选择器"))
}

fn playlist_link_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("li a").expect("分集链接选择器"))
}

fn script_sel() -> &'static Selector {
    static SEL: OnceLock<Selector> = OnceLock::new();
    SEL.get_or_init(|| Selector::parse("script").expect("script 选择器"))
}

/* ============================================================
 * 正则
 * ============================================================ */

fn type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/type/(\d+)\.html").expect("分类正则"))
}

fn detail_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/detail/(\d+)\.html").expect("详情正则"))
}

fn play_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/play/(\d+)-(\d+)-(\d+)\.html").expect("播放正则"))
}

fn urls_re() -> &'static Regex {
    // 原实现：`(?<=urls\s=\s').*?(?=')`；Rust regex 不支持 lookbehind，改用捕获组。
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"urls\s*=\s*'([^']*)'").expect("urls 正则"))
}

fn vid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"vid\s*=\s*'([^']*)'").expect("vid 正则"))
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

/// 把 `&nbsp;` 等不可见空白归一成普通空格，便于用 `\S+` 切标签值。
fn flat_text(raw: &str) -> String {
    raw.replace('\u{a0}', " ").replace('\u{3000}', " ")
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Accept".to_string(), ACCEPT.to_string());
    h.insert("Accept-Language".to_string(), ACCEPT_LANGUAGE.to_string());
    h.insert("Referer".to_string(), format!("{base}/"));
    h
}

/// 从 ext 里解析基址：支持「http URL」「`a$$$b$$$c` 文本」「JSON 对象」三种形态。
fn base_from_ext(ext: &str) -> String {
    let trimmed = ext.trim();
    if trimmed.is_empty() {
        return DEFAULT_BASE.to_string();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.trim_end_matches('/').to_string();
    }
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
        for key in ["siteUrl", "url", "spider"] {
            if let Some(s) = map.get(key).and_then(|v| v.as_str()) {
                let s = s.trim();
                if s.starts_with("http://") || s.starts_with("https://") {
                    return s.trim_end_matches('/').to_string();
                }
            }
        }
    }
    for seg in trimmed.split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

fn base_of(site: &Site) -> String {
    base_from_ext(&site.ext_string())
}

fn sn_parse_prefix(from: &str) -> Option<&'static str> {
    SN_TABLE
        .iter()
        .find(|(key, _, _)| *key == from)
        .map(|(_, _, prefix)| *prefix)
}

fn is_known_show(show: &str) -> bool {
    SN_TABLE.iter().any(|(_, name, _)| *name == show)
}

/// 分类页 URL：12 段参数用 `-` 连接，`[0]=typeId`、`[8]=page`。
fn show_url(base: &str, cat: &str, page: u32) -> String {
    let mut segs = [""; 12];
    segs[0] = cat;
    let page_text = page.to_string();
    segs[8] = &page_text;
    format!("{base}/show/{}.html", segs.join("-"))
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn meta_value(flat: &str, label: &str) -> Option<String> {
    let pattern = format!(r"{}\s*[：:]\s*(\S+)", regex::escape(label));
    let re = Regex::new(&pattern).ok()?;
    re.captures(flat)
        .map(|caps| caps[1].trim().to_string())
        .filter(|s| !s.is_empty())
}

/* ============================================================
 * 入口
 * ============================================================ */

/// 首页分类：抓首页导航（失败时回退到内置 5 项）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &format!("{base}/"), &headers(&base), cfg.http.retry)?;
    let parsed = parse_categories(&html);
    if parsed.is_empty() {
        return Ok(fallback_categories());
    }
    Ok(parsed)
}

fn fallback_categories() -> Vec<Category> {
    [("1", "电影"), ("2", "剧集"), ("4", "动漫"), ("15", "日韩剧"), ("16", "欧美剧")]
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
    let url = show_url(&base, cat, page);
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(&base), cfg.http.retry)?;
    Ok(parse_list(&html))
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        format!("{base}/detail/{id}.html")
    };
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &url, &headers(&base), cfg.http.retry)?;
    parse_detail(site, id, &base, &html)
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = format!(
        "{base}/index.php/ajax/suggest?mid=1&wd={}&limit=10&timestamp={}",
        percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC),
        now_millis()
    );
    let client = crate::http::client_for(&cfg.http)?;
    let body = crate::http::get_text(&client, &url, &headers(&base), cfg.http.retry)?;
    parse_search(site, &base, &body)
}

/// 播放：播放页 → SN 解析页 → 直链。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let page_url = format!("{base}/play/{id}.html");
    let client = crate::http::client_for(&cfg.http)?;
    let html = crate::http::get_text(&client, &page_url, &headers(&base), cfg.http.retry)?;

    let (from, raw_url) = parse_player_page(&html).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("播放页未找到 var player_ 数据（{page_url} 可能已改版）"),
        )
        .into_core()
    })?;
    let prefix = sn_parse_prefix(&from).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::HostApi,
            site,
            format!("未知播放源 from={from}（SN 表未收录）"),
        )
        .into_core()
    })?;
    let target = format!("{prefix}{raw_url}");

    let mut play_headers = HashMap::new();
    play_headers.insert("User-Agent".to_string(), UA_PLAY.to_string());
    play_headers.insert("Referer".to_string(), base.clone());
    let played = crate::http::get_text(&client, &target, &play_headers, cfg.http.retry)?;
    let direct = if from.contains("aliyunline2") {
        extract_var(&played, vid_re())
    } else {
        extract_var(&played, urls_re())
    };
    let direct = direct.ok_or_else(|| {
        CatvodError::new(
            ErrorClass::HostApi,
            site,
            format!("解析页 {target} 未找到 urls/vid 直链变量"),
        )
        .into_core()
    })?;

    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": direct,
        "header": {
            "User-Agent": UA_PLAY,
            "Referer": format!("{base}"),
        },
    }))
}

/* ============================================================
 * 解析
 * ============================================================ */

pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for a in doc.select(header_menu_sel()) {
        let name = text_of(&a);
        if !HOME_CATEGORIES.contains(&name.as_str()) {
            continue;
        }
        let Some(href) = attr_of(&a, "href") else {
            continue;
        };
        if let Some(caps) = type_re().captures(&href) {
            out.push(Category {
                type_id: caps[1].to_string(),
                type_name: name,
            });
        }
    }
    out
}

pub fn parse_list(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(list_row_sel()) {
        let Some(a) = row.select(anchor_sel()).next() else {
            continue;
        };
        let Some(href) = attr_of(&a, "href") else {
            continue;
        };
        let Some(caps) = detail_re().captures(&href) else {
            continue;
        };
        let name = attr_of(&a, "title")
            .or_else(|| attr_of(&a, "title2"))
            .unwrap_or_default();
        let cover = attr_of(&a, "data-original").filter(|s| !s.is_empty());
        let remark = row
            .select(pic_text_sel())
            .next()
            .map(|el| text_of(&el))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: caps[1].to_string(),
            name,
            cover,
            remark,
        });
    }
    out
}

pub fn parse_detail(site: &Site, id: &str, base: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let cover = doc
        .select(cover_sel())
        .next()
        .and_then(|el| attr_of(&el, "data-original"))
        .filter(|s| !s.is_empty())
        .map(|s| html_rule::join_url(base, &s));
    let title = doc
        .select(title_sel())
        .next()
        .map(|el| text_of(&el))
        .unwrap_or_default();

    let flat = flat_text(
        &doc.select(data_p_sel())
            .map(|el| text_of(&el))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let desc = doc
        .select(desc_sel())
        .next()
        .map(|el| text_of(&el))
        .filter(|s| !s.is_empty());

    let heads: Vec<String> = doc
        .select(group_h3_sel())
        .map(|el| text_of(&el))
        .collect();
    let playlists: Vec<_> = doc.select(playlist_sel()).collect();
    let mut episodes = Vec::new();
    for (idx, list) in playlists.iter().enumerate() {
        let show = heads.get(idx).map(|s| s.trim().to_string()).unwrap_or_default();
        if show.is_empty() || !is_known_show(&show) {
            continue;
        }
        let mut items = Vec::new();
        for a in list.select(playlist_link_sel()) {
            let href = attr_of(&a, "href").unwrap_or_default();
            let Some(caps) = play_re().captures(&href) else {
                continue;
            };
            let title = text_of(&a);
            let fallback = format!("第{}集", items.len() + 1);
            items.push(Episode {
                title: if title.is_empty() { fallback } else { title },
                url: format!("{}-{}-{}", &caps[1], &caps[2], &caps[3]),
            });
        }
        if !items.is_empty() {
            episodes.push(EpisodeGroup {
                group: show,
                items,
            });
        }
    }

    if title.is_empty() && episodes.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("详情页 {base}/detail/{id}.html 缺少标题与播放线路（站点改版或非 stui 模板）"),
        )
        .into_core());
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name: title,
        cover,
        desc,
        year: meta_value(&flat, "年份"),
        area: meta_value(&flat, "地区"),
        director: meta_value(&flat, "导演"),
        actor: meta_value(&flat, "主演"),
        episodes,
    })
}

pub fn parse_search(site: &Site, base: &str, body: &str) -> Result<Vec<VideoItem>> {
    let value: Value = serde_json::from_str(body).map_err(|err| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("搜索响应不是 JSON：{err}"),
        )
        .into_core()
    })?;
    let total = value.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
    if total <= 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<Value> = match value.get("list") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for row in rows {
        let id = match row.get("id") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        let name = row
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let pic = row
            .get("pic")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let cover = if pic.is_empty() {
            None
        } else {
            Some(html_rule::join_url(base, &pic))
        };
        out.push(VideoItem {
            id,
            name,
            cover,
            remark: None,
        });
    }
    Ok(out)
}

/// 从播放页里找 `var player_` 脚本，解析出 `(from, url)`。
pub fn parse_player_page(html: &str) -> Option<(String, String)> {
    let doc = Html::parse_document(html);
    for script in doc.select(script_sel()) {
        let text = text_of(&script);
        if !text.starts_with("var player_") {
            continue;
        }
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end <= start {
            continue;
        }
        let value: Value = serde_json::from_str(&text[start..=end]).ok()?;
        let from = value.get("from")?.as_str()?.to_string();
        let url = value.get("url")?.as_str()?.to_string();
        return Some((from, url));
    }
    None
}

fn extract_var(body: &str, re: &Regex) -> Option<String> {
    re.captures(body)
        .map(|caps| caps[1].trim().to_string())
        .filter(|s| !s.is_empty())
}

/* ============================================================
 * 单测
 * ============================================================ */

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
        assert!(handles("csp_Libvio"));
        assert!(handles(" CSP_LIBVIO "));
        assert!(!handles("csp_LibvioGuard"));
    }

    #[test]
    fn builds_show_url_with_page_in_slot_eight() {
        assert_eq!(
            show_url("https://x.test", "1", 1),
            "https://x.test/show/1--------1---.html"
        );
        assert_eq!(
            show_url("https://x.test", "1", 2),
            "https://x.test/show/1--------2---.html"
        );
    }

    #[test]
    fn resolves_base_from_ext_forms() {
        assert_eq!(
            base_from_ext("https://www.libvio.pw/"),
            "https://www.libvio.pw"
        );
        assert_eq!(
            base_from_ext("./lib/token.json$$$https://www.libvio.fun$$$null"),
            "https://www.libvio.fun"
        );
        assert_eq!(
            base_from_ext("{\"siteUrl\":\"https://www.libvio.me/\"}"),
            "https://www.libvio.me"
        );
        assert_eq!(base_from_ext(""), DEFAULT_BASE);
    }

    #[test]
    fn parses_home_categories() {
        let html = r#"
        <ul class="stui-header__menu">
          <li><a href="/">首页</a></li>
          <li><a href="/type/1.html">电影</a></li>
          <li><a href="/type/2.html">剧集</a></li>
          <li><a href="/type/30.html">综艺</a></li>
          <li><a href="/type/4.html">动漫</a></li>
        </ul>"#;
        let cats = parse_categories(html);
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[2].type_name, "动漫");
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"
        <div class="stui-vodlist__box">
          <a href="/detail/123.html" title="示例电影" data-original="//img.test/a.jpg">
            <span class="pic-text text-right">更新至10集</span>
          </a>
        </div>
        <div class="stui-vodlist__box">
          <a href="/detail/456.html" title="另一部" data-original="/pic/b.jpg"></a>
        </div>"#;
        let rows = parse_list(html);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "123");
        assert_eq!(rows[0].name, "示例电影");
        assert_eq!(rows[0].remark.as_deref(), Some("更新至10集"));
        assert_eq!(rows[1].cover.as_deref(), Some("/pic/b.jpg"));
        assert!(rows[1].remark.is_none());
    }

    #[test]
    fn parses_detail_with_sn_groups_and_metadata() {
        let html = r#"
        <div class="stui-content__thumb"><a href="/detail/1.html"><img data-original="/upload/cover.jpg"></a></div>
        <div class="stui-content__detail"><h1>示例剧</h1></div>
        <p class="data">类型：剧情&nbsp;地区：中国大陆&nbsp;年份：2021</p>
        <p class="data">主演：张三&nbsp;导演：李四</p>
        <p class="data">更新：更新至10集</p>
        <span class="detail-content"> 一段简介 </span>
        <div class="stui-vodlist__head"><h3>AI播放</h3></div>
        <ul class="stui-content__playlist">
          <li><a href="/play/1-1-1.html">第1集</a></li>
          <li><a href="/play/1-1-2.html">第2集</a></li>
        </ul>
        <div class="stui-vodlist__head"><h3>未知线路</h3></div>
        <ul class="stui-content__playlist">
          <li><a href="/play/1-9-9.html">第9集</a></li>
        </ul>"#;
        let d = parse_detail(&site("https://x.test"), "1", "https://x.test", html).expect("详情");
        assert_eq!(d.name, "示例剧");
        assert_eq!(d.year.as_deref(), Some("2021"));
        assert_eq!(d.area.as_deref(), Some("中国大陆"));
        assert_eq!(d.actor.as_deref(), Some("张三"));
        assert_eq!(d.director.as_deref(), Some("李四"));
        assert_eq!(d.cover.as_deref(), Some("https://x.test/upload/cover.jpg"));
        assert_eq!(d.desc.as_deref(), Some("一段简介"));
        assert_eq!(d.episodes.len(), 1, "未知线路应被 SN 表过滤");
        assert_eq!(d.episodes[0].group, "AI播放");
        assert_eq!(d.episodes[0].items[1].url, "1-1-2");
    }

    #[test]
    fn detail_without_content_reports_parse_error() {
        let err = parse_detail(&site("https://x.test"), "9", "https://x.test", "<html></html>")
            .expect_err("应报错");
        let text = format!("{err:?}");
        assert!(text.contains("catvod.result.parse"), "{text}");
    }

    #[test]
    fn parses_search_with_string_list() {
        let body = r#"{"total":1,"list":"[{\"id\":\"7\",\"name\":\"示例\",\"pic\":\"/p.jpg\"}]"}"#;
        let rows = parse_search(&site("https://x.test"), "https://x.test", body).expect("搜索");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "7");
        assert_eq!(rows[0].cover.as_deref(), Some("https://x.test/p.jpg"));
    }

    #[test]
    fn parses_search_with_array_list_and_zero_total() {
        let body = r#"{"total":2,"list":[{"id":1,"name":"A","pic":"https://cdn/a.jpg"},{"id":2,"name":"B"}]}"#;
        let rows = parse_search(&site("https://x.test"), "https://x.test", body).expect("搜索");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "1");
        assert_eq!(rows[1].cover, None);

        let empty = parse_search(&site("https://x.test"), "https://x.test", r#"{"total":0,"list":""}"#)
            .expect("空搜索");
        assert!(empty.is_empty());
    }

    #[test]
    fn extracts_player_vars_and_direct_links() {
        let html = r#"<script>var player_ = {"from":"line401","url":"https://cdn.test/x.m3u8","id":"1"};</script>"#;
        let (from, url) = parse_player_page(html).expect("播放页");
        assert_eq!(from, "line401");
        assert_eq!(url, "https://cdn.test/x.m3u8");

        let page = r#"<script>player_aaaa.url = "x"; var urls = 'https://cdn.test/a.m3u8';</script>"#;
        assert_eq!(
            extract_var(page, urls_re()).as_deref(),
            Some("https://cdn.test/a.m3u8")
        );
        let page2 = r#"<script>var vid = 'https://cdn.test/b.m3u8';</script>"#;
        assert_eq!(
            extract_var(page2, vid_re()).as_deref(),
            Some("https://cdn.test/b.m3u8")
        );
        assert!(extract_var("<html></html>", urls_re()).is_none());
    }

    #[test]
    fn sn_table_contains_expected_sources() {
        assert_eq!(
            sn_parse_prefix("ali"),
            Some("https://p2.cfnode1.xyz/ai.php?url=")
        );
        assert_eq!(sn_parse_prefix("line401"), Some(""));
        assert!(sn_parse_prefix("nope").is_none());
        assert!(is_known_show("AI播放"));
        assert!(!is_known_show("未知线路"));
    }
}
