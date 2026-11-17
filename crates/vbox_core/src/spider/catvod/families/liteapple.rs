//! `csp_LiteApple`：小苹果影视（`item.xpgtv.com`）原生还原。
//!
//! 来源：`_rea/decompiled/5fc50e17-5199d8f8bcbe3c6d6938ded33cb90596/deobf/
//! com/github/catvod/spider/LiteApple.java`（785 行、0 skip，字符串用
//! `C0378a.b(byte[],byte[])` 两数组循环异或解码）。
//!
//! 协议（全部 `GET`，请求头见 [`headers`]）：
//! - 分类：`/api.php/v2.vod/androidtypes` → `data[]` 的 `type_id/type_name/classes/areas/years`；
//! - 列表：`/api.php/v2.vod/androidfilter?page={p}&type={id}`，再追加非空筛选项 `area/year/sortby`；
//! - 首页推荐：`/api.php/v2.main/androidhome` → `data.list[]`，每组 `title` + 最多 4 条（原类重复拉 4 次，见偏差 2）；
//! - 详情：`/api.php/v3.vod/androiddetail2?vod_id={id}` → `data.urls[]` 的 `key/url`，单线路 `LiteApple`；
//! - 搜索：`/api.php/v2.vod/androidsearch10086?page=1&wd={enc}`，**只保留名称包含关键词的行**（原类行为）；
//! - 播放：`{cdn}/m3u8/{token}.m3u8`，`{token}` 即详情 `urls[].url`（`#EXTM3U` 直播清单，已真站验证）。
//!
//! 偏差（4 条，均记录在附录 EL）：
//! 1. 原类用「伪设备串 + RC4 + MD5」生成 `token/hash`；桌面端固定 `token=""`、
//!    `hash=md5("null"+version+ts)[8..12]`，2026-11 实测各接口均 200（真站验证见单测 `#[ignore]`）。
//! 2. 原类 `homeVideoContent` 把同一首页接口重复拉 4 次，这里只拉 1 次（数据同源，行为等价）。
//! 3. 播放结果不含原类的 `Proxy.getUrl()?do=diydanmu` 弹幕地址（桌面无 `Proxy` 代理）。
//! 4. `ext` 覆盖基址仅作测试钩子（原类 `init` 不读 ext）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_LiteApple";
pub const DEFAULT_BASE: &str = "http://item.xpgtv.com";
/// 播放 CDN（原类 `playerContent` 里硬编码）。
const CDN_BASE: &str = "http://c.xpgtv.net";
/// 设备/版本串（`d()` 里拼接的版本号，服务端校验 `hash` 时使用）。
const VERSION: &str = "XPGBOX com.phoenix.tv1.5.5";
/// 固定 token2（原类常量）。
const TOKEN2: &str = "XFxIummRrngadHB4TCzeUaleebTX10Vl/ftCvGLPeI5tN2Y/liZ5tY5e4t8=";
/// 原类 UA。
const UA: &str = "okhttp/3.12.11";
/// 详情线路名（原类 `vod_play_from` 固定值）。
const GROUP: &str = "LiteApple";
/// 首页推荐最多条目数（原类 `jSONArray.length() > 30` 即停）。
const HOME_MAX: usize = 30;
/// 每组首页推荐最多取 4 条（原类 `i6 < i4`，`i4 = 4`）。
const HOME_GROUP_MAX: usize = 4;
/// 首页推荐最多遍历前 4 组（原类 `i >= 5` 即停）。
const HOME_GROUP_COUNT: usize = 4;

/// 是否属于本家族（大小写不敏感，与原类 `api` 声明一致）。
pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

/// 基址：`ext` 含 http 时覆盖（测试/镜像钩子），否则用默认线上地址。
pub fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let ext = ext.trim();
    for token in ext.split(['#', ';', '|']) {
        let token = token.trim();
        if token.starts_with("http://") || token.starts_with("https://") {
            return token.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

/// 原类 `d()` 的请求头。`token` 固定空串、`hash` 按「null + 版本 + 秒级时间戳」取 MD5 第 8~12 位。
pub fn headers() -> HashMap<String, String> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seed = format!("null{VERSION}{ts}");
    let digest = format!("{:x}", md5::compute(seed.as_bytes()));
    let hash = digest.get(8..12).unwrap_or("").to_string();
    let mut map = HashMap::new();
    map.insert("token".to_string(), String::new());
    map.insert("token2".to_string(), TOKEN2.to_string());
    map.insert("user_id".to_string(), "XPGBOX".to_string());
    map.insert("version".to_string(), VERSION.to_string());
    map.insert("timestamp".to_string(), ts.to_string());
    map.insert("hash".to_string(), hash);
    map.insert("screenx".to_string(), "2331".to_string());
    map.insert("screeny".to_string(), "1121".to_string());
    map.insert("User-Agent".to_string(), UA.to_string());
    map
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn fetch(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(), 2)
}

fn get_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let text = fetch(cfg, url)?;
    serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("LiteApple JSON 解析失败: {e}; url={url}")))
}

/// 接口 URL：`{base}/api.php/{path}`。
fn api_url(base: &str, path: &str) -> String {
    format!("{base}/api.php/{path}")
}

/// 列表页 URL：`androidfilter?page=&type=` + 非空筛选项（按 key 排序，保证可复现）。
pub fn list_url(base: &str, type_id: &str, page: u32, filter: &HashMap<String, String>) -> String {
    let mut url = format!("{}/?page={}&type={}", api_url(base, "v2.vod/androidfilter"), page, urlencode(type_id));
    let mut keys: Vec<&String> = filter.keys().collect();
    keys.sort();
    for key in keys {
        let value = filter.get(key).map(|v| v.trim()).unwrap_or("");
        if value.is_empty() {
            continue;
        }
        url.push('&');
        url.push_str(key);
        url.push('=');
        url.push_str(&urlencode(value));
    }
    url
}

/// 搜索 URL（原类固定 `page=1`）。
pub fn search_url(base: &str, keyword: &str) -> String {
    format!(
        "{}?page=1&wd={}",
        api_url(base, "v2.vod/androidsearch10086"),
        urlencode(keyword)
    )
}

/// 详情 URL。
pub fn detail_url(base: &str, id: &str) -> String {
    format!(
        "{}?vod_id={}",
        api_url(base, "v3.vod/androiddetail2"),
        urlencode(id)
    )
}

/// 播放 URL（详情 `urls[].url` 即 `{token}`）。
pub fn play_url(token: &str) -> String {
    format!("{CDN_BASE}/m3u8/{token}.m3u8")
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn str_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .or_else(|| v.as_f64().map(|n| n.to_string()))
        })
        .unwrap_or_default()
}

/// 分类 `data[]` → `Category`。
pub fn parse_categories(data: &Value) -> Vec<Category> {
    data.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let type_id = str_of(row, "type_id");
                    let type_name = str_of(row, "type_name");
                    if type_id.is_empty() || type_name.is_empty() {
                        return None;
                    }
                    Some(Category { type_id, type_name })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 列表 `data[]` → `VideoItem`。
pub fn parse_items(data: &Value) -> Vec<VideoItem> {
    data.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let id = str_of(row, "id");
                    let name = str_of(row, "name");
                    if id.is_empty() || name.is_empty() {
                        return None;
                    }
                    let pic = str_of(row, "pic");
                    let remark = str_of(row, "updateInfo");
                    Some(VideoItem {
                        id,
                        name,
                        cover: (!pic.is_empty()).then_some(pic),
                        remark: (!remark.is_empty()).then_some(remark),
                        ..Default::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 首页推荐 `data.list[]`：前 4 组各取前 4 条，总计不超过 30 条。
pub fn parse_home(data: &Value) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let groups = data.get("list").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for group in groups.iter().take(HOME_GROUP_COUNT) {
        let rows = group.get("list").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for row in rows.iter().take(HOME_GROUP_MAX) {
            if out.len() >= HOME_MAX {
                return out;
            }
            let id = str_of(row, "id");
            let name = str_of(row, "name");
            if id.is_empty() || name.is_empty() {
                continue;
            }
            let pic = str_of(row, "pic");
            let remark = str_of(row, "updateInfo");
            out.push(VideoItem {
                id,
                name,
                cover: (!pic.is_empty()).then_some(pic),
                remark: (!remark.is_empty()).then_some(remark),
                ..Default::default()
            });
        }
    }
    out
}

/// 详情 `data` → `VideoDetail`：`urls[]` 全量收进单线路 `LiteApple`。
pub fn parse_detail(data: &Value, id: &str) -> Option<VideoDetail> {
    let name = str_of(data, "name");
    if name.is_empty() {
        return None;
    }
    let pic = str_of(data, "pic");
    let content = str_of(data, "content");
    let compact: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let year = str_of(data, "year");
    let area = str_of(data, "area");
    let director = str_of(data, "director");
    let actor = str_of(data, "actor");
    let remark = str_of(data, "updateInfo");
    let class_name = str_of(data, "className");
    let mut desc_parts = Vec::new();
    if !compact.is_empty() {
        desc_parts.push(compact);
    }
    for (label, value) in [("分类", class_name), ("更新", remark)] {
        if !value.is_empty() {
            desc_parts.push(format!("{label}：{value}"));
        }
    }
    let urls = data.get("urls").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let items: Vec<Episode> = urls
        .iter()
        .filter_map(|row| {
            let key = str_of(row, "key");
            let url = str_of(row, "url");
            if url.is_empty() {
                return None;
            }
            let title = if key.is_empty() { url.clone() } else { key };
            Some(Episode { title, url })
        })
        .collect();
    Some(VideoDetail {
        id: id.to_string(),
        name,
        cover: (!pic.is_empty()).then_some(pic),
        desc: (!desc_parts.is_empty()).then_some(desc_parts.join("\n")),
        year: (!year.is_empty()).then_some(year),
        area: (!area.is_empty()).then_some(area),
        director: (!director.is_empty()).then_some(director),
        actor: (!actor.is_empty()).then_some(actor),
        episodes: if items.is_empty() {
            Vec::new()
        } else {
            vec![EpisodeGroup { group: GROUP.to_string(), items }]
        },
    })
}

/// 搜索 `data[]` → 只保留名称包含关键词的行（原类 `string.contains(str)`）。
pub fn parse_search(data: &Value, keyword: &str) -> Vec<VideoItem> {
    parse_items(data)
        .into_iter()
        .filter(|item| item.name.contains(keyword))
        .collect()
}

/// 播放结果：`token` 含 `http` 时按直链返回，否则拼 `{cdn}/m3u8/{token}.m3u8`。
pub fn player_json(token: &str) -> Value {
    let token = token.trim();
    let url = if token.to_ascii_lowercase().contains("http") {
        token.to_string()
    } else {
        play_url(token)
    };
    let header = serde_json::to_string(&headers()).unwrap_or_else(|_| "{}".to_string());
    json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": header,
    })
}

/* ============================================================
 * TVBox 五入口
 * ============================================================ */

/// 分类：拉 `androidtypes`；失败时返回空（与原类 catch 一致）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Vec<Category> {
    let base = base_of(site);
    match get_json(cfg, site, &api_url(&base, "v2.vod/androidtypes")) {
        Ok(value) => parse_categories(value.get("data").unwrap_or(&Value::Null)),
        Err(_) => Vec::new(),
    }
}

/// 列表：`androidfilter?page=&type=` + 非空筛选项。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
    filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let value = get_json(cfg, site, &list_url(&base, type_id, page, filter))?;
    Ok(parse_items(value.get("data").unwrap_or(&Value::Null)))
}

/// 详情：`androiddetail2?vod_id=`；无播放地址时报明确错误。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let value = get_json(cfg, site, &detail_url(&base, id))?;
    let data = value.get("data").unwrap_or(&Value::Null);
    let detail = parse_detail(data, id)
        .ok_or_else(|| parse_err(site, format!("LiteApple 详情缺少 data.name（vod_id={id}）")))?;
    if detail.episodes.is_empty() {
        return Err(parse_err(
            site,
            format!("LiteApple 条目无播放地址（vod_id={id}，可能已下架）"),
        ));
    }
    Ok(detail)
}

/// 搜索：`androidsearch10086?page=1&wd=`，按名称包含过滤。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let value = get_json(cfg, site, &search_url(&base, keyword))?;
    Ok(parse_search(
        value.get("data").unwrap_or(&Value::Null),
        keyword,
    ))
}

/// 播放：`token` 即详情 `urls[].url`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let _ = (cfg, site);
    let token = id.trim();
    if token.is_empty() {
        return Err(parse_err(site, "LiteApple 播放 id 为空"));
    }
    Ok(player_json(token))
}

/* ============================================================
 * 单测
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with(ext: &str) -> Site {
        Site {
            key: "liteapple".to_string(),
            name: "小苹果".to_string(),
            kind: 3,
            api: API.to_string(),
            ext: Some(serde_json::Value::String(ext.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_LiteApple"));
        assert!(handles("csp_liteapple"));
        assert!(!handles("csp_LiteAppleX"));
        assert!(!handles("csp_Other"));
    }

    #[test]
    fn base_of_uses_default_or_ext_http() {
        assert_eq!(base_of(&site_with("")), DEFAULT_BASE);
        assert_eq!(base_of(&site_with("http://127.0.0.1:9")), "http://127.0.0.1:9");
        assert_eq!(
            base_of(&site_with("a###https://mirror.example.com/###b")),
            "https://mirror.example.com"
        );
        assert_eq!(base_of(&site_with("not-a-url")), DEFAULT_BASE);
    }

    #[test]
    fn headers_carry_expected_constants() {
        let h = headers();
        assert_eq!(h.get("token").map(String::as_str), Some(""));
        assert_eq!(h.get("user_id").map(String::as_str), Some("XPGBOX"));
        assert_eq!(h.get("version").map(String::as_str), Some(VERSION));
        assert_eq!(h.get("User-Agent").map(String::as_str), Some(UA));
        let hash = h.get("hash").expect("hash");
        assert_eq!(hash.len(), 4, "hash 取 MD5 第 8~12 位：{hash}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn list_url_appends_sorted_filters_and_skips_empty() {
        let mut filter = HashMap::new();
        filter.insert("year".to_string(), "2024".to_string());
        filter.insert("area".to_string(), "大陆".to_string());
        filter.insert("sortby".to_string(), String::new());
        let url = list_url("http://x", "1", 3, &filter);
        assert!(url.starts_with("http://x/api.php/v2.vod/androidfilter/?page=3&type=1&"), "{url}");
        assert!(url.ends_with("&area=%E5%A4%A7%E9%99%86&year=2024"), "筛选项按 key 排序且跳过空值：{url}");
        assert!(!url.contains("sortby"), "{url}");
    }

    #[test]
    fn search_and_detail_urls_are_stable() {
        assert_eq!(
            search_url("http://item.xpgtv.com", "三体"),
            "http://item.xpgtv.com/api.php/v2.vod/androidsearch10086?page=1&wd=%E4%B8%89%E4%BD%93"
        );
        assert_eq!(
            detail_url("http://item.xpgtv.com", "132221"),
            "http://item.xpgtv.com/api.php/v3.vod/androiddetail2?vod_id=132221"
        );
    }

    #[test]
    fn parse_categories_requires_id_and_name() {
        let data = json!([
            {"type_id": 1, "type_name": "电影"},
            {"type_id": 2, "type_name": "电视剧"},
            {"type_id": 3},
            {"type_name": "无 id"}
        ]);
        let cats = parse_categories(&data);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "2");
    }

    #[test]
    fn parse_items_maps_optional_fields() {
        let data = json!([
            {"id": 10, "name": "影片", "pic": "http://p/1.jpg", "updateInfo": "更新至2集"},
            {"id": 11, "name": "无封面"}
        ]);
        let items = parse_items(&data);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "10");
        assert_eq!(items[0].cover.as_deref(), Some("http://p/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新至2集"));
        assert!(items[1].cover.is_none());
        assert!(items[1].remark.is_none());
    }

    #[test]
    fn parse_home_takes_four_groups_of_four() {
        let mut groups = Vec::new();
        for g in 0..6 {
            let rows: Vec<Value> = (0..6)
                .map(|i| json!({"id": g * 100 + i, "name": format!("g{g}-{i}")}))
                .collect();
            groups.push(json!({"title": format!("组{g}"), "list": rows}));
        }
        let data = json!({ "list": groups });
        let items = parse_home(&data);
        assert_eq!(items.len(), 16, "4 组 × 每组 4 条");
        assert_eq!(items[0].name, "g0-0");
        assert_eq!(items[15].name, "g3-3");
    }

    #[test]
    fn parse_detail_collects_urls_into_single_group() {
        let data = json!({
            "id": 132221,
            "name": " 另一个妈妈 ",
            "pic": "http://p/x.jpg",
            "year": "2026",
            "area": "大陆",
            "director": "导演A",
            "actor": "演员B",
            "className": "恐怖",
            "updateInfo": "更新至1集",
            "content": "  故事  讲述…  ",
            "urls": [
                {"type": 1, "key": "蓝光", "url": "V1RjSllm"},
                {"type": 1, "key": "", "url": "abc"}
            ]
        });
        let detail = parse_detail(&data, "132221").expect("detail");
        assert_eq!(detail.name, " 另一个妈妈 ");
        assert_eq!(detail.year.as_deref(), Some("2026"));
        assert_eq!(detail.area.as_deref(), Some("大陆"));
        assert_eq!(detail.director.as_deref(), Some("导演A"));
        assert_eq!(detail.actor.as_deref(), Some("演员B"));
        let desc = detail.desc.expect("desc");
        assert!(desc.contains("故事 讲述…"), "{desc}");
        assert!(desc.contains("分类：恐怖"), "{desc}");
        assert!(desc.contains("更新：更新至1集"), "{desc}");
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, GROUP);
        assert_eq!(detail.episodes[0].items[0].title, "蓝光");
        assert_eq!(detail.episodes[0].items[0].url, "V1RjSllm");
        assert_eq!(detail.episodes[0].items[1].title, "abc", "key 为空时用 url 当标题");
    }

    #[test]
    fn parse_detail_without_urls_has_no_episodes() {
        let data = json!({"id": 1, "name": "无资源", "urls": []});
        let detail = parse_detail(&data, "1").expect("detail");
        assert!(detail.episodes.is_empty());
        assert!(parse_detail(&json!({"id": 1}), "1").is_none(), "缺 name → None");
    }

    #[test]
    fn parse_search_keeps_only_name_matches() {
        let data = json!([
            {"id": 1, "name": "三体"},
            {"id": 2, "name": "三体前传"},
            {"id": 3, "name": "狂飙"}
        ]);
        let items = parse_search(&data, "三体");
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.name.contains("三体")));
    }

    #[test]
    fn player_json_uses_cdn_or_direct_url() {
        let value = player_json("V1RjSllm");
        assert_eq!(value["parse"], 0);
        assert_eq!(value["playUrl"], "");
        assert_eq!(value["url"], format!("{CDN_BASE}/m3u8/V1RjSllm.m3u8"));
        assert!(value["header"].as_str().unwrap_or("").contains("token2"));

        let direct = player_json("https://cdn.example.com/a.m3u8");
        assert_eq!(direct["url"], "https://cdn.example.com/a.m3u8");
        let upper = player_json("HTTP://cdn.example.com/b.m3u8");
        assert_eq!(upper["url"], "HTTP://cdn.example.com/b.m3u8", "大小写不敏感");
    }
}
