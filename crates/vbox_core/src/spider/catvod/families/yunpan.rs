//! `csp_YunPan` 原生实现（云盘「弹幕 | 云盘」聚合搜索站）。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/YunPan.java`
//!   （jadx 429 行；vineflower 全量恢复 600 行、字符串 42 条全部解出，
//!   见 `_rea/tmp/r129/out-YunPan/YunPan.decoded.java`）。
//! * 站点 `_rea/samples/inventory/sites.json`：1 站 `YunPan`（「弹幕 | 云盘[jar]」），
//!   ext `./lib/token.json$$$null$$$proxy$$$1`。
//!
//! 协议（原类逐段还原）：
//! * `extends Push`；ext 按 `$$$` 切：段 1 长度 > 7 时覆盖基址、段 2 为代理地址
//!   （`C0063a.i` → `e.z(proxy)`）、段 3 为弹幕开关（`"1"`）。
//! * 首页 `homeContent`：GET 基址 → 固定 1 个分类「首页」(`type_id="/"`)，同页解析卡片列表。
//! * 分类 `categoryContent`：GET `{base}/?PageIndex={page}&pageSize=20&Keyword&Type=&Tag=`。
//! * 搜索 `searchContent`：GET `{base}/?PageIndex=1&pageSize=20&Keyword={kw}&Type=&Tag=`。
//! * 卡片解析（`b(hVar)`）：`div.row > div.col`；`a[onclick~=window.open]` 的 onclick
//!   用正则 `window.open\(['"]([^'"]*)['"]\)` 取分享链接；按域名分类
//!   阿里/夸克/迅雷/移动（YD），其它丢弃；标题 = `img` 的 `alt`；封面 = `img` 的 `src`
//!   （原类经代理 `W.a.s`）；备注 = `{网盘}/{.card-footer>small}/{.card-body 第 3 个 p}`。
//!   条目 id = `分享链接###标题`。
//! * 详情 `detailContent`：id 按 `###` 拆成「链接串 + 标题」，链接串按 `;` 拆分，
//!   只收集 阿里/夸克/迅雷（原类如此，YD 不进入详情），分别用 `super.a/b/c` 引擎
//!   展开成线路（`$$$` 连线路、`#` 连剧集）。
//! * 播放 `playerContent`：线路名含「夸克/迅雷/YD」分派，否则阿里；弹幕开启时原类
//!   另包一层 `V.h.h` 弹幕注入（Android WebView 能力）。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. 迅雷云盘与天翼云盘没有 W9 引擎：详情保留线路、播放返回
//!    `catvod.host.unsupported`（与 xiaozhang/wogg/xpanpan 同策略）；
//! 2. `proxy`（段 2）与 `./lib/token.json`（段 0）不参与请求（桌面端不做本地代理）；
//! 3. 弹幕分支不实现；
//! 4. 站点 `https://res.yunpan.win/` 在 2026-11 探测为 HTTP 200 但 0 字节
//!    （面板空响应），因此本家族只用 mock 夹具验收，无真站 `#[ignore]` 用例。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::p115::P115Client;
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};

/// 本家族覆盖的 api。
pub const API: &str = "csp_YunPan";
/// 默认基址（ext 段 1 可覆盖）。
pub const DEFAULT_BASE: &str = "https://res.yunpan.win";
/// 请求 UA（原类固定 Chrome/122）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 每页条数（原类 `pageSize=20`）。
pub const PAGE_SIZE: u32 = 20;
/// 详情最多展开的分享数（原类上限 100，此处保守）。
const MAX_RESOURCES: usize = 12;
/// 剧集上限。
const MAX_EPISODES: usize = 500;

/// 判断 api 是否属于本家族（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case("csp_YunPan")
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// ext 分段（`$$$`）。
fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

/// 基址：ext 段 1 长度 > 7 且以 http 开头时覆盖（原类 `split[1].trim().length() > 7`）。
pub fn base_of(site: &Site) -> String {
    let segs = ext_segments(site);
    if let Some(seg) = segs.get(1) {
        if seg.len() > 7 && seg.starts_with("http") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

/// 弹幕开关：ext 段 3 为 `1`（原类还要求全局开关未禁用）。
pub fn danmaku_enabled(site: &Site) -> bool {
    ext_segments(site)
        .get(3)
        .map(|s| s == "1")
        .unwrap_or(false)
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map.insert("Referer".to_string(), format!("{}/", base.trim_end_matches('/')));
    map
}

fn absolute(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("https:{href}")
    } else if href.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), href)
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            href.trim_start_matches('/')
        )
    }
}

fn sel(expr: &str) -> Option<Selector> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, &'static Selector>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(s) = map.get(expr) {
            return Some((*s).clone());
        }
    }
    let parsed = Selector::parse(expr).ok()?;
    let leaked: &'static Selector = Box::leak(Box::new(parsed));
    if let Ok(mut map) = cache.lock() {
        map.insert(expr.to_string(), leaked);
    }
    Some(leaked.clone())
}

fn text_of(el: ElementRef) -> String {
    let raw = el.text().collect::<Vec<_>>().join(" ");
    strip_tags(&raw)
}

fn attr_of(el: ElementRef, name: &str) -> String {
    el.value().attr(name).unwrap_or("").trim().to_string()
}

/// 文本标签剥离（卡片标题可能带高亮标签）。
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn onclick_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"window\.open\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap())
}

/// 网盘分类顺序与 `Push` 基类一致：阿里 → 夸克 → 迅雷 → YD。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawProvider {
    Ali,
    Quark,
    Thunder,
    Yd,
}

impl RawProvider {
    pub fn display(self) -> &'static str {
        match self {
            RawProvider::Ali => "阿里",
            RawProvider::Quark => "夸克",
            RawProvider::Thunder => "迅雷",
            RawProvider::Yd => "移动",
        }
    }

    pub fn pan(self) -> Option<PanProvider> {
        match self {
            RawProvider::Ali => Some(PanProvider::Ali),
            RawProvider::Quark => Some(PanProvider::Quark),
            // 迅雷 / 天翼（YD）没有桌面引擎。
            RawProvider::Thunder | RawProvider::Yd => None,
        }
    }
}

/// 按域名分类网盘分享（原类 `Ali.b/Quark.b/Thunder.b/YD.b` 正则顺序）。
pub fn classify_link(raw: &str) -> Option<RawProvider> {
    let s = raw.trim();
    if s.contains("alipan.com/s/") || s.contains("aliyundrive.com/s/") {
        Some(RawProvider::Ali)
    } else if s.contains("pan.quark.cn/s/") {
        Some(RawProvider::Quark)
    } else if s.contains("pan.xunlei.com/s/") {
        Some(RawProvider::Thunder)
    } else if s.contains("cloud.189.cn/") {
        Some(RawProvider::Yd)
    } else {
        None
    }
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 分类页 URL（原类 `categoryContent` 拼接）。
pub fn videos_url(base: &str, page: u32) -> String {
    format!(
        "{}/?PageIndex={}&pageSize={}&Keyword&Type=&Tag=",
        base.trim_end_matches('/'),
        page,
        PAGE_SIZE
    )
}

/// 搜索 URL（原类 `searchContent` 拼接）。
pub fn search_url(base: &str, keyword: &str) -> String {
    format!(
        "{}/?PageIndex=1&pageSize={}&Keyword={}&Type=&Tag=",
        base.trim_end_matches('/'),
        PAGE_SIZE,
        urlencode(keyword)
    )
}

/// 解析卡片列表。
pub fn parse_cards(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let Some(col_sel) = sel("div.row > div.col") else {
        return Vec::new();
    };
    let link_sel = sel("a[onclick]");
    let img_sel = sel("img");
    let small_sel = sel(".card-footer > small");
    let p_sel = sel(".card-body > p");
    let mut out = Vec::new();
    for col in doc.select(&col_sel) {
        let mut onclick = String::new();
        if let Some(link_sel) = link_sel.as_ref() {
            for a in col.select(link_sel) {
                let attr = attr_of(a, "onclick");
                if attr.contains("window.open") {
                    onclick = attr;
                    break;
                }
            }
        }
        let Some(caps) = onclick_re().captures(&onclick) else {
            continue;
        };
        let share = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let Some(provider) = classify_link(&share) else {
            continue;
        };
        let mut title = String::new();
        let mut cover: Option<String> = None;
        if let Some(img_sel) = img_sel.as_ref() {
            if let Some(img) = col.select(img_sel).next() {
                title = attr_of(img, "alt");
                let src = attr_of(img, "src");
                if !src.is_empty() {
                    cover = Some(absolute(base, &src));
                }
            }
        }
        if title.is_empty() {
            // 原类只取 alt；缺失时按分享链接尾部兜底，避免空标题条目。
            title = share
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string();
        }
        let mut small = String::new();
        if let Some(small_sel) = small_sel.as_ref() {
            if let Some(el) = col.select(small_sel).next() {
                small = text_of(el);
            }
        }
        let mut third_p = String::new();
        if let Some(p_sel) = p_sel.as_ref() {
            let ps: Vec<_> = col.select(p_sel).collect();
            if let Some(el) = ps.get(2) {
                third_p = text_of(*el);
            }
        }
        let mut parts = vec![provider.display().to_string()];
        if !small.is_empty() {
            parts.push(small);
        }
        if !third_p.is_empty() {
            parts.push(third_p);
        }
        let remark = parts.join("/");
        out.push(VideoItem {
            id: format!("{share}###{title}"),
            name: title,
            cover,
            remark: Some(remark),
        });
    }
    out
}

/// 拆 `分享链接###标题`（标题可含 `###`，只拆一次）。
pub fn split_id(id: &str) -> Option<(Vec<String>, String)> {
    let (links, title) = id.split_once("###")?;
    let links = links
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if links.is_empty() {
        return None;
    }
    Some((links, title.trim().to_string()))
}

/// 原类只收集 阿里/夸克/迅雷 三类（YD 不进入详情）。
fn detail_provider(provider: RawProvider) -> Option<RawProvider> {
    match provider {
        RawProvider::Ali | RawProvider::Quark | RawProvider::Thunder => Some(provider),
        RawProvider::Yd => None,
    }
}

/*
 * ============================================================
 * 网盘引擎展开（与 xpanpan/share 族同构）
 * ============================================================
 */

pub(crate) fn load_pan_config(site: &Site) -> PanConfig {
    for seg in site.ext_string().split("$$$") {
        let seg = seg.trim();
        if seg.starts_with('{') {
            return PanConfig::parse(seg);
        }
    }
    PanConfig::default()
}

/// Ali 客户端：`PanConfig::ali_base` 非空时接测试/镜像根，否则用引擎默认根。
pub(crate) fn ali_client(cfg: &TvConfig, conf: &PanConfig) -> Result<AliClient> {
    if conf.ali_base.trim().is_empty() {
        AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())
    } else {
        let client = crate::http::client_for(&cfg.http)?;
        Ok(AliClient::with_base(client, conf.effective_ali_cookie(), conf.ali_base.trim()))
    }
}

pub(crate) fn browse_share(cfg: &TvConfig, conf: &PanConfig, link: &crate::pan::ShareLink) -> Result<Vec<PanEntry>> {
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
            let c = ali_client(cfg, conf)?;
            Ok(c.browse(link)?.files)
        }
        PanProvider::P115 => {
            let c = P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?;
            Ok(c.browse(link)?.files)
        }
    }
}

pub(crate) fn is_video_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "mp4" | "mkv" | "m3u8" | "avi" | "ts" | "flv" | "wmv" | "mov" | "rmvb" | "webm"
                | "m4v" | "mpg" | "mpeg" | "iso"
        )
    )
}

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

pub(crate) fn resolve_play_url(cfg: &TvConfig, conf: &PanConfig, link: &crate::pan::ShareLink, fid: &str) -> Result<String> {
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
            let c = ali_client(cfg, conf)?;
            let token = c.share_token(&link.share_id, &link.passcode)?;
            c.download_url(&link.share_id, fid, &token.share_token)
        }
        PanProvider::P115 => {
            let c = P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?;
            c.download_url(&link.share_id, &link.passcode, fid)
        }
    }
}

/// 单条网盘分享 → (线路名, 剧集列表)；引擎展开失败时降级成一条单集。
fn expand_link(cfg: &TvConfig, conf: &PanConfig, provider: RawProvider, url: &str) -> (String, Vec<Episode>) {
    let line = provider.display().to_string();
    let fallback = || {
        vec![Episode {
            title: provider.display().to_string(),
            url: url.to_string(),
        }]
    };
    let Some(pan) = provider.pan() else {
        return (line, fallback());
    };
    let link = match parse_share_link(url) {
        Some(l) if l.provider == pan => l,
        _ => return (line, fallback()),
    };
    let files = match browse_share(cfg, conf, &link) {
        Ok(files) => files,
        Err(_) => return (line, fallback()),
    };
    let mut episodes: Vec<Episode> = files
        .into_iter()
        .filter(|f| !f.is_dir && is_video_name(&f.name))
        .take(MAX_EPISODES)
        .map(|f| Episode {
            title: f.name.clone(),
            url: format!("{}###{}", link.url, f.fid),
        })
        .collect();
    if episodes.is_empty() {
        episodes = fallback();
    }
    (line, episodes)
}

/*
 * ============================================================
 * 5 个宿主入口
 * ============================================================
 */

/// 分类：原类固定 1 个「首页」。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Vec<Category> {
    vec![Category {
        type_id: "/".to_string(),
        type_name: "首页".to_string(),
    }]
}

/// 首页推荐（原类 `homeContent` 里的列表）。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = format!("{}/", base.trim_end_matches('/'));
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_cards(&html, &base))
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, _type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = videos_url(&base, page);
    let html = fetch_text(cfg, &base, &url)?;
    let items = parse_cards(&html, &base);
    if items.is_empty() {
        return Err(parse_err(
            site,
            format!("分类列表未解析出条目（{url}）：页面结构变化或上游空响应"),
        ));
    }
    Ok(items)
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = search_url(&base, keyword);
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_cards(&html, &base))
}

/// 详情：拆 id → 按网盘分类 → 引擎展开成线路。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let Some((links, title)) = split_id(id) else {
        return Err(parse_err(
            site,
            format!("详情 id 缺少 `分享链接###标题` 结构：{id}"),
        ));
    };
    if title.is_empty() {
        return Err(parse_err(site, format!("详情 id 标题为空：{id}")));
    }
    let conf = load_pan_config(site);
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for raw in links.iter().take(MAX_RESOURCES) {
        let Some(provider) = classify_link(raw).and_then(detail_provider) else {
            continue;
        };
        if groups.iter().any(|g| g.group == provider.display()) {
            continue;
        }
        let (line, items) = expand_link(cfg, &conf, provider, raw);
        if items.is_empty() {
            continue;
        }
        groups.push(EpisodeGroup { group: line, items });
    }
    if groups.is_empty() {
        return Err(parse_err(
            site,
            "详情里的网盘分享全都不受支持（或展开失败）".to_string(),
        ));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: title,
        episodes: groups,
        ..Default::default()
    })
}

/// 播放线路 → 网盘类型（原类按 `flag.contains` 判断）。
pub fn provider_for_flag(flag: &str) -> RawProvider {
    if flag.contains("夸克") {
        RawProvider::Quark
    } else if flag.contains("迅雷") {
        RawProvider::Thunder
    } else if flag.contains("YD") {
        RawProvider::Yd
    } else {
        // 原类默认落到阿里分支。
        RawProvider::Ali
    }
}

/// 播放：`id = 分享###fid`。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<serde_json::Value> {
    let provider = provider_for_flag(flag);
    let Some((share, fid)) = id.rsplit_once("###") else {
        return Err(host_err(
            site,
            format!("播放 id 缺少 `分享###fid` 结构（原类无法定位网盘文件）：{id}"),
        ));
    };
    if provider.pan().is_none() {
        return Err(host_err(
            site,
            format!(
                "{} 网盘播放引擎未实现（桌面端无对应 parser），分享：{share}",
                provider.display()
            ),
        ));
    }
    let conf = load_pan_config(site);
    let Some(link) = parse_share_link(share) else {
        return Err(parse_err(
            site,
            format!("无法识别网盘分享：{share}"),
        ));
    };
    let url = resolve_play_url(cfg, &conf, &link, fid)?;
    Ok(serde_json::json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(ext: Option<&str>) -> Site {
        Site {
            key: "yunpan-test".to_string(),
            name: "云盘".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: ext.map(|e| json!(e)),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_YunPan"));
        assert!(handles("CSP_YUNPAN"));
        assert!(!handles("csp_YunPanX"));
    }

    #[test]
    fn base_of_overrides_only_with_http_segment() {
        assert_eq!(base_of(&site(None)), DEFAULT_BASE);
        assert_eq!(
            base_of(&site(Some("./lib/token.json$$$null$$$proxy$$$1"))),
            DEFAULT_BASE
        );
        assert_eq!(
            base_of(&site(Some("./lib/token.json$$$https://cdn.example.com/$$$null"))),
            "https://cdn.example.com"
        );
    }

    #[test]
    fn danmaku_flag_reads_fourth_segment() {
        assert!(danmaku_enabled(&site(Some("./x$$$null$$$proxy$$$1"))));
        assert!(!danmaku_enabled(&site(Some("./x$$$null$$$proxy$$$0"))));
        assert!(!danmaku_enabled(&site(None)));
    }

    #[test]
    fn categories_is_single_home() {
        let cats = categories(&TvConfig::default(), &site(None));
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "/");
        assert_eq!(cats[0].type_name, "首页");
    }

    #[test]
    fn parse_cards_reads_share_title_cover_and_remark() {
        let html = r#"<div class="row">
          <div class="col">
            <a onclick="window.open('https://pan.quark.cn/s/abc123')"><img src="/img/q.jpg" alt="资源甲"></a>
            <div class="card-footer"><small>夸克网盘</small><a onclick="window.open('https://pan.quark.cn/s/abc123')">下载</a></div>
            <div class="card-body"><p>p0</p><p>p1</p><p>2026-11-11</p></div>
          </div>
          <div class="col">
            <a onclick='window.open("https://www.alipan.com/s/def456")'><img src="/img/a.jpg" alt="资源乙"></a>
            <div class="card-footer"><small>阿里云盘</small></div>
            <div class="card-body"><p>x</p><p>y</p><p>2026-11-10</p></div>
          </div>
        </div>"#;
        let items = parse_cards(html, "https://res.yunpan.win");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "资源甲");
        assert_eq!(items[0].id, "https://pan.quark.cn/s/abc123###资源甲");
        assert_eq!(items[0].cover.as_deref(), Some("https://res.yunpan.win/img/q.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("夸克/夸克网盘/2026-11-11"));
        assert_eq!(items[1].remark.as_deref(), Some("阿里/阿里云盘/2026-11-10"));
    }

    #[test]
    fn parse_cards_skips_unknown_providers() {
        let html = r#"<div class="row"><div class="col">
          <a onclick="window.open('https://example.org/x')"><img src="/i.jpg" alt="未知"></a>
        </div></div>"#;
        assert!(parse_cards(html, "https://res.yunpan.win").is_empty());
    }

    #[test]
    fn split_id_splits_links_and_title() {
        let (links, title) = split_id("https://pan.quark.cn/s/a;https://www.alipan.com/s/b###片名").unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(title, "片名");
        assert!(split_id("no-separator").is_none());
        assert!(split_id("###只标题").is_none());
    }

    #[test]
    fn classify_link_order_matches_push() {
        assert_eq!(classify_link("https://www.alipan.com/s/x"), Some(RawProvider::Ali));
        assert_eq!(classify_link("https://pan.quark.cn/s/x"), Some(RawProvider::Quark));
        assert_eq!(classify_link("https://pan.xunlei.com/s/x"), Some(RawProvider::Thunder));
        assert_eq!(classify_link("https://cloud.189.cn/t/x"), Some(RawProvider::Yd));
        assert_eq!(classify_link("https://example.org/x"), None);
    }

    #[test]
    fn videos_and_search_urls_match_protocol() {
        assert_eq!(
            videos_url("https://res.yunpan.win/", 2),
            "https://res.yunpan.win/?PageIndex=2&pageSize=20&Keyword&Type=&Tag="
        );
        assert_eq!(
            search_url("https://res.yunpan.win", "海贼 王"),
            "https://res.yunpan.win/?PageIndex=1&pageSize=20&Keyword=%E6%B5%B7%E8%B4%BC+%E7%8E%8B&Type=&Tag="
        );
    }

    #[test]
    fn flags_map_to_expected_providers() {
        assert_eq!(provider_for_flag("夸克云盘"), RawProvider::Quark);
        assert_eq!(provider_for_flag("迅雷网盘"), RawProvider::Thunder);
        assert_eq!(provider_for_flag("YD云盘"), RawProvider::Yd);
        assert_eq!(provider_for_flag("阿里云盘"), RawProvider::Ali);
        assert_eq!(provider_for_flag("其它"), RawProvider::Ali);
    }

    #[test]
    fn player_rejects_unexpanded_and_unsupported() {
        let cfg = TvConfig::default();
        let s = site(None);
        let err = player_content(&cfg, &s, "夸克", "no-separator").unwrap_err();
        assert!(err.to_string().contains("分享###fid"), "{err}");
        let err = player_content(&cfg, &s, "迅雷", "https://pan.xunlei.com/s/x###fid").unwrap_err();
        assert!(err.to_string().contains("引擎未实现"), "{err}");
        let err = player_content(&cfg, &s, "YD", "https://cloud.189.cn/t/x###fid").unwrap_err();
        assert!(err.to_string().contains("引擎未实现"), "{err}");
    }

    #[test]
    fn detail_rejects_bad_ids() {
        let cfg = TvConfig::default();
        let s = site(None);
        let err = detail(&cfg, &s, "no-separator").unwrap_err();
        assert!(err.to_string().contains("###"), "{err}");
    }

    #[test]
    fn absolute_joins_relative_urls() {
        assert_eq!(absolute("https://a.com", "/x.jpg"), "https://a.com/x.jpg");
        assert_eq!(absolute("https://a.com/", "x.jpg"), "https://a.com/x.jpg");
        assert_eq!(absolute("https://a.com", "//b.com/y"), "https://b.com/y");
        assert_eq!(absolute("https://a.com", "https://c.com/z"), "https://c.com/z");
    }

    #[test]
    fn expand_link_degrades_thunder_to_single_episode() {
        let cfg = TvConfig::default();
        let conf = PanConfig::default();
        let (line, eps) = expand_link(&cfg, &conf, RawProvider::Thunder, "https://pan.xunlei.com/s/x");
        assert_eq!(line, "迅雷");
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].url, "https://pan.xunlei.com/s/x");
    }

    #[test]
    fn json_helper_is_unused_but_compiles() {
        // 保持与模型 JSON 输出的兼容性检查（detail 输出 camelCase）。
        let v = serde_json::to_value(VideoDetail::default()).unwrap();
        assert!(v.is_object());
        let _ = json!({"ok": true});
    }
}
