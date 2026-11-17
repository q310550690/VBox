//! `csp_FourKHDR` 原生实现（4KHDR 论坛分享站）。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/0f047745-FxzHm/{deobf,sources}/com/github/catvod/spider/FourKHDR.java`
//!   （jadx 390 行、1 处方法跳过；`_rea/tmp/r124/ys.jar` 取 class 后用 Vineflower
//!   `-dgs=1` 全量恢复 2038 行，见 `_rea/tmp/r130/out/FourKHDR.java`）。
//! * 字符串：`_rea/tmp/r124/dec-full.mjs` 解出 95 条；另 5 条 URL 模板在
//!   `x.b(...)` / `w.c(...)` / `e.B(...)` / `A.b(...)` / `T.a.e(...)` 里，用
//!   `_rea/tmp/r130/dec-any.mjs` 通用解码，得到：
//!   - 分类：`/forum.php?mod=forumdisplay&fid=2&filter=typeid&typeid={type_id}&page={page}`
//!   - 首页：`/forum.php?mod=forumdisplay&fid=2&page=1`
//!   - 播放：`x.b("flag:", sb, flag)` + `A.b(" id:", sb, id)`（拼弹幕标题用）
//! * 站点：1 站「4KHDR」`@www.4khdr.cn`，ext `./lib/token.json$$$...`；2026-11 探测
//!   ENOTFOUND/超时 → 只用 mock 验收，无 `#[ignore]` 真站用例。
//!
//! 协议（原类逐段还原）：
//! * `extends Push`；`init` 硬编码 11 个 Discuz 版块分类；ext 按 `$$$` 拆：
//!   段 0 交给 `Push.init`（token 文件，桌面端不参与），段 1 为 Cookie 来源，
//!   段 2 为 `"1"` 时开启 WebView/弹幕播放拦截（`this.k`）。
//! * `homeContent` 只返回 `{"class": 11 分类}`；`homeVideoContent` GET 首页列表。
//! * `categoryContent` GET `...typeid={type_id}&page={page}`，解析 `ul#waterfall li`。
//! * 详情 `detailContent`：GET `h + id`（`n.o` 补全绝对地址），标题
//!   `#thread_subject` 文本取 `/` 前一段，封面 `img.zoom[src]`，简介与备注都取
//!   `td[id^="postmessage_"] font` 文本；网盘链接取 `div.pcb table.t_table a[href]`
//!   （最多 100 条），按 Quark/Ali/Thunder/YD 正则分组，`magnet:` 单独成磁力线路。
//! * `playerContent`：磁力/电驴/ed2k 原样直出（`parse:0`）；夸克→Quark 引擎、
//!   迅雷→Thunder、YD→YD、其余→Ali；`this.k` 为真时走 WebView 弹幕封装。
//! * `searchContent`：先 GET `/search.php` 取 `input[name=formhash]`，再 POST
//!   `/search.php`（formhash/srchtxt/searchsubmit=yes），解析 `div#threadlist ul li`。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. 迅雷云盘与天翼云盘没有 W9 引擎：详情保留线路、播放返回
//!    `catvod.host.unsupported`（与 yunpan/xiaozhang/wogg 同策略）；
//! 2. `this.k`（ext 段 2 = `"1"`）的 WebView 弹幕封装不实现，一律走普通引擎分支；
//! 3. ext 段 1 的 Cookie 文件在桌面端没有 `127.0.0.1:9978` 文件服务器，若该段是
//!    裸 Cookie 文本（含 `=`）则直接作 Cookie 头，否则忽略；
//! 4. Discuz 搜索表单原类对关键词做了两次 URL 编码，桌面端按标准单次编码；
//! 5. 站点不可达，本家族只用 mock 夹具验收；
//! 6. 测试/镜像钩子：ext 段 3 为 http URL 时覆盖基址（原类无此行为）。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, get_text, post_form_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::p115::P115Client;
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};

/// 本家族覆盖的 api。
pub const API: &str = "csp_FourKHDR";
/// 默认基址（原类固定；ext 不覆盖基址）。
pub const DEFAULT_BASE: &str = "https://www.4khdr.cn";
/// 请求 UA（原类固定 Chrome/109）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
/// 详情最多处理的分享链接数（原类循环上限 100）。
const MAX_LINKS: usize = 100;
/// 详情最多展开的网盘线路数。
const MAX_RESOURCES: usize = 12;
/// 单线路最多剧集数。
const MAX_EPISODES: usize = 500;

/// 原类 `init` 硬编码的 11 个 Discuz 版块分类。
pub const CATEGORIES: [(&str, &str); 11] = [
    ("3", "4K电影"),
    ("8", "4K美剧"),
    ("15", "4K华语"),
    ("6", "4K动画"),
    ("11", "4K纪录片"),
    ("4", "4K日韩"),
    ("29", "蓝光电影"),
    ("31", "蓝光美剧"),
    ("33", "蓝光华语"),
    ("32", "蓝光动画"),
    ("34", "蓝光日韩"),
];

/// api 归属判定。字面量放在函数体里，便于门禁工具静态扫描。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case("csp_FourKHDR")
}

/*
 * ============================================================
 * 基础工具
 * ============================================================
 */

fn sel(expr: &'static str) -> &'static Selector {
    static CB: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static Selector>>> = OnceLock::new();
    let m = CB.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = m.lock().unwrap();
    if let Some(s) = guard.get(expr) {
        return s;
    }
    let s: &'static Selector = Box::leak(Box::new(Selector::parse(expr).expect("valid selector")));
    guard.insert(expr, s);
    s
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn attr_of(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").trim().to_string()
}

fn absolute(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
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

/// 原类 `b()`：UA + Accept + Cookie。
fn headers(cookie: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Accept".to_string(), "application/json, text/plain, */*".to_string());
    if !cookie.is_empty() {
        h.insert("Cookie".to_string(), cookie.to_string());
    }
    h
}

fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string().split("$$$").map(|s| s.trim().to_string()).collect()
}

/// 基址：原类固定为常量；本节额外支持 ext 段 3 的 http URL 覆盖（仅测试/镜像用途，
/// 见模块头偏差 6，原类没有该行为）。
fn base_of(site: &Site) -> String {
    let segs = ext_segments(site);
    if let Some(seg) = segs.get(3) {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.trim_end_matches('/').to_string()
}

/// ext 段 1 的 Cookie：裸 Cookie 文本才使用，文件路径/本地服务器地址忽略（偏差 3）。
fn cookie_of(site: &Site) -> String {
    let segs = ext_segments(site);
    let Some(seg) = segs.get(1) else { return String::new() };
    let seg = seg.trim();
    if seg.is_empty() || seg.eq_ignore_ascii_case("null") {
        return String::new();
    }
    if seg.starts_with("http") || seg.starts_with('.') || seg.starts_with('/') {
        return String::new();
    }
    if seg.contains('=') {
        seg.to_string()
    } else {
        String::new()
    }
}

/// ext 段 2 = `"1"` 且未全局禁用弹幕 → 原类开启 WebView 弹幕播放（本实现忽略，见偏差 2）。
#[allow(dead_code)]
fn danmaku_enabled(site: &Site) -> bool {
    ext_segments(site).get(2).map(|s| s == "1").unwrap_or(false)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

fn fetch(cfg: &TvConfig, url: &str, cookie: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(cookie), 2)
}

/*
 * ============================================================
 * 列表解析（ul#waterfall li）
 * ============================================================
 */

fn item_from(el: &ElementRef<'_>, base: &str) -> Option<VideoItem> {
    let a = el.select(sel("a")).next()?;
    let name = attr_of(&a, "title")
        .or_else(|| {
            let t = text_of(&a);
            if t.is_empty() { None } else { Some(t) }
        })?;
    let id = attr_of(&a, "href")?;
    let cover = el
        .select(sel("img"))
        .next()
        .and_then(|img| attr_of(&img, "src"))
        .map(|src| absolute(base, &src));
    let remark = el
        .select(sel("div.auth.cl"))
        .next()
        .map(|d| text_of(&d))
        .filter(|s| !s.is_empty());
    Some(VideoItem {
        id: absolute(base, &id),
        name,
        cover,
        remark,
    })
}

fn parse_items(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    doc.select(sel("ul#waterfall li"))
        .filter_map(|li| item_from(&li, base))
        .take(MAX_LINKS)
        .collect()
}

fn home_url(base: &str) -> String {
    format!("{base}/forum.php?mod=forumdisplay&fid=2&page=1")
}

fn list_url(base: &str, type_id: &str, page: u32) -> String {
    let type_id = type_id.trim();
    if type_id.is_empty() || type_id == "/" {
        format!("{base}/forum.php?mod=forumdisplay&fid=2&page={page}")
    } else {
        format!(
            "{base}/forum.php?mod=forumdisplay&fid=2&filter=typeid&typeid={type_id}&page={page}"
        )
    }
}

/*
 * ============================================================
 * 详情解析
 * ============================================================
 */

/// 详情页解析出的原始结构（未展开网盘引擎）。
#[derive(Debug, Default)]
struct RawDetail {
    name: String,
    cover: Option<String>,
    desc: Option<String>,
    /// 非磁力链接，按页面顺序保存。
    links: Vec<String>,
    /// `magnet:` 链接，按页面顺序保存。
    magnets: Vec<String>,
}

fn parse_detail(html: &str, base: &str) -> RawDetail {
    let doc = Html::parse_document(html);
    let mut out = RawDetail::default();
    if let Some(t) = doc.select(sel("#thread_subject")).next() {
        out.name = text_of(&t)
            .split('/')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
    }
    if let Some(img) = doc.select(sel("img.zoom")).next() {
        out.cover = attr_of(&img, "src").map(|s| absolute(base, &s));
    }
    if let Some(font) = doc.select(sel(r#"td[id^="postmessage_"] font"#)).next() {
        let text = text_of(&font);
        if !text.is_empty() {
            out.desc = Some(strip_html(&text));
        }
    }
    for a in doc.select(sel("div.pcb table.t_table a")) {
        let Some(href) = attr_of(&a, "href") else { continue };
        if out.links.len() >= MAX_LINKS {
            break;
        }
        if href.starts_with("magnet:") {
            out.magnets.push(href);
        } else {
            out.links.push(href);
        }
    }
    out
}

/*
 * ============================================================
 * Push 网盘分类
 * ============================================================
 */

/// 原类 Push 基类的网盘类型（阿里/夸克/迅雷/YD）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawProvider {
    Ali,
    Quark,
    Thunder,
    Yd,
}

impl RawProvider {
    fn display(self) -> &'static str {
        match self {
            RawProvider::Ali => "阿里",
            RawProvider::Quark => "夸克",
            RawProvider::Thunder => "迅雷",
            RawProvider::Yd => "YD",
        }
    }

    fn pan(self) -> Option<PanProvider> {
        match self {
            RawProvider::Ali => Some(PanProvider::Ali),
            RawProvider::Quark => Some(PanProvider::Quark),
            RawProvider::Thunder | RawProvider::Yd => None,
        }
    }
}

fn provider_regexes() -> &'static (Regex, Regex, Regex, Regex) {
    static CB: OnceLock<(Regex, Regex, Regex, Regex)> = OnceLock::new();
    CB.get_or_init(|| {
        (
            Regex::new(r"(?i)pan\.quark\.cn/s/").unwrap(),
            Regex::new(r"(?i)(alipan\.com|aliyundrive\.com)/s/").unwrap(),
            Regex::new(r"(?i)pan\.xunlei\.com/s/").unwrap(),
            Regex::new(r"(?i)cloud\.189\.cn/").unwrap(),
        )
    })
}

fn classify_link(url: &str) -> Option<RawProvider> {
    let (quark, ali, thunder, yd) = provider_regexes();
    if quark.is_match(url) {
        return Some(RawProvider::Quark);
    }
    if ali.is_match(url) {
        return Some(RawProvider::Ali);
    }
    if thunder.is_match(url) {
        return Some(RawProvider::Thunder);
    }
    if yd.is_match(url) {
        return Some(RawProvider::Yd);
    }
    None
}

fn provider_for_flag(flag: &str) -> RawProvider {
    if flag.starts_with("夸克") {
        RawProvider::Quark
    } else if flag.starts_with("迅雷") {
        RawProvider::Thunder
    } else if flag.starts_with("YD") {
        RawProvider::Yd
    } else {
        RawProvider::Ali
    }
}

/*
 * ============================================================
 * 5 个宿主入口
 * ============================================================
 */

/// 分类：原类 `init` 硬编码 11 个 Discuz 版块。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 首页推荐：GET 版块首页，解析 `ul#waterfall li`。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let cookie = cookie_of(site);
    let url = home_url(&base);
    let html = fetch(cfg, &url, &cookie)?;
    Ok(parse_items(&html, &base))
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let cookie = cookie_of(site);
    let url = list_url(&base, type_id, page);
    let html = fetch(cfg, &url, &cookie)?;
    let items = parse_items(&html, &base);
    if items.is_empty() {
        return Err(parse_err(
            site,
            format!("分类列表未解析出条目（{url}）：页面结构变化或上游空响应"),
        ));
    }
    Ok(items)
}

/*
 * ============================================================
 * 搜索（Discuz：先取 formhash，再 POST）
 * ============================================================
 */

fn formhash_from(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let input = doc.select(sel(r#"input[name="formhash"]"#)).next()?;
    attr_of(&input, "value")
}

fn search_url(base: &str) -> String {
    format!("{base}/search.php")
}

/// 搜索页行解析（`div#threadlist ul li`）。
fn parse_search(html: &str, base: &str, keyword: &str, quick: bool) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(sel("div#threadlist ul li")) {
        let Some(h3) = li.select(sel("h3")).next() else { continue };
        let mut name = text_of(&h3);
        if name.is_empty() {
            continue;
        }
        if quick && !name.contains(keyword) {
            name = keyword.to_string();
        }
        let ps: Vec<ElementRef<'_>> = li.select(sel("p")).collect();
        let remark = ps
            .get(3)
            .map(|p| text_of(p))
            .or_else(|| ps.get(2).map(|p| strip_html(&p.html())))
            .filter(|s| !s.is_empty());
        let id = li.select(sel("a")).next().and_then(|a| attr_of(&a, "href"));
        let Some(id) = id else { continue };
        out.push(VideoItem {
            id: absolute(base, &id),
            name,
            cover: None,
            remark,
        });
    }
    out
}

/// 搜索：GET 取 formhash → POST 搜索表单 → 解析。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let cookie = cookie_of(site);
    let client = client_for(&cfg.http)?;
    let url = search_url(&base);

    let first = get_text(&client, &url, &headers(&cookie), 2)?;
    let formhash = formhash_from(&first).ok_or_else(|| {
        parse_err(
            site,
            format!("搜索页未找到 formhash（{url}）：可能需要 Cookie 或站点改版"),
        )
    })?;

    // 偏差 4：原类对关键词做了两次 URL 编码；桌面端交给表单编码器做单次编码。
    let mut form: HashMap<String, String> = HashMap::new();
    form.insert("formhash".to_string(), formhash);
    form.insert("srchtxt".to_string(), keyword.to_string());
    form.insert("searchsubmit".to_string(), "yes".to_string());
    let html = post_form_text(&client, &url, &headers(&cookie), &form)?;
    Ok(parse_search(&html, &base, keyword, false))
}

/*
 * ============================================================
 * 网盘引擎辅助（与 yunpan 同语义）
 * ============================================================
 */

fn load_pan_config(site: &Site) -> PanConfig {
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

fn find_url(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if s.starts_with("http") => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(find_url),
        Value::Object(m) => {
            for key in ["url", "play_url", "download_url", "raw_url"] {
                if let Some(Value::String(s)) = m.get(key) {
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

fn resolve_play_url(cfg: &TvConfig, conf: &PanConfig, link: &crate::pan::ShareLink, fid: &str) -> Result<String> {
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

/// 单条网盘分享 → 剧集列表；引擎展开失败时降级成一条单集。
fn expand_link(cfg: &TvConfig, conf: &PanConfig, provider: RawProvider, url: &str) -> Vec<Episode> {
    let fallback = || {
        vec![Episode {
            title: provider.display().to_string(),
            url: url.to_string(),
        }]
    };
    let Some(pan) = provider.pan() else {
        return fallback();
    };
    let link = match parse_share_link(url) {
        Some(l) if l.provider == pan => l,
        _ => return fallback(),
    };
    let files = match browse_share(cfg, conf, &link) {
        Ok(files) => files,
        Err(_) => return fallback(),
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
    episodes
}

/// 详情：解析页面 → 按 阿里/夸克/迅雷/YD 顺序分线路展开 → 磁力各成一线。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let cookie = cookie_of(site);
    let url = absolute(&base, id);
    let html = fetch(cfg, &url, &cookie)?;
    let raw = parse_detail(&html, &base);
    if raw.name.is_empty() {
        return Err(parse_err(
            site,
            format!("详情页未解析出标题（{url}）：页面结构变化或需要 Cookie"),
        ));
    }
    let conf = load_pan_config(site);
    let mut groups: Vec<EpisodeGroup> = Vec::new();

    for provider in [RawProvider::Ali, RawProvider::Quark, RawProvider::Thunder, RawProvider::Yd] {
        let links: Vec<&String> = raw
            .links
            .iter()
            .filter(|l| classify_link(l) == Some(provider))
            .take(MAX_RESOURCES)
            .collect();
        if links.is_empty() {
            continue;
        }
        let mut items: Vec<Episode> = Vec::new();
        for link in links {
            items.extend(expand_link(cfg, &conf, provider, link));
        }
        groups.push(EpisodeGroup {
            group: provider.display().to_string(),
            items,
        });
    }

    for (idx, magnet) in raw.magnets.iter().take(MAX_RESOURCES).enumerate() {
        let name = format!("磁力{}", idx + 1);
        groups.push(EpisodeGroup {
            group: name.clone(),
            items: vec![Episode {
                title: name,
                url: magnet.clone(),
            }],
        });
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name: raw.name,
        cover: raw.cover,
        desc: raw.desc,
        episodes: groups,
        ..Default::default()
    })
}

/*
 * ============================================================
 * 播放
 * ============================================================
 */

/// 播放：磁力/电驴直出；其余按线路名分派引擎。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let flag = flag.trim();
    if flag.starts_with("磁力")
        || flag.starts_with("电驴")
        || id.starts_with("ed2k")
        || id.starts_with("magnet")
    {
        return Ok(json!({
            "parse": 0,
            "url": id,
            "jx": 0,
            "playUrl": "",
            "header": {},
        }));
    }

    let provider = provider_for_flag(flag);
    let pan = provider.pan().ok_or_else(|| {
        host_err(
            site,
            format!("{} 网盘播放引擎未实现（桌面端无对应 parser）", provider.display()),
        )
    })?;

    let Some((share, fid)) = id.rsplit_once("###") else {
        return Err(parse_err(
            site,
            format!("播放地址缺少 `分享###fid` 结构：{id}"),
        ));
    };
    let Some(link) = parse_share_link(share) else {
        return Err(parse_err(site, format!("无法识别网盘分享：{share}")));
    };
    if link.provider != pan {
        return Err(parse_err(
            site,
            format!("线路与分享类型不一致：{flag} vs {:?}", link.provider),
        ));
    }

    let conf = load_pan_config(site);
    let url = resolve_play_url(cfg, &conf, &link, fid)?;
    Ok(json!({
        "parse": 0,
        "url": url,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<&str>) -> Site {
        Site {
            key: "fourkhdr-test".to_string(),
            name: "4KHDR".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: ext.map(|e| json!(e)),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles(API));
        assert!(handles("csp_fourkhdr"));
        assert!(!handles("csp_YunPan"));
    }

    #[test]
    fn categories_are_the_eleven_hardcoded_blocks() {
        let c = categories(&TvConfig::default(), &site(None));
        assert_eq!(c.len(), 11);
        assert_eq!(c[0].type_id, "3");
        assert_eq!(c[0].type_name, "4K电影");
        assert_eq!(c[10].type_id, "34");
        assert_eq!(c[10].type_name, "蓝光日韩");
    }

    #[test]
    fn urls_match_discuz_forum_protocol() {
        let base = DEFAULT_BASE.trim_end_matches('/');
        assert_eq!(
            home_url(base),
            "https://www.4khdr.cn/forum.php?mod=forumdisplay&fid=2&page=1"
        );
        assert_eq!(
            list_url(base, "3", 2),
            "https://www.4khdr.cn/forum.php?mod=forumdisplay&fid=2&filter=typeid&typeid=3&page=2"
        );
        assert_eq!(
            list_url(base, "/", 1),
            "https://www.4khdr.cn/forum.php?mod=forumdisplay&fid=2&page=1"
        );
    }

    #[test]
    fn parse_items_reads_waterfall_rows() {
        let html = r##"<ul id="waterfall">
          <li><a href="/thread-1-1-1.html" title="4K电影甲"><img src="/img/a.jpg"></a>
              <div class="auth cl">2026-11-11 / 4K</div></li>
          <li><a href="https://www.4khdr.cn/thread-2-1-1.html">
              <img data-src="/img/b.jpg" src="/img/b.jpg">纯文本标题</a></li>
        </ul>"##;
        let items = parse_items(html, "https://www.4khdr.cn");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "4K电影甲");
        assert_eq!(items[0].id, "https://www.4khdr.cn/thread-1-1-1.html");
        assert_eq!(items[0].cover.as_deref(), Some("https://www.4khdr.cn/img/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("2026-11-11 / 4K"));
        assert_eq!(items[1].name, "纯文本标题");
    }

    #[test]
    fn parse_detail_collects_fields_and_links() {
        let html = r##"<div id="thread_subject">4K电影乙 / 别名</div>
          <img class="zoom" src="/img/z.jpg">
          <table><tr><td id="postmessage_1"><font>剧情简介文本</font></td></tr></table>
          <div class="pcb"><table class="t_table">
            <tr><td><a href="https://pan.quark.cn/s/q1">夸克</a></td></tr>
            <tr><td><a href="https://www.alipan.com/s/a1">阿里</a></td></tr>
            <tr><td><a href="https://pan.xunlei.com/s/t1">迅雷</a></td></tr>
            <tr><td><a href="https://cloud.189.cn/t/y1">YD</a></td></tr>
            <tr><td><a href="magnet:?xt=urn:btih:aa">磁力1</a></td></tr>
            <tr><td><a href="magnet:?xt=urn:btih:bb">磁力2</a></td></tr>
          </table></div>"##;
        let raw = parse_detail(html, "https://www.4khdr.cn");
        assert_eq!(raw.name, "4K电影乙");
        assert_eq!(raw.cover.as_deref(), Some("https://www.4khdr.cn/img/z.jpg"));
        assert_eq!(raw.desc.as_deref(), Some("剧情简介文本"));
        assert_eq!(raw.links.len(), 4);
        assert_eq!(raw.magnets.len(), 2);
    }

    #[test]
    fn classify_links_by_provider() {
        assert_eq!(classify_link("https://pan.quark.cn/s/x"), Some(RawProvider::Quark));
        assert_eq!(classify_link("https://www.alipan.com/s/x"), Some(RawProvider::Ali));
        assert_eq!(classify_link("https://aliyundrive.com/s/x"), Some(RawProvider::Ali));
        assert_eq!(classify_link("https://pan.xunlei.com/s/x"), Some(RawProvider::Thunder));
        assert_eq!(classify_link("https://cloud.189.cn/t/x"), Some(RawProvider::Yd));
        assert_eq!(classify_link("https://example.org/x"), None);
    }

    #[test]
    fn cookie_comes_from_plain_ext_segment() {
        assert_eq!(cookie_of(&site(Some("./lib/token.json$$$uid=1; pass=2"))), "uid=1; pass=2");
        assert_eq!(cookie_of(&site(Some("./lib/token.json$$$./cookie.txt"))), "");
        assert_eq!(cookie_of(&site(Some("./lib/token.json$$$http://127.0.0.1:9978/x"))), "");
        assert_eq!(cookie_of(&site(Some("./lib/token.json$$$null"))), "");
        assert_eq!(cookie_of(&site(None)), "");
    }

    #[test]
    fn flag_maps_to_provider_like_push() {
        assert_eq!(provider_for_flag("夸克云盘"), RawProvider::Quark);
        assert_eq!(provider_for_flag("迅雷网盘"), RawProvider::Thunder);
        assert_eq!(provider_for_flag("YD云盘"), RawProvider::Yd);
        assert_eq!(provider_for_flag("阿里云盘"), RawProvider::Ali);
        assert_eq!(provider_for_flag("其它"), RawProvider::Ali);
    }

    #[test]
    fn player_passes_magnet_through() {
        let cfg = TvConfig::default();
        let s = site(None);
        let v = player_content(&cfg, &s, "磁力1", "magnet:?xt=urn:btih:aa").unwrap();
        assert_eq!(v["parse"], 0);
        assert_eq!(v["url"], "magnet:?xt=urn:btih:aa");
    }

    #[test]
    fn player_rejects_unsupported_and_malformed() {
        let cfg = TvConfig::default();
        let s = site(None);
        let err = player_content(&cfg, &s, "迅雷", "https://pan.xunlei.com/s/x###fid").unwrap_err();
        assert!(err.to_string().contains("引擎未实现"), "{err}");
        let err = player_content(&cfg, &s, "YD", "https://cloud.189.cn/t/x###fid").unwrap_err();
        assert!(err.to_string().contains("引擎未实现"), "{err}");
        let err = player_content(&cfg, &s, "夸克", "no-separator").unwrap_err();
        assert!(err.to_string().contains("分享###fid"), "{err}");
        let err = player_content(&cfg, &s, "夸克", "https://www.alipan.com/s/x###fid").unwrap_err();
        assert!(err.to_string().contains("不一致"), "{err}");
    }

    #[test]
    fn formhash_and_search_rows_parse() {
        let form = r#"<form><input type="hidden" name="formhash" value="abc123"></form>"#;
        assert_eq!(formhash_from(form).as_deref(), Some("abc123"));
        let html = r##"<div id="threadlist"><ul>
          <li><h3>命中标题</h3><p>0</p><p>1</p><p>作者</p><p>2026-11-11</p>
              <a href="/thread-9-1-1.html">查看</a></li>
          <li><h3>无关标题</h3><p>0</p><p>1</p><p>作者</p><p>2026-11-10</p>
              <a href="/thread-10-1-1.html">查看</a></li>
        </ul></div>"##;
        let items = parse_search(html, "https://www.4khdr.cn", "命中", false);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "命中标题");
        assert_eq!(items[0].id, "https://www.4khdr.cn/thread-9-1-1.html");
        assert_eq!(items[0].remark.as_deref(), Some("2026-11-11"));
        let quick = parse_search(html, "https://www.4khdr.cn", "命中", true);
        assert_eq!(quick[1].name, "命中");
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
        let eps = expand_link(&cfg, &conf, RawProvider::Thunder, "https://pan.xunlei.com/s/x");
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].url, "https://pan.xunlei.com/s/x");
    }
}
