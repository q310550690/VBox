//! `csp_Auete`（Auete 影视）原生实现。
//!
//! 来源：`_rea/tmp/rec-Auete.java`（原类 497 行，由
//! `jadx --show-bad-code --single-class com.github.catvod.spider.Auete` 从
//! `69dadb9a-Yoursmile` 等 jar 变体恢复；反编译目录里的 322 行版本缺少
//! `homeContent`/`categoryContent`，恢复后完整）。
//!
//! 原实现是一个 MacCMS 风格的影视站（静态 HTML + 直链 m3u8）：
//! * 基址写死 `https://auete.art/`；请求头 `Host`/macOS Chrome 91 UA/
//!   `Accept-Language`/`Upgrade-Insecure-Requests`/`DNT`。
//! * `homeContent`：`ul[class='navbar-nav mr-auto']> li a` 里文本为
//!   电影/电视剧/综艺/动漫/其他的项，`href` 里 `/(\w+)/index.html` 的 slug 作
//!   `type_id`；filters 是写死的 `SN`（Movie/Tv/Zy/Dm/qita 各含子分类 key）。
//!   首页列表取第一个 `ul.threadlist` 的 `li`：`h2 a` 的 `title` 作名称、
//!   `a img` 的 `src` 作封面、`a button` 文本作备注、`href` 里
//!   `/(\w+/\w+/\w+)/` 作 id。
//! * `categoryContent`：`{base}//{id}`（filter 非空再拼 `/{value}`），
//!   第 1 页 `/index.html`、第 N 页 `/index{N}.html`；列表行同首页；
//!   分页来自 `ul.pagination li`（`li.active` 是当前页，「尾页」的
//!   `index{N}.html` 决定 pagecount）；`limit=20`。
//! * `detailContent`：`{base}//{id}/`；封面/标题在 `div.cover a` 的
//!   `href`/`title`（2026 版站改为 `meta[property="og:*"]`）；简介取
//!   `div.message>p` 最后一条；分类/年份/地区/影片备注/导演/主演从
//!   `div.message>p` 的 `关键词: 值` 里切；播放表 `div#player_list>h2` 的
//!   `『片名』线路名：` 与 `div#player_list>ul` 一一对应，线路名匹配写死的
//!   14 行 `yq` 表（显示名/解析前缀/是否需要解析），`li>a` 的
//!   `(/\w+/\w+/\w+/play-\d+-\d+.html)` 作播放 id，`名称$路径` 用 `#`
//!   连接、多线路用 `$$$` 连接。
//! * `playerContent`：GET `{base}/` + 播放 id（即播放页路径），取
//!   `div>script` 文本按 `var` 切开，找含 `now` 的语句，值是
//!   `base64decode("…")` 时先解 base64；`http` 开头直出（`parse:0`），
//!   否则含 `m3u8` 时补 `haozhansou` 缓存前缀。
//! * `searchContent`：`{base}//auete1so.php?searchword={kw}`，行
//!   `div.card-body>ul>li.media`，名称/备注取 `.subject a span` 与
//!   `.text-muted span`，再逐条抓详情 `div.cover a` 的 `href` 当封面。
//!
//! 有意偏差（均因桌面端无 Android 宿主能力或上游改版）：
//! 1. 原基址 `auete.art` 已 ENOTFOUND、配置里的 `auete.pro` 连接超时；
//!    现役站是 `https://www.auete.com/`（2026 版），本实现默认用它，
//!    并允许 `ext` 首段 http(s) 覆盖基址（原实现不读 ext）。
//! 2. 2026 版列表行没有 `a button`，备注移到 `span.hdtag`；实现先取
//!    `a button`、再回退 `span.hdtag`。
//! 3. 2026 版详情页以 `meta[property="og:*"]` 和 `span.detail-label` 为主，
//!    原 `div.message>p` 的 `关键词: 值` 仅作兜底。
//! 4. 原实现只收录 `yq` 表里已知线路，实测现站多了「云播J线」；实现保留
//!    未知线路（已知线路按表顺序在前，未知线路按出现顺序在后）。
//! 5. 原 `playerContent` 在既非 http 也不含 m3u8 时返回空对象；实现返回
//!    `ErrorClass::Parse`，避免把「无地址」当成功。
//! 6. 原 `searchContent` 会为每条结果再抓一次详情取封面（N+1 请求）；
//!    实现只用搜索页里的 `img`，缺失时封面为空。现站搜索入口
//!    `/auete4so.php` 返回「系统安全验证」WAF 页，搜索仅 mock 验收。
//! 7. filter（`SN` 子分类）未接入：VBox 家族分派目前不把筛选值传进来，
//!    分类 URL 只按 cate 生成，与原实现在无筛选时的行为一致。

use std::collections::HashMap;

use base64::Engine as _;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Auete";
/// 原实现写死 `https://auete.art/`（现已 ENOTFOUND）；现役站是 auete.com。
pub const DEFAULT_BASE: &str = "https://www.auete.com";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/91.0.4472.106 Safari/537.36";
const ACCEPT_LANGUAGE: &str = "zh-CN,zh;q=0.9,en;q=0.8";

/// 原实现里 `now` 是相对路径时补的 m3u8 缓存前缀。
const M3U8_CACHE_PREFIX: &str =
    "https://datas-s8pwfqdu9yystn90fb----------------cache.haozhansou.com/";

/// 首页导航里算分类的文本（原实现按文本等值过滤）。
const NAV_CATEGORIES: [&str; 5] = ["电影", "电视剧", "综艺", "动漫", "其他"];

/// 原实现的 `yq` 线路表：`(key, 显示名, 解析前缀)`；`or` 全为 999，
/// 原 `TreeMap` 比较器在相等时恒返回 1，因此实际近似「出现顺序」。
/// 这里只用来给已知线路排序，解析前缀在现站已由播放页直接给出。
const LINE_TABLE: [(&str, &str, &str); 14] = [
    ("byun", "云播B线", ""),
    ("yyun", "云播Y线", ""),
    ("cyun", "云播C线", ""),
    ("dbm3u8", "云播D线", ""),
    ("i8i", "云播E线", ""),
    ("m3u8hd", "云播F线", "https://auete.org/api/?url="),
    ("languang", "云播G线", "https://auete.org/api/?url="),
    ("hyun", "云播H线", "https://auete.org/api/?url="),
    ("kyun", "云播K线", "https://auete.org/api/?url="),
    ("lyun", "云播L线", "https://auete.org/api/?url="),
    ("bpyueyu", "云播粤语", ""),
    ("bpguoyu", "云播国语", ""),
    ("myun", "云播M线", ""),
    ("dp", "Dplayer", ""),
];

/// 是否本家族可处理的 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 基址：原实现写死 `https://auete.art/`；允许 ext 首段 http(s) 覆盖，
/// 便于镜像站与 mock（dc 配置的 ext 就是旧基址）。
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
    h.insert("Accept-Language".into(), ACCEPT_LANGUAGE.into());
    h.insert("Referer".into(), format!("{base}/"));
    h
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    Ok(get_text(&client, url, &headers(base), 2)?)
}

fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("auete: 非法 CSS 选择器")
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join("")
}

fn collapse(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value()
        .attr(name)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// 列表/详情 id：`/(\w+/\w+/\w+)/`（如 `/Dm/donghua/baxian_/`）。
fn id_from_href(href: &str) -> Option<String> {
    let re = Regex::new(r"/([A-Za-z0-9_]+/[A-Za-z0-9_]+/[A-Za-z0-9_]+)").ok()?;
    re.captures(href)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// 导航 slug：`/Movie/index.html` → `Movie`。
fn category_from_href(href: &str) -> Option<String> {
    let re = Regex::new(r"/([A-Za-z]+)/index\.html").ok()?;
    re.captures(href)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// 分页里的页码：`/Dm/index3.html` → 3（`index.html` 与 `index0.html` 不计）。
fn page_from_href(href: &str) -> Option<u32> {
    let re = Regex::new(r"index(\d+)\.html").ok()?;
    let n: u32 = re.captures(href)?.get(1)?.as_str().parse().ok()?;
    if n == 0 {
        None
    } else {
        Some(n)
    }
}

/// 列表 URL：`cat` 为空就是首页；第 1 页 `/{cat}/index.html`，
/// 第 N 页 `/{cat}/index{N}.html`（2026 版站就是这套，原实现的分支被
/// jadx 重排成相反方向，见文件头偏差说明）。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let cat = cat.trim().trim_matches('/');
    if cat.is_empty() {
        return format!("{base}/");
    }
    let page = page.max(1);
    if page == 1 {
        format!("{base}/{cat}/index.html")
    } else {
        format!("{base}/{cat}/index{page}.html")
    }
}

pub fn detail_url(base: &str, id: &str) -> String {
    let id = id.trim().trim_matches('/');
    format!("{base}/{id}/")
}

fn play_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.starts_with('/') {
        format!("{base}{id}")
    } else {
        format!("{base}/{id}")
    }
}

fn clean_title(input: &str) -> String {
    input
        .replace("迅雷下载", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 首页导航 → 分类（原实现在 `homeContent` 里现抓）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let mut out: Vec<Category> = Vec::new();
    for a in doc.select(&selector("ul.navbar-nav.mr-auto li a")) {
        let name = collapse(&text_of(&a));
        if !NAV_CATEGORIES.contains(&name.as_str()) {
            continue;
        }
        let href = a.value().attr("href").unwrap_or_default();
        let Some(slug) = category_from_href(href) else {
            continue;
        };
        if out.iter().any(|c| c.type_id == slug) {
            continue;
        }
        out.push(Category {
            type_id: slug,
            type_name: name,
        });
    }
    out
}

/// 列表行：`ul.threadlist li`。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(&selector("ul.threadlist li")) {
        let Some(a) = li.select(&selector("h2 a")).next() else {
            continue;
        };
        let name = attr(&a, "title").unwrap_or_else(|| collapse(&text_of(&a)));
        let name = clean_title(&name);
        if name.is_empty() {
            continue;
        }
        let href = a.value().attr("href").unwrap_or_default();
        let Some(id) = id_from_href(href) else {
            continue;
        };
        let cover = li
            .select(&selector("a img"))
            .next()
            .and_then(|img| attr(&img, "src").or_else(|| attr(&img, "data-src")))
            .map(|s| abs_url(base, &s))
            .filter(|s| !s.is_empty());
        let remark = li
            .select(&selector("a button"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                li.select(&selector("span.hdtag"))
                    .next()
                    .map(|e| collapse(&text_of(&e)))
                    .filter(|s| !s.is_empty())
            });
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

/// 分页信息：`(当前页, 总页数)`；无分页时两者都回退到请求页。
pub fn page_info(html: &str, page: u32) -> (u32, u32) {
    let doc = Html::parse_document(html);
    let page = page.max(1);
    let mut current = page;
    let mut pagecount = page;
    for li in doc.select(&selector("nav ul.pagination li")) {
        let active = li.value().attr("class").unwrap_or_default().contains("active");
        let Some(a) = li.select(&selector("a.page-link")).next() else {
            continue;
        };
        let href = a.value().attr("href").unwrap_or_default();
        if let Some(n) = page_from_href(href) {
            pagecount = pagecount.max(n);
        }
        if active {
            if let Ok(n) = collapse(&text_of(&a)).parse::<u32>() {
                if n > 0 {
                    current = n;
                }
            } else if let Some(n) = page_from_href(href) {
                current = n;
            }
        }
    }
    (current, pagecount.max(current))
}

/// 详情页解析结果。
#[derive(Debug, Default)]
pub struct DetailData {
    pub name: String,
    pub cover: Option<String>,
    pub desc: Option<String>,
    pub year: Option<String>,
    pub area: Option<String>,
    pub director: Option<String>,
    pub actor: Option<String>,
    /// `(线路名, 剧集列表)`，已按已知线路优先排序。
    pub cards: Vec<(String, Vec<Episode>)>,
}

fn og(doc: &Html, property: &str) -> Option<String> {
    let sel = Selector::parse(&format!("meta[property=\"{property}\"]")).ok()?;
    doc.select(&sel)
        .next()
        .and_then(|m| m.value().attr("content"))
        .map(|s| collapse(s))
        .filter(|s| !s.is_empty())
}

fn line_rank(name: &str) -> Option<usize> {
    LINE_TABLE.iter().position(|(_, sh, _)| *sh == name)
}

/// 播放卡头部文本 `『片名』云播J线` → `云播J线`。
fn line_name(header: &str) -> String {
    let text = collapse(header);
    let tail = match text.rsplit_once('』') {
        Some((_, rest)) => rest,
        None => text.as_str(),
    };
    tail.trim()
        .trim_end_matches(|c| c == '：' || c == ':')
        .trim()
        .to_string()
}

/// 详情页：og meta 优先，`span.detail-label` 次之，`div.message>p` 的
/// `关键词: 值` 兜底；播放卡 `div.play-card`。
pub fn parse_detail(html: &str, base: &str) -> Option<DetailData> {
    let doc = Html::parse_document(html);

    let mut name = og(&doc, "og:title").unwrap_or_default();
    let cover = og(&doc, "og:image").map(|s| abs_url(base, &s));
    let mut desc = og(&doc, "og:description");
    let mut year = None;
    let mut area = og(&doc, "og:video:area");
    let mut director = og(&doc, "og:video:director");
    let mut actor = og(&doc, "og:video:actor");

    // `span.detail-label` + `<b>值</b>`。
    for p in doc.select(&selector("div.message p")) {
        let Some(label_el) = p.select(&selector("span.detail-label")).next() else {
            continue;
        };
        let label = collapse(&text_of(&label_el))
            .trim_end_matches(|c| c == '：' || c == ':')
            .trim()
            .to_string();
        let value = p
            .select(&selector("b"))
            .next()
            .map(|b| collapse(&text_of(&b)))
            .unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        if label.contains("片名") && name.is_empty() {
            name = value;
        } else if label.contains("导演") && director.is_none() {
            director = Some(value);
        } else if label.contains("主演") && actor.is_none() {
            actor = Some(value);
        } else if label.contains("地区") && area.is_none() {
            area = Some(value);
        } else if label.contains("上映年份") && year.is_none() {
            year = Some(value);
        }
    }

    // 旧版 `div.message>p` 的 `关键词: 值`。
    let mut message_texts: Vec<String> = Vec::new();
    for p in doc.select(&selector("div.message p")) {
        let text = collapse(&text_of(&p));
        if text.is_empty() {
            continue;
        }
        message_texts.push(text.clone());
        let Some((key, value)) = text.split_once(": ") else {
            continue;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        let key = key.trim();
        if key.contains("分类") && name.is_empty() {
            // 名称这里拿不到，交给 `og:title`/标签；分类字段模型里没有。
        } else if key.contains("年份") && year.is_none() {
            year = Some(value);
        } else if key.contains("地区") && area.is_none() {
            area = Some(value);
        } else if key.contains("导演") && director.is_none() {
            director = Some(value);
        } else if key.contains("主演") && actor.is_none() {
            actor = Some(value);
        }
    }
    if desc.is_none() {
        desc = message_texts.last().cloned();
    }

    let mut cards: Vec<(String, Vec<Episode>)> = Vec::new();
    for card in doc.select(&selector("div.play-card")) {
        let header = card
            .select(&selector("div.card-header b"))
            .next()
            .map(|b| collapse(&text_of(&b)))
            .or_else(|| {
                card.select(&selector("div.card-header"))
                    .next()
                    .map(|h| collapse(&text_of(&h)))
            })
            .unwrap_or_default();
        let group = {
            let n = line_name(&header);
            if n.is_empty() {
                format!("线路{}", cards.len() + 1)
            } else {
                n
            }
        };
        let mut items = Vec::new();
        for a in card.select(&selector("ul.episode-list li a")) {
            let href = a.value().attr("href").unwrap_or_default().trim();
            if href.is_empty() {
                continue;
            }
            let title = attr(&a, "title").unwrap_or_else(|| collapse(&text_of(&a)));
            items.push(Episode {
                title: if title.is_empty() {
                    "正片".to_string()
                } else {
                    title
                },
                url: href.to_string(),
            });
        }
        if !items.is_empty() {
            cards.push((group, items));
        }
    }
    cards.sort_by_key(|(name, _)| (line_rank(name).is_none(), line_rank(name).unwrap_or(usize::MAX)));

    if name.is_empty() {
        return None;
    }
    Some(DetailData {
        name: clean_title(&name),
        cover,
        desc,
        year,
        area,
        director,
        actor,
        cards,
    })
}

/// 播放页 `var now = …`：`base64decode("…")` / 直接字面量 / 相对 m3u8。
pub fn parse_now(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let re = Regex::new(r"var\s+now\s*=\s*([^;]+);").ok()?;
    for script in doc.select(&selector("script")) {
        let text = script.text().collect::<Vec<_>>().join("");
        let Some(caps) = re.captures(&text) else {
            continue;
        };
        let raw = caps.get(1)?.as_str().trim();
        let value = if let Some(open) = raw.find('(') {
            if raw[..open].trim().ends_with("base64decode") {
                let close = raw.rfind(')')?;
                let inner = raw[open + 1..close].trim().trim_matches('"').trim_matches('\'');
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(inner.trim())
                    .ok()?;
                String::from_utf8(bytes).ok()?
            } else {
                strip_literal(raw)
            }
        } else {
            strip_literal(raw)
        };
        let value = value.trim().to_string();
        if value.starts_with("http://") || value.starts_with("https://") {
            return Some(value);
        }
        if value.contains("m3u8") {
            return Some(format!("{M3U8_CACHE_PREFIX}{value}"));
        }
    }
    None
}

fn strip_literal(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

/// 搜索行：`div.card-body>ul>li.media`。
pub fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for li in doc.select(&selector("div.card-body ul li.media")) {
        let Some(a) = li
            .select(&selector("div.media-body div.subject a"))
            .next()
            .or_else(|| li.select(&selector("div.media-body a")).next())
        else {
            continue;
        };
        let href = a.value().attr("href").unwrap_or_default();
        let Some(id) = id_from_href(href) else {
            continue;
        };
        let name = li
            .select(&selector("div.media-body div.subject a span"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .or_else(|| Some(collapse(&text_of(&a))))
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let remark = li
            .select(&selector("div.media-body div.text-muted span"))
            .next()
            .map(|e| collapse(&text_of(&e)))
            .filter(|s| !s.is_empty());
        let cover = li
            .select(&selector("img"))
            .next()
            .and_then(|img| attr(&img, "src").or_else(|| attr(&img, "data-src")))
            .map(|s| abs_url(base, &s))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id,
            name,
            cover,
            remark,
        });
    }
    out
}

/// 分类（从首页导航现抓，原实现同样如此）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let html = fetch_text(cfg, &base, &format!("{base}/"))?;
    let cats = parse_categories(&html);
    if cats.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "首页导航没有解析出分类（站点改版或非 Auete 模板？）",
        )
        .into_core());
    }
    Ok(cats)
}

/// 列表：`cat` 为空抓首页，否则抓分类页。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cat, page);
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_list(&html, &base))
}

/// 详情：首屏 + 播放卡。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let target = id.trim();
    if target.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "详情 id 为空").into_core());
    }
    let url = detail_url(&base, target);
    let html = fetch_text(cfg, &base, &url)?;
    let head = parse_detail(&html, &base).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少标题（站点改版或非 Auete 模板？）",
        )
        .into_core()
    })?;
    let episodes = head
        .cards
        .into_iter()
        .map(|(group, items)| EpisodeGroup { group, items })
        .collect();
    Ok(VideoDetail {
        id: target.trim_matches('/').to_string(),
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

/// 搜索：现站入口 `/auete4so.php?searchword=`（原实现是 `auete1so.php`）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let enc = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let url = format!("{base}/auete4so.php?searchword={enc}");
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_search(&html, &base))
}

/// 播放：播放页里 `now` 解码后直出（`parse:0`）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let target = play_url(&base, id);
    let html = fetch_text(cfg, &base, &target)?;
    let url = parse_now(&html).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "播放页没有解析出 now 播放地址（站点改版？）",
        )
        .into_core()
    })?;
    Ok(json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": "",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAV: &str = r#"<ul class="navbar-nav mr-auto">
        <li class="nav-item home"><a class="nav-link" href="/">首页</a></li>
        <li class="nav-item"><a class="nav-link" href="/Movie/index.html">电影</a></li>
        <li class="nav-item"><a class="nav-link" href="/Tv/index.html">电视剧</a></li>
        <li class="nav-item"><a class="nav-link" href="/Zy/index.html">综艺</a></li>
        <li class="nav-item"><a class="nav-link" href="/Dm/index.html">动漫</a></li>
        <li class="nav-item"><a class="nav-link" href="/qita/index.html">其他</a></li>
        <li class="nav-item"><a class="nav-link" href="/go/auete.html">地址发布页</a></li>
    </ul>"#;

    const LIST: &str = r#"<ul class="threadlist">
        <li data-href="/Dm/donghua/baxian_/" data-tid="43357">
            <a href="/Dm/donghua/baxian_/" class="pic"><img src="https://img.test/1.jpg" alt="八仙！
"><span class="hdtag">HD国语</span></a>
            <h2><a href="/Dm/donghua/baxian_/" title="八仙！">八仙！</a></h2>
        </li>
        <li>
            <a href="/Movie/xjp/abc_/" class="pic"><img src="/upload/2.jpg" alt="测试"><button
                class="btn">更新至10集</button></a>
            <h2><a href="/Movie/xjp/abc_/" title="测试 迅雷下载">测试 迅雷下载</a></h2>
        </li>
    </ul>"#;

    const PAGER: &str = r#"<nav class="mb-4"><ul class="pagination">
        <li class="page-item"><a href="/Dm/index.html" class="page-link">首页</a></li>
        <li class="page-item"><a href="/Dm/index0.html" class="page-link">上一页</a></li>
        <li class="page-item active"><a href="/Dm/index.html" class="page-link">1</a></li>
        <li class="page-item"><a href="/Dm/index2.html" class="page-link">2</a></li>
        <li class="page-item"><a href="/Dm/index5.html" class="page-link">尾页</a></li>
    </ul></nav>"#;

    const DETAIL: &str = r#"<html><head>
        <meta property="og:title" content="八仙！">
        <meta property="og:image" content="https://img.test/cover.jpg">
        <meta property="og:description" content="八个仙人过海。">
        <meta property="og:video:area" content="大陆">
        <meta property="og:video:director" content="某导演">
        <meta property="og:video:actor" content="甲,乙">
    </head><body>
        <div class="message break-all">
            <p><span class="detail-label">◎影片片名：</span><b>八仙！</b></p>
            <p><span class="detail-label">◎上映年份：</span><b>2026-07-18</b></p>
        </div>
        <div class="card mb-3 play-card" id="player_list">
            <div class="card-header"><span><b>『八仙！』云播J线</b></span></div>
            <div class="card-body"><ul class="episode-list">
                <li id="00"><a title="正片国语" href="/Dm/donghua/baxian_/play-0-0.html">正片国语</a></li>
            </ul></div>
        </div>
        <div class="card mb-3 play-card" id="player_list">
            <div class="card-header"><span><b>『八仙！』云播F线</b></span></div>
            <div class="card-body"><ul class="episode-list">
                <li id="10"><a title="正片国语" href="/Dm/donghua/baxian_/play-1-0.html">正片国语</a></li>
                <li id="11"><a title="第2集" href="/Dm/donghua/baxian_/play-1-1.html">第2集</a></li>
            </ul></div>
        </div>
    </body></html>"#;

    const PLAY: &str = r#"<html><head><script>var seatype="play";</script></head><body>
        <script>var vid="43357";var vfrom="0";var vpart="0"; var now=base64decode("aHR0cHM6Ly9oZC5rdWt0eHUuY29tL3BsYXkvYjJrZ3laamQvaW5kZXgubTN1OA==");var pn="kvd";</script>
    </body></html>"#;

    const SEARCH: &str = r#"<div class="card-body"><ul>
        <li class="media"><img src="https://img.test/s1.jpg">
            <div class="media-body"><div class="subject"><a href="/Dm/donghua/baxian_/"><span>八仙！</span></a></div>
            <div class="d-flex"><div class="text-muted"><span>2026 · 动漫</span></div></div></div>
        </li>
    </ul></div>"#;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Auete"));
        assert!(handles("CSP_AUETE"));
        assert!(!handles("csp_Ddrk"));
        assert!(!handles("csp_Auete2"));
    }

    #[test]
    fn base_defaults_and_ext_override() {
        let mut s = Site::default();
        assert_eq!(base_of(&s), DEFAULT_BASE);
        s.ext = Some(json!("https://auete.pro/"));
        assert_eq!(base_of(&s), "https://auete.pro");
    }

    #[test]
    fn list_url_pages() {
        let base = DEFAULT_BASE;
        assert_eq!(list_url(base, "", 1), "https://www.auete.com/");
        assert_eq!(list_url(base, "Dm", 1), "https://www.auete.com/Dm/index.html");
        assert_eq!(
            list_url(base, "/Dm/", 3),
            "https://www.auete.com/Dm/index3.html"
        );
    }

    #[test]
    fn categories_come_from_nav_and_skip_non_sections() {
        let cats = parse_categories(NAV);
        let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
        assert_eq!(names, vec!["电影", "电视剧", "综艺", "动漫", "其他"]);
        let ids: Vec<&str> = cats.iter().map(|c| c.type_id.as_str()).collect();
        assert_eq!(ids, vec!["Movie", "Tv", "Zy", "Dm", "qita"]);
    }

    #[test]
    fn list_rows_use_title_hdtag_and_href_id() {
        let items = parse_list(LIST, DEFAULT_BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "Dm/donghua/baxian_");
        assert_eq!(items[0].name, "八仙！");
        assert_eq!(
            items[0].cover.as_deref(),
            Some("https://img.test/1.jpg")
        );
        assert_eq!(items[0].remark.as_deref(), Some("HD国语"));
        assert_eq!(items[1].name, "测试");
        assert_eq!(
            items[1].cover.as_deref(),
            Some("https://www.auete.com/upload/2.jpg")
        );
        assert_eq!(items[1].remark.as_deref(), Some("更新至10集"));
    }

    #[test]
    fn page_info_reads_active_and_last_page() {
        let (current, count) = page_info(PAGER, 1);
        assert_eq!(current, 1);
        assert_eq!(count, 5);
        let (current, count) = page_info("", 4);
        assert_eq!(current, 4);
        assert_eq!(count, 4);
    }

    #[test]
    fn detail_reads_og_labels_and_play_cards() {
        let head = parse_detail(DETAIL, DEFAULT_BASE).expect("detail");
        assert_eq!(head.name, "八仙！");
        assert_eq!(head.cover.as_deref(), Some("https://img.test/cover.jpg"));
        assert_eq!(head.desc.as_deref(), Some("八个仙人过海。"));
        assert_eq!(head.area.as_deref(), Some("大陆"));
        assert_eq!(head.director.as_deref(), Some("某导演"));
        assert_eq!(head.actor.as_deref(), Some("甲,乙"));
        assert_eq!(head.year.as_deref(), Some("2026-07-18"));
        assert_eq!(head.cards.len(), 2);
        assert_eq!(head.cards[0].0, "云播F线");
        assert_eq!(head.cards[0].1.len(), 2);
        assert_eq!(head.cards[1].0, "云播J线");
        assert_eq!(
            head.cards[1].1[0].url,
            "/Dm/donghua/baxian_/play-0-0.html"
        );
    }

    #[test]
    fn player_reads_base64_now() {
        assert_eq!(
            parse_now(PLAY).as_deref(),
            Some("https://hd.kuktxu.com/play/b2kgyZjd/index.m3u8")
        );
        assert_eq!(parse_now("<html></html>"), None);
    }

    #[test]
    fn player_prefixes_relative_m3u8() {
        let html = r#"<script>var now="abc/index.m3u8";</script>"#;
        assert_eq!(
            parse_now(html).as_deref(),
            Some("https://datas-s8pwfqdu9yystn90fb----------------cache.haozhansou.com/abc/index.m3u8")
        );
    }

    #[test]
    fn search_rows_parse_name_remark_cover() {
        let items = parse_search(SEARCH, DEFAULT_BASE);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "Dm/donghua/baxian_");
        assert_eq!(items[0].name, "八仙！");
        assert_eq!(items[0].remark.as_deref(), Some("2026 · 动漫"));
        assert_eq!(items[0].cover.as_deref(), Some("https://img.test/s1.jpg"));
    }
}
