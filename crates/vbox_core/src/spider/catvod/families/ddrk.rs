//! `csp_Ddrk`（低端影视 `ddys`）原生实现。
//!
//! 来源：`_rea/decompiled/ddaf13d7-0206/deobf/com/github/catvod/spider/Ddrk.java`
//! （322 行，其中 `homeContent`/`categoryContent` 被 jadx 跳过）。本轮用
//! `jadx --show-bad-code --single-class com.github.catvod.spider.Ddrk`
//! 对 `ddaf13d7-0206.jpg`（伪装成图片的 dex-jar）重跑，恢复到 **563 行**，
//! 两个核心方法均成为可读 Java，另外 5 个变体（375/390 行）协议一致。
//!
//! 原实现是一个 WordPress 影视站：
//! * 基址写死 `https://ddys.pro`，请求头 `User-Agent: Chrome/102` + `Referer`。
//! * `homeContent`：`li.menu-item a` 的 `title` 作为分类名、`href` 里
//!   `/category/{slug}` 的 slug 作为 `type_id`；首页列表 `div.post-box-container`。
//! * `categoryContent`：`/category/{cat}/`，第 N 页 `/category/{cat}/page/{N}/`；
//!   filter 非空时在分类后追加 `/{value}`（见偏差 3）。
//! * 列表行：`.post-box-title a` 文本 → 名称；文本含 `(` 时括号内为备注，
//!   否则按 `\S+(.*)` 取首个空格后的剩余部分作备注；封面取 `.post-box-image`
//!   的 `style` 里的图片 URL；`vod_id` 是详情 URL 的最后一段。
//! * `detailContent`：`div.post img[data-src]` 封面、`h1.post-title` 标题、
//!   `div.abstract` 简介（顺带用 `类型:`/`年份:`/`地区:`/`演员:`/`导演:`/`简介:`
//!   六条正则切出元数据）；播放列表来自 `.wp-playlist-script` 里的
//!   `{"tracks":[…]}` JSON，每集 `caption$https://ddys.pro/getvddr/video?id={src1}&type=mix|https://ddys.pro/subddr/{subsrc}`，
//!   归入「第1季」；`.post-page-numbers` 里的每个数字 n 再抓 `/{id}/{n}/`
//!   得到「第 n 季」。
//! * `playerContent`：id 是 `视频接口|字幕`，GET 视频接口 JSON 取 `url` 直出（`parse:0`）。
//! * `searchContent`：`/?s={kw}&post_type=post`，行 `article`，
//!   标题取 `.post-title` 内的 `a`，备注是 `time` 文本 + `cat-links` 文本，封面固定。
//!
//! 同一家族同时覆盖 `csp_Ddys`（另一份同样写死 `https://ddys.pro` 的实现，
//! 来源 `_rea/decompiled/5fc50e17-5199d8f8bcbe3c6d6938ded33cb90596/deobf/com/github/catvod/spider/Ddys.java`，
//! 原 493 行、`categoryContent` 等 2 个方法被跳过，用 `--show-bad-code` 恢复到
//! 779 行）。与 `csp_Ddrk` 的差异：
//! * 列表：分类页第一页拼 `/category/{cat}/page/1/`（见偏差 7）。
//! * 播放列表：`srctype==1` → `{base}/getvddr2/video?id={src1}&type=mix`；
//!   `srctype==4` → `https://v.ddys.pro{src3}`；其余为空串；剧集串仍为
//!   `caption$视频接口|{base}/subddr/{subsrc}`。
//! * 播放：`视频接口` 含 `getvddr` 时 GET 其 JSON 取 `url`；返回头带
//!   `User-Agent: Chrome/91 (Mac)` + `Referer`。
//! * 搜索：`/?s={kw}&post_type=post`，行 `div#container article`，标题取
//!   `h2` 且必须包含关键词，备注 `li.meta_categories`，封面固定 favicon。
//!
//! 有意偏差：
//! 1. 上游 `ddys.pro` 已 ENOTFOUND，`ddys.app`/`ddys.tv` 均被 `ddys-protect`
//!    ALTCHA 门禁挡住（HTML 里没有 `post-box-container`），本轮只能 mock 验收；
//!    因此 `base_of` 允许 `ext` 首段 http(s) 覆盖基址（原实现写死基址），
//!    便于镜像站与测试，站点本身 ext 为 null 时行为与原实现一致。
//! 2. 原 `homeContent` 的分类条件被 jadx 重排成「`z && !equals(热映中) && … &&
//!    !equals(综艺)`」，14 个分类名全在排除表里；按该字面意思会得不到分类，
//!    判断为反编译重排失真。本实现按结构语义只跳过 `热映中`/`全部` 两个聚合页，
//!    其余 `/category/` 菜单项按出现顺序去重输出。
//! 3. 原 `categoryContent` 的页码分支同样被重排（`page==1` 才拼 `/page/1`）；
//!    本实现按 WordPress 惯例 `page=1 → {cat}/`、`page>1 → {cat}/page/{N}/`。
//! 4. filter 追加段：原实现遍历 HashMap 取最后一个非空值，顺序不确定；本实现
//!    暂不消费 filter（VBox 家族分派没有把筛选值传进来），记录为待接入。
//! 5. 播放返回去掉原实现的 `subf`/`subt`（`Proxy.proxy(do=ddrk)` 依赖 Android
//!    宿主代理路由，桌面端无等价能力），保留 `url`/`parse:0`。
//! 6. 元数据正则未命中时原实现回退为整段简介文本，本实现回退为 `None`
//!    （避免把整段简介塞进 `type_name`/`vod_year` 等字段）。

use std::collections::HashMap;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Ddrk";
pub const DEFAULT_BASE: &str = "https://ddys.pro";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.62 Safari/537.36";
/// 首页导航里不是分类、需要跳过的聚合页。
const NON_CATEGORY_NAV: [&str; 2] = ["热映中", "全部"];
/// 搜索结果原实现给固定封面。
const SEARCH_COVER: &str = "https://s1.ax1x.com/2023/01/30/pSw9wrV.png";
const PLAY_FROM_FIRST: &str = "第1季";
/// `csp_Ddys` 的 api（与 `csp_Ddrk` 同站点、不同 jar 实现）。
pub const DDYS_API: &str = "csp_Ddys";
/// Ddys 原实现用的 UA（Mac Chrome/91）。
const DDYS_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.114 Safari/537.36";
/// Ddys 搜索结果原实现给固定封面（站点 favicon）。
const DDYS_SEARCH_COVER: &str = "https://ddys.pro/favicon-32x32.png";

/// 是否本家族可处理的 api。
pub fn handles(api: &str) -> bool {
    // `live-coverage`/`site-coverage` 生成器靠扫描本函数体内的 `"csp_…"` 字面量
    // 判断家族是否接管某个 api，因此这里必须显式列出全部 api（不能只藏在
    // `is_ddrk`/`is_ddys` 里头，否则矩阵会把家族标成 no-native-api）。
    matches!(api, "csp_Ddrk" | "csp_Ddys") || is_ddrk(api) || is_ddys(api)
}

/// 是否 `csp_Ddrk`。
pub fn is_ddrk(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 是否 `csp_Ddys`（同站点、另一份实现）。
pub fn is_ddys(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(DDYS_API)
}

/// 请求 UA 按 api 区分（原两份实现各自写死）。
fn ua_of(site: &Site) -> &'static str {
    if is_ddys(&site.api) {
        DDYS_UA
    } else {
        UA
    }
}

/// 基址：原实现写死 `https://ddys.pro`；这里允许 ext 首段 http(s) 覆盖，
/// 方便镜像站与 mock 测试（站点自身 ext 为 null，行为与原实现一致）。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let first = ext.split('$').next().unwrap_or("").trim();
    let base = if first.starts_with("http://") || first.starts_with("https://") {
        first.to_string()
    } else {
        DEFAULT_BASE.to_string()
    };
    base.trim_end_matches('/').to_string()
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".into(), UA.into());
    h.insert("Referer".into(), format!("{base}/"));
    h
}

fn fetch_text(cfg: &TvConfig, site: &Site, base: &str, url: &str) -> Result<String> {
    let mut h = headers(base);
    h.insert("User-Agent".into(), ua_of(site).into());
    let client = client_for(&cfg.http)?;
    Ok(get_text(&client, url, &h, 2)?)
}

fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("ddrk: 非法 CSS 选择器")
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join("")
}

fn collapse(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn abs_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with("//") {
        format!("https:{href}")
    } else {
        join_url(&format!("{base}/"), href)
    }
}

/// 详情 URL 的最后一段：`https://ddys.pro/movie-name/` → `movie-name`。
fn id_from_href(href: &str) -> Option<String> {
    let href = href.trim().split(['?', '#']).next().unwrap_or("").trim();
    let trimmed = href.trim_end_matches('/');
    let seg = trimmed.rsplit('/').next().unwrap_or("").trim();
    if seg.is_empty() {
        None
    } else {
        Some(seg.to_string())
    }
}

/// `href` 里的分类 slug：`/category/{slug}`。
fn category_from_href(href: &str) -> Option<String> {
    let re = Regex::new(r"/category/([^/\s?#]+)").ok()?;
    re.captures(href)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 列表行标题拆成「名称 + 备注」。
///
/// 原实现：含 `(` 时括号内是备注；否则用 `\S+(.*)` 取第一个空格后的剩余部分。
fn split_title(raw: &str) -> (String, Option<String>) {
    let raw = collapse(raw);
    if let Some((name, rest)) = raw.split_once('(') {
        let remark = rest.trim_end_matches(')').trim();
        return (
            name.trim().to_string(),
            if remark.is_empty() { None } else { Some(remark.to_string()) },
        );
    }
    match raw.find(char::is_whitespace) {
        Some(idx) => {
            let name = raw[..idx].trim().to_string();
            let remark = raw[idx..].trim().to_string();
            (name, if remark.is_empty() { None } else { Some(remark) })
        }
        None => (raw, None),
    }
}

/// 从 `style="background-image:url(https://…jpg)"` 里取图片 URL。
fn style_url(style: &str) -> Option<String> {
    let re = Regex::new(r#"https?://[^)'"\s]+"#).ok()?;
    re.find(style).map(|m| m.as_str().to_string())
}

/// 分类页 URL（第 1 页 `/category/{cat}/`，第 N 页 `/category/{cat}/page/{N}/`）。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let base = base.trim_end_matches('/');
    let cat = cat.trim().trim_matches('/');
    if cat.is_empty() {
        return format!("{base}/");
    }
    if page <= 1 {
        format!("{base}/category/{cat}/")
    } else {
        format!("{base}/category/{cat}/page/{page}/")
    }
}

/// 详情页 URL：id 已是 URL 时原样，否则拼 `{base}/{id}/`。
pub fn detail_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), id.trim_matches('/'))
    }
}

/// 解析列表行（首页与分类页共用 `div.post-box-container`）。
pub fn parse_list(html: &str, _base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for box_el in doc.select(&selector("div.post-box-container")) {
        let a = match box_el.select(&selector(".post-box-title a")).next() {
            Some(a) => a,
            None => continue,
        };
        let href = a.value().attr("href").unwrap_or_default();
        let id = match id_from_href(href) {
            Some(id) => id,
            None => continue,
        };
        let raw = text_of(&a);
        let (name, remark) = split_title(&raw);
        if name.is_empty() {
            continue;
        }
        let cover = box_el
            .select(&selector(".post-box-image"))
            .next()
            .and_then(|el| el.value().attr("style"))
            .and_then(style_url);
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

/// 解析首页导航分类（`li.menu-item a`，`/category/{slug}`）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out: Vec<Category> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for a in doc.select(&selector("li.menu-item a")) {
        let title = a
            .value()
            .attr("title")
            .map(collapse)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| collapse(&text_of(&a)));
        if title.is_empty() || NON_CATEGORY_NAV.contains(&title.as_str()) {
            continue;
        }
        if seen.contains(&title) {
            continue;
        }
        let href = a.value().attr("href").unwrap_or_default();
        let Some(type_id) = category_from_href(href) else {
            continue;
        };
        seen.push(title.clone());
        out.push(Category {
            type_id,
            type_name: title,
        });
    }
    out
}

/// 详情页头部信息（不含分季请求）。
struct DetailHead {
    name: String,
    /// 原实现把括号内文本作为备注（`VideoDetail` 无对应字段），保留解析结果备查。
    _remark: String,
    cover: Option<String>,
    /// 原实现切出「类型」后未写入 `vod_*`（与当前模型字段不对应），保留解析结果备查。
    _type_name: Option<String>,
    year: Option<String>,
    area: Option<String>,
    actor: Option<String>,
    director: Option<String>,
    desc: Option<String>,
}

fn pick(pattern: &str, text: &str) -> Option<String> {
    let re = Regex::new(pattern).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_detail_head(html: &str) -> Option<DetailHead> {
    let doc = Html::parse_document(html);
    let title_el = doc.select(&selector("h1.post-title")).next()?;
    let raw_title = collapse(&text_of(&title_el));
    if raw_title.is_empty() {
        return None;
    }
    let (name, remark) = if let Some((n, rest)) = raw_title.split_once('(') {
        (
            n.trim().to_string(),
            rest.trim_end_matches(')').trim().to_string(),
        )
    } else {
        (raw_title.clone(), "全".to_string())
    };

    let cover = doc
        .select(&selector("div.post img"))
        .next()
        .and_then(|img| {
            img.value()
                .attr("data-src")
                .or_else(|| img.value().attr("src"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

    // 原实现 `.replace(" ", "").replace("<br>", "").replace("<p></p>", "")`，
    // 本实现再统一去掉空白，避免 `.` 不跨行导致正则失配。
    let abstract_html = doc
        .select(&selector("div.abstract"))
        .next()
        .map(|el| el.inner_html())
        .unwrap_or_default();
    let flat: String = abstract_html
        .replace("<br>", "")
        .replace("<p></p>", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    let type_name = pick(r"类型:(.*)制", &flat);
    let year = pick(r"年份:(.*)简", &flat);
    let area = pick(r"地区:(.*)年份", &flat);
    let actor = pick(r"演员:(.*)类", &flat);
    let director = pick(r"导演:(.*)演", &flat);
    let desc = pick(r"简介:(.*)", &flat);

    Some(DetailHead {
        name,
        _remark: remark,
        cover,
        _type_name: type_name,
        year,
        area,
        actor,
        director,
        desc,
    })
}

/// 从 `.wp-playlist-script` 的 JSON 里取 `tracks`（`caption`/`src1`/`subsrc`）。
/// `.wp-playlist-script` 里的一条 track（`srctype`/`src1`/`src3` 供各实现取用）。
struct Track {
    caption: String,
    src1: String,
    src3: String,
    subsrc: String,
    srctype: String,
}

fn parse_tracks(html: &str) -> Vec<Track> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for el in doc.select(&selector(".wp-playlist-script")) {
        let raw = el.text().collect::<Vec<_>>().join("");
        let Some(start) = raw.find('{') else { continue };
        let Some(end) = raw.rfind('}') else { continue };
        if end <= start {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&raw[start..=end]) else {
            continue;
        };
        let Some(tracks) = value.get("tracks").and_then(Value::as_array) else {
            continue;
        };
        for (idx, track) in tracks.iter().enumerate() {
            let field = |key: &str| {
                track
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("")
                    .to_string()
            };
            let src1 = field("src1");
            let src3 = field("src3");
            if src1.is_empty() && src3.is_empty() {
                continue;
            }
            let caption = {
                let c = field("caption");
                if c.is_empty() {
                    format!("第{}集", idx + 1)
                } else {
                    c
                }
            };
            out.push(Track {
                caption,
                src1,
                src3,
                subsrc: field("subsrc"),
                srctype: field("srctype"),
            });
        }
    }
    out
}

fn track_episode(base: &str, track: &Track) -> Episode {
    let api = format!("{base}/getvddr/video?id={}&type=mix", track.src1);
    let sub = if track.subsrc.is_empty() {
        format!("{base}/subddr/")
    } else {
        format!("{base}/subddr/{}", track.subsrc)
    };
    Episode {
        title: track.caption.clone(),
        url: format!("{api}|{sub}"),
    }
}

/// Ddys 版剧集：`srctype` 决定视频接口，`srctype==4` 走 CDN（`v.ddys.pro`）。
fn ddys_track_episode(base: &str, track: &Track) -> Episode {
    let api = match track.srctype.as_str() {
        "1" => format!("{base}/getvddr2/video?id={}&type=mix", track.src1),
        "4" => format!("https://v.ddys.pro{}", track.src3),
        _ => String::new(),
    };
    let sub = format!("{base}/subddr/{}", track.subsrc);
    Episode {
        title: track.caption.clone(),
        url: format!("{api}|{sub}"),
    }
}

/// 按 api 选择剧集构造。
fn episode_of(ddys: bool, base: &str, track: &Track) -> Episode {
    if ddys {
        ddys_track_episode(base, track)
    } else {
        track_episode(base, track)
    }
}

/// 分季页码：`.post-page-numbers` 的文本（数字）。
fn season_numbers(html: &str) -> Vec<u32> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for el in doc.select(&selector(".post-page-numbers")) {
        let text = collapse(&text_of(&el));
        if let Ok(n) = text.parse::<u32>() {
            if n >= 2 && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// 分类（从首页导航现抓，原实现同样在 `homeContent` 里现抓）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = fetch_text(cfg, site, &base, &format!("{base}/"))?;
    let cats = parse_categories(&html);
    if cats.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "首页导航没有解析出分类（站点改版或非 Ddrk 模板？）",
        )
        .into_core());
    }
    Ok(cats)
}

/// 列表：`cat` 为空时抓首页推荐，否则抓分类页。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = if is_ddys(&site.api) {
        ddys_list_url(&base, cat, page)
    } else {
        list_url(&base, cat, page)
    };
    let html = fetch_text(cfg, site, &base, &url)?;
    Ok(parse_list(&html, &base))
}

/// Ddys 的分类页 URL：原实现第 1 页拼 `/page/1/`（见偏差 7，本实现第 N 页统一
/// 拼 `/page/{N}/`，避免原分支把 2 页后的请求打回分类根）。
pub fn ddys_list_url(base: &str, cat: &str, page: u32) -> String {
    let base = base.trim_end_matches('/');
    let cat = cat.trim().trim_matches('/');
    if cat.is_empty() {
        return format!("{base}/");
    }
    let page = page.max(1);
    format!("{base}/category/{cat}/page/{page}/")
}

/// 详情：首屏 + `.post-page-numbers` 的每季后置请求。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let target = id.trim();
    if target.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "详情 id 为空").into_core());
    }
    let url = detail_url(&base, target);
    let html = fetch_text(cfg, site, &base, &url)?;
    let head = parse_detail_head(&html).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少 h1.post-title（站点改版或非 Ddrk 模板？）",
        )
        .into_core()
    })?;

    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    let ddys = is_ddys(&site.api);
    let first: Vec<Episode> = parse_tracks(&html)
        .iter()
        .map(|t| episode_of(ddys, &base, t))
        .collect();
    if !first.is_empty() {
        episodes.push(EpisodeGroup {
            group: PLAY_FROM_FIRST.to_string(),
            items: first,
        });
    }

    let season_id = id_from_href(target).unwrap_or_else(|| target.to_string());
    for n in season_numbers(&html) {
        let season_url = format!("{}/{}/{n}/", base.trim_end_matches('/'), season_id.trim_matches('/'));
        let Ok(season_html) = fetch_text(cfg, site, &base, &season_url) else {
            continue;
        };
        let items: Vec<Episode> = parse_tracks(&season_html)
            .iter()
            .map(|t| episode_of(ddys, &base, t))
            .collect();
        if !items.is_empty() {
            episodes.push(EpisodeGroup {
                group: format!("第{n}季"),
                items,
            });
        }
    }

    Ok(VideoDetail {
        id: season_id,
        name: head.name,
        cover: head.cover,
        desc: head.desc,
        year: head.year,
        area: head.area,
        director: head.director,
        actor: head.actor,
        episodes,
    })
}

/// 搜索：`/?s={kw}&post_type=post`，行 `article`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let enc = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let url = format!("{base}/?s={enc}&post_type=post");
    let html = fetch_text(cfg, site, &base, &url)?;
    if is_ddys(&site.api) {
        Ok(parse_ddys_search(&html, keyword))
    } else {
        Ok(parse_search(&html))
    }
}

/// Ddys 搜索页：`div#container article`，`h2` 文本须含关键词。
fn parse_ddys_search(html: &str, keyword: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for article in doc.select(&selector("div#container article")) {
        let Some(h2) = article.select(&selector("h2")).next() else {
            continue;
        };
        let name = collapse(&text_of(&h2));
        if name.is_empty() || !name.contains(keyword) {
            continue;
        }
        let Some(a) = article.select(&selector("a")).next() else {
            continue;
        };
        let href = a.value().attr("href").unwrap_or_default();
        let Some(id) = id_from_href(href) else {
            continue;
        };
        let remark = article
            .select(&selector("li.meta_categories"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id,
            name,
            cover: Some(DDYS_SEARCH_COVER.to_string()),
            remark,
        });
    }
    out
}

fn parse_search(html: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for article in doc.select(&selector("article")) {
        let Some(a) = article.select(&selector(".post-title a")).next() else {
            continue;
        };
        let href = a.value().attr("href").unwrap_or_default();
        let Some(id) = id_from_href(href) else {
            continue;
        };
        let name = collapse(&text_of(&a));
        if name.is_empty() {
            continue;
        }
        let time = article
            .select(&selector("time"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .unwrap_or_default();
        let cats = article
            .select(&selector(".cat-links"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .unwrap_or_default();
        let remark = format!("{time} {cats}").trim().to_string();
        out.push(VideoItem {
            id,
            name,
            cover: Some(SEARCH_COVER.to_string()),
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// 播放 id 拆成 `(视频接口, 字幕)`。
pub fn split_player_id(id: &str) -> Option<(String, String)> {
    let (api, sub) = id.split_once('|')?;
    let api = api.trim();
    if api.is_empty() {
        return None;
    }
    Some((api.to_string(), sub.trim().to_string()))
}

/// 播放：GET 视频接口 JSON 的 `url` 直出。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let ddys = is_ddys(&site.api);
    let (api_url, _sub) = split_player_id(id)
        .ok_or_else(|| CatvodError::new(ErrorClass::Parse, site, "播放 id 需要 `视频接口|字幕` 两段").into_core())?;
    let target = abs_url(&base, &api_url);
    let body = fetch_text(cfg, site, &base, &target)?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| CatvodError::new(ErrorClass::Parse, site, "视频接口返回的不是 JSON").into_core())?;
    let url = value.get("url").and_then(Value::as_str).unwrap_or("").trim();
    if url.is_empty() {
        return Err(CatvodError::new(ErrorClass::HostApi, site, "视频接口 JSON 里没有 url").into_core());
    }
    let header = if ddys {
        json!({ "User-Agent": DDYS_UA, "Referer": format!("{base}/") }).to_string()
    } else {
        String::new()
    };
    Ok(json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": header,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Ddrk"));
        assert!(handles("CSP_DDRK"));
        assert!(handles("csp_Ddys"));
        assert!(handles("csp_ddys"));
        assert!(!handles("csp_DiDuan"));
        assert!(!handles("csp_Douban"));
    }

    #[test]
    fn ddys_api_helpers_split_apart() {
        assert!(is_ddrk("csp_Ddrk"));
        assert!(!is_ddys("csp_Ddrk"));
        assert!(is_ddys("csp_Ddys"));
        assert!(!is_ddrk("csp_Ddys"));
    }

    #[test]
    fn ddys_list_url_uses_page_segment() {
        let base = "https://ddys.pro";
        assert_eq!(ddys_list_url(base, "oumeiju", 1), "https://ddys.pro/category/oumeiju/page/1/");
        assert_eq!(ddys_list_url(base, "oumeiju", 3), "https://ddys.pro/category/oumeiju/page/3/");
        assert_eq!(ddys_list_url(base, "", 1), "https://ddys.pro/");
    }

    #[test]
    fn ddys_tracks_use_srctype_and_src3() {
        let html = r#"<div class="wp-playlist-script">
          {"tracks":[{"caption":"第01集","srctype":"1","src1":"aaa","subsrc":"s1"},
                     {"caption":"第02集","srctype":"4","src3":"/v/bbb.m3u8","subsrc":"s2"},
                     {"caption":"第03集","srctype":"9","src1":"ccc","subsrc":"s3"}]}</div>"#;
        let tracks = parse_tracks(html);
        assert_eq!(tracks.len(), 3);
        assert_eq!(
            ddys_track_episode("https://ddys.pro", &tracks[0]).url,
            "https://ddys.pro/getvddr2/video?id=aaa&type=mix|https://ddys.pro/subddr/s1"
        );
        assert_eq!(
            ddys_track_episode("https://ddys.pro", &tracks[1]).url,
            "https://v.ddys.pro/v/bbb.m3u8|https://ddys.pro/subddr/s2"
        );
        assert_eq!(
            ddys_track_episode("https://ddys.pro", &tracks[2]).url,
            "|https://ddys.pro/subddr/s3"
        );
    }

    #[test]
    fn ddys_search_filters_by_keyword_and_uses_favicon() {
        let html = r##"<div id="container">
          <article><h2>幻视任务 全08集</h2><a href="https://ddys.pro/huanshi/"></a>
            <li class="meta_categories">欧美剧</li></article>
          <article><h2>另一部</h2><a href="https://ddys.pro/other/"></a></article>
        </div>"##;
        let items = parse_ddys_search(html, "幻视");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "huanshi");
        assert_eq!(items[0].name, "幻视任务 全08集");
        assert_eq!(items[0].cover.as_deref(), Some("https://ddys.pro/favicon-32x32.png"));
        assert_eq!(items[0].remark.as_deref(), Some("欧美剧"));
    }

    #[test]
    fn base_defaults_and_ext_override() {
        let mut s = Site::default();
        assert_eq!(base_of(&s), DEFAULT_BASE);
        s.ext = Some(json!("https://ddys.app/"));
        assert_eq!(base_of(&s), "https://ddys.app");
    }

    #[test]
    fn list_url_pages() {
        let base = "https://ddys.pro";
        assert_eq!(list_url(base, "oumeiju", 1), "https://ddys.pro/category/oumeiju/");
        assert_eq!(
            list_url(base, "oumeiju", 3),
            "https://ddys.pro/category/oumeiju/page/3/"
        );
        assert_eq!(list_url(base, "", 1), "https://ddys.pro/");
    }

    #[test]
    fn parses_list_rows_with_spanish_names() {
        let html = r#"<div class="post-box-container">
          <a href="https://ddys.pro/huanshi/" class="post-box-image"
             style="background-image: url(https://img.test/a.jpg)"></a>
          <h2 class="post-box-title"><a href="https://ddys.pro/huanshi/">幻视任务(更新至08集)</a></h2>
        </div>
        <div class="post-box-container">
          <a href="https://ddys.pro/other/" class="post-box-image"
             style="background-image:url('https://img.test/b.jpg')"></a>
          <h2 class="post-box-title"><a href="https://ddys.pro/other/">另一部电影 全30集</a></h2>
        </div>"#;
        let rows = parse_list(html, "https://ddys.pro");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "huanshi");
        assert_eq!(rows[0].name, "幻视任务");
        assert_eq!(rows[0].remark.as_deref(), Some("更新至08集"));
        assert_eq!(rows[0].cover.as_deref(), Some("https://img.test/a.jpg"));
        assert_eq!(rows[1].id, "other");
        assert_eq!(rows[1].name, "另一部电影");
        assert_eq!(rows[1].remark.as_deref(), Some("全30集"));
        assert_eq!(rows[1].cover.as_deref(), Some("https://img.test/b.jpg"));
    }

    #[test]
    fn parses_categories_and_skips_aggregate_nav() {
        let html = r#"<ul><li class="menu-item"><a title="热映中" href="/">热映中</a></li>
          <li class="menu-item"><a title="全部" href="/movie/">全部</a></li>
          <li class="menu-item"><a title="欧美剧" href="https://ddys.pro/category/oumeiju/">欧美剧</a></li>
          <li class="menu-item"><a title="欧美剧" href="https://ddys.pro/category/oumeiju/">欧美剧</a></li>
          <li class="menu-item"><a title="日剧" href="https://ddys.pro/category/riju/">日剧</a></li>
          <li class="menu-item"><a title="关于我们" href="/about/">关于我们</a></li></ul>"#;
        let cats = parse_categories(html);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "oumeiju");
        assert_eq!(cats[0].type_name, "欧美剧");
        assert_eq!(cats[1].type_id, "riju");
        assert_eq!(cats[1].type_name, "日剧");
    }

    #[test]
    fn detail_head_parses_abstract_fields() {
        let html = r#"<div class="post"><img data-src="https://img.test/cover.jpg"></div>
          <h1 class="post-title">幻视任务(更新至08集)</h1>
          <div class="abstract">类型: 剧情/科幻 制片国家/地区: 美国 年份: 2025 简介: 一部科幻剧。 演员: 甲/乙 类 导演: 丙 演</div>
          <div class="wp-playlist-script">{"tracks":[{"caption":"第01集","src1":"v1","subsrc":"s1"}]}</div>"#;
        let head = parse_detail_head(html).expect("head");
        assert_eq!(head.name, "幻视任务");
        assert_eq!(head._remark, "更新至08集");
        assert_eq!(head.cover.as_deref(), Some("https://img.test/cover.jpg"));
        assert_eq!(head._type_name.as_deref(), Some("剧情/科幻"));
        // 年份正则到「简」，简介正则从「简介:」到结尾（含后面的演员/导演文本）。
        assert_eq!(head.year.as_deref(), Some("2025"));
        assert_eq!(head.area.as_deref(), Some("美国"));
        assert!(head.desc.is_some());
    }

    #[test]
    fn parses_tracks_and_builds_episode_urls() {
        let html = r#"<div class="wp-playlist-script">
          {"tracks":[{"caption":"第01集","src1":"abc","subsrc":"xyz"},
                     {"caption":"第02集","src1":"def","subsrc":"uvw"}]}</div>"#;
        let tracks = parse_tracks(html);
        assert_eq!(tracks.len(), 2);
        let ep = track_episode("https://ddys.pro", &tracks[0]);
        assert_eq!(ep.title, "第01集");
        assert_eq!(
            ep.url,
            "https://ddys.pro/getvddr/video?id=abc&type=mix|https://ddys.pro/subddr/xyz"
        );
    }

    #[test]
    fn season_numbers_are_deduped() {
        let html = r#"<span class="post-page-numbers">1</span>
          <span class="post-page-numbers">2</span>
          <span class="post-page-numbers">2</span>
          <span class="post-page-numbers">3</span>"#;
        assert_eq!(season_numbers(html), vec![2, 3]);
    }

    #[test]
    fn search_rows_use_fixed_cover_and_joined_remark() {
        let html = r#"<article>
          <div class="post-title"><a href="https://ddys.pro/huanshi/">幻视任务</a></div>
          <time>2025-01-02</time><div class="cat-links">欧美剧</div>
        </article>"#;
        let rows = parse_search(html);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "huanshi");
        assert_eq!(rows[0].name, "幻视任务");
        assert_eq!(rows[0].cover.as_deref(), Some(SEARCH_COVER));
        assert_eq!(rows[0].remark.as_deref(), Some("2025-01-02 欧美剧"));
    }

    #[test]
    fn player_id_split() {
        assert_eq!(
            split_player_id("https://ddys.pro/getvddr/video?id=abc&type=mix|https://ddys.pro/subddr/xyz"),
            Some((
                "https://ddys.pro/getvddr/video?id=abc&type=mix".to_string(),
                "https://ddys.pro/subddr/xyz".to_string()
            ))
        );
        assert!(split_player_id("only-one-part").is_none());
    }

    #[test]
    fn detail_without_title_is_none() {
        assert!(parse_detail_head("<div class='post'><p>没有标题</p></div>").is_none());
    }
}
