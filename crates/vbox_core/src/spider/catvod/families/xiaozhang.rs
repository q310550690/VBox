//! `csp_XiaoZhang`（校长影视 · `https://xzyshd.com`）原生实现。
//!
//! 来源：`_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/XiaoZhang.java`
//! （554 行；`searchContent` 被 jadx 跳过 1144 指令，按现站搜索页重写并记录偏差）。
//! 原类 `extends Push`：继承 App 内的网盘搜索底座，自身覆盖首页分类/列表/详情/播放：
//! * `init`：分类硬编码 4 项（电影 `dy` / 电视剧 `dsj` / 动漫 `dm` / 综艺 `zy`），
//!   ext 形如 `./lib/token.json$$$<base>$$$<proxy>$$$<danmaku>`（第 2 段 >7 字符时覆盖基址）；
//! * `categoryContent`：`{base}/{type}.html?page={page}`，行 `div.list-boxes`
//!   （`img[alt]` 标题、`img[src]` 封面、`a[href]` 详情相对链接）；
//! * `homeVideoContent`：`div.container div.row a:has(>img)`；
//! * `detailContent`：`div.container div.row` 内 `h1` 标题、`img[src]` 封面、
//!   `div.article-infobox` 备注、`div#info`/`div#article_content` 简介；
//!   再把页面内所有 `a[href]` 按网盘域名正则归类（夸克/UC/阿里/迅雷/YD/115），
//!   交给内网盘引擎展开成 `vod_play_from`/`vod_play_url`（`$$$` 连接）；
//! * `playerContent`：按线路名（含「夸克」「迅雷」「YD」「阿里」「UC」）分派到对应网盘播放器，
//!   `parse:0` 直出直链；
//! * 搜索：原方法被跳过；现站搜索已改为 `/browse/?q=`（新模板 `article.resource-card`），
//!   本实现按现站页面解析，详情可能落到新模板 `/item/{slug}/`。
//!
//! 现站（2026-11）核验：`https://xzyshd.com` 200；分类页 4 个均 200/20 条；
//! 旧详情页 `/xzys/subject/{n}.html` 直接给出 `pan.quark.cn` 分享链接；
//! 新详情页 `/item/{slug}/` 把网盘链接藏在 **CSRF POST 表单**后
//! （`form[action=/item/{slug}/link/{id}/open/]` + `csrfmiddlewaretoken`），
//! POST 返回 303 → 真实分享链接，本实现按此取链接（跟随重定向后读最终 URL）。
//!
//! 有意偏差：
//! 1. 迅雷 / YD（天翼云盘）播放引擎在桌面端未实现 → 线路仍展示（可看到分享链接），
//!    播放时给出明确 `catvod.host.unsupported`；
//! 2. 原实现用 `Push` 底座在 App 内并行展开（15s 超时），本实现顺序展开、最多 12 条资源；
//! 3. 弹幕（`danmaku` 通道）与 `Proxy` 不实现（桌面端无对应宿主能力）；
//! 4. 搜索解析按现站新模板（旧站搜索页 `search.html` 已空）。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::json;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::p115::P115Client;
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};

/// 画像里的 api id。
pub const API: &str = "csp_XiaoZhang";
/// 默认基址（原类写死 `https://xzyshd.com`）。
pub const DEFAULT_BASE: &str = "https://xzyshd.com";
/// 请求 UA（原类 Chrome/109 系列）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
/// 分类硬编码（原类 `init` 里的 4 项）。
pub const CATEGORIES: [(&str, &str); 4] = [
    ("电影", "dy"),
    ("电视剧", "dsj"),
    ("动漫", "dm"),
    ("综艺", "zy"),
];

/// 单次详情最多展开的网盘资源数（原实现上限 100 个链接、展开任务 15s 超时）。
const MAX_RESOURCES: usize = 12;
/// 单个分享最多收集的视频文件数。
const MAX_EPISODES: usize = 500;

pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

fn net_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Network, site, detail).into_core()
}

/// 基址：`ext` 第 2 段（原类 `split[1].length() > 7` 时覆盖），否则默认域名。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let segs: Vec<&str> = ext.split("$$$").collect();
    if segs.len() > 1 {
        let seg = segs[1].trim();
        if seg.len() > 7 && (seg.starts_with("http://") || seg.starts_with("https://")) {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), format!("{base}/"));
    h.insert(
        "Accept".to_string(),
        "application/json, text/plain, */*".to_string(),
    );
    h
}

fn header_map(extra: &HashMap<String, String>) -> reqwest::header::HeaderMap {
    let mut hm = reqwest::header::HeaderMap::new();
    for (k, v) in extra {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            hm.insert(name, value);
        }
    }
    hm
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

/// 拉页面并捕获 `Set-Cookie`（新模板的 CSRF POST 需要 `csrftoken` 会话）。
fn fetch_page(cfg: &TvConfig, base: &str, url: &str) -> Result<(String, String)> {
    let client = crate::http::client_for(&cfg.http)?;
    let resp = client
        .get(url)
        .headers(header_map(&headers(base)))
        .send()
        .map_err(|e| net_err(&Site::default(), format!("请求失败 {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(net_err(
            &Site::default(),
            format!("HTTP {} -> {url}", resp.status().as_u16()),
        ));
    }
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let html = resp
        .text()
        .map_err(|e| net_err(&Site::default(), format!("读取响应失败 {url}: {e}")))?;
    Ok((html, cookie))
}

/// 分类：原类硬编码 4 项，不发请求。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(CATEGORIES
        .iter()
        .map(|(name, id)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect())
}

/// 列表 URL：`{base}/{type}.html?page={page}`（原类 `str + ".html?page=" + str2`）。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let cat = cat.trim().trim_matches('/');
    format!("{base}/{cat}.html?page={page}")
}

fn absolute(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if href.starts_with('/') {
        format!("{base}{href}")
    } else {
        format!("{base}/{href}")
    }
}

/// 分类视频列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cat, page);
    let html = fetch_text(cfg, &base, &url)?;
    let items = parse_list(&html, &base);
    if items.is_empty() {
        return Err(parse_err(site, "分类页未解析出条目（页面结构可能已改版）"));
    }
    Ok(items)
}

/// 列表行：`div.list-boxes` 块内取 `a[href]` / `img[src]` / `img[alt]`（旧模板）。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    static HREF: OnceLock<Regex> = OnceLock::new();
    static SRC: OnceLock<Regex> = OnceLock::new();
    static ALT: OnceLock<Regex> = OnceLock::new();
    static SPAN: OnceLock<Regex> = OnceLock::new();
    let href_re = HREF.get_or_init(|| Regex::new(r#"(?i)href="([^"]+)""#).expect("href"));
    let src_re = SRC.get_or_init(|| Regex::new(r#"(?i)src="([^"]+)""#).expect("src"));
    let alt_re = ALT.get_or_init(|| Regex::new(r#"(?i)alt="([^"]*)""#).expect("alt"));
    let span_re = SPAN.get_or_init(|| {
        Regex::new(r#"(?s)<span[^>]*class="[^"]*text_title_p[^"]*"[^>]*>([^<]+)</span>"#)
            .expect("span")
    });
    let mut out: Vec<VideoItem> = Vec::new();
    for chunk in html.split(r#"class="list-boxes""#).skip(1) {
        let chunk = &chunk[..chunk.len().min(1600)];
        let Some(href) = href_re.captures(chunk).map(|c| c[1].to_string()) else {
            continue;
        };
        let name = alt_re
            .captures(chunk)
            .map(|c| c[1].trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| span_re.captures(chunk).map(|c| c[1].trim().to_string()))
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let cover = src_re
            .captures(chunk)
            .map(|c| absolute(base, &c[1]))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: absolute(base, &href),
            name,
            cover,
            remark: None,
        });
        if out.len() >= 100 {
            break;
        }
    }
    out
}

/* ============================================================
 * 网盘归类（原类 Quark/UC/Ali/Thunder/YD/P115 的域名正则）
 * ============================================================ */

/// 页面里识别出的原始网盘类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawProvider {
    Quark,
    Uc,
    Ali,
    P115,
    Thunder,
    Yd,
}

impl RawProvider {
    /// 原实现的线路显示名（`playerContent` 用 `contains` 匹配这些名字）。
    pub fn display(self) -> &'static str {
        match self {
            RawProvider::Quark => "夸克",
            RawProvider::Uc => "UC",
            RawProvider::Ali => "阿里",
            RawProvider::P115 => "115",
            RawProvider::Thunder => "迅雷",
            RawProvider::Yd => "YD",
        }
    }

    /// 桌面端已实现的引擎（迅雷/YD 暂无）。
    pub fn pan(self) -> Option<PanProvider> {
        match self {
            RawProvider::Quark => Some(PanProvider::Quark),
            RawProvider::Uc => Some(PanProvider::Uc),
            RawProvider::Ali => Some(PanProvider::Ali),
            RawProvider::P115 => Some(PanProvider::P115),
            RawProvider::Thunder | RawProvider::Yd => None,
        }
    }
}

/// 按域名归类网盘分享链接。
pub fn classify_link(raw: &str) -> Option<RawProvider> {
    let s = raw.trim().to_ascii_lowercase();
    if s.contains("pan.quark.cn/s/") || s.contains("quark.cn/s/") {
        Some(RawProvider::Quark)
    } else if s.contains("drive.uc.cn/s/") || s.contains("pc.uc.cn/s/") {
        Some(RawProvider::Uc)
    } else if s.contains("alipan.com/s/") || s.contains("aliyundrive.com/s/") {
        Some(RawProvider::Ali)
    } else if s.contains("115.com/s/") || s.contains("115cdn.com/s/") {
        Some(RawProvider::P115)
    } else if s.contains("pan.xunlei.com/s/") {
        Some(RawProvider::Thunder)
    } else if s.contains("cloud.189.cn/") {
        Some(RawProvider::Yd)
    } else {
        None
    }
}

/* ============================================================
 * 详情页解析（旧模板 / 新模板）
 * ============================================================ */

/// 旧模板 `/xzys/subject/{n}.html` 的解析结果。
#[derive(Debug, Default, Clone)]
pub struct LegacyDetail {
    pub name: String,
    pub cover: Option<String>,
    pub desc: Option<String>,
    /// `(线路显示名, 分享链接)`，按页面顺序去重。
    pub links: Vec<(String, String)>,
}

fn strip_tags(raw: &str) -> String {
    static TAGS: OnceLock<Regex> = OnceLock::new();
    let re = TAGS.get_or_init(|| Regex::new(r"(?s)<[^>]+>").expect("tags"));
    let text = re.replace_all(raw, " ");
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_capture(html: &str, pattern: &str) -> Option<String> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let re = {
        let mut guard = cache.lock().ok()?;
        guard
            .entry(pattern.to_string())
            .or_insert_with(|| Regex::new(pattern).expect("dynamic regex"))
            .clone()
    };
    re.captures(html).map(|c| c[1].to_string())
}

/// 旧模板详情：`h1` 标题、首图封面、`div.article-infobox` 备注、`a[href]` 里的网盘链接。
pub fn parse_legacy_detail(html: &str, base: &str) -> LegacyDetail {
    static HREF: OnceLock<Regex> = OnceLock::new();
    let href_re = HREF.get_or_init(|| Regex::new(r#"(?i)href="([^"]+)""#).expect("href"));
    let name = first_capture(html, r"(?is)<h1[^>]*>(.*?)</h1>")
        .map(|s| strip_tags(&s))
        .unwrap_or_default();
    let cover = first_capture(html, r#"(?is)<img[^>]+src="([^"]+)""#).map(|s| absolute(base, &s));
    let desc = first_capture(
        html,
        r#"(?is)<div[^>]*class="[^"]*article-infobox[^"]*"[^>]*>(.*?)</div>"#,
    )
    .or_else(|| first_capture(html, r#"(?is)<div[^>]*id="info"[^>]*>(.*?)</div>"#))
    .or_else(|| first_capture(html, r#"(?is)<div[^>]*id="article_content"[^>]*>(.*?)</div>"#))
    .map(|s| strip_tags(&s))
    .filter(|s| !s.is_empty());

    let mut links: Vec<(String, String)> = Vec::new();
    for cap in href_re.captures_iter(html) {
        let raw = cap[1].trim();
        let Some(kind) = classify_link(raw) else {
            continue;
        };
        let url = absolute(base, raw);
        if links.iter().any(|(_, u)| *u == url) {
            continue;
        }
        links.push((kind.display().to_string(), url));
    }
    LegacyDetail {
        name,
        cover,
        desc,
        links,
    }
}

/// 新模板 `/item/{slug}/` 的「打开网盘」表单卡片。
#[derive(Debug, Default, Clone)]
pub struct NewCard {
    pub display: String,
    pub action: String,
}

/// 新模板详情解析结果。
#[derive(Debug, Default, Clone)]
pub struct NewDetail {
    pub name: String,
    pub cover: Option<String>,
    pub desc: Option<String>,
    pub csrf: Option<String>,
    pub cards: Vec<NewCard>,
}

/// 新模板详情：`detail-intro h1`、`detail-cover img[src]`、`detail-summary`，
/// `link-card` 卡片里的 `b` 线路名 + `form[action]` 打开网盘动作。
pub fn parse_new_detail(html: &str) -> NewDetail {
    static CARD: OnceLock<Regex> = OnceLock::new();
    static B: OnceLock<Regex> = OnceLock::new();
    static ACTION: OnceLock<Regex> = OnceLock::new();
    let card_re = CARD.get_or_init(|| Regex::new(r#"(?is)<section[^>]*class="[^"]*link-card[^"]*".*?</section>"#).expect("card"));
    let b_re = B.get_or_init(|| Regex::new(r"(?is)<b[^>]*>(.*?)</b>").expect("b"));
    let action_re = ACTION.get_or_init(|| Regex::new(r#"(?is)<form[^>]*action="([^"]+)""#).expect("action"));
    let name = first_capture(
        html,
        r#"(?is)<div[^>]*class="[^"]*detail-intro[^"]*".*?<h1[^>]*>(.*?)</h1>"#,
    )
        .or_else(|| first_capture(html, r"(?is)<h1[^>]*>(.*?)</h1>"))
        .map(|s| strip_tags(&s))
        .unwrap_or_default();
    let cover = first_capture(
        html,
        r#"(?is)<div[^>]*class="[^"]*detail-cover[^"]*"[^>]*>.*?<img[^>]+src="([^"]+)""#,
    );
    let desc = first_capture(
        html,
        r#"(?is)<p[^>]*class="[^"]*detail-summary[^"]*"[^>]*>(.*?)</p>"#,
    )
    .map(|s| strip_tags(&s))
    .filter(|s| !s.is_empty());
    let csrf = first_capture(
        html,
        r#"name="csrfmiddlewaretoken" value="([^"]+)""#,
    );
    let mut cards: Vec<NewCard> = Vec::new();
    for cap in card_re.captures_iter(html) {
        let block = &cap[0];
        let display = b_re
            .captures(block)
            .map(|c| strip_tags(&c[1]))
            .unwrap_or_default();
        let Some(action) = action_re.captures(block).map(|c| c[1].to_string()) else {
            continue;
        };
        if cards.iter().any(|c| c.action == action) {
            continue;
        }
        cards.push(NewCard { display, action });
    }
    NewDetail {
        name,
        cover,
        desc,
        csrf,
        cards,
    }
}

/// 在 HTML/文本里找第一个网盘分享链接（POST 返回页面时的兜底）。
fn find_pan_url(text: &str) -> Option<String> {
    static URL: OnceLock<Regex> = OnceLock::new();
    let re = URL.get_or_init(|| Regex::new(r#"https?://[^\s"'<>\\]+"#).expect("url"));
    for m in re.find_iter(text) {
        let raw = m.as_str().trim_end_matches([')', ',', ';']);
        if classify_link(raw).is_some() {
            return Some(raw.to_string());
        }
    }
    None
}

/// 打开新模板的「网盘链接」：POST CSRF 表单并跟随 303，返回真实分享链接。
fn open_pan_link(
    cfg: &TvConfig,
    base: &str,
    referer: &str,
    action: &str,
    csrf: Option<&str>,
    cookie: &str,
) -> Result<Option<String>> {
    let url = absolute(base, action);
    let client = crate::http::client_for(&cfg.http)?;
    let mut hm = header_map(&headers(base));
    if let Ok(value) = reqwest::header::HeaderValue::from_str(referer) {
        hm.insert(reqwest::header::REFERER, value);
    }
    let mut form: Vec<(&str, &str)> = Vec::new();
    if let Some(token) = csrf.filter(|t| !t.is_empty()) {
        form.push(("csrfmiddlewaretoken", token));
    }
    let resp = client
        .post(&url)
        .headers(hm)
        .header(reqwest::header::COOKIE, cookie)
        .form(&form)
        .send()
        .map_err(|e| net_err(&Site::default(), format!("打开网盘表单失败 {url}: {e}")))?;
    let final_url = resp.url().to_string();
    let body = resp.text().unwrap_or_default();
    if final_url != url && classify_link(&final_url).is_some() {
        return Ok(Some(final_url));
    }
    if let Some(found) = find_pan_url(&body) {
        return Ok(Some(found));
    }
    if classify_link(&url).is_some() {
        return Ok(Some(url));
    }
    Ok(None)
}

/// 详情：解析页面 → 把网盘分享交给 W9 引擎展开成剧集。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = absolute(&base, id);
    let (name, cover, desc, links) = if url.contains("/item/") {
        let (html, cookie) = fetch_page(cfg, &base, &url)?;
        let parsed = parse_new_detail(&html);
        if parsed.name.is_empty() {
            return Err(parse_err(site, "详情页缺少标题（页面结构可能已改版）"));
        }
        let mut links: Vec<(String, String)> = Vec::new();
        for card in parsed.cards.iter().take(MAX_RESOURCES) {
            let share = open_pan_link(
                cfg,
                &base,
                &url,
                &card.action,
                parsed.csrf.as_deref(),
                &cookie,
            )?;
            let Some(share) = share else { continue };
            let display = if card.display.is_empty() {
                classify_link(&share)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "网盘".to_string())
            } else {
                card.display.clone()
            };
            links.push((display, share));
        }
        (parsed.name, parsed.cover.map(|c| absolute(&base, &c)), parsed.desc, links)
    } else {
        let html = fetch_text(cfg, &base, &url)?;
        let parsed = parse_legacy_detail(&html, &base);
        if parsed.name.is_empty() {
            return Err(parse_err(site, "详情页缺少标题（页面结构可能已改版）"));
        }
        (parsed.name, parsed.cover, parsed.desc, parsed.links)
    };

    let conf = load_pan_config(site);
    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    for (display, share) in links.into_iter().take(MAX_RESOURCES) {
        let mut items: Vec<Episode> = Vec::new();
        if let Some(kind) = classify_link(&share) {
            if let Some(_provider) = kind.pan() {
                if let Some(link) = parse_share_link(&share) {
                    if let Ok(files) = browse_share(cfg, &conf, &link) {
                        items = files
                            .iter()
                            .filter(|f| !f.is_dir && is_video_name(&f.name))
                            .take(MAX_EPISODES)
                            .map(|f| Episode {
                                title: f.name.clone(),
                                url: format!("{}###{}", link.url, f.fid),
                            })
                            .collect();
                    }
                }
            }
        }
        if items.is_empty() {
            items.push(Episode {
                title: display.clone(),
                url: share.clone(),
            });
        }
        episodes.push(EpisodeGroup {
            group: display,
            items,
        });
    }
    Ok(VideoDetail {
        id: url,
        name,
        cover,
        desc,
        episodes,
        ..Default::default()
    })
}

/* ============================================================
 * 搜索（现站新模板 `/browse/?q=`）
 * ============================================================ */

/// 搜索：现站 `/browse/?q={kw}`（旧站 `search.html` 已空壳，`/search/` 404）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let kw = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let url = format!("{base}/browse/?q={kw}");
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_search(&html, &base))
}

/// 新模板搜索结果：`article.resource-card` 内含媒体链接、标题、封面与评分。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    static CARD: OnceLock<Regex> = OnceLock::new();
    static HREF: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static IMG: OnceLock<Regex> = OnceLock::new();
    static SCORE: OnceLock<Regex> = OnceLock::new();
    let card_re = CARD.get_or_init(|| {
        Regex::new(r#"(?is)<article[^>]*class="[^"]*resource-card[^"]*"[^>]*>(.*?)</article>"#)
            .expect("card")
    });
    let href_re = HREF.get_or_init(|| Regex::new(r#"(?i)<a[^>]+href="([^"]+)""#).expect("href"));
    let title_re =
        TITLE.get_or_init(|| Regex::new(r"(?is)<h3[^>]*>(.*?)</h3>").expect("title"));
    let img_re = IMG.get_or_init(|| Regex::new(r#"(?is)<img[^>]+src="([^"]+)""#).expect("img"));
    let score_re = SCORE.get_or_init(|| Regex::new(r"(?is)<b[^>]*>\s*([\d.]+)\s*</b>").expect("score"));

    let mut items = Vec::new();
    for cap in card_re.captures_iter(html) {
        let block = &cap[1];
        let Some(href) = href_re.captures(block).map(|c| absolute(base, &c[1])) else {
            continue;
        };
        let title = title_re
            .captures(block)
            .map(|c| strip_tags(&c[1]))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                img_re
                    .captures(block)
                    .map(|c| c[1].to_string())
                    .unwrap_or_default()
            });
        if title.is_empty() {
            continue;
        }
        let cover = img_re.captures(block).map(|c| absolute(base, &c[1]));
        let remark = score_re
            .captures(block)
            .map(|c| format!("豆瓣 {}", &c[1]));
        items.push(VideoItem {
            id: href,
            name: title,
            cover,
            remark,
            ..Default::default()
        });
    }
    items
}

/* ============================================================
 * 播放（网盘分享 → 直链；迅雷/YD 无引擎）
 * ============================================================ */

/// 播放：`id` 形如 `{分享链接}###{fid}`（详情展开时写入；单集降级时无 fid）。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<serde_json::Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let (link_url, fid) = match id.split_once("###") {
        Some((u, f)) => (u.trim().to_string(), f.trim().to_string()),
        None => (id.to_string(), String::new()),
    };
    let kind = classify_link(&link_url)
        .ok_or_else(|| host_err(site, format!("不支持的播放地址：{link_url}")))?;
    let Some(_provider) = kind.pan() else {
        return Err(host_err(
            site,
            format!(
                "{} 网盘播放引擎未实现（桌面端无对应 parser，请在宿主配置网盘 Cookie 后使用对应引擎）",
                kind.display()
            ),
        ));
    };
    let link = parse_share_link(&link_url)
        .ok_or_else(|| host_err(site, format!("不支持或无直链的播放地址：{link_url}")))?;
    let conf = load_pan_config(site);
    let fid = if fid.is_empty() {
        let files = browse_share(cfg, &conf, &link)?;
        files
            .into_iter()
            .find(|f| !f.is_dir && is_video_name(&f.name))
            .ok_or_else(|| host_err(site, "分享内未找到视频文件"))?
            .fid
    } else {
        fid
    };
    let play = resolve_play_url(cfg, &conf, &link, &fid).map_err(|e| {
        let class = ErrorClass::classify_pan_error(&e).unwrap_or(ErrorClass::HostApi);
        CatvodError::new(class, site, format!("网盘直链解析失败：{e}")).into_core()
    })?;
    Ok(json!({
        "url": play,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * W9 网盘引擎适配（与 moli 家族同构）
 * ============================================================ */

/// 从 ext 中取网盘配置（首个以 `{` 开头的段；否则空配置）。
pub fn load_pan_config(site: &Site) -> PanConfig {
    for seg in site.ext_string().split("$$$") {
        let seg = seg.trim();
        if seg.starts_with('{') {
            return PanConfig::parse(seg).with_overlay();
        }
    }
    PanConfig::default()
}

fn browse_share(
    cfg: &TvConfig,
    conf: &PanConfig,
    link: &crate::pan::ShareLink,
) -> Result<Vec<PanEntry>> {
    match link.provider {
        PanProvider::Quark => {
            let c = QuarkClient::with_rules(&cfg.http, conf.effective_cookie())?;
            Ok(c.browse(link)?.files)
        }
        PanProvider::Uc => {
            let c = UcClient::with_rules(&cfg.http, conf.effective_uc_cookie())?;
            Ok(c.browse(link)?.files)
        }
        PanProvider::Ali => {
            let c = AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())?;
            Ok(c.browse(link)?.files)
        }
        PanProvider::P115 => {
            let c = P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?;
            Ok(c.browse(link)?.files)
        }
    }
}

fn resolve_play_url(
    cfg: &TvConfig,
    conf: &PanConfig,
    link: &crate::pan::ShareLink,
    fid: &str,
) -> Result<String> {
    match link.provider {
        PanProvider::Quark => {
            let c = QuarkClient::with_rules(&cfg.http, conf.effective_cookie())?;
            if let Ok(info) = c.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            c.download_url(fid)
        }
        PanProvider::Uc => {
            let c = UcClient::with_rules(&cfg.http, conf.effective_uc_cookie())?;
            if let Ok(info) = c.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            c.download_url(fid)
        }
        PanProvider::Ali => {
            let c = AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())?;
            let token = c.share_token(&link.share_id, &link.passcode)?;
            c.download_url(&link.share_id, fid, &token.share_token)
        }
        PanProvider::P115 => {
            let c = P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?;
            c.download_url(&link.share_id, &link.passcode, fid)
        }
    }
}

/// 递归找 URL 字段（引擎返回结构多层嵌套）。
fn find_url(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) if s.starts_with("http") => Some(s.clone()),
        serde_json::Value::Array(a) => a.iter().find_map(find_url),
        serde_json::Value::Object(m) => {
            for key in ["url", "play_url", "download_url", "raw_url"] {
                if let Some(serde_json::Value::String(s)) = m.get(key) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            m.values().find_map(find_url)
        }
        _ => None,
    }
}

fn is_video_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "mp4" | "mkv" | "m3u8" | "avi" | "ts" | "flv" | "wmv" | "mov" | "rmvb" | "webm"
                | "m4v" | "mpg" | "mpeg" | "iso"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "xiaozhang".into();
        s.name = "校长影视".into();
        s.api = API.into();
        s.ext = Some(json!(ext));
        s
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_XiaoZhang"));
        assert!(handles("csp_xiaozhang"));
        assert!(!handles("csp_Douban"));
    }

    #[test]
    fn base_defaults_and_ext_override() {
        assert_eq!(base_of(&site("")), DEFAULT_BASE);
        assert_eq!(
            base_of(&site("./lib/token.json$$$https://xzyshd.com/$$$null")),
            "https://xzyshd.com"
        );
        // 第 2 段太短（如 ext 里常见的 "1"）时不覆盖基址。
        assert_eq!(base_of(&site("./lib/token.json$$$1$$$null")), DEFAULT_BASE);
    }

    #[test]
    fn categories_are_fixed_four() {
        let cats = categories(&TvConfig::default(), &site("")).expect("categories");
        assert_eq!(cats.len(), 4);
        assert_eq!(cats[0].type_id, "dy");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[3].type_id, "zy");
    }

    #[test]
    fn list_url_has_page_parameter() {
        assert_eq!(
            list_url(DEFAULT_BASE, "dy", 1),
            format!("{DEFAULT_BASE}/dy.html?page=1")
        );
        assert_eq!(
            list_url(DEFAULT_BASE, "/dsj/", 3),
            format!("{DEFAULT_BASE}/dsj.html?page=3")
        );
    }

    #[test]
    fn parse_list_reads_boxes() {
        let html = r#"
        <div class="list-boxes"><div class="left_ly">
          <a href="/xzys/subject/37396.html"><img class="image_left" src="https://img.binhd.com/a.webp" alt="另一个妈妈"><span class="text_title_p">另一个妈妈</span></a>
        </div></div>
        <div class="list-boxes"><div class="left_ly">
          <a href="/xzys/subject/37397.html"><img class="image_left" src="/cover/b.jpg" alt=""><span class="text_title_p">备用标题</span></a>
        </div></div>
        "#;
        let items = parse_list(html, DEFAULT_BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "另一个妈妈");
        assert_eq!(items[0].id, format!("{DEFAULT_BASE}/xzys/subject/37396.html"));
        assert_eq!(items[0].cover.as_deref(), Some("https://img.binhd.com/a.webp"));
        assert_eq!(items[1].name, "备用标题");
        let expected_cover = format!("{DEFAULT_BASE}/cover/b.jpg");
        assert_eq!(items[1].cover.as_deref(), Some(expected_cover.as_str()));
    }

    #[test]
    fn classify_link_recognizes_providers() {
        assert_eq!(classify_link("https://pan.quark.cn/s/abc"), Some(RawProvider::Quark));
        assert_eq!(classify_link("https://drive.uc.cn/s/abc"), Some(RawProvider::Uc));
        assert_eq!(classify_link("https://www.alipan.com/s/abc"), Some(RawProvider::Ali));
        assert_eq!(classify_link("https://115.com/s/abc"), Some(RawProvider::P115));
        assert_eq!(
            classify_link("https://pan.xunlei.com/s/abc?pwd=1#"),
            Some(RawProvider::Thunder)
        );
        assert_eq!(classify_link("https://cloud.189.cn/t/abc"), Some(RawProvider::Yd));
        assert_eq!(classify_link("https://example.com/v/1.html"), None);
        assert_eq!(RawProvider::Quark.display(), "夸克");
        assert_eq!(RawProvider::Thunder.pan(), None);
    }

    #[test]
    fn parse_legacy_detail_collects_links() {
        let html = r#"<div class="container tc-main"><div class="row">
          <div class="article-box"><h1>另一个妈妈</h1>
          <p><a href="https://pan.quark.cn/s/c8a50b6b7014"><button>夸克网盘</button></a></p>
          <p><a href="https://pan.xunlei.com/s/VP69YAJe?pwd=jwfc#">迅雷网盘</a> 提取码：jwfc</p>
          <p><a href="/xzys/subject/1.html">相关</a></p>
          </div></div></div>"#;
        let parsed = parse_legacy_detail(html, DEFAULT_BASE);
        assert_eq!(parsed.name, "另一个妈妈");
        assert_eq!(parsed.links.len(), 2);
        assert_eq!(parsed.links[0].0, "夸克");
        assert_eq!(parsed.links[0].1, "https://pan.quark.cn/s/c8a50b6b7014");
        assert_eq!(parsed.links[1].0, "迅雷");
    }

    #[test]
    fn parse_new_detail_reads_csrf_and_cards() {
        let html = r#"
        <div class="detail-cover"><img src="/media/c.jpg"></div>
        <div class="detail-intro"><h1>妈妈</h1><p class="detail-summary">剧情简介。</p></div>
        <div class="download-panel">
          <section class="link-card"><b>夸克网盘</b>
            <form method="post" action="/item/legacy-post-21766/link/39704/open/">
              <input type="hidden" name="csrfmiddlewaretoken" value="tok123">
            </form></section>
          <section class="link-card"><b>迅雷网盘</b>
            <form method="post" action="/item/legacy-post-21766/link/39705/open/"></form></section>
        </div>"#;
        let parsed = parse_new_detail(html);
        assert_eq!(parsed.name, "妈妈");
        assert_eq!(parsed.cover.as_deref(), Some("/media/c.jpg"));
        assert_eq!(parsed.desc.as_deref(), Some("剧情简介。"));
        assert_eq!(parsed.csrf.as_deref(), Some("tok123"));
        assert_eq!(parsed.cards.len(), 2);
        assert_eq!(parsed.cards[0].display, "夸克网盘");
        assert_eq!(
            parsed.cards[1].action,
            "/item/legacy-post-21766/link/39705/open/"
        );
    }

    #[test]
    fn find_pan_url_from_text() {
        let text = r#"<a href="https://pan.quark.cn/s/ffb68a825564">点击</a>"#;
        assert_eq!(
            find_pan_url(text).as_deref(),
            Some("https://pan.quark.cn/s/ffb68a825564")
        );
        assert_eq!(find_pan_url("<p>没有链接</p>"), None);
    }

    #[test]
    fn parse_search_reads_cards() {
        let html = r#"
        <article class="resource-card">
          <a class="resource-card__media" href="/item/legacy-post-21766/"><img src="/m/1.jpg" alt="妈妈封面"></a>
          <h3><a href="/item/legacy-post-21766/">妈妈</a></h3><b>7.1</b>
        </article>
        <article class="resource-card">
          <a class="resource-card__media" href="/item/x/"><img src="https://img.example/2.jpg" alt="x"></a>
          <h3><a href="/item/x/">另一部</a></h3>
        </article>"#;
        let items = parse_search(html, DEFAULT_BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "妈妈");
        assert_eq!(items[0].id, format!("{DEFAULT_BASE}/item/legacy-post-21766/"));
        assert_eq!(items[0].remark.as_deref(), Some("豆瓣 7.1"));
        assert_eq!(items[1].name, "另一部");
        assert_eq!(items[1].remark, None);
        assert!(items[0]
            .cover
            .as_deref()
            .unwrap()
            .starts_with(&format!("{DEFAULT_BASE}/m/1.jpg")));
    }

    #[test]
    fn player_rejects_thunder_with_unsupported_code() {
        let err = player_content(
            &TvConfig::default(),
            &site("./lib/token.json"),
            "迅雷",
            "https://pan.xunlei.com/s/VP69YAJe###1",
        )
        .expect_err("thunder unsupported");
        let text = err.to_string();
        assert!(text.contains("迅雷"), "err={text}");
        assert!(text.contains("网盘播放引擎未实现"), "err={text}");
    }

    #[test]
    fn player_rejects_unknown_link() {
        let err = player_content(
            &TvConfig::default(),
            &site(""),
            "夸克",
            "https://example.com/play/1",
        )
        .expect_err("unknown link");
        assert!(err.to_string().contains("不支持的播放地址"));
    }
}
