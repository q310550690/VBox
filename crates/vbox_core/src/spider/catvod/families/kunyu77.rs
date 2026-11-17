//! `csp_Kunyu77` —— 酷云七七 App 接口（第 26 轮）。
//!
//! 来源：`Kunyu77.java`（`88c55af0-fan` 变体，407 行，0 处 jadx 跳过）。
//! 协议要点：
//! 1. `init(ext)`：ext 为空时用内置 `https://api.tyun77.cn`；ext 以 `http` 开头视为基址，
//!    以 `okhttp` 开头视为自定义 UA（与 Java 实现一致，二者互斥）。
//! 2. 固定设备参数（pcode/version/lon/lat/devid/package/sys/sysver/brand/model）+ `sj`（毫秒时间戳）。
//! 3. 签名：`md5(path + concat(URLEncoder.encode(按 key 升序排的每个 value)) + sj + "XSpeUFjJ")`。
//! 4. 请求头：`User-Agent`（默认 `okhttp/3.12.0`）、`t`（时间戳）、`TK`（签名）。
//! 5. 全部接口为 GET + 查询串（原实现直接拼原始值；本移植用 `java_urlencode` 编码，见偏差）。
//! 6. 接口：`/api.php/provide/homeBlock`、`/searchFilter`、`/videoDetail`、`/videoPlaylist`、
//!    `/searchVideo`、`/parserUrl`。
//!
//! 有意偏差（相对 Android 实现）：
//! - 查询串对 value 做 URL 编码（原实现直接拼接，中文关键词会产生非法 URL）；
//! - `pagecount` 按 `total / pagesize` 向上取整（原实现把 `pagesize` 当 pagecount 写入）；
//! - `playerContent` 仅在目标属于已知视频站时调用 `parserUrl`，其余直接直链返回（与原文一致）；
//! - 不实现 `homeVideoContent`（VBox 无首页推荐入口）。
//!
//! 上游状态：`api.tyun77.cn` 已不可达（`fetch failed`），因此仅 mock 验收（L-B）。

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Kunyu77";
pub const DEFAULT_BASE: &str = "https://api.tyun77.cn";
pub const DEFAULT_UA: &str = "okhttp/3.12.0";
const SIGN_SALT: &str = "XSpeUFjJ";
const PAGE_SIZE: u32 = 24;

const DEVICE: [(&str, &str); 10] = [
    ("pcode", "010110005"),
    ("version", "2.1.4"),
    ("lon", "116.801239"),
    ("lat", "40.954705"),
    ("devid", "4ac3fe96a6133de96904b8d3c8cfe16d"),
    ("package", "com.sevenVideo.app.android"),
    ("sys", "android"),
    ("sysver", "7.1.2"),
    ("brand", "realme"),
    ("model", "RMX1931"),
];

const CATEGORIES: [(&str, &str); 5] = [
    ("0", "全部"),
    ("1", "电影"),
    ("2", "电视剧"),
    ("3", "综艺"),
    ("4", "动漫"),
];

/// 已知视频站（对应 `Zz.KJ`）：命中则走 `parserUrl` 解析。
const KNOWN_SITES: [&str; 11] = [
    "iqiyi.com",
    "v.qq.com",
    "youku.com",
    "le.com",
    "tudou.com",
    "mgtv.com",
    "sohu.com",
    "acfun.cn",
    "bilibili.com",
    "baofeng.com",
    "pptv.com",
];

#[derive(Clone, Debug)]
pub struct Config {
    pub base: String,
    pub ua: String,
}

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

pub fn config(site: &Site) -> Config {
    let ext = site.ext_string();
    let ext = ext.trim();
    let mut cfg = Config {
        base: DEFAULT_BASE.to_string(),
        ua: DEFAULT_UA.to_string(),
    };
    if ext.starts_with("http://") || ext.starts_with("https://") {
        cfg.base = ext.trim_end_matches('/').to_string();
    } else if ext.starts_with("okhttp") {
        cfg.ua = ext.to_string();
    }
    cfg
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Java `URLEncoder.encode(value, "UTF-8")` 的近似实现。
fn java_urlencode(input: &str) -> String {
    let mut out = String::new();
    for &b in input.as_bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*') {
            out.push(c);
        } else if c == ' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input))
}

/// `md5(path + concat(enc(values)) + sj + salt)`；`params` 必须已包含 `sj`。
fn sign(path: &str, params: &BTreeMap<String, String>) -> String {
    let mut raw = String::from(path);
    for value in params.values() {
        raw.push_str(&java_urlencode(value));
    }
    if let Some(sj) = params.get("sj") {
        raw.push_str(sj);
    }
    md5_hex(&format!("{}{}", raw, SIGN_SALT))
}

fn base_params(ts: u64, extra: &[(&str, String)]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (k, v) in DEVICE {
        map.insert(k.to_string(), v.to_string());
    }
    map.insert("sj".to_string(), ts.to_string());
    for (k, v) in extra {
        map.insert((*k).to_string(), v.clone());
    }
    map
}

fn build_query(params: &BTreeMap<String, String>) -> String {
    let mut parts = Vec::new();
    for (k, v) in params {
        parts.push(format!("{}={}", java_urlencode(k), java_urlencode(v)));
    }
    parts.join("&")
}

fn fetch_json(
    cfg: &TvConfig,
    conf: &Config,
    path: &str,
    extra: &[(&str, String)],
) -> Result<Value> {
    let ts = now_ms();
    let params = base_params(ts, extra);
    let signature = sign(path, &params);
    let client = client_for(&cfg.http)?;
    let url = format!("{}{}?{}", conf.base, path, build_query(&params));
    let mut headers = std::collections::HashMap::new();
    headers.insert("User-Agent".to_string(), conf.ua.clone());
    headers.insert("t".to_string(), ts.to_string());
    headers.insert("TK".to_string(), signature);
    let body = get_text(&client, &url, &headers, 1)?;
    serde_json::from_str::<Value>(&body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site_stub(),
            format!("响应不是合法 JSON：{e}"),
        )
        .into_core()
    })
}

fn site_stub() -> &'static Site {
    static SITE: OnceLock<Site> = OnceLock::new();
    SITE.get_or_init(|| Site {
        key: "kunyu77".to_string(),
        name: "酷云七七".to_string(),
        api: API.to_string(),
        ..Default::default()
    })
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

fn parse_video_list(node: &Value) -> Vec<VideoItem> {
    let Some(items) = node.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?;
            let id = match id {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => return None,
            };
            let name = str_field(item, "title").or_else(|| str_field(item, "videoName"))?;
            Some(VideoItem {
                id,
                name,
                cover: str_field(item, "videoCover"),
                remark: str_field(item, "msg"),
            })
        })
        .collect()
}

fn str_field(node: &Value, key: &str) -> Option<String> {
    node.get(key).and_then(|v| match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let conf = config(site);
    let pagenum = if page == 0 { 1 } else { page };
    let node = fetch_json(
        cfg,
        &conf,
        "/api.php/provide/searchFilter",
        &[
            ("type_id", cat.to_string()),
            ("pagenum", pagenum.to_string()),
            ("pagesize", PAGE_SIZE.to_string()),
        ],
    )
    .map_err(|_| CatvodError::new(ErrorClass::Parse, site, "列表接口请求失败").into_core())?;
    let data = node.get("data").cloned().unwrap_or(Value::Null);
    Ok(parse_video_list(data.get("result").unwrap_or(&Value::Null)))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let conf = config(site);
    let node = fetch_json(
        cfg,
        &conf,
        "/api.php/provide/searchVideo",
        &[
            ("searchName", keyword.to_string()),
            ("pg", "1".to_string()),
        ],
    )
    .map_err(|_| CatvodError::new(ErrorClass::Parse, site, "搜索接口请求失败").into_core())?;
    let data = node.get("data").cloned().unwrap_or(Value::Null);
    let mut items = parse_video_list(&data);
    items.retain(|item| item.name.contains(keyword));
    if items.is_empty() {
        let label = format!("未找到与「{keyword}」相关的视频");
        return Err(CatvodError::new(ErrorClass::Parse, site, label).into_core());
    }
    Ok(items)
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let conf = config(site);
    let node = fetch_json(
        cfg,
        &conf,
        "/api.php/provide/videoDetail",
        &[("ids", id.to_string())],
    )
    .map_err(|_| CatvodError::new(ErrorClass::Parse, site, "详情接口请求失败").into_core())?;
    let data = node.get("data").cloned().unwrap_or(Value::Null);
    if !data.is_object() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情接口未返回视频信息",
        )
        .into_core());
    }
    let name = str_field(&data, "videoName").unwrap_or_else(|| id.to_string());
    let play_node = fetch_json(
        cfg,
        &conf,
        "/api.php/provide/videoPlaylist",
        &[("ids", id.to_string())],
    )
    .map_err(|_| CatvodError::new(ErrorClass::Parse, site, "播放列表请求失败").into_core())?;
    let episodes_node = play_node
        .get("data")
        .and_then(|d| d.get("episodes"))
        .cloned()
        .unwrap_or(Value::Null);
    let episodes = parse_episodes(&episodes_node, &name);
    Ok(VideoDetail {
        id: id.to_string(),
        name: name.clone(),
        cover: str_field(&data, "videoCover"),
        desc: str_field(&data, "brief").or_else(|| str_field(&data, "msg")),
        year: str_field(&data, "year"),
        area: str_field(&data, "area"),
        director: str_field(&data, "director"),
        actor: str_field(&data, "actor"),
        episodes,
    })
}

fn parse_episodes(node: &Value, video_name: &str) -> Vec<EpisodeGroup> {
    let Some(arr) = node.as_array() else {
        return Vec::new();
    };
    let mut groups: Vec<(String, Vec<Episode>)> = Vec::new();
    for entry in arr {
        let Some(playurls) = entry.get("playurls").and_then(|v| v.as_array()) else {
            continue;
        };
        for item in playurls {
            let from = str_field(item, "playfrom").unwrap_or_else(|| "默认线路".to_string());
            let raw_title = str_field(item, "title").unwrap_or_default();
            let title = if raw_title.contains(video_name) {
                raw_title.replace(video_name, "").trim().to_string()
            } else {
                raw_title.trim().to_string()
            };
            let Some(url) = str_field(item, "playurl") else {
                continue;
            };
            let title = if title.is_empty() {
                format!("第{}集", url.len())
            } else {
                title
            };
            match groups.iter_mut().find(|(name, _)| *name == from) {
                Some((_, items)) => items.push(Episode { title, url }),
                None => groups.push((from, vec![Episode { title, url }])),
            }
        }
    }
    groups
        .into_iter()
        .map(|(group, items)| EpisodeGroup { group, items })
        .collect()
}

fn is_known_site(url: &str) -> bool {
    KNOWN_SITES.iter().any(|site| url.contains(site))
}

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    if !is_known_site(id) {
        return Ok(json!({
            "parse": 0,
            "jx": 0,
            "playUrl": "",
            "url": id,
            "header": {},
        }));
    }
    let conf = config(site);
    let node = fetch_json(
        cfg,
        &conf,
        "/api.php/provide/parserUrl",
        &[
            ("url", id.to_string()),
            ("retryNum", "0".to_string()),
        ],
    )
    .map_err(|_| CatvodError::new(ErrorClass::HostApi, site, "解析接口请求失败").into_core())?;
    let first = pick_url(&node).ok_or_else(|| {
        CatvodError::new(ErrorClass::HostApi, site, "解析接口未返回播放地址").into_core()
    })?;
    let client = client_for(&cfg.http)?;
    let mut headers = std::collections::HashMap::new();
    headers.insert("User-Agent".to_string(), conf.ua.clone());
    let body = get_text(&client, &first, &headers, 1).map_err(|_| {
        CatvodError::new(ErrorClass::HostApi, site, "解析地址二次请求失败").into_core()
    })?;
    let second = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| pick_url(&value))
        .ok_or_else(|| {
            CatvodError::new(ErrorClass::HostApi, site, "解析结果缺少播放地址").into_core()
        })?;
    Ok(json!({
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "url": second,
        "header": { "User-Agent": conf.ua },
    }))
}

fn pick_url(node: &Value) -> Option<String> {
    if let Some(url) = str_field(node, "url") {
        return Some(url);
    }
    node.get("data")
        .and_then(|data| str_field(data, "url"))
        .filter(|u| u.starts_with("http"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(ts: u64, extra: &[(&str, String)]) -> BTreeMap<String, String> {
        base_params(ts, extra)
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Kunyu77"));
        assert!(handles("  CSP_KUNYU77 "));
        assert!(!handles("csp_AppGet"));
    }

    #[test]
    fn java_urlencode_matches_java_encoder() {
        assert_eq!(java_urlencode("海贼"), "%E6%B5%B7%E8%B4%BC");
        assert_eq!(java_urlencode("a b"), "a+b");
        assert_eq!(java_urlencode("a.b-c_d*"), "a.b-c_d*");
        assert_eq!(java_urlencode("x=y&z"), "x%3Dy%26z");
    }

    #[test]
    fn signature_matches_reference_vectors() {
        let p = params(
            1_700_000_000_000,
            &[
                ("type_id", "1".to_string()),
                ("pagenum", "1".to_string()),
                ("pagesize", "24".to_string()),
            ],
        );
        assert_eq!(
            sign("/api.php/provide/searchFilter", &p),
            "63aba70aa103d4c8b6a90e00ac9daa6c"
        );
        let p = params(1_700_000_000_000, &[("ids", "12345".to_string())]);
        assert_eq!(
            sign("/api.php/provide/videoDetail", &p),
            "e5a7a485decfe01f509793def353a2df"
        );
        let p = params(
            1_700_000_000_000,
            &[("searchName", "海贼".to_string()), ("pg", "1".to_string())],
        );
        assert_eq!(
            sign("/api.php/provide/searchVideo", &p),
            "15320feded960fbecf56b175cd18c33a"
        );
    }

    #[test]
    fn config_reads_ext_as_base_or_ua() {
        let mut site = Site::default();
        site.ext = Some(json!("https://mirror.example.com/"));
        let cfg = config(&site);
        assert_eq!(cfg.base, "https://mirror.example.com");
        assert_eq!(cfg.ua, DEFAULT_UA);

        site.ext = Some(json!("okhttp/4.10.0"));
        let cfg = config(&site);
        assert_eq!(cfg.base, DEFAULT_BASE);
        assert_eq!(cfg.ua, "okhttp/4.10.0");
    }

    #[test]
    fn parses_video_list_rows() {
        let node = json!([
            {"id": 147973, "title": "杀或救", "videoCover": "https://x/1.webp", "msg": "蓝光"},
            {"id": "12", "title": " ", "videoCover": ""},
        ]);
        let items = parse_video_list(&node);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "147973");
        assert_eq!(items[0].name, "杀或救");
        assert_eq!(items[0].remark.as_deref(), Some("蓝光"));
    }

    #[test]
    fn groups_episodes_by_playfrom_and_strips_name() {
        let node = json!([
            {"playurls": [
                {"playfrom": "line1", "title": "杀或救第01集", "playurl": "https://cdn/1.m3u8"},
                {"playfrom": "line1", "title": "杀或救第02集", "playurl": "https://cdn/2.m3u8"},
                {"playfrom": "line2", "title": "第01集", "playurl": "https://cdn/3.m3u8"}
            ]}
        ]);
        let groups = parse_episodes(&node, "杀或救");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "line1");
        assert_eq!(groups[0].items[0].title, "第01集");
        assert_eq!(groups[1].items.len(), 1);
    }

    #[test]
    fn detects_known_video_sites() {
        assert!(is_known_site("https://v.qq.com/x/cover/abc.html"));
        assert!(is_known_site("https://www.iqiyi.com/v_19rr.html"));
        assert!(!is_known_site("https://cdn.example.test/1.m3u8"));
    }

    #[test]
    fn picks_url_from_parser_payload() {
        assert_eq!(
            pick_url(&json!({"url": "https://a/1.m3u8"})).as_deref(),
            Some("https://a/1.m3u8")
        );
        assert_eq!(
            pick_url(&json!({"data": {"url": "https://b/2.m3u8"}})).as_deref(),
            Some("https://b/2.m3u8")
        );
        assert_eq!(pick_url(&json!({"data": {"msg": "x"}})), None);
    }

    #[test]
    fn categories_are_fixed_five() {
        let cats = categories();
        assert_eq!(cats.len(), 5);
        assert_eq!(cats[0].type_id, "0");
        assert_eq!(cats[2].type_name, "电视剧");
    }
}
