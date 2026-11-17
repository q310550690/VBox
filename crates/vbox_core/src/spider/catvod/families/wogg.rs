//! `csp_Wogg` / `csp_Wobg` / `csp_Wo4k` / `csp_WoGG` 原生实现（玩偶哥哥系模板）。
//!
//! 来源（REA 反编译证据）：
//! * 基类 `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Wogg.java`
//!   （297 行；`detailContent`/`init` 被 jadx 跳过，用
//!   `jadx --show-bad-code --single-class` 恢复为
//!   `_rea/tmp/Wogg.recovered.java`（622 行），并用
//!   `_rea/tmp/dec-order3.mjs` 解出全部 58 条 `C0338a.a(byte[],byte[])` XOR 字符串）；
//! * 子类 `Wobg.java`（25 行，仅覆写 init 的基址/正则/URL 模板）、
//!   `Wo4k.java`（17 行，覆写 `x`/`y` 列表与搜索模板）；
//! * `csp_WoGG` 复用同一套 HTML 模板语义（站点 ext 为首段 token、第二段基址、
//!   末段「弹」的旧式写法，或 `{"siteUrl":…}` JSON）。
//!
//! 协议（Wogg 基类，HTML 模板）：
//! * 分类：首页 `.nav-link` 的 `href`，用 `/vodtype/(\\w+).html` 抽 cateId；
//!   Wobg 用 `/vod/type/id/(\\w+).html`，Wo4k 用 `/type/(\\w+).html`；
//! * 列表：Wogg `%s/index.php/vodshow/%s.html`，其中 `%s` 为 12 段 `-` 拼接
//!   （段 0 = cateId、段 8 = page）；Wobg
//!   `%s/index.php/vod/type/id/%s/page/%s.html`；Wo4k `%s/pianku/%s.html`；
//!   列表行 `.module-item`（`.video-name a` id/title、`.module-item-pic > img[data-src]`
//!   封面、`.module-item-text` 备注）；总页数取页面 `$(".mac_total").text('N')`，
//!   每页 72 条；
//! * 搜索：Wogg `%s/index.php/vodsearch/%s----------%s---.html`，Wobg
//!   `%s/index.php/vod/search.html?wd=%s&page=%s`，Wo4k
//!   `%s/wosearch/-------------.html?wd=%s&page=%s`；结果行 `.module-search-item`
//!   （`.video-serial` 的 href/title、`.module-item-pic > img[data-src]`、
//!   `.video-tag-icon` 备注）；
//! * 详情：`.video-info-header > .page-title` 标题、`.module-item-pic img[data-src]`
//!   封面、`.video-info-header a.tag-link` 元数据、`.video-info-header div.tag-link a`
//!   备注（`,` 连接）；网盘分享 = `.module-row-text` 的 `data-clipboard-text`
//!   属性集合，按 Quark/Ali/Thunder/YD 域名分类后由网盘引擎展开成线路；
//! * 播放：按线路名含「夸克」→ Quark、「迅雷」→ Thunder、「YD」→ YD、否则 Ali
//!   分派（原实现开启弹幕时再包一层 `C0116a.e("danmaku", …)`）。
//!
//! 有意偏差（与桌面端能力/实现取舍相关）：
//! 1. `init` 里的 `./lib/token.json`（弹幕 token）、`./wogg.json`（filters 大 JSON）、
//!    `proxy`/`noproxy` 代理模式都不参与请求：桌面端不做弹幕与本地代理，
//!    filters 也不在 `list_videos` 签名内（TTian 同策略）；
//! 2. 迅雷云盘（`pan.xunlei.com`）与天翼云盘（`cloud.189.cn`）没有 W9 引擎，
//!    线路会展示但播放返回 `catvod.host.unsupported`；
//! 3. 详情页 `.sqjj_a` 的「直链播放组」（原实现的另一条播放来源）不实现，
//!    只保留网盘分享展开这条主链路；找不到网盘分享时报 `catvod.result.parse`；
//! 4. 弹幕分支不实现（桌面端无 WebView 弹幕层）。
//!
//! 现站核验（2026-11）：`tvfan.xxooo.cf`、`tv.yydsys.top`、`mogg.top` 可访问；
//! `wobge.run.goorm.io`、`www.wo4k.com`、`www.wogg.net` 等镜像由 ext 覆盖基址。

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

/// 本家族覆盖的四个 api（画像里各自独立计站点）。
pub const APIS: [&str; 4] = ["csp_Wogg", "csp_Wobg", "csp_Wo4k", "csp_WoGG"];
/// 请求 UA（原类 `getHeader()` 里 Chrome/122 系列）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 每页条数（原类 `categoryContent` 固定 72）。
const PAGE_SIZE: u64 = 72;
/// 单次详情最多展开的网盘分享数（原类上限约 100，这里取更保守的 12）。
const MAX_RESOURCES: usize = 12;
/// 剧集上限。
const MAX_EPISODES: usize = 500;

/// 站点变体：四个 api 的 URL 模板与默认基址不同，解析语义一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `csp_Wogg`：12 段 `-` 拼接的 vodshow 列表。
    Wogg,
    /// `csp_Wobg`：`vod/type/id/{id}/page/{n}.html`。
    Wobg,
    /// `csp_Wo4k`：`pianku/{id}.html` 列表（无页码模板，按 `?page=` 兜底）。
    Wo4k,
    /// `csp_WoGG`：与 Wobg 同模板，ext 首段为旧式 token 路径。
    WoggV2,
}

impl Kind {
    fn default_base(self) -> &'static str {
        match self {
            Kind::Wogg => "https://tvfan.xxooo.cf",
            Kind::Wobg => "https://wobge.run.goorm.io",
            Kind::Wo4k => "https://www.wo4k.net/",
            Kind::WoggV2 => "https://www.wogg.net/",
        }
    }

    fn nav_regex(self) -> &'static str {
        match self {
            Kind::Wogg | Kind::WoggV2 => r#"/vodtype/(\w+)\.html"#,
            Kind::Wobg => r#"/vod/type/id/(\w+)\.html"#,
            Kind::Wo4k => r#"/type/(\w+)\.html"#,
        }
    }

    fn list_template(self) -> &'static str {
        match self {
            Kind::Wogg => "%s/index.php/vodshow/%s.html",
            Kind::Wobg | Kind::WoggV2 => "%s/index.php/vod/type/id/%s/page/%s.html",
            Kind::Wo4k => "%s/pianku/%s.html",
        }
    }

    /// 分类 id 提取用的路径正则（详情链接形状随模板不同）。
    fn dash_list(self) -> bool {
        matches!(self, Kind::Wogg)
    }
}

/// 判断 api 是否属于本家族（大小写不敏感，注册表传入的是原值）。
pub fn handles(api: &str) -> bool {
    // 字面量集中在这里：`site-coverage`/`live-coverage` 通过静态扫描 `handles`
    // 收集家族接管的 api（大小写必须与订阅配置一致，见 sites.json）。
    if matches!(
        api.trim(),
        "csp_Wogg" | "csp_Wobg" | "csp_Wo4k" | "csp_WoGG"
    ) {
        return true;
    }
    kind_of(api).is_some()
}

fn kind_of(api: &str) -> Option<Kind> {
    let raw = api.trim();
    let lower = raw.to_ascii_lowercase();
    // `csp_Wogg` 与 `csp_WoGG` 小写完全相同，只能按原始大小写区分。
    if lower == "csp_wogg" {
        return Some(if raw.contains("WoGG") {
            Kind::WoggV2
        } else {
            Kind::Wogg
        });
    }
    match lower.as_str() {
        "csp_wobg" => Some(Kind::Wobg),
        "csp_wo4k" => Some(Kind::Wo4k),
        "csp_woggv2" | "csp_wogg2" => Some(Kind::WoggV2),
        _ => None,
    }
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// ext 原文（JSON 形式会得到 `{…}` 文本）。
fn ext_raw(site: &Site) -> String {
    site.ext_string()
}

/// 从 ext 里取基址：JSON 取 `siteUrl`；字符串按 `$$$` 分段，优先第 2 段，
/// 否则取第一个不含 `127.0.0.1` 的 http 段。
fn base_of(kind: Kind, site: &Site) -> String {
    let raw = ext_raw(site);
    let text = raw.trim();
    if text.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            for key in ["siteUrl", "site_url", "url", "base"] {
                if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                    let s = s.trim();
                    if s.starts_with("http") {
                        return s.trim_end_matches('/').to_string();
                    }
                }
            }
        }
    }
    let segs: Vec<&str> = text.split("$$$").map(|s| s.trim()).collect();
    if let Some(seg) = segs.get(1) {
        if seg.starts_with("http") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    for seg in &segs {
        if seg.starts_with("http") && !seg.contains("127.0.0.1") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    kind.default_base().trim_end_matches('/').to_string()
}

/// 弹幕开关：ext 段里出现 `弹` 或独立 `1`（原类 `init` 的 `p` 字段）。
pub fn danmaku_enabled(site: &Site) -> bool {
    let raw = ext_raw(site);
    if raw.trim().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
            return v.get("danMu").and_then(|x| x.as_str()).is_some();
        }
        return false;
    }
    raw.split("$$$").map(|s| s.trim()).any(|s| s == "弹" || s == "1")
}

fn re_nav(kind: Kind) -> &'static Regex {
    static WO: OnceLock<Regex> = OnceLock::new();
    static WB: OnceLock<Regex> = OnceLock::new();
    static W4: OnceLock<Regex> = OnceLock::new();
    match kind {
        Kind::Wogg | Kind::WoggV2 => WO.get_or_init(|| Regex::new(Kind::Wogg.nav_regex()).unwrap()),
        Kind::Wobg => WB.get_or_init(|| Regex::new(Kind::Wobg.nav_regex()).unwrap()),
        Kind::Wo4k => W4.get_or_init(|| Regex::new(Kind::Wo4k.nav_regex()).unwrap()),
    }
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
        format!("{}/{}", base.trim_end_matches('/'), href.trim_start_matches('/'))
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
        .trim()
        .to_string()
}

/// 在 `seg` 的 `marker` 之后取第一个 `attr="value"`。
fn attr_after(seg: &str, marker: &str, attr: &str) -> Option<String> {
    let pos = seg.find(marker)?;
    let rest = &seg[pos..];
    let key = format!("{attr}=\"");
    let start = rest.find(&key)? + key.len();
    let tail = &rest[start..];
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

/// 在 `marker` 之后取第一个 `>text<` 的文本（剥标签）。
fn text_after(seg: &str, marker: &str) -> Option<String> {
    let pos = seg.find(marker)?;
    let rest = &seg[pos + marker.len()..];
    let gt = rest.find('>')?;
    let tail = &rest[gt + 1..];
    let lt = tail.find('<')?;
    let text = strip_tags(&tail[..lt]);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 在 `marker` 之后取「到第一个闭合标签前」的文本（可包含嵌套标签，如 `<em>`/`<a>`）。
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

/// 列表行里同时取 `href`（`video-name` 内的 a 标签）与标题文本。
fn link_and_text(seg: &str) -> Option<(String, String)> {
    let pos = seg.find("href=\"")?;
    let rest = &seg[pos + 6..];
    let end = rest.find('"')?;
    let href = rest[..end].to_string();
    let tail = &rest[end + 1..];
    let gt = tail.find('>')?;
    let after = &tail[gt + 1..];
    let lt = after.find('<')?;
    let text = strip_tags(&after[..lt]);
    if text.is_empty() {
        None
    } else {
        Some((href, text))
    }
}

fn re_item() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<div[^>]*class="(?:[^"]*\s)?module-item(?:\s[^"]*)?"[^>]*>"#).unwrap()
    })
}

fn re_search_item() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<div[^>]*class="(?:[^"]*\s)?module-search-item(?:\s[^"]*)?"[^>]*>"#).unwrap()
    })
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

fn fetch_text(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(), 2)
}

/// 分类：首页导航链接（`.nav-link`）按各变体的路径正则抽 cateId。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let kind = kind_of(&site.api).unwrap_or(Kind::Wogg);
    let base = base_of(kind, site);
    let html = fetch_text(cfg, &format!("{base}/"))?;
    let cats = parse_categories(&html, kind);
    if cats.is_empty() {
        return Err(parse_err(
            site,
            "首页导航未解析出分类（页面结构可能已改版）",
        ));
    }
    Ok(cats)
}

/// 从首页 HTML 抽分类（拆出来便于单测）。
pub fn parse_categories(html: &str, kind: Kind) -> Vec<Category> {
    let re = re_nav(kind);
    let mut out: Vec<Category> = Vec::new();
    for m in re.captures_iter(html) {
        let Some(cap) = m.get(1) else { continue };
        let id = cap.as_str();
        if out.iter().any(|c| c.type_id == id) {
            continue;
        }
        // 链接标签闭合后的第一段文本作为分类名。
        let end = m.get(0).map(|x| x.end()).unwrap_or(0);
        let rest = &html[end..];
        let name = rest
            .find('>')
            .map(|p| &rest[p + 1..])
            .and_then(|t| t.find('<').map(|p| strip_tags(&t[..p])))
            .filter(|s| !s.is_empty());
        let name = match name {
            Some(n) => n,
            None => id.to_string(),
        };
        out.push(Category {
            type_id: id.to_string(),
            type_name: name,
        });
    }
    out
}

/// 分类页 URL。Wogg 为 12 段 `-` 拼接（段 0 = cateId、段 8 = page）。
pub fn list_url(kind: Kind, base: &str, type_id: &str, page: u64) -> String {
    if kind.dash_list() {
        let mut seg = [""; 12];
        seg[0] = type_id;
        let page_s = page.to_string();
        seg[8] = &page_s;
        let joined = seg.join("-");
        return format!("{}/index.php/vodshow/{}.html", base.trim_end_matches('/'), joined);
    }
    match kind.list_template().matches("%s").count() {
        3 => format!(
            "{}/index.php/vod/type/id/{}/page/{}.html",
            base.trim_end_matches('/'),
            type_id,
            page
        ),
        _ => {
            let mut url = format!(
                "{}/pianku/{}.html",
                base.trim_end_matches('/'),
                type_id
            );
            if page > 1 {
                url.push_str(&format!("?page={page}"));
            }
            url
        }
    }
}

/// 列表页解析（`.module-item`）。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for block in split_blocks(html, re_item()) {
        let Some((href, title)) = link_and_text(block) else { continue };
        let cover = attr_after(block, "module-item-pic", "data-src")
            .map(|src| absolute(base, &src));
        let remark = text_block_after(block, "module-item-text");
        out.push(VideoItem {
            id: absolute(base, &href),
            name: title,
            cover,
            remark,
        });
    }
    out
}

/// 总页数：原类读 `$(".mac_total").text('N')`，每页 72 条。
pub fn page_count(html: &str, page: u64) -> u64 {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"\$\("\.mac_total"\)\.text\('(\d+)'\);"#).unwrap()
    });
    let total = re
        .captures(html)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok());
    let page = page.max(1);
    match total {
        Some(total) if total > PAGE_SIZE => (total + PAGE_SIZE - 1) / PAGE_SIZE,
        Some(_) => 1,
        None => page,
    }
}

/// 分类列表（dispatch 的 videos 入口）。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    let page = page as u64;
    let kind = kind_of(&site.api).unwrap_or(Kind::Wogg);
    let base = base_of(kind, site);
    let url = list_url(kind, &base, type_id, page);
    let html = fetch_text(cfg, &url)?;
    let items = parse_list(&html, &base);
    if items.is_empty() {
        return Err(parse_err(site, format!("列表第 {page} 页无条目：{url}")));
    }
    Ok(items)
}

/// 首页推荐（原类 `homeContent` 的列表部分，供集成测试直接调用）。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let kind = kind_of(&site.api).unwrap_or(Kind::Wogg);
    let base = base_of(kind, site);
    let html = fetch_text(cfg, &format!("{base}/"))?;
    Ok(parse_list(&html, &base))
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let kind = kind_of(&site.api).unwrap_or(Kind::Wogg);
    let base = base_of(kind, site);
    let enc = urlencode(keyword);
    let url = match kind {
        Kind::Wogg => format!(
            "{}/index.php/vodsearch/{}----------1---.html",
            base.trim_end_matches('/'),
            enc
        ),
        Kind::Wobg | Kind::WoggV2 => format!(
            "{}/index.php/vod/search.html?wd={}&page=1",
            base.trim_end_matches('/'),
            enc
        ),
        Kind::Wo4k => format!(
            "{}/wosearch/-------------.html?wd={}&page=1",
            base.trim_end_matches('/'),
            enc
        ),
    };
    let html = fetch_text(cfg, &url)?;
    Ok(parse_search(&html, &base))
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 搜索结果解析（`.module-search-item`）。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for block in split_blocks(html, re_search_item()) {
        let Some((href, title)) = link_and_text(block) else { continue };
        let cover = attr_after(block, "module-item-pic", "data-src")
            .map(|src| absolute(base, &src))
            .or_else(|| attr_after(block, "module-item-pic", "src").map(|src| absolute(base, &src)));
        let remark = text_block_after(block, "video-tag-icon");
        out.push(VideoItem {
            id: absolute(base, &href),
            name: title,
            cover,
            remark,
        });
    }
    out
}

/// 网盘来源（原类用 `Quark.b/Ali.b/Thunder.b/YD.b` 正则分类）。
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
    fn display(self) -> &'static str {
        match self {
            RawProvider::Quark => "夸克",
            RawProvider::Uc => "UC",
            RawProvider::Ali => "阿里",
            RawProvider::P115 => "115",
            RawProvider::Thunder => "迅雷",
            RawProvider::Yd => "YD",
        }
    }

    fn pan(self) -> Option<PanProvider> {
        match self {
            RawProvider::Quark => Some(PanProvider::Quark),
            RawProvider::Uc => Some(PanProvider::Uc),
            RawProvider::Ali => Some(PanProvider::Ali),
            RawProvider::P115 => Some(PanProvider::P115),
            RawProvider::Thunder | RawProvider::Yd => None,
        }
    }
}

/// 按域名识别网盘分享。
pub fn classify_link(raw: &str) -> Option<RawProvider> {
    let low = raw.to_ascii_lowercase();
    if low.contains("pan.quark.cn/s/") {
        Some(RawProvider::Quark)
    } else if low.contains("drive.uc.cn/s/") || low.contains("pc.uc.cn/s/") {
        Some(RawProvider::Uc)
    } else if low.contains("alipan.com/s/") || low.contains("aliyundrive.com/s/") {
        Some(RawProvider::Ali)
    } else if low.contains("115.com/s/") || low.contains("115cdn.com/s/") {
        Some(RawProvider::P115)
    } else if low.contains("pan.xunlei.com/s/") {
        Some(RawProvider::Thunder)
    } else if low.contains("cloud.189.cn/") {
        Some(RawProvider::Yd)
    } else {
        None
    }
}

/// 详情页解析结果。
#[derive(Debug, Default, Clone)]
pub struct DetailData {
    pub name: String,
    pub cover: Option<String>,
    pub desc: Option<String>,
    pub links: Vec<String>,
}

/// 详情页 HTML → 结构化字段（拆出来便于单测，不触网）。
pub fn parse_detail(html: &str, base: &str) -> DetailData {
    let name = text_after(html, "page-title")
        .or_else(|| text_after(html, "<h1"))
        .unwrap_or_default();
    let cover = attr_after(html, "module-item-pic", "data-src").map(|s| absolute(base, &s));
    let mut metas: Vec<String> = Vec::new();
    let mut from = 0usize;
    while let Some(pos) = html[from..].find("tag-link") {
        let start = from + pos;
        if let Some(text) = text_block_after(&html[start..], "tag-link") {
            if !metas.contains(&text) {
                metas.push(text);
            }
        }
        from = start + "tag-link".len();
    }
    let desc = if metas.is_empty() {
        None
    } else {
        Some(metas.join(" / "))
    };
    let mut links: Vec<String> = Vec::new();
    let mut from = 0usize;
    while let Some(pos) = html[from..].find("module-row-text") {
        let start = from + pos;
        let window = &html[start..(start + 800).min(html.len())];
        if let Some(url) = attr_after(window, "module-row-text", "data-clipboard-text") {
            let url = url.trim().to_string();
            if url.starts_with("http") && !links.contains(&url) {
                links.push(url);
            }
        }
        from = start + "module-row-text".len();
    }
    DetailData {
        name,
        cover,
        desc,
        links,
    }
}

fn load_pan_config(site: &Site) -> PanConfig {
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

/// 单条网盘分享 → (线路名, 剧集列表)。引擎展开失败时降级成「一条单集」。
fn expand_link(
    cfg: &TvConfig,
    conf: &PanConfig,
    provider: RawProvider,
    url: &str,
) -> (String, Vec<Episode>) {
    let line = provider.display().to_string();
    let pan = match provider.pan() {
        Some(p) => p,
        None => {
            return (
                line,
                vec![Episode {
                    title: provider.display().to_string(),
                    url: url.to_string(),
                }],
            )
        }
    };
    let link = match parse_share_link(url) {
        Some(l) if l.provider == pan => l,
        _ => {
            return (
                line,
                vec![Episode {
                    title: provider.display().to_string(),
                    url: url.to_string(),
                }],
            )
        }
    };
    let files = match browse_share(cfg, conf, &link) {
        Ok(files) => files,
        Err(_) => {
            return (
                line,
                vec![Episode {
                    title: provider.display().to_string(),
                    url: url.to_string(),
                }],
            )
        }
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
        episodes.push(Episode {
            title: provider.display().to_string(),
            url: url.to_string(),
        });
    }
    (line, episodes)
}

/// 详情：解析字段 + 展开网盘分享成线路。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let kind = kind_of(&site.api).unwrap_or(Kind::Wogg);
    let base = base_of(kind, site);
    let url = absolute(&base, id);
    let html = fetch_text(cfg, &url)?;
    let data = parse_detail(&html, &base);
    if data.links.is_empty() {
        return Err(parse_err(
            site,
            format!("详情页未找到网盘分享（.module-row-text[data-clipboard-text]）：{url}"),
        ));
    }
    let conf = load_pan_config(site);
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in data.links.iter().take(MAX_RESOURCES) {
        let Some(provider) = classify_link(raw) else {
            continue;
        };
        // 同一线路名只保留第一组，避免重复。
        if groups.iter().any(|g| g.group == provider.display()) {
            continue;
        }
        if seen.contains(&provider.display().to_string()) {
            continue;
        }
        seen.push(provider.display().to_string());
        let (line, items) = expand_link(cfg, &conf, provider, raw);
        if items.is_empty() {
            continue;
        }
        groups.push(EpisodeGroup {
            group: line,
            items,
        });
    }
    if groups.is_empty() {
        return Err(parse_err(
            site,
            "详情页有网盘链接但都不受支持（或展开失败）",
        ));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: data.name,
        cover: data.cover,
        desc: data.desc,
        ..Default::default()
    }
    .with_groups(groups))
}

impl VideoDetail {
    fn with_groups(mut self, groups: Vec<EpisodeGroup>) -> VideoDetail {
        self.episodes = groups;
        self
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
        // 桌面端没有 WebView 弹幕层；这里只记录偏差，不影响网盘直链分支。
        tracing::debug!("wogg: 站点启用了弹幕，原生实现走网盘直链分支");
    }
    let (share, fid) = match id.split_once("###") {
        Some((share, fid)) if !share.is_empty() && !fid.is_empty() => (share, fid),
        _ => {
            return Err(host_err(
                site,
                format!("线路「{flag}」没有展开出剧集标识（缺少 ###fid）"),
            ))
        }
    };
    let provider = classify_link(share)
        .ok_or_else(|| parse_err(site, format!("无法识别网盘分享：{share}")))?;
    if provider.pan().is_none() {
        return Err(host_err(
            site,
            format!(
                "{}网盘播放引擎未实现（桌面端无对应 parser），线路「{flag}」不可播放",
                provider.display()
            ),
        ));
    }
    let conf = load_pan_config(site);
    let link = parse_share_link(share)
        .ok_or_else(|| parse_err(site, format!("无法识别的网盘分享：{share}")))?;
    let play = resolve_play_url(cfg, &conf, &link, fid).map_err(|e| {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn site(api: &str, ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "wogg-test".to_string();
        s.name = "Wogg".to_string();
        s.api = api.to_string();
        s.ext = Some(json!(ext));
        s
    }

    fn cfg() -> TvConfig {
        TvConfig::default()
    }

    #[test]
    fn handles_is_case_insensitive() {
        for api in ["csp_Wogg", "CSP_WOBG", "csp_Wo4k", "csp_WoGG"] {
            assert!(handles(api), "{api}");
        }
        assert!(!handles("csp_Foo"));
        assert!(!handles(""));
    }

    #[test]
    fn kind_of_maps_each_api() {
        assert_eq!(kind_of("csp_Wogg"), Some(Kind::Wogg));
        assert_eq!(kind_of("csp_Wobg"), Some(Kind::Wobg));
        assert_eq!(kind_of("csp_Wo4k"), Some(Kind::Wo4k));
        assert_eq!(kind_of("csp_WoGG"), Some(Kind::WoggV2));
        assert_eq!(kind_of("csp_woggv2"), Some(Kind::WoggV2));
        assert_eq!(kind_of("csp_Other"), None);
    }

    #[test]
    fn base_of_reads_second_http_segment_and_json_site_url() {
        let plain = "./lib/token.json$$$https://wobge.run.goorm.io$$$null$$$1";
        assert_eq!(base_of(Kind::Wobg, &site("csp_Wobg", plain)), "https://wobge.run.goorm.io");

        let json_ext =
            r##"{"Cloud-drive":"tvfan/Cloud-drive.txt","from":"4k|auto","siteUrl":"https://www.wogg.net/","danMu":"弹"}"##;
        let s = site("csp_WoGG", json_ext);
        assert_eq!(base_of(Kind::WoggV2, &s), "https://www.wogg.net");
        assert!(danmaku_enabled(&s));

        // 空 ext → 各变体默认基址。
        let empty = Site::default();
        assert_eq!(base_of(Kind::Wogg, &empty), "https://tvfan.xxooo.cf");
        assert_eq!(base_of(Kind::Wobg, &empty), "https://wobge.run.goorm.io");
        assert_eq!(base_of(Kind::Wo4k, &empty), "https://www.wo4k.net");
    }

    #[test]
    fn danmaku_flag_detects_both_string_and_json_forms() {
        assert!(danmaku_enabled(&site("csp_WoGG", "http://x$$$https://www.wogg.xyz/$$$弹")));
        assert!(danmaku_enabled(&site("csp_Wogg", "./lib/a$$$https://a$$$null$$$1")));
        assert!(!danmaku_enabled(&site("csp_Wogg", "./lib/a$$$https://a$$$null$$$0")));
        assert!(!danmaku_enabled(&site("csp_Wogg", "./lib/a$$$https://a$$$proxy")));
    }

    #[test]
    fn list_url_uses_dash_join_for_wogg_and_templates_for_subclasses() {
        let url = list_url(Kind::Wogg, "https://tvfan.xxooo.cf", "7", 2);
        assert!(url.starts_with("https://tvfan.xxooo.cf/index.php/vodshow/7"));
        assert!(url.ends_with("2---.html"), "{url}");

        let wobg = list_url(Kind::Wobg, "https://wobge.run.goorm.io", "22", 3);
        assert_eq!(
            wobg,
            "https://wobge.run.goorm.io/index.php/vod/type/id/22/page/3.html"
        );

        let wo4k = list_url(Kind::Wo4k, "https://www.wo4k.net", "5", 1);
        assert_eq!(wo4k, "https://www.wo4k.net/pianku/5.html");
        let wo4k2 = list_url(Kind::Wo4k, "https://www.wo4k.net", "5", 2);
        assert_eq!(wo4k2, "https://www.wo4k.net/pianku/5.html?page=2");
    }

    #[test]
    fn parse_categories_finds_nav_links() {
        let html = r##"<nav><a class="nav-link" href="/vodtype/1.html">电影</a>
<a class="nav-link" href="/vodtype/2.html">电视剧</a></nav>"##;
        let cats = parse_categories(html, Kind::Wogg);
        assert_eq!(cats.len(), 2, "{cats:?}");
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "2");
        assert_eq!(cats[1].type_name, "电视剧");
    }

    #[test]
    fn parse_list_reads_module_items() {
        let html = r##"
<div class="module-item">
  <div class="module-item-pic"><img data-src="/upload/a.jpg" alt="a"></div>
  <div class="video-name"><a href="/detail/1.html">第一部</a></div>
  <div class="module-item-text">更新至 12 集</div>
</div>
<div class="module-item module-item-2">
  <div class="module-item-pic"><img data-src="https://cdn/b.jpg"></div>
  <div class="video-name"><a href="https://tvfan.xxooo.cf/detail/2.html">第二部</a></div>
  <div class="module-item-text"><em>全 24 集</em></div>
</div>"##;
        let items = parse_list(html, "https://tvfan.xxooo.cf");
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0].id, "https://tvfan.xxooo.cf/detail/1.html");
        assert_eq!(items[0].name, "第一部");
        assert_eq!(
            items[0].cover.as_deref(),
            Some("https://tvfan.xxooo.cf/upload/a.jpg")
        );
        assert_eq!(items[0].remark.as_deref(), Some("更新至 12 集"));
        assert_eq!(items[1].id, "https://tvfan.xxooo.cf/detail/2.html");
        assert_eq!(items[1].name, "第二部");
        assert_eq!(items[1].cover.as_deref(), Some("https://cdn/b.jpg"));
        assert_eq!(items[1].remark.as_deref(), Some("全 24 集"));
    }

    #[test]
    fn page_count_rounds_up_by_72() {
        let html = r##"<script>$(".mac_total").text('145');</script>"##;
        assert_eq!(page_count(html, 1), 3);
        let small = r##"$(".mac_total").text('10');"##;
        assert_eq!(page_count(small, 1), 1);
        assert_eq!(page_count("<html></html>", 4), 4);
    }

    #[test]
    fn parse_search_reads_search_items() {
        let html = r##"
<div class="module-search-item">
  <div class="module-item-pic"><img data-src="/s/a.jpg"></div>
  <div class="video-serial"><a href="/detail/9.html" title="搜索命中">第九部</a></div>
  <div class="video-tag-icon">HD</div>
</div>"##;
        let items = parse_search(html, "https://www.wogg.net");
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].id, "https://www.wogg.net/detail/9.html");
        assert_eq!(items[0].name, "第九部");
        assert_eq!(items[0].cover.as_deref(), Some("https://www.wogg.net/s/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("HD"));
    }

    #[test]
    fn classify_link_covers_every_provider() {
        let cases = [
            ("https://pan.quark.cn/s/abc", RawProvider::Quark),
            ("https://drive.uc.cn/s/abc", RawProvider::Uc),
            ("https://www.alipan.com/s/abc", RawProvider::Ali),
            ("https://115.com/s/abc", RawProvider::P115),
            ("https://pan.xunlei.com/s/abc", RawProvider::Thunder),
            ("https://cloud.189.cn/t/abc", RawProvider::Yd),
        ];
        for (url, want) in cases {
            assert_eq!(classify_link(url), Some(want), "{url}");
        }
        assert_eq!(classify_link("https://example.org/x"), None);
        assert!(RawProvider::Thunder.pan().is_none());
        assert!(RawProvider::Quark.pan().is_some());
    }

    #[test]
    fn parse_detail_reads_fields_and_dedupes_links() {
        let html = r##"
<div class="video-info-header">
  <h1 class="page-title">测试影片</h1>
  <a class="tag-link" href="#">2024</a>
  <a class="tag-link" href="#">国产剧</a>
  <div class="tag-link"><a href="#">剧情</a><a href="#">都市</a></div>
</div>
<div class="module-item-pic"><img data-src="/p/cover.jpg"></div>
<div class="module-row-text" data-clipboard-text="https://pan.quark.cn/s/aaa">夸克</div>
<div class="module-row-text" data-clipboard-text="https://pan.quark.cn/s/aaa">夸克</div>
<div class="module-row-text" data-clipboard-text="https://pan.xunlei.com/s/bbb">迅雷</div>"##;
        let d = parse_detail(html, "https://www.wogg.net");
        assert_eq!(d.name, "测试影片");
        assert_eq!(
            d.cover.as_deref(),
            Some("https://www.wogg.net/p/cover.jpg")
        );
        assert_eq!(d.links.len(), 2, "{:?}", d.links);
        assert_eq!(d.links[0], "https://pan.quark.cn/s/aaa");
        assert_eq!(d.links[1], "https://pan.xunlei.com/s/bbb");
        let desc = d.desc.unwrap_or_default();
        assert!(desc.contains("2024") && desc.contains("剧情"), "{desc}");
    }

    #[test]
    fn player_rejects_unexpanded_and_unsupported_lines() {
        let s = site("csp_Wogg", "./lib/token.json$$$https://tvfan.xxooo.cf$$$null$$$1");
        let err = player_content(&cfg(), &s, "夸克", "https://pan.quark.cn/s/aaa")
            .expect_err("缺少 ###fid 应报错");
        assert!(
            err.to_string().contains("剧集标识"),
            "err={err}"
        );

        let err = player_content(
            &cfg(),
            &s,
            "迅雷",
            "https://pan.xunlei.com/s/bbb###fid1",
        )
        .expect_err("迅雷无引擎应报错");
        let msg = err.to_string();
        assert!(msg.contains("迅雷"), "{msg}");
        assert!(msg.contains("网盘播放引擎未实现"), "{msg}");
    }
}
