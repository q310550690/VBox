//! `csp_Czsapp`（厂长资源 czzy 系）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.Czsapp`，站点 1 个（dc 仓 `xhztv`，key
//!   `csp_Czsapp`，站点名「🏭厂长」，ext `https://www.czzy55.com/`）。
//! * 证据：`_rea/decompiled/5fc50e17-…/deobf/com/github/catvod/spider/Czsapp.java`
//!   （241 行、`detailContent` 被 jadx 跳过）；用
//!   `jadx --show-bad-code --single-class com.github.catvod.spider.Czsapp` 恢复为
//!   `_rea/tmp/r143/jadx/Czsapp.java`（352 行，`detailContent` 完整）。
//!   字符串解码器为 `merge.FM.a.C0378a.b(data,key)`（循环异或 + UTF-8），本仓
//!   `_rea/tmp/r143/dec-detail.mjs` 已解出全部选择器。
//!
//! 协议（逐段还原）：
//! * `init`：ext 直接作为站点基址（`d = str`）。
//! * `homeContent`：GET 基址 → 导航 `ul.navlist > li > a`（跳过「首页」）→
//!   分类；推荐行 `div.bt_img.mi_ne_kd > ul > li`（标题 `h3.dytit > a`、封面
//!   `img[data-original]`、备注 `div.jidi > span`）。
//! * `categoryContent`：`{base}{type}/page/{page}`（原类字符串拼接，`type`
//!   为导航 href）；行选择器同上（封面属性用 `img[src]`）。
//! * `detailContent`：GET 详情页；标题 `div.moviedteail_tt > h1`、封面
//!   `div.dyimg img[src]`、简介 `div.yp_context`、信息行 `ul.moviedteail_list
//!   > li`（前缀两字：`类型/地区/年份/豆瓣`，值取 `[3..]`），导演取第 6 个
//!   `li > a`、主演取第 8 个 `li > a`；播放列表 `div.paly_list_btn > a`
//!   （`文本$href` 以 `#` 连接，线路固定「厂长资源」）。
//! * `searchContent`：`{base}xsss1O1?q={kw}`，行选择器同上（封面 `img[src]`、
//!   id 补全为绝对地址）。
//! * `playerContent`：GET 播放页，正则
//!   `"([^"]+)";var [\d\w]+=function dncry.*md5.enc.Utf8.parse\("([\d\w]+)".*md5.enc.Utf8.parse\(([\d]+)\)`
//!   取 Base64 密文/密钥/IV → AES/CBC/PKCS7 解密 → `video: {url: "…"}`；
//!   无密文时回退 `div.videoplay > iframe[src]`：iframe 若本身带 m3u8/mp4
//!   直链则直接使用，否则 GET iframe 页面取 `var result_v2 = ({…});` 的
//!   `data` → 反转字符串 → 十六进制解码得到地址；输出 `parse:0`。
//!
//! 有意偏差（桌面端能力边界 / 上游改版）：
//! 1. 站点 ext 仍指向 `www.czzy55.com`，该域名当前只剩「默认跳转页」（提示新地址
//!    `www.4kcz.com`）。本实现不改写默认基址：`ext` 的首个 `http(s)` 段可覆盖
//!    （原类即此语义），因此运行时以订阅里的新地址为准；离线/真站测试用
//!    `https://www.4kcz.com/` 验证协议。
//! 2. 原类播放成功后还会接入宿主代理做字幕（`subf`/`subt`），桌面端没有该宿主
//!    能力：只返回视频地址，不返回字幕元数据。
//! 3. 站点 2026-11 已改版：播放页不再下发 `dncry` 密文，而是
//!    `div.videoplay > iframe[src]` 里直接带 m3u8。原类此时会得到空地址；
//!    本实现增加「iframe 直链兜底」（仍在原回退分支内，属增量恢复）。
//! 4. 搜索接口被站点 WAF（SafeLine）拦截 403：协议照原类实现，运行时可能
//!    返回空列表；这属于上游策略，不是解析差异。
//! 5. 原类 `detailContent` 的导演/主演取 `li > a`，但现站「主演」是 `span`：
//!    本实现先取 `li > a`，为空再退化为该 `li` 去前缀后的整段文本。

use std::collections::HashMap;
use std::sync::OnceLock;

use base64::Engine as _;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::http::client_for;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_Czsapp";

/// 原站点 ext 里的基址（当前只剩跳转页；ext 可覆盖）。
const DEFAULT_BASE: &str = "https://www.czzy55.com/";

/// 页面请求 UA（原类 `c()` 的 Chrome/94 Windows）。
const UA_PAGE: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.81 Safari/537.36";

/// 播放页请求 UA（原类 `a()/b()` 的 Android 15 WebView）。
const UA_PLAY: &str = "Mozilla/5.0 (Linux; Android 15; PJZ110 Build/AP3A.240617.008; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/136.0.7103.127 Mobile Safari/537.36";

/// 播放页请求语言（原类 `b()`）。
const ACCEPT_LANG: &str = "zh-CN,zh;q=0.9";

/// 播放线路名（原类 `j("厂长资源")`）。
const PLAY_FROM: &str = "厂长资源";

/// `handles`：api 名大小写不敏感匹配。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

macro_rules! selector {
    ($name:ident, $css:expr) => {
        fn $name() -> &'static Selector {
            static SEL: OnceLock<Selector> = OnceLock::new();
            SEL.get_or_init(|| Selector::parse($css).expect("valid css selector"))
        }
    };
}

selector!(nav_link_sel, "ul.navlist > li > a");
selector!(row_sel, "div.bt_img.mi_ne_kd > ul > li");
selector!(row_title_sel, "h3.dytit > a");
selector!(row_img_sel, "img");
selector!(row_remark_sel, "div.jidi > span");
selector!(dt_title_sel, "div.moviedteail_tt > h1");
selector!(dt_cover_sel, "div.dyimg img");
selector!(dt_content_sel, "div.yp_context");
selector!(dt_meta_sel, "ul.moviedteail_list > li");
selector!(dt_director_sel, "ul.moviedteail_list > li:nth-child(6) > a");
selector!(dt_actor_sel, "ul.moviedteail_list > li:nth-child(8) > a");
selector!(play_link_sel, "div.paly_list_btn > a");
selector!(play_iframe_sel, "div.videoplay > iframe");

fn re_video() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"video: \{url: "([^"]+)""#).expect("valid regex"))
}

fn re_encrypted() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#""([^"]+)";var [\d\w]+=function dncry.*md5\.enc\.Utf8\.parse\("([\d\w]+)".*md5\.enc\.Utf8\.parse\(([\d]+)\)"#)
            .expect("valid regex")
    })
}

fn re_result_v2() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"var result_v2 = (\{.*?\});").expect("valid regex")
    })
}

fn re_media_url() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"https?://[^\s"'<>]+?\.(?:m3u8|mp4)(?:\?[^\s"'<>]*)?"#).expect("valid regex")
    })
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

/// ext 首个 `http(s)` 段覆盖基址（原类 ext 即基址）。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    for seg in ext.split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.trim_end_matches('/').to_string()
}

/// 相对/绝对 URL 归一（`//x` → https，`/x` → origin 拼接）。
pub fn join_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    let origin = {
        let scheme_idx = base.find("://").map(|i| i + 3).unwrap_or(0);
        match base[scheme_idx..].find('/') {
            Some(i) => base[..scheme_idx + i].to_string(),
            None => base.to_string(),
        }
    };
    if let Some(rest) = href.strip_prefix('/') {
        return format!("{origin}/{rest}");
    }
    format!("{}/{}", base.trim_end_matches('/'), href)
}

fn headers_page(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA_PAGE.to_string());
    h.insert("Referer".to_string(), base.to_string());
    h
}

fn headers_play(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA_PLAY.to_string());
    h.insert("Referer".to_string(), base.to_string());
    h.insert("Accept-Language".to_string(), ACCEPT_LANG.to_string());
    h
}

fn fetch(cfg: &TvConfig, url: &str, headers: &HashMap<String, String>) -> Result<String> {
    let client = client_for(&cfg.http)?;
    crate::http::get_text(&client, url, headers, 2)
}

fn text_of(el: ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn attr_of(el: ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().trim().to_string()
}

/* ============================================================
 * 解析（纯函数，便于单测）
 * ============================================================ */

/// 首页导航 → 分类（跳过「首页」；id 取 href 去 origin/首斜杠）。
pub fn parse_categories(html: &str, base: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for a in doc.select(nav_link_sel()) {
        let name = text_of(a);
        if name.is_empty() || name == "首页" {
            continue;
        }
        let href = attr_of(a, "href");
        let id = href
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        if out.iter().any(|c: &Category| c.type_id == id) {
            continue;
        }
        // 分类链接若为绝对地址且与基址同源，也可以正常使用。
        let _ = join_url(base, &href);
        out.push(Category { type_id: id, type_name: name });
    }
    out
}

/// 列表行（首页推荐 / 分类 / 搜索共用）→ 条目。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(row_sel()) {
        let Some(a) = li.select(row_title_sel()).next() else {
            continue;
        };
        let href = attr_of(a, "href");
        let id = join_url(base, &href);
        if id.is_empty() {
            continue;
        }
        let name = text_of(a);
        let cover = li
            .select(row_img_sel())
            .next()
            .map(|img| {
                let lazy = attr_of(img, "data-original");
                if lazy.is_empty() {
                    attr_of(img, "src")
                } else {
                    lazy
                }
            })
            .unwrap_or_default();
        let remark = li
            .select(row_remark_sel())
            .next()
            .map(text_of)
            .unwrap_or_default();
        out.push(VideoItem {
            id,
            name,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark: if remark.is_empty() { None } else { Some(remark) },
            ..Default::default()
        });
    }
    out
}

/// 详情信息行的前缀（两字）与值（`[3..]`）：返回 `(前缀, 值)`。
fn split_meta_line(line: &str) -> Option<(String, String)> {
    let chars: Vec<char> = line.trim().chars().collect();
    if chars.len() < 4 {
        return None;
    }
    let prefix: String = chars[..2].iter().collect();
    let value: String = chars[3..].iter().collect();
    Some((prefix, value.trim().to_string()))
}

/// 详情页 → `VideoDetail`（含播放列表）。
pub fn parse_detail(html: &str, id: &str, base: &str) -> Option<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc.select(dt_title_sel()).next().map(text_of).unwrap_or_default();
    if name.is_empty() {
        return None;
    }

    let cover = doc
        .select(dt_cover_sel())
        .next()
        .map(|img| join_url(base, &attr_of(img, "src")))
        .filter(|s| !s.is_empty());
    let content = doc.select(dt_content_sel()).next().map(text_of).unwrap_or_default();

    let mut type_name = String::new();
    let mut area = String::new();
    let mut year = String::new();
    let mut rating = String::new();
    let metas: Vec<String> = doc.select(dt_meta_sel()).map(text_of).collect();
    for line in &metas {
        let Some((prefix, value)) = split_meta_line(line) else {
            continue;
        };
        match prefix.as_str() {
            "类型" => type_name = value,
            "地区" => area = value,
            "年份" => year = value,
            "豆瓣" => rating = value,
            _ => {}
        }
    }

    let director = doc
        .select(dt_director_sel())
        .next()
        .map(text_of)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            metas
                .iter()
                .find(|l| l.trim_start().starts_with("导演"))
                .and_then(|l| split_meta_line(l).map(|(_, v)| v))
        })
        .unwrap_or_default();
    let actor = doc
        .select(dt_actor_sel())
        .next()
        .map(text_of)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            metas
                .iter()
                .find(|l| l.trim_start().starts_with("主演"))
                .and_then(|l| split_meta_line(l).map(|(_, v)| v))
        })
        .unwrap_or_default();

    let episodes = parse_playlist(html, base);

    let mut desc = String::new();
    if !type_name.is_empty() {
        desc.push_str(&format!("类型：{type_name}"));
    }
    if !rating.is_empty() {
        if !desc.is_empty() {
            desc.push_str(" ｜ ");
        }
        desc.push_str(&format!("豆瓣：{rating}"));
    }
    if !content.is_empty() {
        if !desc.is_empty() {
            desc.push('\n');
        }
        desc.push_str(&content);
    }

    Some(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc: if desc.is_empty() { None } else { Some(desc) },
        year: if year.is_empty() { None } else { Some(year) },
        area: if area.is_empty() { None } else { Some(area) },
        director: if director.is_empty() { None } else { Some(director) },
        actor: if actor.is_empty() { None } else { Some(actor) },
        episodes,
        ..Default::default()
    })
}

/// 播放列表 `div.paly_list_btn > a` → 单线路剧集组。
pub fn parse_playlist(html: &str, base: &str) -> Vec<EpisodeGroup> {
    let doc = Html::parse_document(html);
    let items: Vec<Episode> = doc
        .select(play_link_sel())
        .filter_map(|a| {
            let href = attr_of(a, "href");
            let url = join_url(base, &href);
            if url.is_empty() {
                return None;
            }
            let title = text_of(a);
            Some(Episode {
                title: if title.is_empty() { url.clone() } else { title },
                url,
            })
        })
        .collect();
    if items.is_empty() {
        Vec::new()
    } else {
        vec![EpisodeGroup { group: PLAY_FROM.to_string(), items }]
    }
}

/// 搜索页 → 条目（行选择器与列表一致）。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    parse_list(html, base)
}

/* ============================================================
 * 播放（AES/CBC 解密 + iframe 回退）
 * ============================================================ */

/// AES/CBC/PKCS7 解密（原类 `C0465a.d(base64, key, iv, "AES/CBC/PKCS7Padding")`）。
pub fn aes_cbc_decrypt_b64(cipher_b64: &str, key: &str, iv: &str) -> Option<String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(cipher_b64.trim())
        .ok()?;
    let key = key.as_bytes();
    let iv = iv.as_bytes();
    let out = match key.len() {
        16 => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&data)
            .ok()?,
        24 => cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&data)
            .ok()?,
        32 => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&data)
            .ok()?,
        _ => return None,
    };
    String::from_utf8(out).ok()
}

/// 原类 `C0465a.g`：可选 `0x` 前缀的十六进制串 → UTF-8 字符串。
pub fn hex_to_string(hex_str: &str) -> Option<String> {
    let mut s = hex_str.trim();
    if let Some(rest) = s.strip_prefix("0x") {
        s = rest;
    }
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let padded = if s.len() % 2 == 1 {
        format!("0{s}")
    } else {
        s.to_string()
    };
    let bytes = hex::decode(padded).ok()?;
    String::from_utf8(bytes).ok()
}

/// 播放页内嵌 iframe（原类 `div.videoplay > iframe`）。
fn iframe_src(html: &str) -> String {
    let doc = Html::parse_document(html);
    doc.select(play_iframe_sel())
        .next()
        .map(|el| attr_of(el, "src"))
        .unwrap_or_default()
}

/// iframe 的 `src` 可能把真实视频地址放在 `url=` 查询参数里（改版后的 py.php 播放器）。
fn iframe_embedded_media(src: &str) -> Option<String> {
    if let Some(idx) = src.find("url=") {
        let rest = &src[idx + 4..];
        let end = rest.find(|c| c == '&' || c == '#').unwrap_or(rest.len());
        let cand = rest[..end].trim();
        if re_media_url().is_match(cand) {
            return Some(cand.to_string());
        }
    }
    if !src.contains("url=") && re_media_url().is_match(src) {
        return Some(src.to_string());
    }
    None
}

/// 播放结果 JSON（`parse:0` + 头；原类 `header` 为播放页请求头）。
pub fn player_json(url: &str, base: &str) -> Value {
    let mut header = serde_json::Map::new();
    header.insert("User-Agent".to_string(), json!(UA_PLAY));
    header.insert("Referer".to_string(), json!(base));
    header.insert("Accept-Language".to_string(), json!(ACCEPT_LANG));
    json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": Value::Object(header),
    })
}

/// 播放页 → 视频地址（密文优先；否则 iframe 回退，含改版直链兜底）。
pub fn resolve_player(cfg: &TvConfig, base: &str, page_url: &str, html: &str) -> Option<String> {
    if let Some(caps) = re_encrypted().captures(html) {
        if let Some(plain) = aes_cbc_decrypt_b64(&caps[1], &caps[2], &caps[3]) {
            if let Some(m) = re_video().captures(&plain) {
                return Some(m[1].to_string());
            }
        }
    }

    let src = iframe_src(html);
    if src.is_empty() {
        return None;
    }
    let src_url = join_url(base, &src);
    if let Some(u) = iframe_embedded_media(&src_url) {
        return Some(u);
    }
    let page = fetch(cfg, &src_url, &headers_play(page_url)).ok()?;
    if let Some(caps) = re_result_v2().captures(&page) {
        if let Ok(v) = serde_json::from_str::<Value>(&caps[1]) {
            if let Some(data) = v.get("data").and_then(Value::as_str) {
                let reversed: String = data.chars().rev().collect();
                if let Some(m) = re_media_url().captures(&reversed) {
                    return Some(m[0].to_string());
                }
                if let Some(decoded) = hex_to_string(&reversed) {
                    if let Some(m) = re_media_url().captures(&decoded) {
                        return Some(m[0].to_string());
                    }
                }
            }
        }
    }
    re_media_url().captures(&page).map(|m| m[0].to_string())
}

/* ============================================================
 * 五个入口
 * ============================================================ */

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = fetch(cfg, &format!("{base}/"), &headers_page(&base))?;
    let cats = parse_categories(&html, &base);
    if cats.is_empty() {
        return Err(parse_err(site, "首页未解析到分类"));
    }
    Ok(cats)
}

pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
    _filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let id = type_id.trim().trim_matches('/');
    if id.is_empty() {
        return Err(parse_err(site, "分类 id 为空"));
    }
    let url = format!("{base}/{id}/page/{}", page.max(1));
    let html = fetch(cfg, &url, &headers_page(&base))?;
    Ok(parse_list(&html, &base))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = join_url(&base, id);
    if url.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let html = fetch(cfg, &url, &headers_page(&base))?;
    parse_detail(&html, id, &base).ok_or_else(|| parse_err(site, "详情页未解析到标题"))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let kw = keyword.trim();
    if kw.is_empty() {
        return Ok(Vec::new());
    }
    let base = base_of(site);
    let url = format!("{base}/xsss1O1?q={}", urlencode(kw));
    let html = fetch(cfg, &url, &headers_page(&base))?;
    Ok(parse_search(&html, &base))
}

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let url = join_url(&base, id);
    if url.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let html = fetch(cfg, &url, &headers_play(&base))?;
    match resolve_player(cfg, &base, &url, &html) {
        Some(video) => Ok(player_json(&video, &base)),
        None => Err(parse_err(site, "播放页未解析到视频地址")),
    }
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://www.4kcz.com";

    fn site(ext: &str) -> Site {
        Site {
            key: "czsapp".to_string(),
            name: "厂长资源".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(Value::String(ext.to_string())),
            ..Default::default()
        }
    }

    const HOME: &str = r#"<html><body>
      <ul class="navlist">
        <li><a href="/">首页</a></li>
        <li><a href="/dbtop250/">电影</a></li>
        <li><a href="/dianshiju/">电视剧</a></li>
        <li><a href="/dbtop250/">电影</a></li>
      </ul>
      <div class="bt_img mi_ne_kd"><ul>
        <li><h3 class="dytit"><a href="/movie/20294.html">峡谷</a></h3>
            <img data-original="/pic/1.jpg" src="/pic/1-thumb.jpg">
            <div class="jidi"><span>HD</span></div></li>
        <li><h3 class="dytit"><a href="/movie/2.html">第二部</a></h3>
            <img src="/pic/2.jpg">
            <div class="jidi"><span>更新至 12 集</span></div></li>
      </ul></div>
    </body></html>"#;

    const DETAIL: &str = r#"<html><body>
      <div class="moviedteail_tt"><h1>峡谷</h1></div>
      <div class="dyimg"><img src="/pic/123.jpg"></div>
      <div class="yp_context">简介文本</div>
      <ul class="moviedteail_list">
        <li>类型：电影</li>
        <li>地区：美国</li>
        <li>年份：2025</li>
        <li>又名：The Gorge</li>
        <li>上映：2025-02-14</li>
        <li><a href="/actor/1">安雅·泰勒</a></li>
        <li>编剧：编剧甲</li>
        <li><a href="/actor/2">迈尔斯·特勒</a></li>
        <li>语言：英语</li>
        <li>时长：127分钟</li>
      </ul>
      <div class="paly_list_btn">
        <a href="/v_play/abc.html">1080P-1</a>
        <a href="/v_play/def.html">1080P-2</a>
      </div>
    </body></html>"#;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Czsapp"));
        assert!(handles("CSP_CZSAPP"));
        assert!(!handles("csp_Czsapp2"));
    }

    #[test]
    fn base_of_prefers_http_ext() {
        assert_eq!(base_of(&site("https://www.4kcz.com/")), BASE);
        assert_eq!(base_of(&site("")), DEFAULT_BASE.trim_end_matches('/'));
        assert_eq!(base_of(&site("token$$$http://127.0.0.1:1/")), "http://127.0.0.1:1");
    }

    #[test]
    fn join_url_forms() {
        assert_eq!(join_url(BASE, "/a/b.html"), "https://www.4kcz.com/a/b.html");
        assert_eq!(join_url(BASE, "a/b.html"), "https://www.4kcz.com/a/b.html");
        assert_eq!(join_url(BASE, "//cdn.test/x.m3u8"), "https://cdn.test/x.m3u8");
        assert_eq!(join_url(BASE, "https://x.test/y"), "https://x.test/y");
        assert_eq!(join_url(BASE, ""), "");
    }

    #[test]
    fn urlencode_encodes_non_ascii() {
        assert_eq!(urlencode("a b&中"), "a%20b%26%E4%B8%AD");
    }

    #[test]
    fn parses_categories_skipping_home() {
        let cats = parse_categories(HOME, BASE);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "dbtop250");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_name, "电视剧");
    }

    #[test]
    fn parses_list_and_search_rows() {
        let items = parse_list(HOME, BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "https://www.4kcz.com/movie/20294.html");
        assert_eq!(items[0].name, "峡谷");
        assert_eq!(items[0].cover.as_deref(), Some("/pic/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("HD"));
        // 无 data-original 时回退 src
        assert_eq!(items[1].cover.as_deref(), Some("/pic/2.jpg"));

        let searched = parse_search(HOME, BASE);
        assert_eq!(searched.len(), 2);
    }

    #[test]
    fn parses_detail_metadata_and_playlist() {
        let d = parse_detail(DETAIL, "https://www.4kcz.com/movie/20294.html", BASE).unwrap();
        assert_eq!(d.name, "峡谷");
        assert_eq!(d.cover.as_deref(), Some("https://www.4kcz.com/pic/123.jpg"));
        assert_eq!(d.year.as_deref(), Some("2025"));
        assert_eq!(d.area.as_deref(), Some("美国"));
        assert_eq!(d.director.as_deref(), Some("安雅·泰勒"));
        assert_eq!(d.actor.as_deref(), Some("迈尔斯·特勒"));
        let desc = d.desc.unwrap_or_default();
        assert!(desc.starts_with("类型：电影"), "desc={desc}");
        assert!(desc.contains("简介文本"));

        assert_eq!(d.episodes.len(), 1);
        let group = &d.episodes[0];
        assert_eq!(group.group, PLAY_FROM);
        assert_eq!(group.items.len(), 2);
        assert_eq!(group.items[0].title, "1080P-1");
        assert_eq!(group.items[0].url, "https://www.4kcz.com/v_play/abc.html");
    }

    #[test]
    fn detail_without_title_is_unparsed() {
        assert!(parse_detail("<html><body></body></html>", "id", BASE).is_none());
    }

    #[test]
    fn meta_line_split() {
        assert_eq!(split_meta_line("类型：电影"), Some(("类型".into(), "电影".into())));
        assert_eq!(split_meta_line("短"), None);
    }

    #[test]
    fn aes_cbc_roundtrip_and_hex_conversion() {
        use base64::Engine as _;
        use cbc::cipher::{block_padding::Pkcs7 as Pk, BlockEncryptMut, KeyIvInit};
        let key = "0123456789abcdef";
        let iv = "1234567890123456";
        let plain = r#"video: {url: "https://cdn.test/enc.m3u8"}"#;
        let enc = cbc::Encryptor::<aes::Aes128>::new_from_slices(key.as_bytes(), iv.as_bytes())
            .unwrap()
            .encrypt_padded_vec_mut::<Pk>(plain.as_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(enc);
        assert_eq!(aes_cbc_decrypt_b64(&b64, key, iv).as_deref(), Some(plain));

        assert_eq!(hex_to_string("4142").as_deref(), Some("AB"));
        assert_eq!(hex_to_string("0x4142").as_deref(), Some("AB"));
        assert_eq!(hex_to_string("zz"), None);
    }

    #[test]
    fn resolves_encrypted_player_page() {
        use base64::Engine as _;
        use cbc::cipher::{block_padding::Pkcs7 as Pk, BlockEncryptMut, KeyIvInit};
        let key = "0123456789abcdef";
        let iv = "1234567890123456";
        let plain = r#"video: {url: "https://cdn.test/enc.m3u8"}"#;
        let enc = cbc::Encryptor::<aes::Aes128>::new_from_slices(key.as_bytes(), iv.as_bytes())
            .unwrap()
            .encrypt_padded_vec_mut::<Pk>(plain.as_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(enc);
        let html = format!(
            r#"<html><script>var a="{b64}";var dncry=function dncry(x){{return md5.enc.Utf8.parse("0123456789abcdef")+md5.enc.Utf8.parse(1234567890123456)}}</script></html>"#
        );
        let cfg = TvConfig::default();
        let got = resolve_player(&cfg, BASE, "https://www.4kcz.com/v_play/abc.html", &html);
        assert_eq!(got.as_deref(), Some("https://cdn.test/enc.m3u8"));
    }

    #[test]
    fn resolves_iframe_direct_media_without_network() {
        let html = r#"<html><body><div class="videoplay">
            <iframe src="https://plaa.test/player?url=https://cdn.test/direct.m3u8"></iframe>
        </div></body></html>"#;
        let cfg = TvConfig::default();
        let got = resolve_player(&cfg, BASE, "https://www.4kcz.com/v_play/x.html", html);
        assert_eq!(got.as_deref(), Some("https://cdn.test/direct.m3u8"));
    }

    #[test]
    fn player_json_has_parse_zero_and_headers() {
        let v = player_json("https://cdn.test/x.m3u8", BASE);
        assert_eq!(v["parse"], 0);
        assert_eq!(v["url"], "https://cdn.test/x.m3u8");
        assert!(v["header"]["User-Agent"].as_str().unwrap().contains("Android"));
        assert_eq!(v["header"]["Referer"], BASE);
    }
}
