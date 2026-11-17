//! `csp_Zxzj`（周星驰影视 / `www.zxzjhd.com`）原生实现。
//!
//! 来源：`_rea/decompiled/88c55af0-fan/deobf/com/github/catvod/spider/Zxzj.java`
//! （532 行、0 处 jadx skip；`a4d0f912` 394 行、`ddaf13d7` 503 行为同一协议的不同年代版本）。
//!
//! 原实现是「stui MacCMS v10」模板站：
//! * 首页导航 `ul.stui-header__menu li a` 里挑「电影/美剧/韩剧/日剧/泰剧/动漫」，
//!   原正则 `/list/(\d+).html` 取分类 id；**实测站点已改为 `/vodtype/{id}.html`**（见偏差 1）。
//! * 分类 URL = `{base}vodshow/{12 段 "-" 拼接}.html`，`[0]=分类`、`[8]=页`（与原实现一致）。
//! * 列表 `div.stui-vodlist__box` → `a.stui-vodlist__thumb` 的 `href`/`title`/`data-original`
//!   与 `span.pic-text` 备注。
//! * 详情 `{base}voddetail/{id}.html`：`div.stui-content__thumb a img[data-original]`、
//!   `div.stui-content__detail h1`、`p.data` 里的「类型/地区/年份/主演/导演/更新」、
//!   `span.detail-content`（简介）、`div.stui-vodlist__head h3` × `ul.stui-content__playlist`
//!   配对成播放线路，分集链接 `/vodplay/{vod}-{sid}-{nid}.html`。
//! * 播放页解析 `var player_aaaa={...}` JSON，直接取 `url`（实测已是可播放地址）并把
//!   `from` 作为线路名。
//! * 搜索 `{base}vodsearch/-------------.html?wd={kw}`，行结构同列表。
//!
//! 有意偏差（与 Android 实现不同，均因站点改版或桌面端无承载位）：
//! 1. 首页导航 href 改匹配 `/vodtype/(\d+)\.html`（原正则 `/list/(\d+)\.html` 实测 404）；
//! 2. 详情 URL 由原实现的 `detail/{id}.html` 改为实测的 `voddetail/{id}.html`
//!    （`/detail/1.html` 实测 404）；
//! 3. 筛选（类型/地区/年份/排序）不实现——VBox `Site` 没有筛选承载位，分类直达默认排序；
//! 4. 播放不再走 `MacPlayerConfig.player_list` 的 SN 表与 `player.js` 的解析前缀：
//!    实测 `player_aaaa.url` 已是可直接获取的播放地址，原样透传 + `Referer` 头即可；
//! 5. `filters` JSON（原 `init` 里内嵌的超大 JSON）不写入画像。

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule::join_url;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Zxzj";
pub const DEFAULT_BASE: &str = "https://www.zxzjhd.com";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.114 Safari/537.36";
const NAV_CATEGORIES: [(&str, &str); 6] = [
    ("1", "电影"),
    ("2", "美剧"),
    ("3", "韩剧"),
    ("4", "日剧"),
    ("5", "泰剧"),
    ("6", "动漫"),
];
const DETAIL_RE: &str = r#"/voddetail/(\d+)\.html"#;
const PLAY_RE: &str = r#"/vodplay/(\d+)-(\d+)-(\d+)\.html"#;
const CATEGORY_RE: &str = r#"/vodtype/(\d+)\.html"#;

/// 是否本家族可处理的 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 站点基址：`ext` 首段为 http(s) 时覆盖内置地址（便于 mock / 镜像站点）。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let first = ext.split("$$$").next().unwrap_or("").trim();
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
    h.insert(
        "Accept".into(),
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8".into(),
    );
    h.insert("Accept-Language".into(), "zh-CN,zh;q=0.9".into());
    h
}

fn selector(pattern: &str) -> Selector {
    Selector::parse(pattern).expect("zxzj: 非法 CSS 选择器")
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join("").trim().to_string()
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

/// 首页导航分类（选择器 + 改版后的 href 正则）。
pub fn parse_categories(html: &str) -> Vec<Category> {
    let doc = Html::parse_document(html);
    let nav = selector("ul.stui-header__menu li a");
    let desired = selector("a");
    let mut out: Vec<Category> = Vec::new();
    for el in doc.select(&nav) {
        let _ = &desired;
        let name = text_of(&el);
        let href = el.value().attr("href").unwrap_or_default().to_string();
        let Some(id) = regex_capture(CATEGORY_RE, &href) else {
            continue;
        };
        if !NAV_CATEGORIES.iter().any(|(_, n)| *n == name) {
            continue;
        }
        if out.iter().any(|c| c.type_id == id) {
            continue;
        }
        out.push(Category {
            type_id: id,
            type_name: name,
        });
    }
    if out.is_empty() {
        out = NAV_CATEGORIES
            .iter()
            .map(|(id, name)| Category {
                type_id: (*id).to_string(),
                type_name: (*name).to_string(),
            })
            .collect();
    }
    out
}

fn regex_capture(pattern: &str, haystack: &str) -> Option<String> {
    let re = regex::Regex::new(pattern).ok()?;
    let caps = re.captures(haystack)?;
    caps.get(1).map(|m| m.as_str().trim().to_string())
}

/// 12 段 `-` 拼接的 vodshow URL（`[0]=分类`、`[8]=页`）。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let mut segs: [String; 12] = Default::default();
    if !cat.is_empty() {
        segs[0] = cat.to_string();
    }
    if page > 1 {
        segs[8] = page.to_string();
    }
    let joined = segs.join("-");
    format!("{base}/vodshow/{joined}.html")
}

fn parse_rows(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = selector("div.stui-vodlist__box");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let a = match row.select(&selector("a.stui-vodlist__thumb")).next() {
            Some(a) => a,
            None => continue,
        };
        let href = a.value().attr("href").unwrap_or_default();
        let Some(id) = regex_capture(DETAIL_RE, href) else {
            continue;
        };
        let name = a
            .value()
            .attr("title")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                row.select(&selector("h4.title a"))
                    .next()
                    .map(|e| text_of(&e))
                    .unwrap_or_default()
            });
        let cover = a
            .value()
            .attr("data-original")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| abs_url(base, s))
            .unwrap_or_default();
        let remark = row
            .select(&selector("span.pic-text"))
            .next()
            .map(|e| text_of(&e))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id,
            name,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark,
        });
    }
    out
}

/// 解析 `p.data` 里的「标签：值」，同一行可能含多个标签（用 ` / ` 分隔）。
fn parse_meta(lines: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in lines {
        for part in line.split(" / ") {
            let Some((k, v)) = part.split_once('：') else {
                continue;
            };
            let key = k.trim();
            let value = v.trim();
            if key.is_empty() || value.is_empty() {
                continue;
            }
            if map.contains_key(key) {
                continue;
            }
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

pub fn parse_detail(base: &str, html: &str) -> Option<VideoDetail> {
    let doc = Html::parse_document(html);
    let title = doc
        .select(&selector("div.stui-content__detail h1"))
        .next()
        .map(|e| text_of(&e))
        .filter(|s| !s.is_empty())?;
    let cover = doc
        .select(&selector("div.stui-content__thumb a img"))
        .next()
        .and_then(|img| {
            img.value()
                .attr("data-original")
                .or_else(|| img.value().attr("src"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| abs_url(base, s))
        });
    let data_lines: Vec<String> = doc
        .select(&selector("div.stui-content__detail p.data"))
        .map(|e| text_of(&e))
        .collect();
    let meta = parse_meta(&data_lines);
    let desc = doc
        .select(&selector("span.detail-content"))
        .next()
        .map(|e| collapse(&text_of(&e)))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            doc.select(&selector("span.detail-sketch"))
                .next()
                .map(|e| collapse(&text_of(&e)))
                .filter(|s| !s.is_empty())
        });

    let group_names: Vec<String> = doc
        .select(&selector("div.stui-vodlist__head h3"))
        .map(|e| collapse(&text_of(&e)))
        .collect();
    let playlists: Vec<ElementRef<'_>> = doc.select(&selector("ul.stui-content__playlist")).collect();

    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    for (idx, list) in playlists.iter().enumerate() {
        let mut items: Vec<Episode> = Vec::new();
        for (n, a) in list.select(&selector("li a")).enumerate() {
            let href = a.value().attr("href").unwrap_or_default();
            if !href.contains("/vodplay/") {
                continue;
            }
            let mut name = collapse(&text_of(&a));
            if name.is_empty() {
                name = format!("第{}集", n + 1);
            }
            items.push(Episode {
                title: name,
                url: abs_url(base, href),
            });
        }
        if items.is_empty() {
            continue;
        }
        let group = group_names
            .get(idx)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("线路{}", idx + 1));
        episodes.push(EpisodeGroup { group, items });
    }
    if episodes.is_empty() {
        return None;
    }

    let detail = VideoDetail {
        id: String::new(),
        name: title,
        cover,
        desc,
        year: meta.get("年份").cloned(),
        area: meta.get("地区").cloned(),
        director: meta.get("导演").cloned(),
        actor: meta.get("主演").cloned(),
        episodes,
    };
    Some(detail)
}

/// 播放页：从 `var player_aaaa={...}` 中取 `url` 与 `from`。
pub fn parse_player(html: &str) -> Option<(String, String)> {
    let marker = "player_aaaa=";
    let start = html.find(marker)?;
    let rest = &html[start + marker.len()..];
    let end = rest.find("</script>").unwrap_or(rest.len().min(8000));
    let raw = rest[..end].trim().trim_end_matches(';').trim();
    let value: Value = serde_json::from_str(raw).ok()?;
    let url = value.get("url").and_then(Value::as_str)?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let from = value
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("zxzj")
        .to_string();
    Some((url, from))
}

fn site_error(site: &Site, detail: impl Into<String>, class: ErrorClass) -> crate::error::CoreError {
    CatvodError::new(class, site, detail.into()).into_core()
}

/// 分类列表。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let client = client_for(&cfg.http)?;
    match get_text(&client, &format!("{base}/"), &headers(&base), 2) {
        Ok(body) => Ok(parse_categories(&body)),
        // 站点首页不可达时退回内置 6 个分类，保证站点仍可用于「直接用 type_id 调分类」的场景。
        Err(_) => Ok(parse_categories("")),
    }
}

/// 分类下的视频列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = list_url(&base, cat.trim(), page);
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(&base), 2)?;
    Ok(parse_rows(&body, &base))
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let vid = id.trim();
    if vid.is_empty() {
        return Err(site_error(site, "详情 id 为空", ErrorClass::Parse));
    }
    let url = if vid.starts_with("http://") || vid.starts_with("https://") {
        vid.to_string()
    } else if vid.starts_with("/voddetail/") {
        format!("{base}{vid}")
    } else {
        let clean = vid.trim_end_matches(".html");
        format!("{base}/voddetail/{clean}.html")
    };
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(&base), 2)?;
    parse_detail(&base, &body).ok_or_else(|| {
        site_error(
            site,
            "详情页没有可播放线路（站点改版或非 Zxzj 模板？）",
            ErrorClass::Parse,
        )
    })
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let enc = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    let url = format!("{base}/vodsearch/-------------.html?wd={enc}");
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(&base), 2)?;
    Ok(parse_rows(&body, &base))
}

/// 播放：抓播放页里的 `player_aaaa.url`。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
    _vip_flags: &[String],
) -> Result<Value> {
    let base = base_of(site);
    let target = id.trim();
    if target.is_empty() {
        return Err(site_error(site, "播放 id 为空", ErrorClass::Parse));
    }
    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else if let Some(path) = regex_capture(PLAY_RE, target) {
        format!("{base}/vodplay/{path}.html")
    } else if target.starts_with('/') {
        format!("{base}{target}")
    } else {
        format!("{base}/vodplay/{target}.html")
    };
    let client = client_for(&cfg.http)?;
    let body = get_text(&client, &url, &headers(&base), 2)?;
    match parse_player(&body) {
        Some((play_url, from)) => Ok(json!({
            "url": play_url,
            "parse": 0,
            "jx": 0,
            "playUrl": "",
            "header": {
                "User-Agent": UA,
                "Referer": format!("{base}/"),
                "note": format!("from={from}"),
            },
        })),
        None => Err(site_error(
            site,
            "播放页未找到 player_aaaa.url（站点改版？）",
            ErrorClass::HostApi,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Zxzj"));
        assert!(handles("CSP_ZXZJ"));
        assert!(handles(" csp_zxzj "));
        assert!(!handles("csp_SixV"));
    }

    #[test]
    fn list_url_uses_twelve_segments() {
        let url = list_url("https://www.zxzjhd.com", "1", 1);
        assert_eq!(url, "https://www.zxzjhd.com/vodshow/1-----------.html");
        let url2 = list_url("https://www.zxzjhd.com", "2", 3);
        assert_eq!(url2, "https://www.zxzjhd.com/vodshow/2--------3---.html");
    }

    #[test]
    fn parses_categories_from_nav() {
        let html = r#"<ul class="stui-header__menu"><li><a href="/vodtype/1.html">电影</a></li>
            <li><a href="/vodtype/2.html">美剧</a></li>
            <li><a href="/vodtype/9.html">综艺</a></li></ul>"#;
        let cats = parse_categories(html);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[1].type_name, "美剧");
    }

    #[test]
    fn parses_list_rows() {
        let html = r#"<ul class="stui-vodlist">
          <li><div class="stui-vodlist__box">
            <a class="stui-vodlist__thumb" href="/voddetail/4601.html" title="铁血战士"
               data-original="https://img1.doubanio.com/a.jpg"><span class="pic-text text-right">已完结</span></a>
            <div class="stui-vodlist__detail"><h4 class="title"><a href="/voddetail/4601.html">铁血战士</a></h4></div>
          </div></li></ul>"#;
        let rows = parse_rows(html, "https://www.zxzjhd.com");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "4601");
        assert_eq!(rows[0].name, "铁血战士");
        assert_eq!(rows[0].remark.as_deref(), Some("已完结"));
        assert_eq!(rows[0].cover.as_deref(), Some("https://img1.doubanio.com/a.jpg"));
    }

    #[test]
    fn parses_detail_groups_and_meta() {
        let html = r#"<div class="stui-content__thumb"><a><img data-original="//img.test/p.jpg"></a></div>
          <div class="stui-content__detail"><h1>努力克服自卑的我们</h1>
            <p class="data">类型：其他 / 地区：韩国 / 年份：2026</p>
            <p class="data">主演：具教焕,高允贞</p><p class="data">导演：车荣勋</p></div>
          <span class="detail-content">简介正文</span>
          <div class="stui-vodlist__head"><h3>播放线路5</h3></div>
          <ul class="stui-content__playlist"><li><a href="/vodplay/4628-1-1.html">第01集</a></li></ul>"#;
        let d = parse_detail("https://www.zxzjhd.com", html).expect("detail");
        assert_eq!(d.name, "努力克服自卑的我们");
        assert_eq!(d.year.as_deref(), Some("2026"));
        assert_eq!(d.area.as_deref(), Some("韩国"));
        assert_eq!(d.actor.as_deref(), Some("具教焕,高允贞"));
        assert_eq!(d.cover.as_deref(), Some("https://img.test/p.jpg"));
        assert_eq!(d.desc.as_deref(), Some("简介正文"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "播放线路5");
        assert_eq!(d.episodes[0].items[0].url, "https://www.zxzjhd.com/vodplay/4628-1-1.html");
    }

    #[test]
    fn parses_player_json() {
        let html = r#"<script>var player_aaaa={"flag":"play","encrypt":3,"url":"https:\/\/ex.zxzja.com:9876\/player-v2\/x.m3u8","from":"line5"};</script>"#;
        let (url, from) = parse_player(html).expect("player");
        assert_eq!(url, "https://ex.zxzja.com:9876/player-v2/x.m3u8");
        assert_eq!(from, "line5");
    }

    #[test]
    fn missing_player_returns_none() {
        assert!(parse_player("<html>no player here</html>").is_none());
    }
}
