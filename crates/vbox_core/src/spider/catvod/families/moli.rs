//! `csp_Moli`（HDmoli 网盘资源站）原生实现。
//!
//! 来源：`_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/Moli.java`
//! （498 行，`playerContent` 被 jadx 跳过 1770 指令，后用 `--show-bad-code`
//! 恢复出 586 行参考）。原实现基于 MyUI（`myui-*`）模板：
//! * `homeContent`：首页导航 `a` → 分类（`/mlist/index{n}.html`）；
//! * `categoryContent`：`{base}/mlist/index{cid}.html`，第 N 页 `...-{N}.html`；
//!   列表行 `ul.myui-vodlist li`（`.myui-vodlist__thumb[href][title]` 标题、
//!   `img.vod-poster[src]` 封面、`.pic-text` 备注）；分页 `x/总页` 或尾页链接；
//! * `detailContent`：`.myui-content__detail h1.title` 标题、
//!   `.myui-content__thumb img.vod-poster[src]` 封面、`meta[name=description]`
//!   简介、`button.resource-option[data-name][data-url]` 网盘资源列表；
//! * `playerContent`：把网盘分享链接交给网盘引擎解析（W9 引擎层：
//!   Quark/UC/115/Ali），`parse:0` 直出。
//!
//! 现站（2026-11）核验：`www.hdmoli.pro` 302 → `www.hdmoli.com`，模板仍为 MyUI，
//! 但资源区从 `ul.downlist a` 改为 `button.resource-option`（夸克/百度/… 多网盘
//! 按钮，`data-url` 为分享链接），故详情按新结构解析、旧结构兜底。
//!
//! 有意偏差：
//! 1. 基址默认 `https://www.hdmoli.com`（原类写死 `https://www.hdmoli.pro`，
//!    现站已 302），`ext` 第二段可覆盖基址；
//! 2. 详情会把网盘分享展开成剧集（经 W9 引擎匿名列目录），失败时降级为
//!    「一个网盘一条线路」并把分享链接直接交给播放（原实现同样在播放期解析，
//!    但展开时机不同）；
//! 3. 播放直链依赖引擎与 Cookie（`quark_cookie`/`uc_cookie`/`p115_cookie`/
//!    `ali_cookie`），无 Cookie 时给出明确 `catvod.host.unsupported` 提示；
//! 4. 搜索用现站 `/search.php?searchtype=5&searchword=`（原类未公开搜索主流程，
//!    按同模板站点惯例实现）；
//! 5. 原实现的弹幕/字幕代理与 `Proxy` 通道不实现（桌面端无对应宿主能力）。

use std::collections::HashMap;

use regex::Regex;
use serde_json::json;
use std::sync::OnceLock;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::p115::P115Client;
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};

/// 画像里的 api id。
pub const API: &str = "csp_Moli";
/// 默认基址（现站域名，`hdmoli.pro` 已 302 到 `hdmoli.com`）。
pub const DEFAULT_BASE: &str = "https://www.hdmoli.com";
/// 请求 UA（原类 Chrome/122 系列）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 详情卡片里「网盘资源」按钮的数据属性。
pub const RESOURCE_ATTR_NAME: &str = "data-name";
/// 详情卡片里分享链接的属性。
pub const RESOURCE_ATTR_URL: &str = "data-url";

/// 单次详情最多展开的网盘资源数。
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

/// 基址：`ext` 里以 `http` 开头的段（原类 ext 形如
/// `./lib/token.json$$$https://www.hdmoli.pro/$$$null$$$1`），否则默认域名。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    for seg in ext.split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), format!("{base}/"));
    h.insert("Accept".to_string(), "text/html,application/xhtml+xml".to_string());
    h
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

/// 分类：首页导航里的 `/mlist/index{n}.html` 链接（原实现按导航取分类）。
pub fn parse_categories(html: &str, base: &str) -> Vec<Category> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?s)<a[^>]+href="(?P<href>/mlist/index\d+\.html)"[^>]*>(?P<name>[^<]{1,24})</a>"#)
            .expect("category regex")
    });
    let mut out: Vec<Category> = Vec::new();
    for cap in re.captures_iter(html) {
        let name = collapse(&cap["name"]);
        if name.is_empty() || out.iter().any(|c| c.type_name == name) {
            continue;
        }
        out.push(Category {
            type_id: cap["href"].to_string(),
            type_name: name,
        });
    }
    if !out.is_empty() {
        return out;
    }
    // 兜底：写死 3 个主分类（现站导航缩略或改版时仍可用）。
    for (id, name) in [("/mlist/index1.html", "电影"), ("/mlist/index2.html", "剧集"), ("/mlist/index41.html", "动画")] {
        out.push(Category {
            type_id: id.to_string(),
            type_name: name.to_string(),
        });
    }
    let _ = base;
    out
}

/// 分类页 URL：`/mlist/index{cid}.html` → 第 N 页 `/mlist/index{cid}-{N}.html`。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let cat = cat.trim();
    let cat = cat.strip_prefix(base).unwrap_or(cat);
    if page <= 1 {
        if cat.starts_with("http") {
            return cat.to_string();
        }
        return format!("{base}{}", if cat.starts_with('/') { cat.to_string() } else { format!("/{cat}") });
    }
    if let Some(rest) = cat.strip_suffix(".html") {
        return format!("{base}{rest}-{page}.html");
    }
    format!("{base}{cat}-{page}.html")
}

/// 总页数：优先 `1/67` 形式，其次尾页链接。
pub fn page_count(html: &str) -> u32 {
    static RE_TOTAL: OnceLock<Regex> = OnceLock::new();
    let re_total = RE_TOTAL.get_or_init(|| Regex::new(r">\s*(\d+)\s*/\s*(\d+)\s*<").expect("total"));
    if let Some(cap) = re_total.captures(html) {
        if let Ok(n) = cap[2].parse::<u32>() {
            if n > 0 {
                return n;
            }
        }
    }
    static RE_TAIL: OnceLock<Regex> = OnceLock::new();
    let re_tail = RE_TAIL.get_or_init(|| {
        Regex::new(r#"href="[^"]*index\d+-(\d+)\.html"[^>]*>尾页"#).expect("tail")
    });
    re_tail
        .captures_iter(html)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max()
        .unwrap_or(1)
}

/// 列表行：`ul.myui-vodlist li` → 标题/封面/备注/详情 id。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let mut out: Vec<VideoItem> = Vec::new();
    // 按 <li 切块（Rust regex 不支持前瞻，不能用 look-ahead 截断）。
    for chunk in html.split("<li").skip(1) {
        let body = chunk.split("</li>").next().unwrap_or(chunk);
        if !body.contains("myui-vodlist__thumb") {
            continue;
        }
        if let Some(item) = parse_row(body, base) {
            if !out.iter().any(|x| x.id == item.id) {
                out.push(item);
            }
        }
    }
    out
}

fn parse_row(body: &str, base: &str) -> Option<VideoItem> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| {
        Regex::new(r#"(?s)<a[^>]+class="[^"]*myui-vodlist__thumb[^"]*"[^>]*>"#).expect("tag")
    });
    let tag_text = tag.find(body)?.as_str();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    let attr = ATTR.get_or_init(|| Regex::new(r#"(?P<k>href|title)="(?P<v>[^"]*)""#).expect("attr"));
    let mut href = String::new();
    let mut title = String::new();
    for cap in attr.captures_iter(tag_text) {
        match &cap["k"] {
            "href" => href = cap["v"].trim().to_string(),
            "title" => title = cap["v"].trim().to_string(),
            _ => {}
        }
    }
    if href.is_empty() || href == "/" {
        return None;
    }
    static IMG: OnceLock<Regex> = OnceLock::new();
    let img = IMG.get_or_init(|| Regex::new(r#"<img[^>]+src="(?P<src>[^"]+)""#).expect("img"));
    let cover = img
        .captures(body)
        .map(|c| join_url(base, &c["src"]));
    static REMARK: OnceLock<Regex> = OnceLock::new();
    let remark = REMARK.get_or_init(|| Regex::new(r#"<span[^>]*class="[^"]*pic-text[^"]*"[^>]*>(?P<t>[^<]*)</span>"#).expect("remark"));
    let remark = remark
        .captures(body)
        .map(|c| collapse(&c["t"]))
        .filter(|s| !s.is_empty());
    let name = {
        let s = collapse(&title);
        if s.is_empty() { None } else { Some(s) }
    };
    let name = name.or_else(|| {
            static H4: OnceLock<Regex> = OnceLock::new();
            let h4 = H4.get_or_init(|| Regex::new(r#"(?s)<h4[^>]*>.*?<a[^>]*>(?P<t>[^<]+)</a>"#).expect("h4"));
            h4.captures(body).map(|c| collapse(&c["t"]))
        })?;
    Some(VideoItem {
        id: join_url(base, &href),
        name,
        cover,
        remark,
    })
}

/// 详情页解析结果。
#[derive(Debug, Default, Clone)]
pub struct DetailData {
    pub name: String,
    pub cover: Option<String>,
    pub desc: Option<String>,
    /// (显示名, 分享链接)
    pub resources: Vec<(String, String)>,
}

pub fn parse_detail(html: &str, base: &str) -> DetailData {
    static H1: OnceLock<Regex> = OnceLock::new();
    let h1 = H1.get_or_init(|| {
        Regex::new(r#"(?s)<h1[^>]*class="[^"]*title[^"]*"[^>]*>(?P<t>[^<]+)</h1>"#).expect("h1")
    });
    let name = h1.captures(html).map(|c| collapse(&c["t"])).unwrap_or_default();
    static COVER: OnceLock<Regex> = OnceLock::new();
    let cover = COVER.get_or_init(|| {
        Regex::new(r#"(?s)<div[^>]*class="[^"]*myui-content__thumb[^"]*".*?<img[^>]+src="(?P<src>[^"]+)""#).expect("cover")
    });
    let cover = cover.captures(html).map(|c| join_url(base, &c["src"]));
    static DESC: OnceLock<Regex> = OnceLock::new();
    let desc = DESC.get_or_init(|| {
        Regex::new(r#"(?s)<div[^>]*class="[^"]*myui-content__desc[^"]*"[^>]*>(?P<t>.*?)</div>"#).expect("desc")
    });
    let desc = desc
        .captures(html)
        .map(|c| strip_tags(&c["t"]))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            static META: OnceLock<Regex> = OnceLock::new();
            let meta = META.get_or_init(|| {
                Regex::new(r#"(?s)<meta[^>]+name="description"[^>]+content="(?P<t>[^"]*)""#).expect("meta")
            });
            meta.captures(html).map(|c| collapse(&c["t"]))
        });
    static RES: OnceLock<Regex> = OnceLock::new();
    let res = RES.get_or_init(|| {
        Regex::new(r#"(?s)<button[^>]*class="[^"]*resource-option[^"]*"(?P<attrs>[^>]*)>"#).expect("res")
    });
    let mut resources = Vec::new();
    for cap in res.captures_iter(html) {
        let attrs = &cap["attrs"];
        let url = attr(attrs, RESOURCE_ATTR_URL).unwrap_or_default();
        if !url.starts_with("http") || parse_share_link(&url).is_none() {
            continue;
        }
        let name = attr(attrs, RESOURCE_ATTR_NAME).unwrap_or_else(|| "网盘".to_string());
        if resources.iter().any(|(_, u)| u == &url) {
            continue;
        }
        resources.push((name, url));
    }
    if resources.is_empty() {
        // 旧模板兜底：`ul.downlist a[href]` 的网盘链接。
        static OLD: OnceLock<Regex> = OnceLock::new();
        let old = OLD.get_or_init(|| {
            Regex::new(r#"(?s)<a[^>]+href="(?P<u>https?://[^"]+)"[^>]*>(?P<t>[^<]{0,24})</a>"#).expect("old")
        });
        for cap in old.captures_iter(html) {
            let url = parse_share_link(cap["u"].trim());
            if url.is_some() {
                resources.push((collapse(&cap["t"]).if_empty("网盘"), cap["u"].to_string()));
            }
        }
    }
    DetailData {
        name,
        cover,
        desc,
        resources,
    }
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"{name}="([^"]*)""#)).ok()?;
    re.captures(attrs).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty())
}

trait IfEmpty {
    fn if_empty(self, alt: &str) -> String;
}
impl IfEmpty for String {
    fn if_empty(self, alt: &str) -> String {
        if self.trim().is_empty() { alt.to_string() } else { self }
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    collapse(&out.replace("&nbsp;", " "))
}

fn join_url(base: &str, url: &str) -> String {
    let url = url.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if url.starts_with("//") {
        return format!("https:{url}");
    }
    format!("{base}/{}", url.trim_start_matches('/'))
}

/// 分类列表（首页导航）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = fetch_text(cfg, &base, &format!("{base}/"))?;
    let cats = parse_categories(&html, &base);
    if cats.is_empty() {
        return Err(parse_err(site, "首页导航未解析出分类（页面结构可能已改版）"));
    }
    Ok(cats)
}

/// 分类视频列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cat, page);
    let html = fetch_text(cfg, &base, &url)?;
    let items = parse_list(&html, &base);
    Ok(items)
}

/// 分类总页数（宿主需要时可调用；本实现直接按请求页返回条目）。
pub fn page_count_of(html: &str) -> u32 {
    page_count(html)
}

/// 详情：解析详情页，并把网盘分享展开成剧集（匿名可列目录时）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        join_url(&base, id)
    };
    let html = fetch_text(cfg, &base, &url)?;
    let data = parse_detail(&html, &base);
    if data.name.is_empty() {
        return Err(parse_err(site, "详情页缺少标题（页面结构可能已改版）"));
    }
    let conf = load_pan_config(site);
    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    for (name, link_url) in data.resources.iter().take(MAX_RESOURCES) {
        let Some(link) = parse_share_link(link_url) else {
            continue;
        };
        let files = match browse_share(cfg, &conf, &link) {
            Ok(files) => files,
            Err(_) => Vec::new(),
        };
        let mut items: Vec<Episode> = files
            .iter()
            .filter(|f| !f.is_dir && is_video_name(&f.name))
            .take(MAX_EPISODES)
            .map(|f| Episode {
                title: f.name.clone(),
                url: format!("{}###{}", link.url, f.fid),
            })
            .collect();
        if items.is_empty() {
            items.push(Episode {
                title: name.clone(),
                url: link.url.clone(),
            });
        }
        episodes.push(EpisodeGroup {
            group: name.clone(),
            items,
        });
    }
    if episodes.is_empty() {
        episodes.push(EpisodeGroup {
            group: "网盘".to_string(),
            items: data
                .resources
                .iter()
                .map(|(name, url)| Episode {
                    title: name.clone(),
                    url: url.clone(),
                })
                .collect(),
        });
    }
    Ok(VideoDetail {
        id: url,
        name: data.name,
        cover: data.cover,
        desc: data.desc,
        episodes,
        ..Default::default()
    })
}

/// 搜索：现站 `/search.php?searchtype=5&searchword={kw}`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let kw = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let url = format!("{base}/search.php?searchtype=5&searchword={kw}");
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_list(&html, &base))
}

/// 播放：解析网盘分享 → 取直链（需引擎与 Cookie）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<serde_json::Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let (link_url, fid) = match id.split_once("###") {
        Some((u, f)) => (u.trim().to_string(), f.trim().to_string()),
        None => (id.to_string(), String::new()),
    };
    let link = parse_share_link(&link_url)
        .ok_or_else(|| host_err(site, format!("不支持或无直链的播放地址：{link_url}")))?;
    let conf = load_pan_config(site);
    let (url, fid) = if fid.is_empty() {
        let files = browse_share(cfg, &conf, &link)?;
        let first = files
            .into_iter()
            .find(|f| !f.is_dir && is_video_name(&f.name))
            .ok_or_else(|| host_err(site, "分享内未找到视频文件"))?;
        (link.url.clone(), first.fid)
    } else {
        (link.url.clone(), fid)
    };
    let play = resolve_play_url(cfg, &conf, &link, &fid).map_err(|e| {
        let class = ErrorClass::classify_pan_error(&e).unwrap_or(ErrorClass::HostApi);
        CatvodError::new(class, site, format!("网盘直链解析失败：{e}")).into_core()
    })?;
    let _ = url;
    Ok(json!({
        "url": play,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

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

fn browse_share(cfg: &TvConfig, conf: &PanConfig, link: &crate::pan::ShareLink) -> Result<Vec<PanEntry>> {
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
        Some("mp4" | "mkv" | "m3u8" | "avi" | "ts" | "flv" | "wmv" | "mov" | "rmvb" | "webm" | "m4v" | "mpg" | "mpeg" | "iso")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "moli".into();
        s.name = "HDmoli".into();
        s.api = API.into();
        s.ext = Some(json!(ext));
        s
    }

    #[test]
    fn base_defaults_and_ext_override() {
        assert_eq!(base_of(&site("")), DEFAULT_BASE);
        assert_eq!(
            base_of(&site("./lib/token.json$$$https://www.hdmoli.pro/$$$null")),
            "https://www.hdmoli.pro"
        );
    }

    #[test]
    fn categories_from_nav() {
        let html = r#"
        <a href="/mlist/index1.html">电影</a>
        <a href="/mlist/index2.html">剧集</a>
        <a href="/mlist/index41.html">动画</a>
        "#;
        let cats = parse_categories(html, DEFAULT_BASE);
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[2].type_id, "/mlist/index41.html");
    }

    #[test]
    fn categories_fallback_when_nav_missing() {
        let cats = parse_categories("<html></html>", DEFAULT_BASE);
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[1].type_name, "剧集");
    }

    #[test]
    fn list_url_first_and_later_pages() {
        let base = DEFAULT_BASE;
        assert_eq!(list_url(base, "/mlist/index1.html", 1), format!("{base}/mlist/index1.html"));
        assert_eq!(list_url(base, "/mlist/index1.html", 3), format!("{base}/mlist/index1-3.html"));
    }

    #[test]
    fn page_count_variants() {
        assert_eq!(page_count(r#"<li class="visible-xs"><a class="btn btn-warm">1/67</a></li>"#), 67);
        assert_eq!(
            page_count(r#"<a class="btn btn-gray" href="/mlist/index1-67.html">尾页</a>"#),
            67
        );
        assert_eq!(page_count("<html></html>"), 1);
    }

    #[test]
    fn parse_list_rows() {
        let html = r#"
        <ul class="myui-vodlist clearfix">
          <li class="col-lg-5"><div class="myui-vodlist__box">
            <a class="myui-vodlist__thumb" href="/movie/index3638.html" title="幻视任务">
              <img class="vod-poster" src="https://img/1.jpg">
              <span class="pic-text text-right">8集全</span>
            </a>
            <div class="myui-vodlist__detail"><h4 class="title"><a href="/movie/index3638.html" title="幻视任务">幻视任务</a></h4></div>
          </div></li>
          <li class="col-lg-5"><div class="myui-vodlist__box">
            <a class="myui-vodlist__thumb" href="/movie/index3639.html" title="奥德赛">
              <img src="https://img/2.jpg"><span class="pic-text">HD</span>
            </a>
          </div></li>
        </ul>"#;
        let items = parse_list(html, DEFAULT_BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "幻视任务");
        assert_eq!(items[0].id, format!("{DEFAULT_BASE}/movie/index3638.html"));
        assert_eq!(items[0].cover.as_deref(), Some("https://img/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("8集全"));
    }

    #[test]
    fn parse_detail_with_resources() {
        let html = r#"
        <div class="myui-content__thumb"><img src="https://img/cover.jpg"></div>
        <div class="myui-content__detail"><h1 class="title text-fff">幻视任务</h1>
          <div class="myui-content__desc">简介文本</div>
        </div>
        <button class="resource-option is-active" data-name="夸克网盘" data-url="https://pan.quark.cn/s/1d4747fa10ca">Q</button>
        <button class="resource-option" data-name="百度网盘" data-url="https://pan.baidu.com/s/abc">B</button>
        <button class="resource-option" data-name="占位" data-url="">X</button>"#;
        let d = parse_detail(html, DEFAULT_BASE);
        assert_eq!(d.name, "幻视任务");
        assert_eq!(d.cover.as_deref(), Some("https://img/cover.jpg"));
        assert_eq!(d.desc.as_deref(), Some("简介文本"));
        assert_eq!(d.resources.len(), 1);
        assert_eq!(d.resources[0], ("夸克网盘".to_string(), "https://pan.quark.cn/s/1d4747fa10ca".to_string()));
    }

    #[test]
    fn parse_detail_falls_back_to_old_downlist() {
        let html = r#"
        <h1 class="title">老剧</h1>
        <ul class="downlist"><li><a href="https://pan.quark.cn/s/abc123">夸克</a></li></ul>"#;
        let d = parse_detail(html, DEFAULT_BASE);
        assert_eq!(d.resources.len(), 1);
        assert_eq!(d.resources[0].0, "夸克");
    }

    #[test]
    fn player_rejects_empty_and_invalid() {
        let cfg = TvConfig::default();
        let err = player_content(&cfg, &site(""), "", "").unwrap_err();
        assert!(err.to_string().contains("播放 id"));
        let err = player_content(&cfg, &site(""), "", "https://example.com/x").unwrap_err();
        assert!(err.to_string().contains("不支持或无直链"), "{err}");
    }

    #[test]
    fn find_url_walks_nested_values() {
        let v = json!({"data": {"items": [{"download_url": "https://cdn/x.m3u8"}]}});
        assert_eq!(find_url(&v).as_deref(), Some("https://cdn/x.m3u8"));
    }
}
