//! `csp_Anime1` 原生实现（Anime1.me 站点族，第四个垂直切片）。
//!
//! 语义来自静态反编译：`_rea/decompiled/9f391d88-candymuj/deobf/com/github/catvod/spider/Anime1.java`
//! （426 行完整版；`69dadb9a-Yoursmile` 等 jar 内是同族变体，常量经 `merge.UUA.d()` XOR 混淆，
//! 已用 `_rea/tools/decode-uua.mjs` 全部解出）。该类 `extends Spider`，无宿主私有 API 依赖。
//!
//! 协议由三段组成：
//! 1. **索引**：`GET https://d1zquzjgwo9yb.cloudfront.net/?_=<ms>`，
//!    返回 JSON 数组的数组 `[link, name, hit, year, season, team]`。
//!    原实现把整表缓存在实例字段 `N` 里，`homeContent`/`categoryContent`/`searchContent` 共用；
//!    本移植是函数式入口（`list_categories` / `list_videos` / `search` 分次调用），
//!    因此每次调用各自拉一次索引（站点侧请求量等价于「每次进分类/搜索」）。
//! 2. **分类**：固定 8 个——`0`=最近更新、`1..6`=最近 6 个年份（当年递减）、`7`=更早；
//!    `categoryContent` 在本地按 `year` 过滤，**不分页**（原实现忽略 page 参数）。
//! 3. **详情**：`GET https://anime1.me/?cat={year}`（WordPress 年份归档），遍历
//!    `article` → `h2 > a` 标题（`[12]` 形式的分集标签）+ `div.vjscontainer video[data-apireq]`
//!    （每个视频一个 `data-apireq`，即播放接口的 `d` 参数），并按 `div.nav-previous a`
//!    翻页直到没有下一页；播放线路单组 `Anime1`，`#{集名}` / `$$${线路}` 语义对齐 VBox 模型。
//! 4. **播放**：`POST https://v.anime1.me/api`（表单 `d=<data-apireq>`），响应 JSON
//!    `s[0].src`（协议相对 URL，补 `https:`），并从 `Set-Cookie` 取前 3 个 cookie
//!    拼进播放头——CDN 校验这组 cookie。
//!
//! 与原实现的有意偏差（均为健壮性，不改变协议语义）：
//! - 索引条目不足 100 条时「最近更新」返回实际条数（原实现会数组越界后被 catch 成空串）；
//! - 标题没有 `[..]` 分集标签时用整段标题当集名（原实现会抛 `StringIndexOutOfBounds` 后返回空）；
//! - 年份带 `/`（如 `2024/10`）时取前 4 位数字（原实现 `substring(5)` 会拿到月份，疑似反编译伪影）。
//!
//! 上游现状（2026-11-11 实测）：索引域名 `d1zquzjgwo9yb.cloudfront.net` 已 **DNS 失效**
//! （`ENOTFOUND`），`https://anime1.me/?cat=2024` 返回 404（站点改版）；因此本族目前只能
//! 走 mock 验收（L-B），真站链路待上游恢复。`ext` 给出 http(s) 基址时，
//! 索引/页面/播放三个端点的**路径**分别按 `{base}/index`、`{base}/?cat=`、`{base}/api`
//! 解析（mock 与镜像站用；这是移植新增的测试通道，原实现不支持 ext）。

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use percent_encoding::percent_decode_str;
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Anime1";

const INDEX_URL: &str = "https://d1zquzjgwo9yb.cloudfront.net/?_=";
const SITE_BASE: &str = "https://anime1.me";
const PLAY_API: &str = "https://v.anime1.me/api";
const COVER: &str = "https://sta.anicdn.com/playerImg/8.jpg";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.134 YaBrowser/22.7.1.755 (beta) Yowser/2.5 Safari/537.36";
const MAX_PAGES: usize = 30;

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

struct Endpoints {
    index: String,
    site: String,
    api: String,
}

fn endpoints(site: &Site) -> Endpoints {
    let ext = site.ext_string();
    let trimmed = ext.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Endpoints {
            index: format!("{trimmed}/index?_="),
            site: trimmed.to_string(),
            api: format!("{trimmed}/api"),
        }
    } else {
        Endpoints {
            index: INDEX_URL.to_string(),
            site: SITE_BASE.to_string(),
            api: PLAY_API.to_string(),
        }
    }
}

macro_rules! selector {
    ($fn_name:ident, $css:expr) => {
        fn $fn_name() -> &'static Selector {
            static SEL: OnceLock<Selector> = OnceLock::new();
            SEL.get_or_init(|| Selector::parse($css).expect(concat!("选择器: ", $css)))
        }
    };
}

selector!(nav_sel, "div.nav-previous a");
selector!(article_sel, "article");
selector!(h2a_sel, "h2 > a");
selector!(h2_sel, "h2");
selector!(video_sel, "div.vjscontainer video");

fn label_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r".*[a-zA-Z\u{4e00}-\u{9fa5}]+.*").expect("集名正则"))
}

fn current_year() -> i32 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let z = (secs / 86400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    year as i32
}

/// 从 `year` 字段取年份数字（`2024` / `2024/10` 都取 2024）。
fn year_of(raw: &str) -> Option<i32> {
    if let Ok(y) = raw.trim().parse::<i32>() {
        return Some(y);
    }
    if raw.contains('/') {
        let head: String = raw.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
        if head.len() == 4 {
            return head.parse().ok();
        }
    }
    None
}

fn err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/* ============================================================
 * 索引
 * ============================================================ */

#[derive(Debug, Default, Clone)]
struct IndexItem {
    link: String,
    name: String,
    hit: String,
    year: String,
    season: String,
    team: String,
}

fn index_headers(host: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("Authority".to_string(), host.to_string());
    h.insert(
        "Accept".to_string(),
        "application/json, text/javascript, */*; q=0.01".to_string(),
    );
    h.insert(
        "Accept-Language".to_string(),
        "zh-TW,zh;q=0.9,en-US;q=0.8,en;q=0.7,ja;q=0.6".to_string(),
    );
    h.insert("Origin".to_string(), SITE_BASE.to_string());
    h.insert("Referer".to_string(), format!("{SITE_BASE}/"));
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Sec-Fetch-Dest".to_string(), "empty".to_string());
    h.insert("Sec-Fetch-Mode".to_string(), "cors".to_string());
    h.insert("Sec-Fetch-Site".to_string(), "cross-site".to_string());
    h.insert("sec-ch-ua-mobile".to_string(), "?0".to_string());
    h.insert("sec-ch-ua-platform".to_string(), "\"Windows\"".to_string());
    h
}

fn html_headers(site_base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("Accept".to_string(), "text/html,application/xhtml+xml".to_string());
    h.insert(
        "Accept-Language".to_string(),
        "zh-TW,zh;q=0.9,en-US;q=0.8,en;q=0.7,ja;q=0.6".to_string(),
    );
    h.insert("Referer".to_string(), format!("{site_base}/"));
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

fn fetch_items(cfg: &TvConfig, site: &Site, eps: &Endpoints) -> Result<Vec<IndexItem>> {
    let client = crate::http::client_for(&cfg.http)?;
    let host = eps
        .index
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let url = format!("{}{}", eps.index, ms);
    let text = crate::http::get_text(&client, &url, &index_headers(&host), cfg.http.retry)?;
    parse_index(site, &text)
}

fn parse_index(site: &Site, text: &str) -> Result<Vec<IndexItem>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| err(site, format!("索引 JSON 解析失败: {e}")))?;
    let rows = value
        .as_array()
        .ok_or_else(|| err(site, "索引 JSON 不是数组"))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let cols = match row.as_array() {
            Some(c) if c.len() >= 6 => c,
            _ => continue,
        };
        let field = |i: usize| cols[i].as_str().map(str::to_string).unwrap_or_else(|| {
            match &cols[i] {
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            }
        });
        out.push(IndexItem {
            link: field(0),
            name: field(1),
            hit: field(2),
            year: field(3),
            season: field(4),
            team: field(5),
        });
    }
    Ok(out)
}

/* ============================================================
 * 分类
 * ============================================================ */

/// 固定 8 个分类：最近更新 + 最近 6 个年份 + 更早。
pub fn categories() -> Vec<Category> {
    let mut out = vec![Category {
        type_id: "0".to_string(),
        type_name: "最近更新".to_string(),
    }];
    let mut year = current_year() + 1;
    for id in 1..7 {
        year -= 1;
        out.push(Category {
            type_id: id.to_string(),
            type_name: year.to_string(),
        });
    }
    out.push(Category {
        type_id: "7".to_string(),
        type_name: "更早".to_string(),
    });
    out
}

/// 按分类 id 过滤索引。语义同 `categoryContent`（`page` 原实现忽略）。
fn filter_by_category<'a>(items: &'a [IndexItem], cat: &str) -> Vec<&'a IndexItem> {
    let id: i32 = match cat.trim().parse() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let pivot = current_year() + 1;
    if id == 0 {
        return items.iter().take(100).collect();
    }
    if id <= 0 || id >= 7 {
        let threshold = pivot - 6;
        return items
            .iter()
            .filter(|item| year_of(&item.year).map(|y| y < threshold).unwrap_or(false))
            .collect();
    }
    let want = pivot - id;
    items
        .iter()
        .filter(|item| year_of(&item.year) == Some(want))
        .collect()
}

fn to_video_item(item: &IndexItem) -> VideoItem {
    VideoItem {
        id: item.link.clone(),
        name: item.name.clone(),
        cover: Some(COVER.to_string()),
        remark: Some(item.hit.clone()),
    }
}

/* ============================================================
 * 入口
 * ============================================================ */

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    let eps = endpoints(site);
    let items = fetch_items(cfg, site, &eps)?;
    Ok(filter_by_category(&items, cat)
        .into_iter()
        .map(to_video_item)
        .collect())
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let eps = endpoints(site);
    let items = fetch_items(cfg, site, &eps)?;
    let kw = keyword.trim();
    Ok(items
        .iter()
        .filter(|item| item.name.contains(kw))
        .take(10)
        .map(to_video_item)
        .collect())
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let eps = endpoints(site);
    let items = fetch_items(cfg, site, &eps)?;
    let found = items.iter().find(|item| item.link == id).cloned().unwrap_or_default();
    let name = if found.name.is_empty() {
        id.to_string()
    } else {
        found.name.clone()
    };
    let year_number = year_of(&found.year).map(|y| y.to_string()).unwrap_or_default();
    let cat = if year_number.is_empty() {
        found.year.clone()
    } else {
        year_number.clone()
    };

    let client = crate::http::client_for(&cfg.http)?;
    let mut url = format!("{}/?cat={}", eps.site, cat);
    let mut episodes: BTreeMap<String, String> = BTreeMap::new();
    for _ in 0..MAX_PAGES {
        let html = crate::http::get_text(&client, &url, &html_headers(&eps.site), cfg.http.retry)?;
        let (next, mut found_rows) = parse_category_page(&html);
        episodes.append(&mut found_rows);
        if next.is_empty() {
            break;
        }
        url = absolutize(&eps.site, &next);
    }
    if episodes.is_empty() {
        return Err(err(site, "详情页没有可播放条目（页面结构可能已变）"));
    }
    let group = EpisodeGroup {
        group: "Anime1".to_string(),
        items: episodes
            .into_iter()
            .map(|(title, url)| Episode { title, url })
            .collect(),
    };
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: Some(COVER.to_string()),
        desc: Some(found.season.clone()).filter(|s| !s.is_empty()),
        year: Some(year_number).filter(|s| !s.is_empty()),
        area: Some("日本".to_string()),
        director: None,
        actor: Some(found.team.clone()).filter(|s| !s.is_empty()),
        episodes: vec![group],
    })
}

fn absolutize(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with('/') {
        format!("{base}{href}")
    } else {
        format!("{base}/{href}")
    }
}

/// 解析一页年份归档：返回（下一页链接，集名→`data-apireq`）。
fn parse_category_page(html: &str) -> (String, BTreeMap<String, String>) {
    let doc = Html::parse_document(html);
    let next = doc
        .select(nav_sel())
        .next()
        .and_then(|a| a.value().attr("href"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let mut out = BTreeMap::new();
    for article in doc.select(article_sel()) {
        let title = article
            .select(h2a_sel())
            .next()
            .map(|el| el.text().collect::<String>())
            .or_else(|| {
                article
                    .select(h2_sel())
                    .next()
                    .map(|el| el.text().collect::<String>())
            })
            .unwrap_or_else(|| "[No Title]".to_string());
        let title = title.trim().to_string();
        let label = match (title.find('['), title.find(']')) {
            (Some(a), Some(b)) if b > a + 1 => title[a + 1..b].trim().to_string(),
            _ => title.clone(),
        };
        let videos: Vec<_> = article.select(video_sel()).collect();
        for (idx, video) in videos.iter().enumerate() {
            let apireq = video.value().attr("data-apireq").unwrap_or("").trim();
            if apireq.is_empty() {
                continue;
            }
            let key = episode_key(&label, idx, videos.len());
            out.insert(key, apireq.to_string());
        }
    }
    (next, out)
}

/// 集名生成规则（对齐原实现的多视频/单视频分支）。
fn episode_key(label: &str, idx: usize, count: usize) -> String {
    if count > 1 {
        if idx < 9 {
            format!("{label}0{}", idx + 1)
        } else {
            format!("{label}{}", idx + 1)
        }
    } else if label_re().is_match(label) {
        label.to_string()
    } else if label.parse::<f64>().map(|v| v < 100.0).unwrap_or(false) {
        format!("0{label}")
    } else {
        label.to_string()
    }
}

/* ============================================================
 * 播放
 * ============================================================ */

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let eps = endpoints(site);
    let client = crate::http::client_for(&cfg.http)?;
    let decoded = percent_decode_str(id).decode_utf8_lossy().to_string();
    let mut form: HashMap<String, String> = HashMap::new();
    form.insert("d".to_string(), decoded);

    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "*/*".to_string());
    headers.insert(
        "Accept-Language".to_string(),
        "en,zh-TW;q=0.9,zh;q=0.8".to_string(),
    );
    headers.insert("Origin".to_string(), eps.site.clone());
    headers.insert("Referer".to_string(), format!("{}/", eps.site));
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Sec-Fetch-Dest".to_string(), "empty".to_string());
    headers.insert("Sec-Fetch-Mode".to_string(), "cors".to_string());
    headers.insert("Sec-Fetch-Site".to_string(), "same-site".to_string());

    let (body, resp_headers) =
        crate::http::post_form_text_with_headers(&client, &eps.api, &headers, &form)?;
    let cookie = resp_headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .take(3)
        .map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| err(site, format!("播放接口 JSON 解析失败: {e}")))?;
    let src = value
        .get("s")
        .and_then(|s| s.get(0))
        .and_then(|o| o.get("src"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if src.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::HostApi,
            site,
            "播放接口没有返回 s[0].src（cookie 校验或接口变更）",
        )
        .into_core());
    }
    let url = if src.starts_with("//") {
        format!("https:{src}")
    } else {
        src.to_string()
    };
    let authority = src.split('/').nth(2).unwrap_or("").to_string();
    let mut play_headers = serde_json::Map::new();
    play_headers.insert("Accept".to_string(), json!("*/*"));
    play_headers.insert("Accept-Encoding".to_string(), json!("identity;q=1, *;q=0"));
    play_headers.insert(
        "Accept-Language".to_string(),
        json!("en,zh-TW;q=0.9,zh;q=0.8"),
    );
    play_headers.insert("Referer".to_string(), json!(SITE_BASE));
    play_headers.insert("User-Agent".to_string(), json!(UA));
    play_headers.insert("Sec-Fetch-Dest".to_string(), json!("video"));
    play_headers.insert("Sec-Fetch-Mode".to_string(), json!("no-cors"));
    play_headers.insert("Sec-Fetch-Site".to_string(), json!("same-site"));
    play_headers.insert("sec-ch-ua-mobile".to_string(), json!("?0"));
    play_headers.insert("sec-ch-ua-platform".to_string(), json!("\"Windows\""));
    if !authority.is_empty() {
        play_headers.insert("Authority".to_string(), json!(authority));
    }
    if !cookie.is_empty() {
        play_headers.insert("cookie".to_string(), json!(cookie));
    }
    Ok(json!({
        "url": url,
        "header": Value::Object(play_headers),
        "parse": 0,
        "playUrl": "",
    }))
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_only_anime1() {
        assert!(handles("csp_Anime1"));
        assert!(handles(" CSP_ANIME1 "));
        assert!(!handles("csp_Ysj"));
    }

    #[test]
    fn categories_have_eight_entries_with_rolling_years() {
        let cats = categories();
        assert_eq!(cats.len(), 8);
        assert_eq!(cats[0].type_id, "0");
        assert_eq!(cats[0].type_name, "最近更新");
        assert_eq!(cats[7].type_id, "7");
        assert_eq!(cats[7].type_name, "更早");
        let y = current_year();
        assert_eq!(cats[1].type_name, y.to_string());
        assert_eq!(cats[6].type_name, (y - 5).to_string());
    }

    #[test]
    fn parses_index_rows_and_skips_short_ones() {
        let site = Site::default();
        let json = r#"[["https://anime1.me/1","甲","1","2024","10","组A"],["x","乙"]]"#;
        let items = parse_index(&site, json).expect("索引");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://anime1.me/1");
        assert_eq!(items[0].year, "2024");
        assert_eq!(items[0].season, "10");
        assert_eq!(items[0].team, "组A");
    }

    #[test]
    fn filters_by_category_id() {
        let y = current_year();
        let items: Vec<IndexItem> = [y, y - 1, y - 6]
            .iter()
            .enumerate()
            .map(|(i, year)| IndexItem {
                link: format!("l{i}"),
                name: format!("n{i}"),
                year: year.to_string(),
                ..Default::default()
            })
            .collect();
        let latest: Vec<_> = filter_by_category(&items, "0");
        assert_eq!(latest.len(), 3);
        let current: Vec<_> = filter_by_category(&items, "1");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].link, "l0");
        let older: Vec<_> = filter_by_category(&items, "7");
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].link, "l2");
        assert!(filter_by_category(&items, "abc").is_empty());
    }

    #[test]
    fn parses_category_page_episodes_and_next_link() {
        let html = r#"
        <div class="nav-previous"><a href="https://anime1.me/?cat=2024&amp;paged=2">上一页</a></div>
        <article>
          <h2><a href="/2024/01/x">[12] 测试番</a></h2>
          <div class="vjscontainer"><video data-apireq="ab123%2Fcd"></video></div>
        </article>
        <article>
          <h2>特别篇</h2>
          <div class="vjscontainer">
            <video data-apireq="p1"></video>
            <video data-apireq="p2"></video>
          </div>
        </article>
        "#;
        let (next, map) = parse_category_page(html);
        assert_eq!(next, "https://anime1.me/?cat=2024&paged=2");
        assert_eq!(map.get("012").map(String::as_str), Some("ab123%2Fcd"));
        assert_eq!(map.get("特别篇01").map(String::as_str), Some("p1"));
        assert_eq!(map.get("特别篇02").map(String::as_str), Some("p2"));
    }

    #[test]
    fn episode_key_rules() {
        assert_eq!(episode_key("12", 0, 1), "012");
        assert_eq!(episode_key("120", 0, 1), "120");
        assert_eq!(episode_key("OVA", 0, 1), "OVA");
        assert_eq!(episode_key("12", 0, 2), "1201");
        assert_eq!(episode_key("12", 9, 12), "1210");
    }

    #[test]
    fn year_parsing_handles_month_suffix() {
        assert_eq!(year_of("2024"), Some(2024));
        assert_eq!(year_of("2024/10"), Some(2024));
        assert_eq!(year_of(""), None);
        assert_eq!(year_of("abc"), None);
    }
}
