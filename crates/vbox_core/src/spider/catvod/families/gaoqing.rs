//! `csp_Gaoqing`（高清网 / 音范丝）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/tmp/r148/jadx/Gaoqing.bad.java`（jadx `--show-bad-code` 657 行）
//! * `_rea/tmp/r148/vf/com/github/catvod/spider/Gaoqing.java`（dex2jar + Vineflower 3420 行）
//! * 字符串解码：`_rea/tmp/r148/Gaoqing.decoded2.java` / `gaoqing-strings2.txt`（69 处、53 条唯一）
//! * 站点 `_rea/samples/inventory/sites.json`：2 站 `Gaoqing`（GYCK/4k），ext 4 段。
//!
//! 协议（原类逐段还原）：
//! * `extends Push`；`init` 把 ext 按 `$$$` 切：段 1（>7 字符）为基址、段 2 为代理、
//!   段 3 `"1"` 为 VIP 解析开关、段 4 为自定义 filters JSON；`getHeader()` 默认 Chrome/128 UA。
//! * `f(url)`：原类用 WebView 过 JS 倒计时挑战后取页；响应含
//!   `window.location.replace('/jump-node')` 时请求 `/jump-node` 再带 Cookie 重试。
//! * 首页 `homeContent`：GET 基址；分类 `.mainmenu li.menu-item a`（跳过「首页」）；
//!   列表 `#post_container li.post`（`img[src]` 封面、`a[href]` id、`h2` 标题、`.info` 备注）。
//! * 分类 `categoryContent`：`{base}{cate}/page/{page}`；搜索：`{base}/?s={kw}`（page>1 空）。
//! * 详情 `detailContent`：标题 `.article_container h1`、封面 `#post_content img[src]`、
//!   链接 `#post_content a[href]` 按引擎正则分流（Quark/UC/Ali/Thunder/YD/P115），
//!   `magnet:` 取 `&` 前段；表格磁力 `table#cili tr:has(a[href^=magnet])` 的 `a span`/`a[href]`。
//!   每个内容磁力一条线路「磁力N」，表格磁力合并成一条「磁力N」；网盘线路按配置顺序
//!   `thunder|uc|yd|115|ali|quark` 由对应引擎展开为剧集。
//! * 播放：`磁力/电驴` 或 `ed2k:/magnet:/ppmag:/pphttp:` 前缀直出 `parse:0`；
//!   `DL+0` 开头为听书音频；其余线路按网盘引擎解析直链。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. 原类 WebView 挑战改为 HTTP 等价模拟（读取 `/jump-node` 的 Set-Cookie 后重试）；
//! 2. 迅雷/天翼（YD）没有 W9 引擎：详情跳过对应线路，播放返回 `catvod.host.unsupported`；
//! 3. `DL+0` 听书分支按基址拼 `/ebookmp4/magnet_*`（原类用宿主代理域名）；
//! 4. `global_magnet_systemopen`/`global_danmu_disabled` 全局开关视为 false；
//! 5. 站点 `yinfans.me` 现已改为跳转页（`/jump-node` → 无关站点）、`gaoqing.la` 不可达，
//!    因此本家族只有 mock 验收，无真站 `#[ignore]` 用例。

use std::collections::{HashMap, HashSet};

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::{parse_share_link, PanProvider};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};
use crate::spider::catvod::families::yunpan;

pub const API: &str = "csp_Gaoqing";
const DEFAULT_BASE: &str = "https://gaoqing.la";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
const GROUP: &str = "高清网";
const MAGNET_LINE: &str = "磁力";
const EBOOK_PREFIX: &str = "DL+0";
const MAX_LINKS: usize = 100;
const MAX_DIRS: usize = 6;
const ENGINE_ORDER: [&str; 6] = ["thunder", "uc", "yd", "115", "ali", "quark"];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

/// 基址：段 1（长度 > 7 才生效），保留原类语义。
pub fn base_of(site: &Site) -> String {
    let segs = ext_segments(site);
    let raw = segs
        .get(1)
        .map(|s| s.trim())
        .filter(|s| s.len() > 7)
        .unwrap_or(DEFAULT_BASE);
    raw.trim_end_matches('/').to_string()
}

/// 段 4 的 filters JSON（原类存入 `q`），失败返回空表。
fn filters_of(site: &Site) -> HashMap<String, String> {
    let segs = ext_segments(site);
    let Some(raw) = segs.get(4) else {
        return HashMap::new();
    };
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return HashMap::new();
    }
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|obj| {
            obj.into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn headers_of(site: &Site) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    let _ = site;
    h
}

fn is_challenge(body: &str) -> bool {
    body.contains("jump-node") || body.contains("id=\"countdown\"")
}

fn origin_of(url: &str) -> Option<String> {
    let scheme = url.find("://")?;
    let rest = &url[scheme + 3..];
    let end = rest.find('/').unwrap_or(rest.len());
    Some(format!("{}{}", &url[..scheme + 3], &rest[..end]))
}

fn absolute(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    // 其它 scheme（magnet:/ed2k:/pphttp:/ppmag: 等）原样保留，不能拼基址。
    if let Some(pos) = href.find(':') {
        if pos > 0 && !href[..pos].contains('/') {
            return href.to_string();
        }
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if let Some(rest) = href.strip_prefix('/') {
        if let Some(origin) = origin_of(base) {
            return format!("{origin}/{rest}");
        }
        return href.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), href)
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

fn sel(expr: &str) -> Option<Selector> {
    Selector::parse(expr).ok()
}

fn select<'a>(root: &'a Html, expr: &str) -> Vec<ElementRef<'a>> {
    let Some(s) = sel(expr) else { return Vec::new() };
    root.select(&s).collect()
}

fn child_select<'a>(root: ElementRef<'a>, expr: &str) -> Vec<ElementRef<'a>> {
    let Some(s) = sel(expr) else { return Vec::new() };
    root.select(&s).collect()
}

fn text_of(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join("").trim().to_string()
}

fn attr_of(el: ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or("").trim().to_string()
}

fn strip_tags(input: &str) -> String {
    let re = Regex::new(r"(?is)<[^>]+>").expect("strip tags");
    re.replace_all(input, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 取页：直连 + JS 倒计时挑战的 HTTP 等价模拟（原类用 WebView）。
fn get_page(cfg: &TvConfig, site: &Site, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let headers = headers_of(site);
    let body = crate::http::get_text(&client, url, &headers, 2)?;
    if !is_challenge(&body) {
        return Ok(body);
    }
    let origin = origin_of(url).ok_or_else(|| parse_err(site, "无法解析站点域名"))?;
    let jump = format!("{origin}/jump-node");
    let mut req = client.get(&jump).header("User-Agent", UA).header("Referer", url);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req
        .send()
        .map_err(|e| parse_err(site, format!("挑战跳转失败: {e}")))?;
    let mut cookie = String::new();
    for (k, v) in resp.headers() {
        if !k.as_str().eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        if let Ok(raw) = v.to_str() {
            if let Some(kv) = raw.split(';').next() {
                if kv.contains('=') {
                    if !cookie.is_empty() {
                        cookie.push_str("; ");
                    }
                    cookie.push_str(kv.trim());
                }
            }
        }
    }
    let mut retry_headers = headers;
    if !cookie.is_empty() {
        retry_headers.insert("Cookie".to_string(), cookie);
    }
    let retry = crate::http::get_text(&client, url, &retry_headers, 1)?;
    if is_challenge(&retry) {
        return Err(parse_err(site, "站点挑战未通过（原类依赖 WebView）"));
    }
    Ok(retry)
}

fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for a in select(&doc, ".mainmenu li.menu-item a") {
        let href = attr_of(a, "href");
        let name = text_of(a);
        if href.is_empty() || name.is_empty() || name.contains("首页") {
            continue;
        }
        if !seen.insert(href.clone()) {
            continue;
        }
        out.push(Category { type_id: href, type_name: name });
    }
    out
}

fn parse_rows(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for li in select(&doc, "#post_container li.post") {
        let id = child_select(li, "a").first().map(|a| attr_of(*a, "href")).unwrap_or_default();
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let name = child_select(li, "h2").first().map(|h| text_of(*h)).unwrap_or_default();
        let cover = child_select(li, "img").first().map(|i| absolute(base, &attr_of(*i, "src")));
        let remark = child_select(li, ".info").first().map(|p| text_of(*p)).filter(|s| !s.is_empty());
        out.push(VideoItem { id, name, cover, remark });
    }
    out
}

fn list_url(base: &str, cate: &str, page: u32) -> String {
    let cate = cate.trim();
    if cate.starts_with("http://") || cate.starts_with("https://") {
        return format!("{}/page/{}", cate.trim_end_matches('/'), page);
    }
    let cate = cate.trim_start_matches('/');
    format!("{}/{}/page/{}", base.trim_end_matches('/'), cate.trim_end_matches('/'), page)
}

fn search_url(base: &str, keyword: &str) -> String {
    format!("{}/?s={}", base.trim_end_matches('/'), urlencode(keyword))
}

/// 详情页 `#post_content a` 的链接分类结果。
#[derive(Debug, Default)]
struct RawLinks {
    magnet: Vec<String>,
    table_magnet: Vec<String>,
    ali: Vec<String>,
    quark: Vec<String>,
    uc: Vec<String>,
    thunder: Vec<String>,
    yd: Vec<String>,
    p115: Vec<String>,
}

impl RawLinks {
    fn has_magnet(&self) -> bool {
        !self.magnet.is_empty() || !self.table_magnet.is_empty()
    }
}

fn classify_link(raw: &str) -> Option<&'static str> {
    let s = raw.trim();
    if s.contains("pan.quark.cn/s/") {
        Some("quark")
    } else if s.contains("alipan.com/s/") || s.contains("aliyundrive.com/s/") {
        Some("ali")
    } else if s.contains("pan.xunlei.com/s/") {
        Some("thunder")
    } else if s.contains("cloud.189.cn/") {
        Some("yd")
    } else if s.contains("115.com/s/") || s.contains("115cdn.com/s/") || s.contains("anxia.com/s/") {
        Some("115")
    } else if s.contains("drive.uc.cn/s/") || s.contains("fast.uc.cn/s/") {
        Some("uc")
    } else {
        None
    }
}

fn clean_magnet(raw: &str) -> String {
    raw.trim().split('&').next().unwrap_or("").trim().to_string()
}

fn parse_detail(html: &str, base: &str, id: &str) -> VideoDetail {
    let doc = Html::parse_document(html);
    let name = select(&doc, ".article_container h1")
        .first()
        .map(|h| text_of(*h))
        .unwrap_or_default();
    let cover = select(&doc, "#post_content img")
        .first()
        .map(|i| absolute(base, &attr_of(*i, "src")))
        .filter(|s| !s.is_empty());
    let mut links = RawLinks::default();
    let anchors = select(&doc, "#post_content a");
    for (idx, a) in anchors.iter().enumerate() {
        if idx >= MAX_LINKS {
            break;
        }
        let href = attr_of(*a, "href");
        if href.is_empty() {
            continue;
        }
        let href = absolute(base, &href);
        if href.starts_with("magnet:") {
            let m = clean_magnet(&href);
            if !links.magnet.contains(&m) && !m.is_empty() {
                links.magnet.push(m);
            }
            continue;
        }
        match classify_link(&href) {
            Some("quark") => links.quark.push(href),
            Some("ali") => links.ali.push(href),
            Some("uc") => links.uc.push(href),
            Some("thunder") => links.thunder.push(href),
            Some("yd") => links.yd.push(href),
            Some("115") => links.p115.push(href),
            _ => {}
        }
    }
    // 表格磁力：`table#cili tr:has(a[href^=magnet])`（scraper 不支持 `:has()`，手工过滤）。
    let mut table_seen = HashSet::new();
    for tr in select(&doc, "table#cili tr") {
        let Some(a) = child_select(tr, "a").into_iter().find(|a| attr_of(*a, "href").starts_with("magnet:")) else {
            continue;
        };
        let m = clean_magnet(&attr_of(a, "href"));
        if m.is_empty() || !table_seen.insert(m.clone()) {
            continue;
        }
        links.table_magnet.push(m);
    }
    let mut groups = Vec::new();
    let mut magnet_no = 0usize;
    for m in &links.magnet {
        magnet_no += 1;
        groups.push(EpisodeGroup {
            group: format!("{MAGNET_LINE}{magnet_no}"),
            items: vec![Episode { title: format!("磁力{magnet_no}"), url: m.clone() }],
        });
    }
    if !links.table_magnet.is_empty() {
        magnet_no += 1;
        groups.push(EpisodeGroup {
            group: format!("{MAGNET_LINE}{magnet_no}"),
            items: links
                .table_magnet
                .iter()
                .enumerate()
                .map(|(i, m)| Episode { title: format!("磁力{}", i + 1), url: m.clone() })
                .collect(),
        });
    }
    let desc = Some(format!(
        "{}（磁力 {} / 阿里 {} / 夸克 {} / UC {}）",
        name,
        links.magnet.len() + links.table_magnet.len(),
        links.ali.len(),
        links.quark.len(),
        links.uc.len()
    ));
    VideoDetail {
        id: id.to_string(),
        name: name.clone(),
        cover,
        desc,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: groups,
    }
}

fn encode_episode(share_url: &str, sub_fid: &str, fid: &str) -> String {
    if sub_fid.is_empty() {
        format!("{share_url}@@{fid}")
    } else {
        format!("{share_url}#/list/share/{sub_fid}@@{fid}")
    }
}

fn split_episode(id: &str) -> Option<(&str, &str)> {
    id.rsplit_once("@@")
}

fn provider_of(link: &crate::pan::ShareLink) -> PanProvider {
    link.provider
}

fn expand_share(
    cfg: &TvConfig,
    _site: &Site,
    conf: &crate::pan::PanConfig,
    share_url: &str,
) -> Result<Vec<Episode>> {
    let Some(link) = parse_share_link(share_url) else {
        return Ok(Vec::new());
    };
    if matches!(provider_of(&link), PanProvider::P115) && conf.effective_p115_cookie().is_empty() {
        return Ok(Vec::new());
    }
    let files = yunpan::browse_share(cfg, conf, &link)?;
    let mut out = Vec::new();
    for f in &files {
        if !f.is_dir {
            if yunpan::is_video_name(&f.name) {
                out.push(Episode {
                    title: f.name.clone(),
                    url: encode_episode(share_url, &link.sub_fid, &f.fid),
                });
            }
            continue;
        }
        if !link.sub_fid.is_empty() {
            continue;
        }
        let mut sub = link.clone();
        sub.sub_fid = f.fid.clone();
        let nested = match yunpan::browse_share(cfg, conf, &sub) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for nf in nested {
            if nf.is_dir || !yunpan::is_video_name(&nf.name) {
                continue;
            }
            out.push(Episode {
                title: nf.name.clone(),
                url: encode_episode(share_url, &f.fid, &nf.fid),
            });
            if out.len() > MAX_LINKS {
                break;
            }
        }
        if out.len() > MAX_LINKS {
            break;
        }
    }
    Ok(out)
}

fn build_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = absolute(&base, id);
    let html = get_page(cfg, site, &url)?;
    let mut detail = parse_detail(&html, &base, id);
    let conf = yunpan::load_pan_config(site);
    let groups: [(&str, Vec<String>); 6] = [
        ("thunder", Vec::new()),
        ("uc", Vec::new()),
        ("yd", Vec::new()),
        ("115", Vec::new()),
        ("ali", Vec::new()),
        ("quark", Vec::new()),
    ];
    let _ = groups;
    // 按原类顺序尝试线路；无引擎的线路跳过（偏差 2）。
    let raw_magnets = detail
        .episodes
        .iter()
        .filter(|g| g.group.starts_with(MAGNET_LINE))
        .count();
    let _ = raw_magnets;
    let mut links = RawLinks::default();
    // 重新解析链接（parse_detail 已产出磁力组，这里补齐网盘分享）。
    let doc_html = html;
    let base2 = base.clone();
    let doc = Html::parse_document(&doc_html);
    for a in select(&doc, "#post_content a").into_iter().take(MAX_LINKS) {
        let href = absolute(&base2, &attr_of(a, "href"));
        match classify_link(&href) {
            Some("quark") => links.quark.push(href),
            Some("ali") => links.ali.push(href),
            Some("uc") => links.uc.push(href),
            Some("thunder") => links.thunder.push(href),
            Some("yd") => links.yd.push(href),
            Some("115") => links.p115.push(href),
            _ => {}
        }
    }
    for engine in ENGINE_ORDER {
        let (name, urls) = match engine {
            "thunder" => ("迅雷", &links.thunder),
            "uc" => ("UC", &links.uc),
            "yd" => ("YD", &links.yd),
            "115" => ("115", &links.p115),
            "ali" => ("阿里", &links.ali),
            "quark" => ("夸克", &links.quark),
            _ => continue,
        };
        if urls.is_empty() {
            continue;
        }
        if matches!(engine, "thunder" | "yd") {
            continue;
        }
        let mut episodes = Vec::new();
        for u in urls.iter().take(MAX_LINKS) {
            match expand_share(cfg, site, &conf, u) {
                Ok(mut v) => episodes.append(&mut v),
                Err(_) => continue,
            }
            if episodes.len() > MAX_LINKS {
                break;
            }
        }
        if episodes.is_empty() {
            continue;
        }
        detail.episodes.push(EpisodeGroup { group: name.to_string(), items: episodes });
    }
    if detail.episodes.is_empty() {
        return Err(parse_err(site, "详情页未解析到播放地址"));
    }
    Ok(detail)
}

fn player_json(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let header = json!({ "User-Agent": UA, "Referer": format!("{base}/") });
    if let Some(rest) = id.strip_prefix(EBOOK_PREFIX) {
        let seed = rest.trim_start_matches('+').trim_start_matches('$');
        let url = format!(
            "{}/ebookmp4/magnet_{}_{}_{}.mp3",
            base.trim_end_matches('/'),
            seed,
            std::process::id(),
            now_ms() % 10_000
        );
        return Ok(json!({ "parse": 0, "playUrl": "", "url": url, "header": header }));
    }
    if flag.contains("磁力") || flag.contains("电驴") || id.starts_with("ed2k:") || id.starts_with("magnet:") {
        if id.starts_with("ppmag:") || id.starts_with("pphttp:") {
            return Err(host_err(site, "PP 加密磁力需要宿主解密能力"));
        }
        let url = if id.starts_with("ed2k:") { id.to_string() } else { clean_magnet(id) };
        return Ok(json!({ "parse": 0, "playUrl": "", "url": url, "header": header }));
    }
    let (share_url, fid) = split_episode(id).ok_or_else(|| parse_err(site, "非法播放地址"))?;
    let link = parse_share_link(share_url).ok_or_else(|| parse_err(site, "非网盘分享地址"))?;
    if matches!(link.provider, PanProvider::P115) {
        let conf = yunpan::load_pan_config(site);
        if conf.effective_p115_cookie().is_empty() {
            return Err(host_err(site, "115 网盘需要 Cookie"));
        }
    }
    let conf = yunpan::load_pan_config(site);
    let url = yunpan::resolve_play_url(cfg, &conf, &link, fid)?;
    Ok(json!({ "parse": 0, "playUrl": "", "url": url, "header": header }))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = get_page(cfg, site, &format!("{base}/"))?;
    Ok(parse_categories(&html))
}

pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let html = get_page(cfg, site, &format!("{base}/"))?;
    Ok(parse_rows(&html, &base))
}

pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, type_id, page.max(1));
    let html = get_page(cfg, site, &url)?;
    Ok(parse_rows(&html, &base))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    if keyword.trim().is_empty() {
        return Ok(Vec::new());
    }
    let base = base_of(site);
    let url = search_url(&base, keyword);
    let html = get_page(cfg, site, &url)?;
    Ok(parse_rows(&html, &base))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    build_detail(cfg, site, id)
}

pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    player_json(cfg, site, flag, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<Value>) -> Site {
        Site { key: "gj".into(), name: "高清网".into(), api: API.into(), ext, ..Site::default() }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Gaoqing"));
        assert!(handles(" CSP_GAOQING "));
        assert!(!handles("csp_Other"));
    }

    #[test]
    fn base_of_reads_ext_override() {
        assert_eq!(base_of(&site(None)), DEFAULT_BASE);
        let s = site(Some(json!("$$$https://mirror.test/base/$$$null$$$0")));
        assert_eq!(base_of(&s), "https://mirror.test/base");
        let short = site(Some(json!("$$$abc$$$null$$$0")));
        assert_eq!(base_of(&short), DEFAULT_BASE);
    }

    #[test]
    fn absolute_handles_four_forms() {
        let base = "https://www.example.com/x";
        assert_eq!(absolute(base, "https://a.b/c"), "https://a.b/c");
        assert_eq!(absolute(base, "//cdn.test/i.jpg"), "https://cdn.test/i.jpg");
        assert_eq!(absolute(base, "/p/1.html"), "https://www.example.com/p/1.html");
        assert_eq!(absolute(base, "p/1.html"), "https://www.example.com/x/p/1.html");
    }

    #[test]
    fn parse_categories_skips_home_and_dedupes() {
        let html = r#"
        <ul class="mainmenu"><li class="menu-item"><a href="/">首页</a></li>
        <li class="menu-item"><a href="/movie">电影</a></li>
        <li class="menu-item"><a href="/tv">电视剧</a></li>
        <li class="menu-item"><a href="/tv">电视剧</a></li></ul>"#;
        let cats = parse_categories(html);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "/tv");
    }

    #[test]
    fn parse_rows_extracts_title_cover_remark() {
        let html = r#"
        <div id="post_container">
          <ul><li class="post"><div class="post-box"><img src="/i/1.jpg"/><a href="/p/1.html"></a>
          <div class="post-info"><h2> 电影甲 </h2><p class="info">HD 1080P</p></div></div></li>
          <li class="post"><div class="post-box"><a href="/p/2.html"></a><h2>电影乙</h2></div></li>
          </ul></div>"#;
        let rows = parse_rows(html, "https://gaoqing.la");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "/p/1.html");
        assert_eq!(rows[0].name, "电影甲");
        assert_eq!(rows[0].cover.as_deref(), Some("https://gaoqing.la/i/1.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("HD 1080P"));
        assert!(rows[1].cover.is_none());
    }

    #[test]
    fn list_url_and_search_url_shapes() {
        assert_eq!(list_url("https://a.test", "/movie", 2), "https://a.test/movie/page/2");
        assert_eq!(list_url("https://a.test/", "movie", 1), "https://a.test/movie/page/1");
        assert_eq!(search_url("https://a.test/", "沙丘 2"), "https://a.test/?s=%E6%B2%99%E4%B8%98+2");
    }

    #[test]
    fn clean_magnet_strips_query() {
        assert_eq!(clean_magnet("magnet:?xt=urn:btih:abc&dn=x"), "magnet:?xt=urn:btih:abc");
    }

    #[test]
    fn parse_detail_collects_magnets_and_table() {
        let html = r#"
        <div class="article_container"><h1>测试影片</h1></div>
        <div id="post_content">
          <img src="/c.jpg"/>
          <a href="magnet:?xt=urn:btih:111&dn=a">磁力一</a>
          <a href="https://pan.quark.cn/s/aaaa">夸克</a>
          <a href="https://www.alipan.com/s/bbbb">阿里</a>
          <a href="https://pan.xunlei.com/s/cccc">迅雷</a>
        </div>
        <table id="cili"><tr><td><a href="magnet:?xt=urn:btih:222&dn=b"><span>高清</span></a></td></tr>
        <tr><td><a href="magnet:?xt=urn:btih:222&dn=b"><span>重复</span></a></td></tr></table>"#;
        let d = parse_detail(html, "https://gaoqing.la", "/p/9.html");
        assert_eq!(d.name, "测试影片");
        assert_eq!(d.cover.as_deref(), Some("https://gaoqing.la/c.jpg"));
        let magnet_groups: Vec<_> = d.episodes.iter().filter(|g| g.group.starts_with(MAGNET_LINE)).collect();
        assert_eq!(magnet_groups.len(), 2, "内容磁力 1 条 + 表格磁力 1 条");
        assert_eq!(magnet_groups[0].items[0].url, "magnet:?xt=urn:btih:111");
        assert_eq!(magnet_groups[1].items.len(), 1, "表格磁力按 url 去重");
    }

    #[test]
    fn episode_id_round_trip() {
        let id = encode_episode("https://www.alipan.com/s/xyz", "", "f1");
        assert_eq!(split_episode(&id), Some(("https://www.alipan.com/s/xyz", "f1")));
        let nested = encode_episode("https://www.alipan.com/s/xyz", "d1", "f2");
        let (url, fid) = split_episode(&nested).expect("split");
        assert!(url.ends_with("#/list/share/d1"));
        assert_eq!(fid, "f2");
    }

    #[test]
    fn player_direct_magnet_and_ed2k() {
        let s = site(None);
        let v = player_json(&TvConfig::default(), &s, "磁力1", "magnet:?xt=urn:btih:abc&dn=x").unwrap();
        assert_eq!(v["parse"], 0);
        assert_eq!(v["url"], "magnet:?xt=urn:btih:abc");
        let e = player_json(&TvConfig::default(), &s, "", "ed2k://|file|a.mkv|1|H|/").unwrap();
        assert_eq!(e["url"], "ed2k://|file|a.mkv|1|H|/");
    }

    #[test]
    fn player_dl_prefix_builds_ebook_url() {
        let s = site(None);
        let v = player_json(&TvConfig::default(), &s, "听书", "DL+0abc123").unwrap();
        let url = v["url"].as_str().unwrap();
        assert!(url.contains("/ebookmp4/magnet_abc123_"), "{url}");
        assert!(url.ends_with(".mp3"));
    }

    #[test]
    fn player_rejects_illegal_id() {
        let s = site(None);
        assert!(player_json(&TvConfig::default(), &s, "", "not-a-share").is_err());
        assert!(player_json(&TvConfig::default(), &s, "", "magnet:?xt=urn:btih:1@@f").is_ok());
    }
}
