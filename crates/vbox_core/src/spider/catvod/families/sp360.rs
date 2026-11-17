//! `csp_SP360` 家族：360 影视（api.web.360kan.com 官方 JSON API + so.360kan.com 搜索）。
//!
//! 来源：`SP360.java`（529 行，`extends Spider`，jadx 无跳段）。协议要点：
//! 1. 固定 4 分类：`1` 电影 / `2` 电视剧 / `3` 综艺 / `4` 动漫；分类路径段
//!    `1→m`、`2→tv`、`3→va`、`4→ct`。
//! 2. 列表：`GET {api}/v1/filter/list?catid={cat}&rank=ranklatest&cat=&year=&area=&act=&size=35`
//!    （第 2 页起追加 `&pageno={page}`）→ `data.movies[]`。
//! 3. 详情：列表/搜索返回的 `vod_id` 是一段 JSON
//!    `{"detailUrl": ".../v1/detail?cat=&id=&callback=", "detailReferer": ".../{m|tv|va|ct}/{id}.html"}`；
//!    详情页按 `allepidetail`（剧集，需按 `&site=&start=&end=` 分批 200 集）、
//!    `defaultepisode`（综艺，按 `tag` 的年份键）或 `playlinksdetail`（电影，`default_url`）三选一。
//! 4. 搜索：`GET {so}/index?force_v=1&kw=&from=&pageno=1&v_ap=1&tab=all` → `data.longData.rows[]`。
//! 5. 播放：原实现只回填 `{"url": 分集地址, "parse": 1, "jx": 1, "header": ...}`，由宿主做二次解析
//!    （桌面端暂不挂弹幕路由，`addDanmaku` 省略）。
//!
//! 与 Android 实现的偏差（均写入计划文档附录）：
//! - 分类筛选（`by/class/year/area/actor`）不接入（VBox `Site` 无筛选承载位），列表固定 `ranklatest`；
//! - 每集标题 `第N集` 由本实现补齐（原实现依赖 `vod_play_url` 的 `$` 分隔，语义一致）；
//! - `ext` 除原本的「屏蔽站点」字符串外，另接受 URL（镜像）与 `{"api","so","web","ban"}` JSON（mock/镜像）；
//! - 弹幕路由（`addDanmaku`）不实现。

use std::collections::HashMap;
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::error::Result;
use crate::http;
use crate::model::{
    Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem,
};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_SP360";

/// 同 jar 的 360kan 旧实现：分类由 ext 的 `classes` 驱动，接口参数更简（`pageno`），
/// 详情/搜索直接使用 `{cat}_{ent}` id。
pub const API_SP33: &str = "csp_SP33";

const DEFAULT_API: &str = "https://api.web.360kan.com";
const DEFAULT_SEARCH: &str = "https://api.so.360kan.com";
const DEFAULT_WEB: &str = "https://www.360kan.com";
const API_REFERER: &str = "https://api.web.360kan.com";
const UA: &str = "Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/39.0.2171.71 Safari/537.36";
const SIZE: u32 = 35;
const BATCH: usize = 200;

const CATEGORIES: [(&str, &str); 4] = [
    ("1", "电影"),
    ("2", "电视剧"),
    ("3", "综艺"),
    ("4", "动漫"),
];

/// 原实现会跳过标题含这些词的条目（幕后花絮类）。
const SKIP_TITLES: [&str; 3] = ["幕后记录", "闭幕式", "赏析"];

const LIST_MARKER: &str = "\"msg\":\"ok\"";
const DETAIL_MARKER: &str = "\"msg\":\"Success\"";

/// 站点端点：默认官方域名，`ext` 可覆盖（镜像/mock）。
struct Endpoints {
    api: String,
    search: String,
    web: String,
    ban: Vec<String>,
}

fn split_ban(raw: &str) -> Vec<String> {
    raw.split([',', '，', ' ', '|'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn endpoints(site: &Site) -> Endpoints {
    let mut ep = Endpoints {
        api: DEFAULT_API.to_string(),
        search: DEFAULT_SEARCH.to_string(),
        web: DEFAULT_WEB.to_string(),
        ban: Vec::new(),
    };
    let raw = site.ext_string();
    let raw = raw.trim();
    if raw.is_empty() {
        return ep;
    }
    if raw.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(s) = v.get("api").and_then(Value::as_str) {
                ep.api = s.trim_end_matches('/').to_string();
            }
            if let Some(s) = v.get("so").and_then(Value::as_str) {
                ep.search = s.trim_end_matches('/').to_string();
            }
            if let Some(s) = v.get("web").and_then(Value::as_str) {
                ep.web = s.trim_end_matches('/').to_string();
            }
            if let Some(s) = v.get("ban").and_then(Value::as_str) {
                ep.ban = split_ban(s);
            } else if let Some(arr) = v.get("ban").and_then(Value::as_array) {
                ep.ban = arr
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
            }
            return ep;
        }
    }
    if raw.starts_with("http") {
        let base = raw.trim_end_matches('/').to_string();
        ep.api = base.clone();
        ep.search = base;
        return ep;
    }
    ep.ban = split_ban(raw);
    ep
}

fn headers(referer: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    if !referer.is_empty() {
        h.insert("Referer".to_string(), referer.to_string());
    }
    h
}

fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    referer: &str,
) -> Result<String> {
    http::get_text(client, url, &headers(referer), 1)
}

/// 原实现 `c(url, marker)`：最多 8 次、每次 500ms，直到响应包含 marker。
/// 这里收敛为 3 次 / 300ms（桌面端避免长阻塞）。
fn fetch_until(
    client: &reqwest::blocking::Client,
    url: &str,
    referer: &str,
    marker: &str,
) -> Result<String> {
    let mut last = String::new();
    for attempt in 0..3 {
        last = fetch(client, url, referer)?;
        if last.contains(marker) {
            return Ok(last);
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    Ok(last)
}

fn type_segment(catid: &str) -> &'static str {
    match catid {
        "1" => "m",
        "2" => "tv",
        "3" => "va",
        "4" => "ct",
        _ => "",
    }
}

fn filter_url(ep: &Endpoints, catid: &str, page: u32) -> String {
    let mut url = format!(
        "{}/v1/filter/list?catid={}&rank=ranklatest&cat=&year=&area=&act=&size={}",
        ep.api, catid, SIZE
    );
    if page > 1 {
        url.push_str(&format!("&pageno={}", page));
    }
    url.push_str("&callback=");
    url
}

fn detail_base(ep: &Endpoints, catid: &str, id: &str) -> String {
    format!("{}/v1/detail?cat={}&id={}", ep.api, catid, id)
}

fn detail_url(ep: &Endpoints, catid: &str, id: &str) -> String {
    format!("{}&callback=", detail_base(ep, catid, id))
}

fn detail_referer(ep: &Endpoints, catid: &str, id: &str) -> String {
    let seg = type_segment(catid);
    if seg.is_empty() {
        format!("{}/", ep.web)
    } else {
        format!("{}/{}/{}.html", ep.web, seg, id)
    }
}

/// 列表/搜索的 `vod_id`：原实现存 JSON 串，详情阶段再解析。
fn video_id(ep: &Endpoints, catid: &str, id: &str) -> String {
    json!({
        "detailUrl": detail_url(ep, catid, id),
        "detailReferer": detail_referer(ep, catid, id),
    })
    .to_string()
}

fn cover_url(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let raw = raw.trim();
    if raw.starts_with("http") {
        return raw.to_string();
    }
    if raw.starts_with("//") {
        return format!("https:{}", raw);
    }
    format!("https://{}", raw)
}

/// 把 JSON 里的标量转成字符串（数字也接受）。
fn json_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn json_i64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// 原实现 `a(JSONArray)`：数组元素直接逗号拼接（去引号/方括号/转义）。
fn join_array(v: Option<&Value>) -> String {
    match v {
        Some(Value::Array(arr)) => arr.iter().map(json_str_any).collect::<Vec<_>>().join(","),
        Some(other) => json_str(Some(other)),
        None => String::new(),
    }
}

fn json_str_any(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn remark_for(movie: &Value, catid: &str) -> String {
    match catid {
        "2" | "4" => {
            let upinfo = json_str(movie.get("upinfo"));
            let total = json_str(movie.get("total"));
            if !upinfo.is_empty() && upinfo == total {
                "已完结".to_string()
            } else if !upinfo.is_empty() {
                format!("更新至{}集", upinfo)
            } else {
                String::new()
            }
        }
        "3" => {
            let tag = json_str(movie.get("tag"));
            if tag.is_empty() {
                String::new()
            } else {
                format!("更新至{}", tag)
            }
        }
        _ => String::new(),
    }
}

fn parse_movies(body: &str, ep: &Endpoints, catid: &str, site: &Site) -> Result<Vec<VideoItem>> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("360kan 列表 JSON 解析失败: {e}"),
        )
        .into_core()
    })?;
    let movies = v
        .get("data")
        .and_then(|d| d.get("movies"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for movie in movies {
        let name = json_str(movie.get("title"));
        if name.is_empty() {
            continue;
        }
        if SKIP_TITLES.iter().any(|s| name.contains(s)) {
            continue;
        }
        let id = json_str(movie.get("id"));
        if id.is_empty() {
            continue;
        }
        let cover = cover_url(&json_str(movie.get("cdncover")));
        let remark = remark_for(&movie, catid);
        out.push(VideoItem {
            id: video_id(ep, catid, &id),
            name,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark: if remark.is_empty() {
                None
            } else {
                Some(remark)
            },
        });
    }
    Ok(out)
}

fn parse_search_rows(body: &str, ep: &Endpoints, site: &Site) -> Result<Vec<VideoItem>> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("360kan 搜索 JSON 解析失败: {e}"),
        )
        .into_core()
    })?;
    let rows = v
        .get("data")
        .and_then(|d| d.get("longData"))
        .and_then(|d| d.get("rows"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for row in rows {
        let name = json_str(row.get("titleTxt"));
        let en_id = json_str(row.get("en_id"));
        if name.is_empty() || en_id.is_empty() {
            continue;
        }
        let catid = json_str(row.get("cat_id"));
        let referer = json_str(row.get("url"));
        let vid = json!({
            "detailUrl": detail_url(ep, &catid, &en_id),
            "detailReferer": if referer.is_empty() {
                detail_referer(ep, &catid, &en_id)
            } else {
                referer
            },
        })
        .to_string();
        let cover = cover_url(&json_str(row.get("cover")));
        let remark = json_str(row.get("cat_name"));
        out.push(VideoItem {
            id: vid,
            name,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark: if remark.is_empty() {
                None
            } else {
                Some(remark)
            },
        });
    }
    Ok(out)
}

pub fn handles(api: &str) -> bool {
    let a = api.trim();
    a.eq_ignore_ascii_case(API) || a.eq_ignore_ascii_case(API_SP33)
}

fn is_sp33(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API_SP33)
}

pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let ep = endpoints(site);
    let catid = if CATEGORIES.iter().any(|(id, _)| *id == cat) {
        cat
    } else {
        "1"
    };
    let client = crate::http::client_for(&cfg.http)?;
    if is_sp33(&site.api) {
        let url = format!("{}/v1/filter/list?catid={}&pageno={}", ep.api, catid, page);
        let body = fetch(&client, &url, API_REFERER)?;
        return parse_movies_sp33(&body, catid, site);
    }
    let body = fetch_until(&client, &filter_url(&ep, catid, page), API_REFERER, LIST_MARKER)?;
    parse_movies(&body, &ep, catid, site)
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let ep = endpoints(site);
    let client = crate::http::client_for(&cfg.http)?;
    let (detail_url, v) = if is_sp33(&site.api) {
        let (cat, ent) = id.split_once('_').ok_or_else(|| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                "csp_SP33 需要列表/搜索返回的 {cat}_{ent} 形式 id".to_string(),
            )
            .into_core()
        })?;
        let url = format!("{}/v1/detail?cat={}&id={}", ep.api, cat, ent);
        let body = fetch(&client, &url, API_REFERER)?;
        let v: Value = serde_json::from_str(&body).map_err(|e| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("360kan 详情 JSON 解析失败: {e}"),
            )
            .into_core()
        })?;
        (url, v)
    } else {
        let spec: Value = serde_json::from_str(id).map_err(|_| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                "csp_SP360 需要列表/搜索返回的 vod_id JSON（含 detailUrl）".to_string(),
            )
            .into_core()
        })?;
        let url = json_str(spec.get("detailUrl"));
        if !url.starts_with("http") {
            return Err(CatvodError::new(
                ErrorClass::Parse,
                site,
                "csp_SP360 vod_id 缺少 detailUrl".to_string(),
            )
            .into_core());
        }
        let body = fetch_until(&client, &url, API_REFERER, DETAIL_MARKER)?;
        let v: Value = serde_json::from_str(&body).map_err(|e| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("360kan 详情 JSON 解析失败: {e}"),
            )
            .into_core()
        })?;
        (url, v)
    };
    let data = v.get("data").ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "360kan 详情缺少 data".to_string(),
        )
        .into_core()
    })?;
    let name = json_str(data.get("title"));
    if name.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "360kan 详情缺少标题".to_string(),
        )
        .into_core());
    }

    let base = detail_url
        .split("&callback=")
        .next()
        .unwrap_or(&detail_url)
        .to_string();
    let sites: Vec<String> = data
        .get("playlink_sites")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !ep.ban.iter().any(|b| s.contains(b.as_str())))
        .collect();

    let mut groups: Vec<EpisodeGroup> = Vec::new();
    if data.get("allepidetail").is_some() {
        let allup = data.get("allupinfo");
        for site_key in &sites {
            let total = json_i64(allup.and_then(|a| a.get(site_key))) as usize;
            let mut items: Vec<Episode> = Vec::new();
            let mut start = 1usize;
            while start <= total {
                let end = (start + BATCH - 1).min(total);
                let url = format!(
                    "{}&site={}&start={}&end={}&callback=",
                    base, site_key, start, end
                );
                let page = fetch_until(&client, &url, API_REFERER, DETAIL_MARKER)?;
                if let Ok(pv) = serde_json::from_str::<Value>(&page) {
                    if let Some(arr) = pv
                        .get("data")
                        .and_then(|d| d.get("allepidetail"))
                        .and_then(|d| d.get(site_key))
                        .and_then(Value::as_array)
                    {
                        for item in arr {
                            let ep_url = json_str(item.get("url"));
                            if ep_url.is_empty() {
                                continue;
                            }
                            let num = json_str(item.get("playlink_num"));
                            let title = if num.is_empty() {
                                format!("第{}集", items.len() + 1)
                            } else {
                                format!("第{}集", num)
                            };
                            items.push(Episode {
                                title,
                                url: ep_url,
                            });
                        }
                    }
                }
                start = end + 1;
            }
            if !items.is_empty() {
                groups.push(EpisodeGroup {
                    group: site_key.clone(),
                    items,
                });
            }
        }
    } else if data.get("defaultepisode").is_some()
        && data.get("tag").map(Value::is_object).unwrap_or(false)
    {
        let tags = data.get("tag").and_then(Value::as_object).cloned().unwrap_or_default();
        for site_key in &sites {
            let mut items: Vec<Episode> = Vec::new();
            for year in tags.keys() {
                let url = format!("{}&site={}&year={}&callback=", base, site_key, year);
                let page = fetch_until(&client, &url, API_REFERER, DETAIL_MARKER)?;
                if let Ok(pv) = serde_json::from_str::<Value>(&page) {
                    if let Some(arr) = pv
                        .get("data")
                        .and_then(|d| d.get("defaultepisode"))
                        .and_then(Value::as_array)
                    {
                        for item in arr {
                            let ep_url = json_str(item.get("url"));
                            if ep_url.is_empty() {
                                continue;
                            }
                            let period = json_str(item.get("period"));
                            let ep_name = json_str(item.get("name"));
                            let title = format!("{} {}", period, ep_name)
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            items.push(Episode {
                                title: if title.is_empty() {
                                    format!("第{}集", items.len() + 1)
                                } else {
                                    title
                                },
                                url: ep_url,
                            });
                        }
                    }
                }
            }
            if !items.is_empty() {
                groups.push(EpisodeGroup {
                    group: site_key.clone(),
                    items,
                });
            }
        }
    } else if let Some(links) = data.get("playlinksdetail").and_then(Value::as_object) {
        for site_key in &sites {
            if let Some(link) = links.get(site_key) {
                let url = json_str(link.get("default_url"));
                if url.is_empty() {
                    continue;
                }
                groups.push(EpisodeGroup {
                    group: site_key.clone(),
                    items: vec![Episode {
                        title: "正片".to_string(),
                        url,
                    }],
                });
            }
        }
    }

    let cover = cover_url(&json_str(data.get("cdncover")));
    let area = join_array(data.get("area"));
    let actor = join_array(data.get("actor"));
    let director = join_array(data.get("director"));
    let desc = json_str(data.get("description"));
    let year = json_str(data.get("pubdate"));
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: if cover.is_empty() { None } else { Some(cover) },
        desc: if desc.is_empty() { None } else { Some(desc) },
        year: if year.is_empty() { None } else { Some(year) },
        area: if area.is_empty() { None } else { Some(area) },
        director: if director.is_empty() {
            None
        } else {
            Some(director)
        },
        actor: if actor.is_empty() { None } else { Some(actor) },
        episodes: groups,
    })
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let ep = endpoints(site);
    let enc = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!(
        "{}/index?force_v=1&kw={}&from=&pageno=1&v_ap=1&tab=all",
        ep.search, enc
    );
    let referer = format!("https://so.360kan.com/?kw={}", enc);
    let client = crate::http::client_for(&cfg.http)?;
    let body = fetch(&client, &url, &referer)?;
    if is_sp33(&site.api) {
        return parse_search_rows_sp33(&body, site);
    }
    parse_search_rows(&body, &ep, site)
}

fn parse_movies_sp33(body: &str, catid: &str, site: &Site) -> Result<Vec<VideoItem>> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("360kan(SP33) 列表 JSON 解析失败: {e}"),
        )
        .into_core()
    })?;
    let movies = v
        .get("data")
        .and_then(|d| d.get("movies"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for movie in movies {
        let name = json_str(movie.get("title"));
        if name.is_empty() {
            continue;
        }
        if SKIP_TITLES.iter().any(|s| name.contains(s)) {
            continue;
        }
        let mut id = json_str(movie.get("id"));
        if id.is_empty() {
            id = json_str(movie.get("vod_id"));
        }
        if id.is_empty() {
            continue;
        }
        let cover = cover_url(&json_str(movie.get("cover")));
        let mut remark = json_str(movie.get("vod_remarks"));
        if remark.is_empty() {
            remark = remark_for(&movie, catid);
        }
        out.push(VideoItem {
            id: format!("{catid}_{id}"),
            name,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark: if remark.is_empty() {
                None
            } else {
                Some(remark)
            },
        });
    }
    Ok(out)
}

fn parse_search_rows_sp33(body: &str, site: &Site) -> Result<Vec<VideoItem>> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("360kan(SP33) 搜索 JSON 解析失败: {e}"),
        )
        .into_core()
    })?;
    let rows = v
        .get("data")
        .and_then(|d| d.get("longData"))
        .and_then(|d| d.get("rows"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for row in rows {
        let name = json_str(row.get("titleTxt"));
        let en_id = json_str(row.get("en_id"));
        if name.is_empty() || en_id.is_empty() {
            continue;
        }
        let catid = json_str(row.get("cat_id"));
        if catid.is_empty() {
            continue;
        }
        let cover = cover_url(&json_str(row.get("cover")));
        let remark = json_str(row.get("cat_name"));
        out.push(VideoItem {
            id: format!("{catid}_{en_id}"),
            name,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark: if remark.is_empty() {
                None
            } else {
                Some(remark)
            },
        });
    }
    Ok(out)
}

pub fn player_content(
    _cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    if id.trim().is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_SP360 播放缺少地址".to_string(),
        )
        .into_core());
    }
    let mut header = serde_json::Map::new();
    header.insert("User-Agent".to_string(), Value::String(UA.to_string()));
    Ok(json!({
        "url": id,
        "parse": 1,
        "jx": if is_sp33(&site.api) { Value::String("1".to_string()) } else { Value::from(1) },
        "playUrl": "",
        "header": Value::Object(header),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            key: "360".to_string(),
            name: "360".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_api_case_insensitively() {
        assert!(handles("csp_SP360"));
        assert!(handles(" CSP_SP360 "));
        assert!(!handles("csp_360"));
    }

    #[test]
    fn categories_are_fixed_four() {
        let cats = categories();
        assert_eq!(cats.len(), 4);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[3].type_name, "动漫");
    }

    #[test]
    fn filter_url_adds_page_after_first() {
        let ep = endpoints(&site());
        let first = filter_url(&ep, "1", 1);
        assert!(first.contains("catid=1"));
        assert!(first.contains("rank=ranklatest"));
        assert!(!first.contains("pageno"));
        assert!(first.ends_with("&callback=&callback=") == false);
        assert!(first.ends_with("&callback="));
        let second = filter_url(&ep, "2", 3);
        assert!(second.contains("&pageno=3&callback="));
    }

    #[test]
    fn handles_sp33_variant() {
        assert!(handles("csp_SP33"));
        assert!(handles(" CSP_SP33 "));
        assert!(!handles("csp_SP33X"));
    }

    #[test]
    fn sp33_list_and_search_ids_are_plain() {
        let list = r#"{"data":{"movies":[
            {"id":"AAA","title":"兰香如故","cover":"//cdn/a.jpg","vod_remarks":"更新至12集"}
        ]}}"#;
        let items = parse_movies_sp33(list, "2", &site()).expect("list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "2_AAA");
        assert_eq!(items[0].cover.as_deref(), Some("https://cdn/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新至12集"));

        let search = r#"{"data":{"longData":{"rows":[
            {"cat_id":"1","en_id":"EN1","titleTxt":"海贼大冒险","cover":"https://img/1.jpg","cat_name":"电影"}
        ]}}}"#;
        let rows = parse_search_rows_sp33(search, &site()).expect("search");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1_EN1");
        assert_eq!(rows[0].remark.as_deref(), Some("电影"));
    }

    #[test]
    fn sp33_player_uses_string_jx() {
        let mut sp = site();
        sp.api = API_SP33.to_string();
        let out = player_content(&TvConfig::default(), &sp, "", "http://www.360kan.com/m/EN1.html")
            .expect("player");
        assert_eq!(out.get("parse").and_then(Value::as_i64), Some(1));
        assert_eq!(out.get("jx").and_then(Value::as_str), Some("1"));
    }

    #[test]
    fn list_parses_and_skips_behind_scenes() {
        let ep = endpoints(&site());
        let body = r#"{"data":{"movies":[
            {"id":"AAA","title":"蜂鸟行动","cdncover":"//p5.qhimg.com/a.jpg","upinfo":"","total":"","tag":""},
            {"id":"BBB","title":"某剧幕后记录","cdncover":"//p5.qhimg.com/b.jpg"},
            {"id":"CCC","title":"兰香如故","cdncover":"//p5.qhimg.com/c.jpg","upinfo":"12","total":"24"}
        ]},"msg":"ok"}"#;
        let items = parse_movies(body, &ep, "2", &site()).expect("parse");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "蜂鸟行动");
        assert_eq!(items[0].cover.as_deref(), Some("https://p5.qhimg.com/a.jpg"));
        assert_eq!(items[1].remark.as_deref(), Some("更新至12集"));
        let vid: Value = serde_json::from_str(&items[1].id).expect("id json");
        let url = vid.get("detailUrl").and_then(Value::as_str).unwrap();
        assert!(url.contains("/v1/detail?cat=2&id=CCC&callback="));
        let referer = vid.get("detailReferer").and_then(Value::as_str).unwrap();
        assert_eq!(referer, "https://www.360kan.com/tv/CCC.html");
    }

    #[test]
    fn detail_all_episodes_groups_by_site() {
        let ep = endpoints(&site());
        assert!(ep.ban.is_empty());
        // 单元层面覆盖分支解析：allepidetail 分页组装在 detail() 内，这里校验关键辅助。
        let body = r#"{"data":{"id":"1","title":"兰香如故","cdncover":"https://cdn/a.jpg",
            "moviecategory":["剧情"],"area":["大陆"],"actor":["甲","乙"],"director":["丙"],
            "pubdate":"2026-09-01","description":"简介",
            "playlink_sites":["qq"],
            "allupinfo":{"qq":"3"},
            "allepidetail":{"qq":[
                {"playlink_num":"1","url":"https://v.qq.com/1.html"},
                {"playlink_num":"2","url":"https://v.qq.com/2.html"}]}}}"#;
        let v: Value = serde_json::from_str(body).unwrap();
        let data = v.get("data").unwrap();
        let allup = data.get("allupinfo");
        assert_eq!(json_i64(allup.and_then(|a| a.get("qq"))), 3);
        let arr = data
            .get("allepidetail")
            .and_then(|d| d.get("qq"))
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(json_str(arr[1].get("playlink_num")), "2");
        assert_eq!(join_array(data.get("actor")), "甲,乙");
        assert_eq!(json_str(data.get("pubdate")), "2026-09-01");
    }

    #[test]
    fn search_rows_build_detail_json() {
        let ep = endpoints(&site());
        let body = r#"{"code":0,"msg":"ok","data":{"longData":{"rows":[
            {"cat_id":"1","en_id":"EN1","titleTxt":"海贼大冒险","cover":"https://img/1.jpg","cat_name":"电影","url":"http://www.360kan.com/m/EN1.html"},
            {"cat_id":"2","en_id":"EN2","titleTxt":"某剧","cover":"//img/2.jpg","cat_name":"电视剧","url":""}
        ]}}}"#;
        let items = parse_search_rows(body, &ep, &site()).expect("parse");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "海贼大冒险");
        assert_eq!(items[0].remark.as_deref(), Some("电影"));
        let vid: Value = serde_json::from_str(&items[1].id).unwrap();
        assert_eq!(
            vid.get("detailReferer").and_then(Value::as_str).unwrap(),
            "https://www.360kan.com/tv/EN2.html"
        );
    }

    #[test]
    fn player_marks_parse_and_jx() {
        let out = player_content(&TvConfig::default(), &site(), "", "https://v.qq.com/x.html")
            .expect("player");
        assert_eq!(out.get("parse").and_then(Value::as_i64), Some(1));
        assert_eq!(out.get("jx").and_then(Value::as_i64), Some(1));
        assert!(out.get("url").and_then(Value::as_str).unwrap().starts_with("https://v.qq.com"));
        assert!(player_content(&TvConfig::default(), &site(), "", "  ").is_err());
    }

    #[test]
    fn ext_switches_endpoints_and_ban() {
        let mut s = site();
        s.ext = Some(Value::String("youku,qiyi".to_string()));
        let ep = endpoints(&s);
        assert_eq!(ep.ban, vec!["youku".to_string(), "qiyi".to_string()]);
        assert_eq!(ep.api, DEFAULT_API);

        let mut mirror = site();
        mirror.ext = Some(Value::String("http://127.0.0.1:1234/".to_string()));
        let ep = endpoints(&mirror);
        assert_eq!(ep.api, "http://127.0.0.1:1234");
        assert_eq!(ep.search, "http://127.0.0.1:1234");

        let mut js = site();
        js.ext = Some(serde_json::json!({"api":"http://a","so":"http://b","ban":["x"]}));
        let ep = endpoints(&js);
        assert_eq!(ep.api, "http://a");
        assert_eq!(ep.search, "http://b");
        assert_eq!(ep.ban, vec!["x".to_string()]);
    }

    #[test]
    fn detail_requires_vod_id_json() {
        let err = detail(&TvConfig::default(), &site(), "raw-id").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("catvod.result.parse"));
    }
}
