//! `csp_Zhaozy`（找资源）：搜索型蜘蛛 + 阿里云盘引擎详情/播放。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/49fb705c-XbyQ/deobf/com/github/catvod/spider/Zhaozy.java`（187 行、0 skip）
//! * 字符串：`merge.WJz.d(...)`，密钥 `ZySGzj`（`_rea/tools/decode-xor.mjs ZySGzj` 全解）
//! * 站点 `_rea/samples/inventory/sites.json`：3 站（`Zhaozy`、`🦊找资源`、`找資源`）
//!
//! 协议（原类逐段还原）：
//! * `init`：ext 为 JSON → `username`/`password`/`searchurl`；`searchurl` 存在则用其 origin
//!   覆盖基址并更新 host；无 ext 用默认基址。
//! * 登录 `getCookie()`：POST `{base}/logiu.html`，表单 `passwoxx`=password、
//!   `username`=username；取响应 `Set-Cookie` 按 host 聚合成 `;` 连接串（原类缓存复用）。
//! * 搜索：GET `{base}/so?filename={enc}`；行 `div.li_con div.news_text`：名称
//!   `div.news_text a h3`、链接 `div.news_text a[href]`、备注 `div.news_text a p`；
//!   只保留名称含关键词的行；`vod_pic` 为固定封面。
//! * 详情：id 命中阿里分享正则 → 直接展开；否则 GET `{base}/{id}` 再从页面提取分享链接。
//! * 播放：原类委托 `PushAgent`；桌面端走 `push::{detail,player_content}`（`pan` 引擎）。
//! * 首页/分类：原类未覆写 → 纯搜索型（空）。
//!
//! 偏差（4 条，记录在附录 EY）：
//! 1. 原类 `PushAgent` 替换为本项目 `push` 语义（阿里分享匿名浏览/直链等价；其它网盘降级单集）。
//! 2. 登录 Cookie 只取首个 `Set-Cookie` 的 `name=value` 并按名去重拼接（原类用 OkHttp CookieJar）。
//! 3. 上游 `zhaoziyuan.pw` 已不可达 → mock-only，无真站 `#[ignore]`。
//! 4. ext 非 JSON 时按 `$$$` 分段启发式解析（多仓里存在 `token-url$$$账号$$$密码` 形态）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::http::{client_for, get_text};
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};
use super::push;

/// 家族 api 名（规则画像里的 key）。
pub const API: &str = "csp_Zhaozy";
/// 原类硬编码基址（`WJz.d` 解码）。
pub const DEFAULT_BASE: &str = "http://zhaoziyuan.pw";
/// 原类 UA（`WJz.d` 解码）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.0.0 Safari/537.36";
/// 原类固定封面。
const COVER: &str = "https://inews.gtimg.com/newsapp_bt/0/13263837859/1000";
/// 阿里分享链接（原类 `f255`：`(https://www.(alipan|aliyundrive).com/s/[^"]+)`）。
const RE_ALI: &str = r#"https://www\.(?:alipan|aliyundrive)\.com/s/[^"\s]+"#;
/// 登录 Cookie 缓存时长（与原类「会话内复用」等价，桌面端给 1 小时）。
const COOKIE_TTL_MS: u64 = 3_600_000;
/// 详情名（原类 `f262/f263` 只用于登录，详情沿用 `PushAgent` 的分享名）。
const GROUP: &str = "找资源";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn re_ali() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(RE_ALI).expect("RE_ALI"))
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("valid selector")
}

/// ext 解析结果（JSON 或 `$$$` 启发式）。
#[derive(Debug, Clone, Default)]
pub struct Conf {
    pub username: String,
    pub password: String,
    pub searchurl: String,
}

/// 家族接管判定（大小写不敏感，带首尾空白容忍）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 解析 ext：JSON 优先；否则按 `$$$` 分段启发式（`token-url$$$账号$$$密码`）。
pub fn conf_of(site: &Site) -> Conf {
    let ext = site.ext_string();
    let ext = ext.trim();
    if ext.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(ext) {
            return Conf {
                username: v.get("username").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                password: v.get("password").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                searchurl: v.get("searchurl").and_then(Value::as_str).unwrap_or("").trim().to_string(),
            };
        }
    }
    // 启发式：首个「像账号」的段（含 @ 或既非 URL 也非路径）作 username，其下一段作 password；
    // 首个不像 token 文件地址的 http 段作 searchurl。
    let segs: Vec<&str> = ext.split("$$$").map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut conf = Conf::default();
    for (i, seg) in segs.iter().enumerate() {
        if (seg.contains('@') || *seg == "user" || *seg == "username") && conf.username.is_empty() {
            conf.username = seg.to_string();
            if let Some(next) = segs.get(i + 1) {
                conf.password = next.to_string();
            }
        } else if seg.starts_with("http") && conf.searchurl.is_empty() {
            let lower = seg.to_ascii_lowercase();
            if !(lower.contains("token") || lower.contains("/file/") || lower.ends_with(".txt")) {
                conf.searchurl = seg.to_string();
            }
        }
    }
    conf
}

/// 从 URL 取 origin（scheme://host[:port]），失败返回原值。
fn origin_of(url: &str) -> String {
    let Some(scheme) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme + 3..];
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    format!("{}://{}", &url[..scheme], host)
}

/// 基址：`searchurl` 的 origin 优先，否则原类默认基址。
pub fn base_of(conf: &Conf) -> String {
    let searchurl = conf.searchurl.trim();
    if searchurl.is_empty() {
        DEFAULT_BASE.to_string()
    } else {
        origin_of(searchurl)
    }
}

fn login_url(base: &str) -> String {
    format!("{}/logiu.html", base.trim_end_matches('/'))
}

/// 搜索地址：`searchurl` 含 `{wd}` 占位则替换，否则用原类默认 `{base}/so?filename=`。
pub fn search_url(conf: &Conf, base: &str, keyword: &str) -> String {
    let enc = urlencode(keyword);
    let searchurl = conf.searchurl.trim();
    if searchurl.contains("{wd}") {
        searchurl.replace("{wd}", &enc)
    } else if let Some(rest) = searchurl.strip_prefix("{base}") {
        format!("{}{}{}", base.trim_end_matches('/'), rest, enc).replace("filename=", "filename=")
    } else {
        format!("{}/so?filename={}", base.trim_end_matches('/'), enc)
    }
}

fn detail_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http") {
        return id.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), id.trim_start_matches('/'))
}

fn headers(cookie: &str, base: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map.insert("Referer".to_string(), format!("{}/", base.trim_end_matches('/')));
    if !cookie.trim().is_empty() {
        map.insert("Cookie".to_string(), cookie.trim().to_string());
    }
    map
}

/// 原类 `URLEncoder.encode(str, "UTF-8")` 的等价实现（空格转 `+`）。
pub fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            b => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

static COOKIES: OnceLock<Mutex<HashMap<String, (String, u64)>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, (String, u64)>> {
    COOKIES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_cookie(base: &str) -> Option<String> {
    let guard = cache().lock().ok()?;
    let (cookie, at) = guard.get(base)?;
    if now_ms().saturating_sub(*at) < COOKIE_TTL_MS {
        Some(cookie.clone())
    } else {
        None
    }
}

fn store_cookie(base: &str, cookie: &str) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(base.to_string(), (cookie.to_string(), now_ms()));
    }
}

/// 登录并返回 Cookie；账号/密码为空时匿名（原类无账号则跳过登录）。
pub fn login(cfg: &TvConfig, site: &Site, conf: &Conf, base: &str) -> Result<String> {
    if conf.username.trim().is_empty() || conf.password.trim().is_empty() {
        return Ok(String::new());
    }
    if let Some(cookie) = cached_cookie(base) {
        return Ok(cookie);
    }
    let client = client_for(&cfg.http)?;
    let resp = client
        .post(login_url(base))
        .header("User-Agent", UA)
        .header("Referer", format!("{}/", base.trim_end_matches('/')))
        .form(&[
            ("passwoxx", conf.password.as_str()),
            ("username", conf.username.as_str()),
        ])
        .send()?;
    let mut parts: Vec<(String, String)> = Vec::new();
    for raw in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let Ok(raw) = raw.to_str() else { continue };
        let Some(pair) = raw.split(';').next() else { continue };
        let Some((name, value)) = pair.split_once('=') else { continue };
        let name = name.trim().to_string();
        if name.is_empty() || parts.iter().any(|(k, _)| *k == name) {
            continue;
        }
        parts.push((name, value.trim().to_string()));
    }
    let cookie = parts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    if !cookie.is_empty() {
        store_cookie(base, &cookie);
    } else {
        let _ = site;
    }
    Ok(cookie)
}

fn page_text(cfg: &TvConfig, url: &str, base: &str, cookie: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(cookie, base), 2)
}

/// 搜索页解析（纯函数，便于单测）。
pub fn parse_search_html(html: &str, keyword: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = sel("div.li_con div.news_text");
    let name_sel = sel("div.news_text a h3");
    let link_sel = sel("div.news_text a");
    let remark_sel = sel("div.news_text a p");
    let kw = keyword.trim();
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let name = row
            .select(&name_sel)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if !kw.is_empty() && !name.contains(kw) {
            continue;
        }
        let href = row
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .unwrap_or("")
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if href.is_empty() {
            continue;
        }
        let remark = row
            .select(&remark_sel)
            .next()
            .map(|p| p.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: href,
            name,
            cover: Some(COVER.to_string()),
            remark,
        });
    }
    out
}

/// 从详情页提取阿里分享链接（纯函数）。
pub fn extract_ali(page: &str) -> Option<String> {
    re_ali().find(page).map(|m| m.as_str().to_string())
}

pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(Vec::new())
}

pub fn videos(_cfg: &TvConfig, _site: &Site, _type_id: &str, _page: u32) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let conf = conf_of(site);
    let base = base_of(&conf);
    let cookie = login(cfg, site, &conf, &base)?;
    let url = search_url(&conf, &base, keyword);
    let html = page_text(cfg, &url, &base, &cookie)?;
    Ok(parse_search_html(&html, keyword))
}

/// 详情：阿里分享链接直接交给 `push`（等价原类 `PushAgent.detailContent`）；
/// 否则抓详情页提取分享链接。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let share = if re_ali().is_match(id) {
        id.to_string()
    } else {
        let conf = conf_of(site);
        let base = base_of(&conf);
        let cookie = login(cfg, site, &conf, &base)?;
        let page = page_text(cfg, &detail_url(&base, id), &base, &cookie)?;
        extract_ali(&page).ok_or_else(|| parse_err(site, "详情页未找到阿里分享链接"))?
    };
    let mut detail = push::detail(cfg, site, &share)?;
    if detail.name.trim().is_empty() || detail.name == share {
        detail.name = format!("{} {}", GROUP, share.rsplit('/').next().unwrap_or("资源"));
    }
    Ok(detail)
}

pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    push::player_content(cfg, site, flag, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        Site {
            key: "zhaozy".into(),
            name: "找资源".into(),
            api: API.into(),
            ext: if ext.is_empty() {
                None
            } else {
                Some(serde_json::Value::String(ext.into()))
            },
            ..Site::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Zhaozy"));
        assert!(handles(" CSP_ZHAOZY "));
        assert!(!handles("csp_Zhaozy2"));
    }

    #[test]
    fn conf_of_reads_json() {
        let conf = conf_of(&site(
            r#"{"username":"u1","password":"p1","searchurl":"https://example.com/so?filename={wd}"}"#,
        ));
        assert_eq!(conf.username, "u1");
        assert_eq!(conf.password, "p1");
        assert_eq!(conf.searchurl, "https://example.com/so?filename={wd}");
    }

    #[test]
    fn conf_of_empty_uses_defaults() {
        let conf = conf_of(&site(""));
        assert!(conf.username.is_empty());
        assert!(conf.password.is_empty());
        assert!(conf.searchurl.is_empty());
        assert_eq!(base_of(&conf), DEFAULT_BASE);
    }

    #[test]
    fn conf_of_reads_dollar_segments() {
        let conf = conf_of(&site(
            "http://127.0.0.1:9978/file/tvfan/token.txt+4k|auto|fhd$$$lmok37266@bazatek.com$$$Yoursmile",
        ));
        assert_eq!(conf.username, "lmok37266@bazatek.com");
        assert_eq!(conf.password, "Yoursmile");
        // token 文件地址不是搜索地址 → 保持默认基址
        assert!(conf.searchurl.is_empty());
        assert_eq!(base_of(&conf), DEFAULT_BASE);
    }

    #[test]
    fn base_follows_searchurl_origin() {
        let conf = Conf {
            searchurl: "https://so.example.com/api?q={wd}".into(),
            ..Conf::default()
        };
        assert_eq!(base_of(&conf), "https://so.example.com");
    }

    #[test]
    fn search_url_default_and_template() {
        let empty = Conf::default();
        assert_eq!(
            search_url(&empty, DEFAULT_BASE, "三体"),
            "http://zhaoziyuan.pw/so?filename=%E4%B8%89%E4%BD%93"
        );
        let conf = Conf {
            searchurl: "https://so.example.com/s?q={wd}".into(),
            ..Conf::default()
        };
        assert_eq!(
            search_url(&conf, "https://so.example.com", "abc"),
            "https://so.example.com/s?q=abc"
        );
    }

    #[test]
    fn detail_url_keeps_absolute() {
        assert_eq!(detail_url("http://x", "abc/1.html"), "http://x/abc/1.html");
        assert_eq!(detail_url("http://x", "/abc/1.html"), "http://x/abc/1.html");
        assert_eq!(
            detail_url("http://x", "https://www.alipan.com/s/xyz"),
            "https://www.alipan.com/s/xyz"
        );
    }

    #[test]
    fn urlencode_handles_space_and_utf8() {
        assert_eq!(urlencode("a b"), "a+b");
        assert_eq!(urlencode("三"), "%E4%B8%89");
        assert_eq!(urlencode("a_b-c.d~e"), "a_b-c.d~e");
    }

    #[test]
    fn headers_include_cookie_when_present() {
        let map = headers("sid=1", "http://zhaoziyuan.pw");
        assert_eq!(map.get("Cookie").map(String::as_str), Some("sid=1"));
        assert_eq!(
            map.get("Referer").map(String::as_str),
            Some("http://zhaoziyuan.pw/")
        );
        let empty = headers("", "http://zhaoziyuan.pw");
        assert!(!empty.contains_key("Cookie"));
    }

    #[test]
    fn parses_search_rows_and_filters_keyword() {
        let html = r#"
        <div class="li_con"><div class="news_text">
          <a href="https://www.alipan.com/s/AAA"><h3>三体 全集</h3><p>2024-01-02</p></a>
        </div></div>
        <div class="li_con"><div class="news_text">
          <a href="/other/1.html"><h3>别的资源</h3><p>备注</p></a>
        </div></div>"#;
        let rows = parse_search_html(html, "三体");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "https://www.alipan.com/s/AAA");
        assert_eq!(rows[0].name, "三体 全集");
        assert_eq!(rows[0].cover.as_deref(), Some(COVER));
        assert_eq!(rows[0].remark.as_deref(), Some("2024-01-02"));
    }

    #[test]
    fn extract_ali_finds_first_share() {
        let page = r#"<p>下载 <a href="https://www.aliyundrive.com/s/AbC123">点此</a>
        另有 https://www.alipan.com/s/XyZ789 备用"#;
        assert_eq!(
            extract_ali(page).as_deref(),
            Some("https://www.aliyundrive.com/s/AbC123")
        );
        assert!(extract_ali("<html>没有分享</html>").is_none());
    }

    #[test]
    fn empty_detail_id_is_rejected() {
        let cfg = TvConfig::default();
        let err = detail(&cfg, &site(""), "  ").unwrap_err();
        assert!(format!("{err}").contains("catvod"), "got {err}");
    }
}
