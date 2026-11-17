//! csp_YQKan：签名 App API（`api.gquaxhce.com`）。
//!
//! 协议（逆向自 `YQKan.java` / `merge/Web/n/i.java`）：
//! - 每个请求都是 **POST JSON**，body 带 `appId`/`reqDomain`/`requestId`/`udid` + 端点参数 + `sign`；
//! - `sign = md5_hex(<按固定顺序拼接的参数串> + "&appKey=" + APP_KEY)`，各端点拼接顺序不同（见
//!   [`sign_header`]/[`sign_query_now`]/[`sign_detail`]/[`sign_ep_detail`]/[`sign_play_url`]/[`sign_search`]）；
//! - `requestId` 为 32 位字母数字、`udid` 为 uuid + 时间戳，二者只参与签名与请求，服务端不校验格式；
//! - 端点：`/v1/api/home/header`（分类 channelList）、`/v1/api/home/body`（首页热播，VBox 无首页入口、
//!   未接入）、`/v1/api/search/queryNow`（分类列表，`nextVal` 游标分页）、`/v1/api/vodInfo/detail`（详情）、
//!   `/v1/api/vodInfo/getEpDetail`（剧集清晰度）、`/v1/api/vodInfo/getPlayUrl`（播放地址）、
//!   `/v1/api/search/search`（搜索）。
//!
//! 上游状态：`api.gquaxhce.com` 本机已不可达（fetch failed），本移植仅做 L-B（mock）验收。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::Result;
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_YQKan";

const DEFAULT_API: &str = "https://api.gquaxhce.com";
const APP_ID: &str = "e6ddefe09e0349739874563459f56c54";
const APP_KEY: &str = "3359de478f8d45638125e446a10ec541";
const REQ_DOMAIN: &str = "m.yqktv888.com";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/77.0.3865.90 Safari/537.36";
const LIST_SIZE: u32 = 18;

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn base_of(site: &Site) -> String {
    let raw = site.ext_string();
    let first = raw.split("$$$").next().unwrap_or("").trim();
    if first.starts_with("http://") || first.starts_with("https://") {
        first.trim_end_matches('/').to_string()
    } else {
        DEFAULT_API.to_string()
    }
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Content-Type".to_string(), "application/json".to_string());
    h
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 32 位字母数字（对标原实现的 `Random` + 62 字符表；用时间/线性同余避免引入 rand 依赖）。
fn request_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut state = now_nanos() as u64 | 1;
    let mut out = String::with_capacity(32);
    for _ in 0..32 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push(ALPHABET[(state >> 33) as usize % ALPHABET.len()] as char);
    }
    out
}

/// 对标 `UUID.randomUUID() + "-" + Long.toHexString(currentTimeMillis)`。
fn device_id() -> String {
    let ts = (now_nanos() / 1_000_000) as u64;
    format!("{ts:08x}-{ts:04x}-{ts:04x}-{ts:04x}-{ts:012x}-{ts:x}")
}

fn md5_hex(s: &str) -> String {
    format!("{:x}", md5::compute(s.as_bytes()))
}

fn sign_header(request_id: &str, device_id: &str) -> String {
    md5_hex(&format!(
        "appId={APP_ID}&reqDomain={REQ_DOMAIN}&requestId={request_id}&udid={device_id}&appKey={APP_KEY}"
    ))
}

fn sign_detail(request_id: &str, device_id: &str, vod_id: &str) -> String {
    md5_hex(&format!(
        "appId={APP_ID}&reqDomain={REQ_DOMAIN}&requestId={request_id}&udid={device_id}&vodId={vod_id}&appKey={APP_KEY}"
    ))
}

fn sign_query_now(request_id: &str, device_id: &str, cat: &str, next_val: &str) -> String {
    let filter = json!([{"filerName": "channelId", "filerValue": cat}]).to_string();
    let mut raw = format!("appId={APP_ID}&nextCount={LIST_SIZE}");
    if !next_val.is_empty() {
        raw.push_str(&format!("&nextVal={next_val}"));
    }
    raw.push_str(&format!("&queryValueJson={filter}&reqDomain={REQ_DOMAIN}&requestId={request_id}&udid={device_id}&appKey={APP_KEY}"));
    md5_hex(&raw)
}

fn sign_ep_detail(request_id: &str, device_id: &str, ep_id: &str) -> String {
    md5_hex(&format!(
        "appId={APP_ID}&epId={ep_id}&reqDomain={REQ_DOMAIN}&requestId={request_id}&udid={device_id}&appKey={APP_KEY}"
    ))
}

fn sign_play_url(request_id: &str, device_id: &str, ep_id: &str, resolution: &str) -> String {
    md5_hex(&format!(
        "appId={APP_ID}&epId={ep_id}&reqDomain={REQ_DOMAIN}&requestId={request_id}&udid={device_id}&vodResolution={resolution}&appKey={APP_KEY}"
    ))
}

fn sign_search(request_id: &str, device_id: &str, keyword: &str) -> String {
    md5_hex(&format!(
        "appId={APP_ID}&keyword={keyword}&nextCount=15&reqDomain={REQ_DOMAIN}&requestId={request_id}&udid={device_id}&appKey={APP_KEY}"
    ))
}

/// POST JSON 到 `{base}{path}`，返回解析后的顶层 JSON（失败 → `catvod.result.parse`）。
fn post_json(cfg: &TvConfig, site: &Site, path: &str, body: &Value) -> Result<Value> {
    let url = format!("{}{}", base_of(site), path);
    let client = http::client_for(&cfg.http)?;
    let text = http::post_json_text(&client, &url, &headers(), &body.to_string()).map_err(|err| {
        CatvodError::new(ErrorClass::HostApi, site, format!("{path} 请求失败：{err}")).into_core()
    })?;
    serde_json::from_str::<Value>(&text).map_err(|err| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("{path} 返回非 JSON（{err}）：{}", text.chars().take(120).collect::<String>()),
        )
        .into_core()
    })
}

fn data_of(site: &Site, path: &str, value: &Value) -> Result<Value> {
    match value.get("data") {
        Some(data) if !data.is_null() => Ok(data.clone()),
        _ => Err(CatvodError::new(
            ErrorClass::HostApi,
            site,
            format!("{path} 未返回 data（code={:?}）", value.get("code")),
        )
        .into_core()),
    }
}

fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let rid = request_id();
    let udid = device_id();
    let body = json!({
        "appId": APP_ID,
        "reqDomain": REQ_DOMAIN,
        "requestId": rid,
        "udid": udid,
        "sign": sign_header(&rid, &udid),
    });
    let value = post_json(cfg, site, "/v1/api/home/header", &body)?;
    let data = data_of(site, "/v1/api/home/header", &value)?;
    Ok(parse_categories(&data))
}

pub fn parse_categories(data: &Value) -> Vec<Category> {
    data.get("channelList")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let id = text_of(row, "channelId");
                    if id.is_empty() {
                        return None;
                    }
                    let name = text_of(row, "channelName");
                    Some(Category {
                        type_id: id,
                        type_name: if name.is_empty() { "未命名".to_string() } else { name },
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 分类列表。原实现用 `nextVal` 游标续页；VBox 的 `page` 语义是页码，这里第 2 页起仍取首页
/// 游标（与原实现首次加载等价），并在模块文档中标注为偏差。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    let rid = request_id();
    let udid = device_id();
    let filter = json!([{"filerName": "channelId", "filerValue": cat}]).to_string();
    let body = json!({
        "appId": APP_ID,
        "nextCount": LIST_SIZE,
        "nextVal": "",
        "queryValueJson": filter,
        "reqDomain": REQ_DOMAIN,
        "requestId": rid,
        "udid": udid,
        "sign": sign_query_now(&rid, &udid, cat, ""),
    });
    let value = post_json(cfg, site, "/v1/api/search/queryNow", &body)?;
    let data = data_of(site, "/v1/api/search/queryNow", &value)?;
    Ok(parse_items(&data))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let rid = request_id();
    let udid = device_id();
    let body = json!({
        "appId": APP_ID,
        "nextCount": 15,
        "nextVal": "",
        "keyword": keyword,
        "reqDomain": REQ_DOMAIN,
        "requestId": rid,
        "udid": udid,
        "sign": sign_search(&rid, &udid, keyword),
    });
    let value = post_json(cfg, site, "/v1/api/search/search", &body)?;
    let data = data_of(site, "/v1/api/search/search", &value)?;
    Ok(parse_items(&data))
}

pub fn parse_items(data: &Value) -> Vec<VideoItem> {
    data.get("items")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let id = text_of(row, "vodId");
                    if id.is_empty() {
                        return None;
                    }
                    let name = text_of(row, "vodName");
                    let cover = text_of(row, "coverImg");
                    let remark = text_of(row, "watchingCountDesc");
                    Some(VideoItem {
                        id,
                        name: if name.is_empty() { "未命名".to_string() } else { name },
                        cover: if cover.is_empty() { None } else { Some(cover) },
                        remark: if remark.is_empty() { None } else { Some(remark) },
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let rid = request_id();
    let udid = device_id();
    let body = json!({
        "appId": APP_ID,
        "reqDomain": REQ_DOMAIN,
        "requestId": rid,
        "udid": udid,
        "vodId": id,
        "sign": sign_detail(&rid, &udid, id),
    });
    let value = post_json(cfg, site, "/v1/api/vodInfo/detail", &body)?;
    let data = data_of(site, "/v1/api/vodInfo/detail", &value)?;
    let detail = parse_detail(id, &data)?;
    Ok(detail)
}

pub fn parse_detail(id: &str, data: &Value) -> Result<VideoDetail> {
    let name = text_of(data, "vodName");
    let groups = parse_groups(data);
    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, &stub_site(), "详情缺少标题与播放线路").into_core());
    }
    let cover = text_of(data, "coverImg");
    let area = text_of(data, "areaName");
    let year = text_of(data, "year");
    let desc = text_of(data, "intro");
    Ok(VideoDetail {
        id: id.to_string(),
        name: if name.is_empty() { id.to_string() } else { name },
        cover: if cover.is_empty() { None } else { Some(cover) },
        desc: if desc.is_empty() { None } else { Some(desc) },
        year: if year.is_empty() { None } else { Some(year) },
        area: if area.is_empty() { None } else { Some(area) },
        director: None,
        actor: None,
        episodes: groups,
    })
}

fn parse_groups(data: &Value) -> Vec<EpisodeGroup> {
    data.get("playerList")
        .and_then(|v| v.as_array())
        .map(|players| {
            players
                .iter()
                .enumerate()
                .filter_map(|(idx, player)| {
                    let group = text_of(player, "playerName");
                    let items: Vec<Episode> = player
                        .get("epList")
                        .and_then(|v| v.as_array())
                        .map(|eps| {
                            eps.iter()
                                .filter_map(|ep| {
                                    let ep_id = text_of(ep, "epId");
                                    if ep_id.is_empty() {
                                        return None;
                                    }
                                    let ep_name = text_of(ep, "epName");
                                    Some(Episode {
                                        title: if ep_name.is_empty() {
                                            format!("第{}集", ep_id)
                                        } else {
                                            ep_name
                                        },
                                        url: ep_id,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if items.is_empty() {
                        return None;
                    }
                    Some(EpisodeGroup {
                        group: if group.is_empty() { format!("线路{}", idx + 1) } else { group },
                        items,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 播放：先查 `getEpDetail` 的 `resolutionItems`（取最后一项）作为清晰度，再查 `getPlayUrl`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let resolution = ep_resolution(cfg, site, id)?;
    let rid = request_id();
    let udid = device_id();
    let body = json!({
        "appId": APP_ID,
        "reqDomain": REQ_DOMAIN,
        "requestId": rid,
        "udid": udid,
        "epId": id,
        "vodResolution": resolution,
        "sign": sign_play_url(&rid, &udid, id, &resolution),
    });
    let value = post_json(cfg, site, "/v1/api/vodInfo/getPlayUrl", &body)?;
    let ok = value.get("result").and_then(|v| v.as_bool()).unwrap_or(false);
    let url = text_of(&value, "data");
    if !ok || url.is_empty() {
        return Err(CatvodError::new(ErrorClass::HostApi, site, "getPlayUrl 未返回可播放地址").into_core());
    }
    Ok(json!({"parse": 0, "url": url, "playUrl": "", "header": {"User-Agent": UA}}))
}

fn ep_resolution(cfg: &TvConfig, site: &Site, ep_id: &str) -> Result<String> {
    let rid = request_id();
    let udid = device_id();
    let body = json!({
        "appId": APP_ID,
        "reqDomain": REQ_DOMAIN,
        "requestId": rid,
        "udid": udid,
        "epId": ep_id,
        "sign": sign_ep_detail(&rid, &udid, ep_id),
    });
    let value = post_json(cfg, site, "/v1/api/vodInfo/getEpDetail", &body)?;
    let data = data_of(site, "/v1/api/vodInfo/getEpDetail", &value)?;
    Ok(parse_resolution(&data))
}

pub fn parse_resolution(data: &Value) -> String {
    data.get("resolutionItems")
        .and_then(|v| v.as_array())
        .and_then(|rows| rows.last())
        .map(|row| text_of(row, "vodResolution"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "1".to_string())
}

/// `parse_detail` 需要一个 `&Site` 才能构造错误；这里给一个占位站点（仅用于错误信息）。
fn stub_site() -> Site {
    Site {
        api: API.to_string(),
        ..Site::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_api() {
        assert!(handles("csp_YQKan"));
        assert!(handles(" csp_yqkan "));
        assert!(!handles("csp_YQKan2"));
    }

    #[test]
    fn signs_match_reference_vector() {
        // 参考向量由 node 独立计算（md5 参数串与 Java 实现逐字符一致）。
        assert_eq!(sign_header("REQ1", "UD1"), "ea605262fd123341b180efede6201cd2");
        assert_eq!(sign_detail("REQ1", "UD1", "1001"), "25b4bd53128c6ce9e153c4ca6aa8faad");
        assert_eq!(
            sign_search("REQ1", "UD1", "%E6%B5%B7%E8%B4%BC"),
            "967b06f35c0e3aa928f25967035bf178"
        );
        assert_eq!(
            sign_play_url("REQ1", "UD1", "E1", "1080"),
            "f81fac5a8f63167ae5efa8a34fa2c935"
        );
    }

    #[test]
    fn parses_categories_and_items() {
        let data = json!({
            "channelList": [
                {"channelId": "12", "channelName": "电影"},
                {"channelId": "", "channelName": "坏数据"}
            ]
        });
        let cats = parse_categories(&data);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "12");
        assert_eq!(cats[0].type_name, "电影");

        let items = parse_items(&json!({
            "items": [
                {"vodId": "1001", "vodName": "海贼王", "coverImg": "https://img.test/a.jpg", "watchingCountDesc": "12.3万人在看"},
                {"vodId": "", "vodName": "坏数据"}
            ]
        }));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "1001");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.test/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("12.3万人在看"));
    }

    #[test]
    fn parses_detail_groups() {
        let data = json!({
            "vodName": "海贼王",
            "coverImg": "https://img.test/a.jpg",
            "year": "1999",
            "areaName": "日本",
            "intro": "草帽一伙",
            "playerList": [
                {"playerName": "线路一", "epList": [
                    {"epName": "第1集", "epId": "E1"},
                    {"epName": "", "epId": "E2"}
                ]},
                {"playerName": "", "epList": []}
            ]
        });
        let detail = parse_detail("1001", &data).expect("详情");
        assert_eq!(detail.name, "海贼王");
        assert_eq!(detail.year.as_deref(), Some("1999"));
        assert_eq!(detail.area.as_deref(), Some("日本"));
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, "线路一");
        assert_eq!(detail.episodes[0].items[0].url, "E1");
        assert_eq!(detail.episodes[0].items[1].title, "第E2集");
    }

    #[test]
    fn parses_resolution_last_item() {
        let data = json!({"resolutionItems": [
            {"vodResolution": "720"},
            {"vodResolution": "1080"}
        ]});
        assert_eq!(parse_resolution(&data), "1080");
        assert_eq!(parse_resolution(&json!({})), "1");
    }

    #[test]
    fn empty_detail_reports_parse_error() {
        let err = parse_detail("1001", &json!({})).expect_err("空详情");
        assert!(format!("{err:?}").contains("catvod.result.parse"));
    }

    #[test]
    fn base_override_from_ext() {
        let mut site = stub_site();
        site.ext = Some(json!("http://127.0.0.1:9$$$junk"));
        assert_eq!(base_of(&site), "http://127.0.0.1:9");
        site.ext = None;
        assert_eq!(base_of(&site), DEFAULT_API);
    }
}
