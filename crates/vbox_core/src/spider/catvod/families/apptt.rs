//! `csp_AppTT` 原生实现（App 影视自助采集站，JSON API）。
//!
//! 语义来自静态反编译：`_rea/decompiled/a4d0f912-_1/deobf/com/github/catvod/spider/AppTT.java`
//! （244 行，`extends Spider`，自包含，无字符串混淆）。类内基址**写死**
//! `http://110.42.1.95:7788`（私有 IP，公开网络不可达），因此本实现允许站点 `ext`
//! 首段以 `http(s)` 覆盖基址，用于 mock/镜像验收；这是相对原实现的唯一功能性偏差，
//! 默认行为与 Android 一致。
//!
//! 协议（全部 GET，JSON）：
//! 1. 分类静态 4 个：1 电影 / 2 连续剧 / 3 动漫 / 4 综艺。
//! 2. 列表 `{base}/api.php/v1.vod?type={cat}&page={page}&pagesize=20` →
//!    `data{page,total,list:[{vod_id,vod_name,vod_pic,vod_remarks}]}`。
//! 3. 详情 `{base}/api.php/v1.vod/detilldata?vod_id={id}` →
//!    `data{videoInfo{ vod_id,vod_name,vod_pic,vod_year,vod_area,vod_remarks,
//!    vod_actor,vod_director,vod_content }, source:[{player_info{show,parse}, list:[…]}]}`。
//!    `list` 元素多为字符串 `标题$地址`：地址含 `.m3u8`/`.mp4` 时直出，否则用该线路的
//!    `parse`（解析器前缀）拼在地址前；个别元素是对象（`name`/`url`）。
//!    线路分组名取 `player_info.show`（缺省 `线路N`），组内 `#`、组间 `$$$`。
//! 4. 搜索 `{base}/api.php/v1.vod?wd={URL 编码关键词}`（与列表同一端点）。
//! 5. 播放 `playerContent`：地址含 `.m3u8`/`.mp4` 直接返回；否则
//!    `GET {地址}&sc=1&token={token}`，`token = Base64(AES-256-CTR(毫秒时间戳))`，
//!    响应为 `Base64(AES-256-CTR(JSON))`，解密后取 `url`。
//!    密钥 `SRK#e%4UYtU#KiEo*vsPqpr0cC4bxAQW`（32 字节 → AES-256）、
//!    初始计数器 `o6sYmm*x5hn#rcCt`（16 字节），JCE `AES/CTR/PKCS5Padding`
//!    （CTR 无填充，等价于纯 CTR 流）。
//! 6. `User-Agent` 按**URL 形态**选择（`zh()`）：
//!    含 `api.php/app|xgapp|freekan` → `Dart/2.14 (dart:io)`；
//!    含 `zsb|fkxs|xays|xcys|szys|dxys|ytys|qnys` → `Dart/2.15 (dart:io)`；
//!    含 `.vod` → `okhttp/4.1.0`（本项目默认基址命中此分支）；否则 `Dalvik/2.1.0`。
//!
//! 相对原实现的有意偏差：
//! - `ext` 首段可覆盖基址（仅用于 mock/自建镜像；默认仍写死私有 IP）；
//! - 首页推荐 `homeVideoContent`（`/api.php/v1.vod/curnavitemlist?type_id=0`）未接入
//!   VBox 模型（VBox 无对应入口）；
//! - `categoryContent` 的 `pagecount/total` 不承载（VBox 侧由分页探测驱动）；
//! - 详情 `source[].list` 的纯对象元素按 `name`/`url` 直接映射，不保留 Android 那种
//!   `toString()` 原始 JSON 片段。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes256;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_AppTT";
/// 原实现写死的基址（私有 IP，公网不可达）。
pub const DEFAULT_BASE: &str = "http://110.42.1.95:7788";

const AES_KEY: &str = "SRK#e%4UYtU#KiEo*vsPqpr0cC4bxAQW";
const AES_IV: &str = "o6sYmm*x5hn#rcCt";
const PAGE_SIZE: u32 = 20;

/// api 名匹配（大小写不敏感、忽略首尾空白）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 请求
 * ============================================================ */

/// 基址：`ext` 首段为 `http(s)` 时覆盖（mock/镜像），否则用原实现写死的私有 IP。
fn ext_base(site: &Site) -> String {
    let ext = site.ext_string();
    if let Some(first) = ext.split("$$$").next() {
        let trimmed = first.trim().trim_end_matches('/');
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return trimmed.to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

/// `zh(url)`：按 URL 形态选择 UA（与原实现逐分支对齐）。
pub fn ua_for(url: &str) -> &'static str {
    if url.contains("api.php/app") || url.contains("xgapp") || url.contains("freekan") {
        "Dart/2.14 (dart:io)"
    } else if url.contains("zsb")
        || url.contains("fkxs")
        || url.contains("xays")
        || url.contains("xcys")
        || url.contains("szys")
        || url.contains("dxys")
        || url.contains("ytys")
        || url.contains("qnys")
    {
        "Dart/2.15 (dart:io)"
    } else if url.contains(".vod") {
        "okhttp/4.1.0"
    } else {
        "Dalvik/2.1.0"
    }
}

fn headers(url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), ua_for(url).to_string());
    map
}

fn fetch_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let client = crate::http::client_for(&cfg.http)?;
    let text = crate::http::get_text(&client, url, &headers(url), 1)?;
    serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("响应不是 JSON（{e}）: {url}"),
        )
        .into_core()
    })
}

/* ============================================================
 * AES-256-CTR（JCE `AES/CTR/PKCS5Padding`：CTR 流模式，无填充）
 * ============================================================ */

/// 计数器自增（大端 128 位），与 JCE `CounterMode` 的 IV 递增一致。
fn increment(counter: &mut [u8; 16]) {
    for byte in counter.iter_mut().rev() {
        let (next, overflow) = byte.overflowing_add(1);
        *byte = next;
        if !overflow {
            break;
        }
    }
}

/// AES-256-CTR 加解密（同一函数，CTR 对称）。
fn aes_ctr_xor(data: &[u8], key: &[u8], iv: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new_from_slice(key).expect("AES-256 需要 32 字节密钥");
    let mut counter = [0u8; 16];
    counter.copy_from_slice(&iv[..16]);
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = aes::Block::clone_from_slice(&counter);
        cipher.encrypt_block(&mut block);
        for (i, byte) in chunk.iter().enumerate() {
            out.push(byte ^ block[i]);
        }
        increment(&mut counter);
    }
    out
}

fn encrypt_plain(plain: &[u8]) -> Vec<u8> {
    aes_ctr_xor(plain, AES_KEY.as_bytes(), AES_IV.as_bytes())
}

fn decrypt_plain(data: &[u8]) -> Vec<u8> {
    aes_ctr_xor(data, AES_KEY.as_bytes(), AES_IV.as_bytes())
}

/// `getToken()`：Base64(AES-CTR(毫秒时间戳))。
pub fn token() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    B64.encode(encrypt_plain(ms.to_string().as_bytes()))
}

/// 解密 Base64 响应体（`decrypt(str, "UTF-8", key, iv)`）。
fn decrypt_b64(b64: &str) -> Option<String> {
    let raw = B64.decode(b64.trim()).ok()?;
    let plain = decrypt_plain(&raw);
    String::from_utf8(plain).ok()
}

/* ============================================================
 * 解析
 * ============================================================ */

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for key in keys {
        if let Some(field) = object.get(*key) {
            match field {
                Value::String(text) => {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
                Value::Number(number) => return Some(number.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn row_item(row: &Value) -> Option<VideoItem> {
    let id = text_field(row, &["vod_id", "id"])?;
    let name = text_field(row, &["vod_name", "name", "title"]).unwrap_or_default();
    Some(VideoItem {
        id,
        name,
        cover: text_field(row, &["vod_pic", "pic", "cover"]),
        remark: text_field(row, &["vod_remarks", "remarks", "remark"]),
    })
}

fn parse_list(value: &Value) -> Vec<VideoItem> {
    let list = value
        .get("data")
        .and_then(|data| data.get("list"))
        .or_else(|| value.get("list"))
        .and_then(|list| list.as_array())
        .cloned()
        .unwrap_or_default();
    list.iter().filter_map(row_item).collect()
}

/// 单集：字符串形态 `标题$地址`（非直链时前置解析器前缀）；对象形态取 `name`/`url`。
fn episode_from(item: &Value, parse_prefix: &str, index: usize) -> Option<Episode> {
    match item {
        Value::String(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            let (title, url) = match raw.split_once('$') {
                Some((title, url)) => (title.trim().to_string(), url.trim().to_string()),
                None => (String::new(), raw.to_string()),
            };
            if url.is_empty() {
                return None;
            }
            let direct = url.contains(".m3u8") || url.contains(".mp4");
            Some(Episode {
                title: if title.is_empty() {
                    format!("第{}集", index + 1)
                } else {
                    title
                },
                url: if direct || parse_prefix.is_empty() {
                    url
                } else {
                    format!("{parse_prefix}{url}")
                },
            })
        }
        Value::Object(object) => {
            let url = text_field(item, &["url", "play_url", "path"])?;
            let title = text_field(item, &["name", "title", "part", "episode"])
                .unwrap_or_else(|| format!("第{}集", index + 1));
            let _ = object;
            Some(Episode { title, url })
        }
        _ => None,
    }
}

fn parse_detail(site: &Site, id: &str, value: &Value) -> Result<VideoDetail> {
    let data = value.get("data").unwrap_or(value);
    let info = data.get("videoInfo").unwrap_or(data);
    let name = text_field(info, &["vod_name", "name"]).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("详情响应缺少 vod_name（id={id}，站点结构可能已变）"),
        )
        .into_core()
    })?;

    let sources = data
        .get("source")
        .and_then(|source| source.as_array())
        .cloned()
        .unwrap_or_default();

    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for (group_index, source) in sources.iter().enumerate() {
        let player_info = source.get("player_info").unwrap_or(source);
        let parse_prefix = text_field(player_info, &["parse"]).unwrap_or_default();
        let items = source
            .get("list")
            .and_then(|list| list.as_array())
            .map(|list| {
                list.iter()
                    .enumerate()
                    .filter_map(|(i, item)| episode_from(item, &parse_prefix, i))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        let group = player_info
            .get("show")
            .and_then(|show| show.as_str())
            .map(|show| show.trim().to_string())
            .filter(|show| !show.is_empty())
            .unwrap_or_else(|| format!("线路{}", group_index + 1));
        groups.push(EpisodeGroup { group, items });
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: text_field(info, &["vod_pic", "pic", "cover"]),
        desc: text_field(info, &["vod_content", "content", "desc"]),
        year: text_field(info, &["vod_year", "year"]),
        area: text_field(info, &["vod_area", "area"]),
        director: text_field(info, &["vod_director", "director"]),
        actor: text_field(info, &["vod_actor", "actor"]),
        episodes: groups,
    })
}

/* ============================================================
 * 五入口
 * ============================================================ */

pub fn categories() -> Vec<Category> {
    [("1", "电影"), ("2", "连续剧"), ("3", "动漫"), ("4", "综艺")]
        .iter()
        .map(|(type_id, type_name)| Category {
            type_id: (*type_id).to_string(),
            type_name: (*type_name).to_string(),
        })
        .collect()
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = ext_base(site);
    let page = page.max(1);
    let url = format!(
        "{base}/api.php/v1.vod?type={}&page={page}&pagesize={PAGE_SIZE}",
        cat.trim()
    );
    let value = fetch_json(cfg, site, &url)?;
    Ok(parse_list(&value))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = ext_base(site);
    let url = format!("{base}/api.php/v1.vod/detilldata?vod_id={}", id.trim());
    let value = fetch_json(cfg, site, &url)?;
    parse_detail(site, id, &value)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = ext_base(site);
    let encoded = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/api.php/v1.vod?wd={encoded}");
    let value = fetch_json(cfg, site, &url)?;
    Ok(parse_list(&value))
}

pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(CatvodError::new(ErrorClass::Parse, site, "播放地址为空").into_core());
    }
    if id.contains(".m3u8") || id.contains(".mp4") {
        return Ok(json!({ "parse": 0, "playUrl": "", "url": id }));
    }

    // 非直链：带上 token 取一次加密响应，再 AES-CTR 解密出真实地址。
    let client = crate::http::client_for(&_cfg.http)?;
    let url = format!("{id}&sc=1&token={}", token());
    let body = crate::http::get_text(&client, &url, &headers(&url), 1)?;
    let plain = decrypt_b64(&body).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "播放地址响应不是合法的 Base64/AES-CTR 密文（密钥可能已更换）",
        )
        .into_core()
    })?;
    let value: Value = serde_json::from_str(&plain).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("解密后的播放响应不是 JSON（{e}）"),
        )
        .into_core()
    })?;
    let real = value
        .get("url")
        .and_then(|url| url.as_str())
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            CatvodError::new(ErrorClass::HostApi, site, "解密后的播放响应没有 url 字段").into_core()
        })?;
    Ok(json!({ "parse": 0, "playUrl": "", "url": real }))
}

/* ============================================================
 * 单元测试
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn site_stub() -> Site {
        Site {
            key: "apptt".to_string(),
            name: "AppTT".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_only_apptt() {
        assert!(handles("csp_AppTT"));
        assert!(handles(" CSP_APPTT "));
        assert!(!handles("csp_AppMao"));
    }

    #[test]
    fn ua_follows_url_shape() {
        assert_eq!(ua_for("http://x/api.php/app/vod"), "Dart/2.14 (dart:io)");
        assert_eq!(ua_for("http://x/freekan/vod"), "Dart/2.14 (dart:io)");
        assert_eq!(ua_for("http://x/zsb/vod"), "Dart/2.15 (dart:io)");
        assert_eq!(ua_for("http://x/api.php/v1.vod?type=1"), "okhttp/4.1.0");
        assert_eq!(ua_for("http://x/plain"), "Dalvik/2.1.0");
    }

    #[test]
    fn ext_overrides_base() {
        let mut site = site_stub();
        assert_eq!(ext_base(&site), DEFAULT_BASE);
        site.ext = Some(json!("http://127.0.0.1:8080/x$$$1"));
        assert_eq!(ext_base(&site), "http://127.0.0.1:8080/x");
        site.ext = Some(json!("null$$$1"));
        assert_eq!(ext_base(&site), DEFAULT_BASE);
    }

    #[test]
    fn categories_are_static_four() {
        let categories = categories();
        assert_eq!(categories.len(), 4);
        assert_eq!(categories[0].type_id, "1");
        assert_eq!(categories[3].type_name, "综艺");
    }

    #[test]
    fn parse_list_rows() {
        let value = json!({
            "data": { "list": [
                { "vod_id": 12, "vod_name": "示例", "vod_pic": "http://p/1.jpg", "vod_remarks": "更新至3集" },
                { "vod_id": "13", "vod_name": "第二条" }
            ]}
        });
        let rows = parse_list(&value);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "12");
        assert_eq!(rows[0].remark.as_deref(), Some("更新至3集"));
        assert_eq!(rows[1].id, "13");
    }

    #[test]
    fn parse_detail_groups_and_prefix() {
        let value = json!({
            "data": {
                "videoInfo": {
                    "vod_id": "42", "vod_name": "示例剧", "vod_pic": "http://p/2.jpg",
                    "vod_year": "2024", "vod_area": "大陆", "vod_actor": "演员",
                    "vod_director": "导演", "vod_content": "  简介  "
                },
                "source": [
                    { "player_info": { "show": "线路A", "parse": "" },
                      "list": ["第1集$http://cdn/1.m3u8", "第2集$http://jx/?url=http://cdn/2.m3u8"] },
                    { "player_info": { "show": "线路B", "parse": "http://jx/?url=" },
                      "list": ["第1集$http://cdn/a.mp4", "第2集$http://cdn/b.mp4"] },
                    { "player_info": { "show": "线路C" }, "list": [ { "name": "HD", "url": "http://cdn/c.m3u8" } ] }
                ]
            }
        });
        let detail = parse_detail(&site_stub(), "42", &value).expect("detail");
        assert_eq!(detail.name, "示例剧");
        assert_eq!(detail.year.as_deref(), Some("2024"));
        assert_eq!(detail.desc.as_deref(), Some("简介"));
        assert_eq!(detail.episodes.len(), 3);
        assert_eq!(detail.episodes[0].group, "线路A");
        assert_eq!(detail.episodes[0].items[0].url, "http://cdn/1.m3u8");
        assert_eq!(detail.episodes[1].items[0].url, "http://cdn/a.mp4");
        assert_eq!(detail.episodes[2].group, "线路C");
        assert_eq!(detail.episodes[2].items[0].title, "HD");
    }

    #[test]
    fn parse_detail_missing_name_reports_parse_error() {
        let value = json!({ "data": { "videoInfo": {} } });
        let err = parse_detail(&site_stub(), "0", &value).expect_err("must fail");
        assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
    }

    #[test]
    fn aes_ctr_matches_node_vector() {
        // node: aes-256-ctr(key, iv).update('{"url":"http://cdn.test/final.m3u8"}')
        let plain = decrypt_b64("T0TrN7c1L8sy8V4ZpeCMtzIh6tGla/n857yfeBEuIB0UsLJG")
            .expect("decrypt");
        assert_eq!(plain, r#"{"url":"http://cdn.test/final.m3u8"}"#);
    }

    #[test]
    fn token_is_base64_of_padding_free_ctr_stream() {
        let token = token();
        let raw = B64.decode(&token).expect("base64");
        let plain = String::from_utf8(decrypt_plain(&raw)).expect("utf8");
        let millis: u128 = plain.parse().expect("timestamp");
        assert!(millis > 1_600_000_000_000);
    }

    #[test]
    fn decrypt_rejects_garbage() {
        assert!(decrypt_b64("not-base64!!").is_none());
    }
}
