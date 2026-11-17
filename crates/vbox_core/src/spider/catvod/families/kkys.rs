//! `csp_Kkys` 原生实现（自建 JSON API，mock-only：上游 DNS 已失效）。
//!
//! 语义来自静态反编译：`_rea/decompiled/69dadb9a-Yoursmile/deobf/com/github/catvod/spider/Kkys.java`
//! （jadx 3 处跳过，Vineflower 全量 2280 行）；字符串池以「小端 short + XOR key」还原
//! （脚本 `_rea/tmp/r218/dec3.mjs`，替换视图 `_rea/tmp/r219/Kkys.view.java`，113/113 处、92 条可读串）。
//!
//! 原实现协议：
//! 1. 基址 `https://api1.baibaipei.com:8899`，UA 为 Dalvik；`ext` 非空时按首段 http(s) 覆盖（测试钩子）。
//! 2. 首页：GET `/api.php/Index/getTopVideoCategory` → `data[].nav_type_id/nav_name`，
//!    跳过 `nav_name=="推荐"`，输出 `type_id/type_name`。
//! 3. 分类：GET `/api.php/Video/getFilterVideoList`，参数 `type,p,sort,area,year,class`；
//!    缺省 sort/area/year 为 `"0"`，class 固定 `"fll---"`；`data.list[]` 取
//!    `vod_id/vod_name/vod_pic/vod_remarks`，limit 12，pagecount 取响应值或按满页推断。
//! 4. 详情：GET `/api.php/Video/getVideoInfo?video_id=` → `data.video` 基本字段；
//!    `vod_play[]` 每项 `{playerForm, url:[{title, play_url}]}`，组内 `title$play_url` 以 `#` 连接，
//!    `playerForm=="jp"` 时给 `play_url` 加 `tvbox-xg:` 前缀；线路名/剧集用 `$$$` 连接。
//! 5. 搜索：GET `/api.php/Search/getSearch?key=&page=`。
//! 6. 播放：`jqq-`/`rr-` 前缀走 `https://cache.json.icu/8520.php?url=` 代理取 `data.url`；
//!    命中 `youku/iqiyi/v.qq.com/pptv/le.com/1905.com/mgtv/bili` 时 `parse:0` + referer + Chrome UA；
//!    `tvbox-xg:` 前缀原样返回 `parse:0`；其余同样走代理。
//!
//! 与 Android 原实现的差异（均有意）：
//! - 上游 `api1.baibaipei.com` 已 ENOTFOUND，`base_of` 的 ext 覆盖用于 mock 测试；
//! - 详情 `vod_content` 仅做空白归一（原实现调用宿主 Html 清洗，桌面端无该工具）；
//! - 播放分支按「代理 / 平台 referer / 原样」三路还原，未复刻原实现的 switch 平坦化细节。

use std::collections::HashMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Kkys";

/// 站点默认基址（原类 `a`）。
const DEFAULT_BASE: &str = "https://api1.baibaipei.com:8899";

/// 安卓端 UA（原类 `init` 赋值，首页/详情/播放共用）。
const UA: &str = "Dalvik/2.1.0 (Linux; U; Android 9; PCRT00 Build/PQ3B.190801.07101926)";

/// 平台播放分支使用的 Chrome UA。
const CHROME_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/80.0.3987.163 Safari/537.36";

/// 需跳过的一级分类名。
const SKIP_NAV: &str = "推荐";

/// 分类页固定参数（原类字符串池）。
const FILTER_CLASS: &str = "fll---";

/// 代理取直链的服务（原类 `jqq-`/`rr-` 分支）。
const PROXY_URL: &str = "https://cache.json.icu/8520.php?url=";

/// `jp` 线路前缀。
const JP_PREFIX: &str = "tvbox-xg:";

/// 平台关键词 → referer（bili 单独用完整站）。
const PLATFORMS: [(&str, &str); 8] = [
    ("youku", "https://www.youku.com"),
    ("iqiyi", "https://www.iqiyi.com"),
    ("v.qq.com", "https://v.qq.com"),
    ("pptv", "https://www.pptv.com"),
    ("le.com", "https://www.le.com"),
    ("1905.com", "https://www.1905.com"),
    ("mgtv", "https://www.mgtv.com"),
    ("bili", "https://www.bilibili.com"),
];

/// 是否处理该 api（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// `ext` 首段为 http(s) 时覆盖基址（mock/镜像用），否则用默认基址。
pub fn base_of(site: &Site) -> String {
    let raw = site.ext_string();
    for token in raw.split(['#', ';', '|']) {
        let token = token.trim();
        if token.starts_with("http://") || token.starts_with("https://") {
            return token.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

/// 代理基址：`proxy=` 段可覆盖（mock 用），否则用线上服务。
fn proxy_of(site: &Site) -> String {
    let raw = site.ext_string();
    for token in raw.split(['#', ';', '|']) {
        let token = token.trim();
        if let Some(rest) = token.strip_prefix("proxy=") {
            let rest = rest.trim();
            if rest.starts_with("http://") || rest.starts_with("https://") {
                return rest.to_string();
            }
        }
    }
    PROXY_URL.to_string()
}

fn api_url(base: &str, path: &str) -> String {
    format!("{base}/api.php/{path}")
}

fn headers() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map
}

fn fetch_text(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(), 2)
}

fn fetch_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let text = fetch_text(cfg, url)?;
    serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("csp_Kkys JSON 解析失败: {e}; url={url}")))
}

fn as_str(v: &Value, key: &str) -> String {
    v.get(key)
        .map(|x| match x {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

/// 首页 `data[]`：跳过「推荐」，保留顺序。
fn parse_categories(v: &Value) -> Vec<Category> {
    let arr = v.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    arr.iter()
        .filter_map(|item| {
            let name = as_str(item, "nav_name");
            if name.trim() == SKIP_NAV || name.trim().is_empty() {
                return None;
            }
            let id = as_str(item, "nav_type_id");
            if id.trim().is_empty() {
                return None;
            }
            Some(Category { type_id: id, type_name: name })
        })
        .collect()
}

/// 分类页/搜索页的 `list[]` → `(items, pagecount)`。
fn parse_list(v: &Value, page: u32) -> (Vec<VideoItem>, u32) {
    let data = v.get("data").unwrap_or(v);
    let list = data
        .get("list")
        .and_then(Value::as_array)
        .or_else(|| data.get("data").and_then(|d| d.get("list")).and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let items = list
        .iter()
        .filter_map(|item| {
            let id = as_str(item, "vod_id");
            if id.trim().is_empty() {
                return None;
            }
            Some(VideoItem {
                id,
                name: as_str(item, "vod_name"),
                cover: non_empty(as_str(item, "vod_pic")),
                remark: non_empty(as_str(item, "vod_remarks")),
            })
        })
        .collect::<Vec<_>>();
    let pagecount = data
        .get("pagecount")
        .and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .map(|x| x as u32)
        .filter(|x| *x > 0)
        .unwrap_or_else(|| if items.len() >= 12 { page.saturating_add(1) } else { page });
    (items, pagecount.max(page))
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

/// 详情 `vod_play[]` → 线路组；组内 `title$play_url`，`jp` 前缀加 `tvbox-xg:`。
fn parse_groups(v: &Value) -> Vec<EpisodeGroup> {
    let Some(arr) = v
        .get("data")
        .and_then(|d| d.get("video"))
        .and_then(|video| video.get("vod_play"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for (idx, group) in arr.iter().enumerate() {
        let form = as_str(group, "playerForm");
        let name = if form.trim().is_empty() {
            format!("线路{}", idx + 1)
        } else {
            form.clone()
        };
        let mut items = Vec::new();
        if let Some(urls) = group.get("url").and_then(Value::as_array) {
            for item in urls {
                let title = as_str(item, "title");
                let mut url = as_str(item, "play_url");
                if form == "jp" && !url.starts_with(JP_PREFIX) {
                    url = format!("{JP_PREFIX}{url}");
                }
                if url.trim().is_empty() {
                    continue;
                }
                items.push(Episode {
                    title: if title.trim().is_empty() { url.clone() } else { title },
                    url,
                });
            }
        }
        if !items.is_empty() {
            groups.push(EpisodeGroup { group: name, items });
        }
    }
    groups
}

/// 首页分类：`getTopVideoCategory`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let value = fetch_json(cfg, site, &api_url(&base, "Index/getTopVideoCategory"))?;
    Ok(parse_categories(&value))
}

/// 分类列表：`getFilterVideoList`（limit 12，sort/area/year 缺省 "0"）。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let page = page.max(1);
    let cat = utf8_percent_encode(cat, NON_ALPHANUMERIC).to_string();
    let url = format!(
        "{}?type={}&p={}&sort=0&area=0&year=0&class={}",
        api_url(&base, "Video/getFilterVideoList"),
        cat,
        page,
        FILTER_CLASS
    );
    let value = fetch_json(cfg, site, &url)?;
    Ok(parse_list(&value, page).0)
}

/// 详情：`getVideoInfo?video_id=`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "csp_Kkys 详情 id 为空"));
    }
    let url = format!(
        "{}?video_id={}",
        api_url(&base, "Video/getVideoInfo"),
        utf8_percent_encode(id, NON_ALPHANUMERIC)
    );
    let value = fetch_json(cfg, site, &url)?;
    let video = value
        .get("data")
        .and_then(|d| d.get("video"))
        .ok_or_else(|| parse_err(site, format!("csp_Kkys 详情缺少 data.video: {value}")))?;
    let desc = as_str(video, "vod_content");
    Ok(VideoDetail {
        id: id.to_string(),
        name: as_str(video, "vod_name"),
        cover: non_empty(as_str(video, "vod_pic")),
        desc: non_empty(normalize_ws(&desc)),
        year: non_empty(as_str(video, "vod_year")),
        area: non_empty(as_str(video, "vod_area")),
        director: non_empty(as_str(video, "vod_director")),
        actor: non_empty(as_str(video, "vod_actor")),
        episodes: parse_groups(&value),
    })
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 搜索：`getSearch?key=&page=`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let key = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{}?key={}&page=1", api_url(&base, "Search/getSearch"), key);
    let value = fetch_json(cfg, site, &url)?;
    Ok(parse_list(&value, 1).0)
}

/// 播放：代理 / 平台 referer / `tvbox-xg:` 原样 三路。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "csp_Kkys 播放地址为空"));
    }
    if id.starts_with(JP_PREFIX) {
        return Ok(json!({"parse": 0, "jx": 0, "playUrl": "", "url": id, "header": {}}));
    }
    if let Some((_, referer)) = PLATFORMS.iter().find(|(key, _)| id.contains(key)) {
        return Ok(json!({
            "parse": 0,
            "jx": 0,
            "playUrl": "",
            "url": id,
            "header": {
                "User-Agent": CHROME_UA,
                "referer": referer,
            },
        }));
    }
    let proxy = format!("{}{}", proxy_of(site), utf8_percent_encode(id, NON_ALPHANUMERIC));
    let value = fetch_json(cfg, site, &proxy)?;
    let real = value
        .get("data")
        .and_then(|d| d.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if real.trim().is_empty() {
        return Err(parse_err(site, format!("csp_Kkys 播放代理未返回 url: id={id}")));
    }
    Ok(json!({
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "url": real,
        "header": { "User-Agent": UA },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_categories_skip_recommend_and_empty_ids() {
        let v = json!({"data": [
            {"nav_type_id": "1", "nav_name": "推荐"},
            {"nav_type_id": "2", "nav_name": "电影"},
            {"nav_type_id": "", "nav_name": "空"},
            {"nav_type_id": "3", "nav_name": "剧集"}
        ]});
        let c = parse_categories(&v);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].type_id, "2");
        assert_eq!(c[1].type_name, "剧集");
    }

    #[test]
    fn list_parses_fields_and_pagecount() {
        let v = json!({"data": {"pagecount": "4", "list": [
            {"vod_id": "10", "vod_name": "A", "vod_pic": "p.jpg", "vod_remarks": "更新"},
            {"vod_id": "", "vod_name": "bad"},
            {"vod_id": 11, "vod_name": "B"}
        ]}});
        let (items, pc) = parse_list(&v, 1);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "10");
        assert_eq!(items[0].cover.as_deref(), Some("p.jpg"));
        assert_eq!(items[1].id, "11");
        assert_eq!(pc, 4);
    }

    #[test]
    fn list_pagecount_derived_from_full_page() {
        let mut list = Vec::new();
        for i in 0..12 {
            list.push(json!({"vod_id": format!("{i}"), "vod_name": "x"}));
        }
        let (items, pc) = parse_list(&json!({"data": {"list": list}}), 2);
        assert_eq!(items.len(), 12);
        assert_eq!(pc, 3);
    }

    #[test]
    fn detail_groups_join_episodes_and_jp_prefix() {
        let v = json!({"data": {"video": {
            "vod_name": "N", "vod_pic": "c", "vod_year": "2024", "vod_area": "CN",
            "vod_remarks": "r", "vod_director": "d", "vod_actor": "a",
            "vod_content": "  a   b ",
            "vod_play": [
                {"playerForm": "jp", "url": [{"title": "1", "play_url": "http://x/1"}]},
                {"playerForm": "", "url": [{"title": "", "play_url": "http://y/2"}, {"title": "3", "play_url": ""}]}
            ]
        }}});
        let g = parse_groups(&v);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].group, "jp");
        assert_eq!(g[0].items[0].url, "tvbox-xg:http://x/1");
        assert_eq!(g[1].group, "线路2");
        assert_eq!(g[1].items.len(), 1);
        assert_eq!(g[1].items[0].title, "http://y/2");
    }

    #[test]
    fn platform_referer_selection() {
        assert_eq!(
            PLATFORMS.iter().find(|(k, _)| "https://www.bilibili.com/x".contains(k)).unwrap().1,
            "https://www.bilibili.com"
        );
        assert!(PLATFORMS.iter().any(|(k, _)| k == &"mgtv"));
    }
}
