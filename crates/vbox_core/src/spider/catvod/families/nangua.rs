//! `csp_NanGua` 原生实现（南瓜影视 App 接口 `http://ys.changmengyun.com`）。
//!
//! 语义来自静态反编译：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/NanGua.java`
//! （593 行，jadx 0 skip；字符串字面量由 `C0338a.a(byte[], byte[])` 逐字节 XOR 还原，
//! 解码脚本 `_rea/tmp/decode-nangua.mjs` → `_rea/tmp/nangua-dec.java`）。类 `extends Spider`，
//! 只依赖 `Build` 常量与 `S.e.B`/`TextUtils`，不依赖 Android 专有能力，可 1:1 重写：
//!
//! 1. `init`：基址写死 `http://ys.changmengyun.com`；`imei` 取 `Settings.Secure.ANDROID_ID`
//!    （缺省 `131251157296170`）；app 版本 `1.0.9` / code `9`；`app=ylys`、`devices=android`、
//!    `deviceScreen=2340*1080`。
//! 2. 签名：`sign = md5_hex(imei + "#uBFszdEM0oL0JRn@" + timeMillis)`，随请求头
//!    `version_name`、`version_code`、`user-agent: okhttp/3.12.0`、`timeMillis`、`sign` 一起下发。
//! 3. `homeContent`：5 个写死分类（1 电影 / 2 电视剧 / 3 综艺 / 4 动漫 / 46 外剧）。
//! 4. `categoryContent(typeId, page)`：`/api.php/provide/vod_list`，`id={typeId}&page={page}`；
//!    响应 `data.list[]` 每项 `id`/`name`/`img`/`msg`；输出 `pagecount=100`、`limit=20`。
//! 5. `detailContent(id)`：`/api.php/provide/vod_detail`，`data` 含 `name`/`img`/`type`/
//!    `year`/`msg`/`info` 与 `player_info[]`（`show` 为线路名，`video_info[]` 每项 `name` +
//!    `url[0]` 为分集地址）；分集 `name$url[0]`、组内 `#`、组间 `$$$`。
//! 6. `searchContent(word)`：`/api.php/provide/search_result?video_name={enc}`；响应
//!    `list[]` 每项 `id`/`video_name`（去 `<em>`）/`img`/`qingxidu`。
//! 7. `playerContent(flag, id)`：`id` 即分集绝对地址；GET 后取 `data.url` 与 `data.header`，
//!    原样返回（`parse=0`）。
//!
//! 相对原实现的有意偏差（均在此标记）：
//! - `ext` 第一段若为 http(s) 地址则作为**基址覆盖**（原实现把第一段当图片代理地址，
//!   这里在保留原始语义的同时让 mock/镜像可注入；真实站点不用 `ext`）；
//! - `ext` 第二段 `1` 表示弹幕开关，与原实现一致**忽略**（桌面端无弹幕渲染）；
//! - 图片代理 `getProxyDownloadUrl` 不实现（原实现把图片下载到本机 `loadPic`），
//!   直接返回原始图片 URL。
//!
//! 上游状态：`ys.changmengyun.com` 已停放（`vod_list` 返回 ICP 备案页），因此本家族只有
//! L-B（mock 协议）验收，L-C 真站验证留待域名恢复（见计划附录 O）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_NanGua";

/// 站点写死基址（`NanGua.java` 中 `http://ys.changmengyun.com/api.php/provide/...`）。
const DEFAULT_BASE: &str = "http://ys.changmengyun.com";

/// 缺省 imei（原实现取 `Settings.Secure.ANDROID_ID`，取不到时用该值）。
const DEFAULT_IMEI: &str = "131251157296170";

const APP_VERSION_NAME: &str = "1.0.9";
const APP_VERSION_CODE: &str = "9";
const APP: &str = "ylys";
const DEVICES: &str = "android";
const DEVICE_SCREEN: &str = "2340*1080";
const DEVICE_MODEL: &str = "Pixel 4";
const DEVICE_BRAND: &str = "google";
const DEVICE_VERSION: &str = "13";

/// 请求 UA（原实现 `okhttp/3.12.0`）。
const UA: &str = "okhttp/3.12.0";

/// 签名盐（原实现 XOR 常量 `#uBFszdEM0oL0JRn@`）。
const SIGN_SALT: &str = "#uBFszdEM0oL0JRn@";

/// 5 个写死分类：`(type_id, type_name)`。
const CATEGORIES: [(&str, &str); 5] = [
    ("1", "电影"),
    ("2", "电视剧"),
    ("3", "综艺"),
    ("4", "动漫"),
    ("46", "外剧"),
];

/// 是否为 `csp_NanGua` 站点。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// `ext` 以 `$$$` 分段（原实现 `ext.split("\\$\\$\\$")`）。
fn ext_segments(site: &Site) -> Vec<String> {
    let ext = site.ext_string();
    if ext.trim().is_empty() {
        return Vec::new();
    }
    ext.split("$$$").map(|s| s.trim().to_string()).collect()
}

/// 接口基址：真实站点写死；`ext` 第一段是 http(s) 地址时覆盖（mock/镜像用，见模块文档偏差 1）。
pub fn base_of(site: &Site) -> String {
    if let Some(first) = ext_segments(site).first() {
        let trimmed = first.trim_end_matches('/');
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return trimmed.to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

/// 当前毫秒时间戳。
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `md5_hex(imei + "#uBFszdEM0oL0JRn@" + timeMillis)`。
fn signature(imei: &str, ts: u128) -> String {
    let payload = format!("{imei}{SIGN_SALT}{ts}");
    format!("{:x}", md5::compute(payload.as_bytes()))
}

/// 带签名的 App 请求头（原实现 `a(long, String)` 拼的 Map）。
fn headers(ts: u128) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("version_name".to_string(), APP_VERSION_NAME.to_string());
    h.insert("version_code".to_string(), APP_VERSION_CODE.to_string());
    h.insert("user-agent".to_string(), UA.to_string());
    h.insert("timeMillis".to_string(), ts.to_string());
    h.insert("sign".to_string(), signature(DEFAULT_IMEI, ts));
    h
}

/// 拼接口 URL：公共参数 + 端点私有参数 + 尾部公共参数（与原实现的 append 顺序一致）。
fn api_url(base: &str, ts: u128, path: &str, extra: &str) -> String {
    format!(
        "{base}{path}?appVersionName={APP_VERSION_NAME}&imei={DEFAULT_IMEI}&time={ts}{extra}\
         &deviceScreen={DEVICE_SCREEN}&appVersionCode={APP_VERSION_CODE}&deviceModel={DEVICE_MODEL}\
         &app={APP}&deviceBrand={DEVICE_BRAND}&devices={DEVICES}&deviceVersion={DEVICE_VERSION}"
    )
}

/// 发一次带签名的 GET 并解析 JSON。
fn fetch_json(cfg: &TvConfig, site: &Site, path: &str, extra: &str) -> Result<Value> {
    let ts = now_ms();
    let url = api_url(&base_of(site), ts, path, extra);
    let client = crate::http::client_for(&cfg.http)?;
    let text = crate::http::get_text(&client, &url, &headers(ts), cfg.http.retry)?;
    serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("南瓜影视接口返回不是 JSON（{url}）：{e}"),
        )
        .into_core()
    })
}

/// 取出 `data` 对象（部分版本把 `list` 直接放顶层，做兼容）。
fn data_of(v: &Value) -> &Value {
    v.get("data").unwrap_or(v)
}

/// 5 个写死分类。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 分类列表：`/api.php/provide/vod_list?id={cat}&page={page}`。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let page = if page == 0 { 1 } else { page };
    let extra = format!("&id={cat}&page={page}");
    let v = fetch_json(cfg, site, "/api.php/provide/vod_list", &extra)?;
    parse_list(&v, site)
}

fn parse_list(v: &Value, site: &Site) -> Result<Vec<VideoItem>> {
    let list = data_of(v)
        .get("list")
        .and_then(|x| x.as_array())
        .ok_or_else(|| {
            CatvodError::new(ErrorClass::Parse, site, "南瓜影视列表缺少 data.list").into_core()
        })?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let id = item.get("id").map(json_id)?;
            if id.is_empty() {
                return None;
            }
            Some(VideoItem {
                id,
                name: item
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                cover: item.get("img").and_then(|x| x.as_str()).map(str::to_string),
                remark: item
                    .get("msg")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string()),
            })
        })
        .collect())
}

/// 详情：`/api.php/provide/vod_detail?id={id}`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let extra = format!("&id={id}");
    let v = fetch_json(cfg, site, "/api.php/provide/vod_detail", &extra)?;
    parse_detail(&v, site, id)
}

fn parse_detail(v: &Value, site: &Site, id: &str) -> Result<VideoDetail> {
    let data = data_of(v);
    let name = data
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    if let Some(player_info) = data.get("player_info").and_then(|x| x.as_array()) {
        for (idx, play) in player_info.iter().enumerate() {
            let group_name = play
                .get("show")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("线路{}", idx + 1));
            let mut items: Vec<Episode> = Vec::new();
            if let Some(videos) = play.get("video_info").and_then(|x| x.as_array()) {
                for video in videos {
                    let title = video
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    let url = video
                        .get("url")
                        .and_then(|x| x.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if url.is_empty() {
                        continue;
                    }
                    items.push(Episode {
                        title: if title.is_empty() {
                            format!("第{}集", items.len() + 1)
                        } else {
                            title
                        },
                        url,
                    });
                }
            }
            if !items.is_empty() {
                groups.push(EpisodeGroup {
                    group: group_name,
                    items,
                });
            }
        }
    }
    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "南瓜影视详情页既无标题也无播放线路（接口改版？）",
        )
        .into_core());
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: data
            .get("img")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        desc: data
            .get("info")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        year: data
            .get("year")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        area: None,
        director: None,
        actor: None,
        episodes: groups,
    })
}

/// 搜索：`/api.php/provide/search_result?video_name={enc}`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let encoded = percent_encoding::utf8_percent_encode(
        keyword.trim(),
        percent_encoding::NON_ALPHANUMERIC,
    );
    let extra = format!("&video_name={encoded}");
    let v = fetch_json(cfg, site, "/api.php/provide/search_result", &extra)?;
    parse_search(&v, site)
}

fn parse_search(v: &Value, site: &Site) -> Result<Vec<VideoItem>> {
    let list = data_of(v)
        .get("list")
        .and_then(|x| x.as_array())
        .ok_or_else(|| {
            CatvodError::new(ErrorClass::Parse, site, "南瓜影视搜索缺少 list").into_core()
        })?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let id = item.get("id").map(json_id)?;
            if id.is_empty() {
                return None;
            }
            let name = item
                .get("video_name")
                .or_else(|| item.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .replace("<em>", "")
                .replace("</em>", "");
            Some(VideoItem {
                id,
                name: name.trim().to_string(),
                cover: item.get("img").and_then(|x| x.as_str()).map(str::to_string),
                remark: item
                    .get("qingxidu")
                    .and_then(|x| x.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            })
        })
        .collect())
}

/// 播放：分集地址本身是可 GET 的接口，返回 `{data:{url,header}}`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        format!("{}{id}", base_of(site))
    };
    let client = crate::http::client_for(&cfg.http)?;
    let ts = now_ms();
    let text = crate::http::get_text(&client, &url, &headers(ts), cfg.http.retry)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("南瓜影视播放接口返回不是 JSON（{url}）：{e}"),
        )
        .into_core()
    })?;
    parse_play(&v, site, &url)
}

fn parse_play(v: &Value, site: &Site, url: &str) -> Result<Value> {
    let data = data_of(v);
    let play_url = data
        .get("url")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("南瓜影视播放接口没有 data.url（{url}）"),
            )
            .into_core()
        })?;
    let mut header = HashMap::new();
    if let Some(obj) = data.get("header").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                header.insert(k.clone(), s.trim().to_string());
            }
        }
    }
    if header.is_empty() {
        header.insert("User-Agent".to_string(), UA.to_string());
    }
    Ok(json!({
        "url": play_url,
        "parse": 0,
        "header": header,
        "playUrl": "",
    }))
}

/// 把 JSON 数字或字符串 id 统一成字符串。
fn json_id(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            key: "nangua".to_string(),
            name: "南瓜".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_only_nangua() {
        assert!(handles("csp_NanGua"));
        assert!(handles(" csp_nangua "));
        assert!(!handles("csp_NanGuaGuard"));
        assert!(!handles("csp_SixV"));
    }

    #[test]
    fn categories_are_the_five_static_ones() {
        let cats = categories();
        assert_eq!(cats.len(), 5);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[4].type_id, "46");
        assert_eq!(cats[4].type_name, "外剧");
    }

    #[test]
    fn signature_is_stable_for_fixed_time() {
        assert_eq!(
            signature(DEFAULT_IMEI, 1_700_000_000_000),
            "e60e49807acd3ddacba96ec9677f5931"
        );
    }

    #[test]
    fn api_url_carries_signed_params() {
        let url = api_url("http://127.0.0.1:9", 1_700_000_000_000, "/api.php/provide/vod_list", "&id=1&page=2");
        assert!(url.starts_with("http://127.0.0.1:9/api.php/provide/vod_list?"));
        assert!(url.contains("appVersionName=1.0.9"));
        assert!(url.contains("imei=131251157296170"));
        assert!(url.contains("time=1700000000000"));
        assert!(url.contains("&id=1&page=2"));
        assert!(url.contains("app=ylys"));
        assert!(url.contains("devices=android"));
        assert!(url.contains("deviceScreen=2340*1080"));
    }

    #[test]
    fn base_can_be_overridden_by_first_ext_segment() {
        let mut s = site();
        s.ext = Some(json!("http://127.0.0.1:12345$$$1"));
        assert_eq!(base_of(&s), "http://127.0.0.1:12345");
        let empty = site();
        assert_eq!(base_of(&empty), DEFAULT_BASE);
    }

    #[test]
    fn parse_list_maps_columns() {
        let v = json!({
            "data": {
                "list": [
                    {"id": 12, "name": " 逆天至尊 ", "img": "http://img/x.jpg", "msg": "更新至10集"},
                    {"id": "", "name": "空 id 跳过"}
                ],
                "page": 1,
                "total": 1
            }
        });
        let rows = parse_list(&v, &site()).expect("should parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "12");
        assert_eq!(rows[0].name, "逆天至尊");
        assert_eq!(rows[0].cover.as_deref(), Some("http://img/x.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("更新至10集"));
    }

    #[test]
    fn parse_detail_builds_groups_and_episodes() {
        let v = json!({
            "data": {
                "name": "逆天至尊",
                "img": "http://img/x.jpg",
                "type": "动漫",
                "year": "2021",
                "msg": "更新至10集",
                "info": "  简介  ",
                "player_info": [
                    {"show": "南瓜线路", "video_info": [
                        {"name": "第1集", "url": ["http://play/1"]},
                        {"name": "", "url": ["http://play/2"]}
                    ]},
                    {"show": "", "video_info": []}
                ]
            }
        });
        let d = parse_detail(&v, &site(), "12").expect("should parse");
        assert_eq!(d.name, "逆天至尊");
        assert_eq!(d.year.as_deref(), Some("2021"));
        assert_eq!(d.desc.as_deref(), Some("简介"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "南瓜线路");
        assert_eq!(d.episodes[0].items.len(), 2);
        assert_eq!(d.episodes[0].items[0].title, "第1集");
        assert_eq!(d.episodes[0].items[1].title, "第2集");
        assert_eq!(d.episodes[0].items[1].url, "http://play/2");
    }

    #[test]
    fn parse_search_strips_em_tags() {
        let v = json!({
            "list": [
                {"id": "9", "video_name": "<em>海贼</em>王", "img": "http://img/y.jpg", "qingxidu": "1080P"}
            ]
        });
        let rows = parse_search(&v, &site()).expect("should parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "9");
        assert_eq!(rows[0].name, "海贼王");
        assert_eq!(rows[0].remark.as_deref(), Some("1080P"));
    }

    #[test]
    fn parse_play_returns_url_and_header() {
        let v = json!({
            "data": {"url": "http://cdn/video.m3u8", "header": {"User-Agent": "okhttp", "Referer": " http://ys.changmengyun.com/"}}
        });
        let out = parse_play(&v, &site(), "http://play/1").expect("should parse");
        assert_eq!(out["url"], "http://cdn/video.m3u8");
        assert_eq!(out["parse"], 0);
        assert_eq!(out["header"]["User-Agent"], "okhttp");
        assert_eq!(out["header"]["Referer"], "http://ys.changmengyun.com/");
    }

    #[test]
    fn parse_play_without_url_reports_parse_error() {
        let v = json!({"data": {"header": {}}});
        let err = parse_play(&v, &site(), "http://play/1").expect_err("should fail");
        let text = format!("{err:?}");
        assert!(text.contains("catvod.result.parse"), "{text}");
    }
}
