//! `csp_Xpanpan` 原生实现（小盘盘 / 弹幕小盘盘，WordPress 博客型网盘站）。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/Xpanpan.java`
//!   （415 行，`extends Push`；另有 200 行同协议变体 `0f047745-FxzHm`）。
//! * 站点 `_rea/samples/inventory/sites.json`：1 站 `Xpanpan`，
//!   ext `./lib/token.json$$$https://xpanpan.site$$$null$$$1`。
//!
//! 协议（原类逐段还原）：
//! * ext 按 `$$$` 切：段 0 为 Push token 文件路径；段 1 长度 > 7 时覆盖基址；
//!   段 2 为代理地址（`C0063a.i`）；段 3 为弹幕开关（`"1"` 且未禁用全局弹幕时开启）。
//! * 首页 `homeContent`：GET 基址 → `nav ul li a` 作分类（`href` 即 type_id、文本作名称）；
//! 同页 `main article` 作首页列表。
//! * 列表行（`e(hVar)`）：`header h4 a` 的 `href` 为 id、`title` 属性为标题
//! （缺失时退回锚文本）、`.cat-links a` 文本用 `/` 连接作备注。
//! * 分类页 `categoryContent`：`{base}{type_id}/page/{page}`（type_id 为首页导航 href）。
//! * 搜索 `searchContent`：`{base}/page/{page}/?s={kw}`（默认 page=1），结果行同上。
//! * 详情 `detailContent`：GET `{base}{id}`；标题 `article header h1`、
//!   备注 `article .entry-meta .posted-on`、简介 `article .entry-content`；
//!   网盘链接 = `article .entry-content a[href]`，按 Quark/UC/Ali/Thunder/YD/P115
//!   域名正则分类（上限 100），再由网盘引擎展开成线路（`$$$` 合并线路、`#` 合并剧集）。
//! * 播放 `playerContent`：线路名以「夸克 / UC / 迅雷 / PP極速 / YD」开头分派对应引擎，
//!   否则走阿里；开启弹幕时原类再包一层 `C0116a.e("danmaku", …)`。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. 迅雷云盘与天翼云盘没有 W9 引擎，线路会展示、播放返回
//!    `catvod.host.unsupported`（与 xiaozhang/wogg 同策略）；
//! 2. `proxy`（段 2）与 `./lib/token.json`（段 0）不参与请求：桌面端不做本地代理；
//! 3. 弹幕分支不实现（无 WebView 弹幕层）；
//! 4. 站点 `https://xpanpan.site` 在 2026-11 已不可达（`fetch failed`），
//!    本家族只能以 mock 夹具验收，无真站 `#[ignore]` 用例。
//!
//! 现站核验：2026-11-12 探测 `https://xpanpan.site/`、`/page/2/`、`/?s=名` 均为
//! `fetch failed`；上游面板可能已下线，本实现按原类协议做等价还原。

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

/// 本家族覆盖的 api。
pub const API: &str = "csp_Xpanpan";
/// 默认基址（ext 段 1 可覆盖）。
pub const DEFAULT_BASE: &str = "https://xpanpan.site";
/// 请求 UA（原类走 Push 默认客户端，这里固定 Chrome/122）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 单次详情最多展开的网盘分享数（原类上限 100，这里取保守值）。
const MAX_RESOURCES: usize = 12;
/// 剧集上限。
const MAX_EPISODES: usize = 500;

/// 判断 api 是否属于本家族（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case("csp_Xpanpan")
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

/// 基址：ext 段 1 长度 > 7 时覆盖（原类 `split[1].trim().length() > 7`）。
pub fn base_of(site: &Site) -> String {
    let segs = ext_segments(site);
    if let Some(seg) = segs.get(1) {
        if seg.len() > 7 && seg.starts_with("http") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

/// 弹幕开关：ext 段 3 为 `1`（原类还要求全局开关未禁用，桌面端无该设置，按开启处理）。
pub fn danmaku_enabled(site: &Site) -> bool {
    let segs = ext_segments(site);
    segs.get(3).map(|s| s == "1").unwrap_or(false)
}

fn headers() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map.insert(
        "Accept".to_string(),
        "text/html,application/xhtml+xml".to_string(),
    );
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

/// 文本标签剥离（列表/详情里标题常带 `<em>` 高亮）。
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

/// 在 `seg` 的 `marker` 之后取第一个 `attr="value"`。

/// 在 `marker` 之后取「到第一个闭合标签前」的文本（可包含嵌套标签）。
fn text_block_after(seg: &str, marker: &str) -> Option<String> {
    let pos = seg.find(marker)?;
    let rest = &seg[pos + marker.len()..];
    let gt = rest.find('>')?;
    let tail = &rest[gt + 1..];
    let end = tail.find("</").unwrap_or(tail.len());
    let text = strip_tags(&tail[..end]);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 按起始标签正则把 HTML 切成若干「块」。
fn split_blocks<'a>(html: &'a str, re: &Regex) -> Vec<&'a str> {
    let starts: Vec<usize> = re.find_iter(html).map(|m| m.start()).collect();
    let mut out = Vec::with_capacity(starts.len());
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(html.len());
        out.push(&html[*start..end]);
    }
    out
}

fn re_article() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<article\b").unwrap())
}

fn fetch_text(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(), 2)
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

/* ============================================================
 * 网盘分享分类
 * ============================================================ */

/// 原类在 `detailContent` 里依次用 Quark/UC/Ali/Thunder/YD/P115 的正则做匹配。
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

    /// 桌面端有对应 W9 引擎的 provider；迅雷/YD 没有。
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

/// 按原始 URL 判断网盘类型（顺序与原类一致，先 Quark 再 UC/Ali/Thunder/YD/P115）。
pub fn classify_link(raw: &str) -> Option<RawProvider> {
    let s = raw.trim();
    if s.contains("pan.quark.cn/s/") {
        Some(RawProvider::Quark)
    } else if s.contains("drive.uc.cn/s/") || s.contains("pc.uc.cn/s/") {
        Some(RawProvider::Uc)
    } else if s.contains("alipan.com/s/") || s.contains("aliyundrive.com/s/") {
        Some(RawProvider::Ali)
    } else if s.contains("pan.xunlei.com/s/") {
        Some(RawProvider::Thunder)
    } else if s.contains("cloud.189.cn/") {
        Some(RawProvider::Yd)
    } else if s.contains("115.com/s/") || s.contains("115cdn.com/s/") {
        Some(RawProvider::P115)
    } else {
        None
    }
}


/* 占位：后续段落继续追加 */
fn attr_value(seg: &str, attr: &str) -> Option<String> {
    let key = format!("{attr}=\"");
    let pos = seg.find(&key)? + key.len();
    let tail = &seg[pos..];
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

/* ============================================================
 * HTML 解析
 * ============================================================ */

/// 首页/分类/搜索三种页面共用的列表行解析（`main article`）。
pub fn parse_list(html: &str, _base: &str) -> Vec<VideoItem> {
    let mut out: Vec<VideoItem> = Vec::new();
    for block in split_blocks(html, re_article()) {
        let Some(header_pos) = block.find("<h4").or_else(|| block.find("header")) else {
            continue;
        };
        let head = &block[header_pos..];
        let href = match attr_value(head, "href") {
            Some(h) if !h.trim().is_empty() => h.trim().to_string(),
            _ => continue,
        };
        let name = attr_value(head, "title")
            .or_else(|| text_block_after(head, "<a"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(name) = name else { continue };
        let remark = extract_cat_links(block);
        if out.iter().any(|v| v.id == href) {
            continue;
        }
        out.push(VideoItem {
            id: href,
            name,
            cover: None,
            remark,
        });
    }
    out
}

/// `.cat-links a` 文本用 `/` 连接（原类 `TextUtils.join("/", …)`）。
fn extract_cat_links(block: &str) -> Option<String> {
    let pos = block.find("cat-links")?;
    let tail = &block[pos..];
    let end = tail.find("</ul>").unwrap_or(tail.len());
    let region = &tail[..end];
    let mut names: Vec<String> = Vec::new();
    for chunk in region.split("<a ").skip(1) {
        if let Some(gt) = chunk.find('>') {
            let rest = &chunk[gt + 1..];
            if let Some(lt) = rest.find("</a>") {
                let text = strip_tags(&rest[..lt]);
                if !text.is_empty() && !names.contains(&text) {
                    names.push(text);
                }
            }
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(names.join("/"))
    }
}

/// 从首页 HTML 抽分类（`nav ul li a` 的 href 作 type_id、文本作名称）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let mut out: Vec<Category> = Vec::new();
    let Some(nav_pos) = html.find("<nav") else {
        return out;
    };
    let nav_tail = &html[nav_pos..];
    let nav_end = nav_tail
        .find("</nav>")
        .map(|p| nav_pos + p)
        .unwrap_or(html.len());
    let nav = &html[nav_pos..nav_end];
    let mut rest = nav;
    while let Some(li_pos) = rest.find("<li") {
        let chunk = &rest[li_pos..];
        let Some(a_pos) = chunk.find("<a") else {
            rest = &chunk[1..];
            continue;
        };
        let anchor = &chunk[a_pos..];
        if let Some(href) = attr_value(anchor, "href") {
            let href = href.trim().to_string();
            if !href.is_empty()
                && href != "#"
                && !out.iter().any(|c| c.type_id == href)
            {
                let name = text_block_after(anchor, "<a")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| href.clone());
                out.push(Category {
                    type_id: href,
                    type_name: name,
                });
            }
        }
        rest = &chunk[1..];
    }
    out
}

/// 分类页 URL：`{base}{type_id}/page/{page}`（type_id 为首页导航 href）。
pub fn list_url(base: &str, type_id: &str, page: u64) -> String {
    let cat = absolute(base, type_id);
    let cat = cat.trim_end_matches('/');
    format!("{cat}/page/{page}")
}

/// 搜索页 URL：`{base}/page/{page}/?s={kw}`。
pub fn search_url(base: &str, keyword: &str, page: u64) -> String {
    format!(
        "{}/page/{}/?s={}",
        base.trim_end_matches('/'),
        page,
        urlencode(keyword)
    )
}

/* ============================================================
 * 站点入口
 * ============================================================ */

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = fetch_text(cfg, &format!("{base}/"))?;
    let cats = parse_categories(&html);
    if cats.is_empty() {
        return Err(parse_err(
            site,
            "首页导航未解析出分类（页面结构可能已改版）",
        ));
    }
    Ok(cats)
}

pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let html = fetch_text(cfg, &format!("{base}/"))?;
    Ok(parse_list(&html, &base))
}

pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, type_id, page as u64);
    let html = fetch_text(cfg, &url)?;
    let items = parse_list(&html, &base);
    if items.is_empty() {
        return Err(parse_err(
            site,
            format!("分类「{type_id}」第 {page} 页未解析出条目：{url}"),
        ));
    }
    Ok(items)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = search_url(&base, keyword, 1);
    let html = fetch_text(cfg, &url)?;
    Ok(parse_list(&html, &base))
}

/* 占位：详情/播放与测试继续追加 */
/* ============================================================
 * 详情 / 网盘展开 / 播放
 * ============================================================ */

#[derive(Debug, Default)]
pub struct DetailData {
    pub name: String,
    pub cover: Option<String>,
    pub remark: Option<String>,
    pub desc: Option<String>,
    pub links: Vec<String>,
}

/// 详情页解析（详情首屏即为文章正文；网盘链接取自 `.entry-content a[href]`）。
pub fn parse_detail(html: &str) -> DetailData {
    let block = split_blocks(html, re_article())
        .into_iter()
        .next()
        .unwrap_or(html);
    let name = text_block_after(block, "<h1")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let remark = text_block_after(block, "posted-on").map(|s| s.trim().to_string());
    let cover = attr_value(block, "src")
        .filter(|s| s.starts_with("http") || s.starts_with('/'))
        .map(|s| s.trim().to_string());
    let (desc, links) = match block.find("entry-content") {
        Some(pos) => {
            let content = &block[pos..];
            let text = strip_tags(content);
            let desc = if text.is_empty() {
                None
            } else if text.len() > 500 {
                Some(text[..500].to_string())
            } else {
                Some(text)
            };
            (desc, scan_pan_links(content))
        }
        None => (None, Vec::new()),
    };
    DetailData {
        name,
        cover,
        remark,
        desc,
        links,
    }
}

/// 扫描一段 HTML 里所有 `href="…"`，按网盘域名分类并去重（原类上限 100）。
fn scan_pan_links(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = content;
    while let Some(pos) = rest.find("href=\"") {
        let tail = &rest[pos + 6..];
        let Some(end) = tail.find('"') else { break };
        let href = tail[..end].trim();
        if classify_link(href).is_some() && !out.iter().any(|x| x == href) {
            out.push(href.to_string());
        }
        rest = &tail[end..];
    }
    out
}

fn load_pan_config(site: &Site) -> PanConfig {
    for seg in site.ext_string().split("$$$") {
        let seg = seg.trim();
        if seg.starts_with('{') {
            return PanConfig::parse(seg);
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

/// 单条网盘分享 → (线路名, 剧集列表)；引擎展开失败时降级成一条单集。
fn expand_link(
    cfg: &TvConfig,
    conf: &PanConfig,
    provider: RawProvider,
    url: &str,
) -> (String, Vec<Episode>) {
    let line = provider.display().to_string();
    let fallback = || {
        vec![Episode {
            title: provider.display().to_string(),
            url: url.to_string(),
        }]
    };
    let pan = match provider.pan() {
        Some(p) => p,
        None => return (line, fallback()),
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

/// 详情：解析字段 + 展开网盘分享成线路。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = absolute(&base, id);
    let html = fetch_text(cfg, &url)?;
    let data = parse_detail(&html);
    if data.name.is_empty() {
        return Err(parse_err(site, format!("详情页未解析出标题：{url}")));
    }
    if data.links.is_empty() {
        return Err(parse_err(
            site,
            format!("详情页未找到网盘分享（article .entry-content a[href]）：{url}"),
        ));
    }
    let conf = load_pan_config(site);
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for raw in data.links.iter().take(MAX_RESOURCES) {
        let Some(provider) = classify_link(raw) else {
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
        return Err(parse_err(site, "详情页有网盘链接但都不受支持（或展开失败）"));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: data.name,
        cover: data.cover,
        desc: data.desc.or(data.remark),
        episodes: groups,
        ..Default::default()
    })
}

fn provider_for_flag(flag: &str) -> RawProvider {
    let f = flag.trim();
    if f.starts_with("夸克") {
        RawProvider::Quark
    } else if f.starts_with("UC") {
        RawProvider::Uc
    } else if f.starts_with("迅雷") || f.starts_with("PP極速") {
        RawProvider::Thunder
    } else if f.starts_with("YD") {
        RawProvider::Yd
    } else {
        // 原类默认落到阿里分支。
        RawProvider::Ali
    }
}

/// 播放：线路名 → 网盘引擎；无引擎的迅雷/YD 明确报错。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
) -> Result<serde_json::Value> {
    if danmaku_enabled(site) {
        tracing::debug!("xpanpan: 站点启用了弹幕，原生实现走网盘直链分支");
    }
    let provider = provider_for_flag(flag);
    let Some(pan) = provider.pan() else {
        return Err(host_err(
            site,
            format!(
                "线路「{flag}」使用 {}，桌面端没有对应解析引擎",
                provider.display()
            ),
        ));
    };
    let (share, fid) = match id.split_once("###") {
        Some((share, fid)) if !share.is_empty() && !fid.is_empty() => (share, fid),
        _ => {
            return Err(host_err(
                site,
                format!("线路「{flag}」没有展开出剧集标识（缺少 ###fid）"),
            ))
        }
    };
    let link = parse_share_link(share)
        .filter(|l| l.provider == pan)
        .ok_or_else(|| parse_err(site, format!("无法识别网盘分享：{share}")))?;
    let conf = load_pan_config(site);
    let url = resolve_play_url(cfg, &conf, &link, fid)?;
    Ok(json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": {"User-Agent": UA},
    }))
}

/* 占位：测试继续追加 */
#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "xpanpan-test".to_string();
        s.name = "小盘盘".to_string();
        s.api = API.to_string();
        s.ext = Some(json!(ext));
        s
    }

    fn cfg() -> TvConfig {
        TvConfig::default()
    }

    const HOME: &str = r##"<html><body>
<nav class="site-nav"><ul>
<li><a href="/category/dianying/">电影</a></li>
<li><a href="/category/dianshiju/">电视剧</a></li>
</ul></nav>
<main class="site-main">
<article class="post">
 <header class="entry-header"><h4 class="entry-title"><a href="/archives/101" title="星际穿越 (2014)">星际穿越</a></h4></header>
 <ul class="cat-links"><li><a href="/tag/sf/">科幻</a></li><li><a href="/tag/movie/">电影</a></li></ul>
</article>
<article class="post">
 <header><h4><a href="/archives/102">流浪地球</a></h4></header>
 <div class="cat-links"><a href="/tag/sf/">科幻</a></div>
</article>
</main></body></html>"##;

    const DETAIL: &str = r##"<html><body><main>
<article class="post">
 <header class="entry-header"><h1 class="entry-title">星际穿越</h1></header>
 <div class="entry-meta"><span class="posted-on">2026-01-02</span></div>
 <div class="entry-content">
 <p>简介：一支探险队穿越虫洞寻找新家园。<img src="https://img.example/cover.jpg"></p>
 <p><a href="https://pan.quark.cn/s/abc123">夸克网盘</a> <a href="https://drive.uc.cn/s/uc123">UC</a></p>
 <p><a href="https://www.alipan.com/s/ali123">阿里</a> <a href="https://pan.xunlei.com/s/th123">迅雷</a></p>
 <p><a href="https://cloud.189.cn/t/yd123">天翼</a> <a href="https://115.com/s/p115x">115</a></p>
 <p><a href="https://example.org/nothing">无关链接</a></p>
 </div>
</article></main></body></html>"##;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Xpanpan"));
        assert!(handles("CSP_XPANPAN"));
        assert!(handles(" csp_xpanpan "));
        assert!(!handles("csp_Wogg"));
    }

    #[test]
    fn ext_splits_base_and_danmaku() {
        let s = site("./lib/token.json$$$https://mirror.example/$$$null$$$1");
        assert_eq!(base_of(&s), "https://mirror.example");
        assert!(danmaku_enabled(&s));

        let none = site("");
        assert_eq!(base_of(&none), DEFAULT_BASE);
        assert!(!danmaku_enabled(&none));

        let short = site("./lib/token.json$$$abc$$$null$$$0");
        assert_eq!(base_of(&short), DEFAULT_BASE);
        assert!(!danmaku_enabled(&short));
    }

    #[test]
    fn parse_categories_from_nav() {
        let cats = parse_categories(HOME);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "/category/dianying/");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "/category/dianshiju/");
        assert_eq!(cats[1].type_name, "电视剧");
    }

    #[test]
    fn parse_list_reads_articles() {
        let items = parse_list(HOME, DEFAULT_BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "/archives/101");
        assert_eq!(items[0].name, "星际穿越 (2014)");
        assert_eq!(items[0].remark.as_deref(), Some("科幻/电影"));
        assert_eq!(items[1].id, "/archives/102");
        assert_eq!(items[1].name, "流浪地球");
        assert_eq!(items[1].remark.as_deref(), Some("科幻"));
        assert!(items[1].cover.is_none());
    }

    #[test]
    fn list_url_handles_relative_and_absolute() {
        assert_eq!(
            list_url(DEFAULT_BASE, "/category/dianying/", 2),
            "https://xpanpan.site/category/dianying/page/2"
        );
        assert_eq!(
            list_url(DEFAULT_BASE, "https://xpanpan.site/cat", 1),
            "https://xpanpan.site/cat/page/1"
        );
    }

    #[test]
    fn search_url_encodes_keyword() {
        let url = search_url(DEFAULT_BASE, "科幻 电影", 1);
        assert_eq!(
            url,
            "https://xpanpan.site/page/1/?s=%E7%A7%91%E5%B9%BB+%E7%94%B5%E5%BD%B1"
        );
    }

    #[test]
    fn parse_detail_extracts_fields_and_links() {
        let data = parse_detail(DETAIL);
        assert_eq!(data.name, "星际穿越");
        assert_eq!(data.remark.as_deref(), Some("2026-01-02"));
        assert_eq!(data.cover.as_deref(), Some("https://img.example/cover.jpg"));
        let desc = data.desc.unwrap_or_default();
        assert!(desc.contains("简介"), "desc={desc}");
        assert_eq!(
            data.links,
            vec![
                "https://pan.quark.cn/s/abc123".to_string(),
                "https://drive.uc.cn/s/uc123".to_string(),
                "https://www.alipan.com/s/ali123".to_string(),
                "https://pan.xunlei.com/s/th123".to_string(),
                "https://cloud.189.cn/t/yd123".to_string(),
                "https://115.com/s/p115x".to_string(),
            ]
        );
    }

    #[test]
    fn classify_link_matches_original_order() {
        assert_eq!(
            classify_link("https://pan.quark.cn/s/abc"),
            Some(RawProvider::Quark)
        );
        assert_eq!(
            classify_link("https://drive.uc.cn/s/abc"),
            Some(RawProvider::Uc)
        );
        assert_eq!(
            classify_link("https://www.alipan.com/s/abc"),
            Some(RawProvider::Ali)
        );
        assert_eq!(
            classify_link("https://pan.xunlei.com/s/abc"),
            Some(RawProvider::Thunder)
        );
        assert_eq!(
            classify_link("https://cloud.189.cn/t/abc"),
            Some(RawProvider::Yd)
        );
        assert_eq!(
            classify_link("https://115.com/s/abc"),
            Some(RawProvider::P115)
        );
        assert_eq!(classify_link("https://example.org/a"), None);
    }

    #[test]
    fn player_rejects_thunder_and_yd() {
        let s = site("https://xpanpan.site");
        for (flag, id) in [
            ("迅雷", "https://pan.xunlei.com/s/th123###1"),
            ("YD", "https://cloud.189.cn/t/yd123###1"),
        ] {
            let err = player_content(&cfg(), &s, flag, id).unwrap_err().to_string();
            assert!(err.contains("引擎"), "flag={flag} err={err}");
        }
    }

    #[test]
    fn player_rejects_unexpanded_episode_id() {
        let s = site("https://xpanpan.site");
        let err = player_content(&cfg(), &s, "夸克", "https://pan.quark.cn/s/abc123")
            .unwrap_err()
            .to_string();
        assert!(err.contains("###fid"), "err={err}");
    }

    #[test]
    fn player_rejects_unrecognised_share() {
        let s = site("https://xpanpan.site");
        let err = player_content(&cfg(), &s, "夸克", "https://example.org/x###1")
            .unwrap_err()
            .to_string();
        assert!(err.contains("无法识别"), "err={err}");
    }
}
