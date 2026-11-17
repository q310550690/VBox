//! `csp_Yj1211`（📽️ 在线直播 / 📺 竞技直播）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.Yj1211`，覆盖 2 个站点（`xhz`、`4k.json`），
//!   ext 均为空。
//! * 最佳变体：`_rea/decompiled/814547f5-PiaoHua/deobf/com/github/catvod/spider/
//!   Yj1211.java`（208 行），仅 `categoryContent` 被 jadx 跳过。
//! * 恢复证据：`java ... jadx.cli.JadxCLI --show-bad-code --single-class
//!   com.github.catvod.spider.Yj1211` → `_rea/tmp/r135/Yj1211.bad.java`
//!   （315 行、0 个 `Method dump skipped`），`categoryContent` 的 switch 与
//!   两种 URL 模板由此完整还原。
//! * 画像：`profiles.json` 中 `csp_Yj1211` 原为 `unported` / `decompiler_gap`。
//!
//! 协议（原类逐段还原）：
//! * 基址 `http://live.yj1211.work`（原类写死；本实现允许 ext 首个 `http(s)` 段
//!   覆盖基址，仅作离线 mock 的测试钩子，见「有意偏差」）。
//! * 请求头仅 `User-Agent`（YaBrowser 22.7 / Chrome 102）。
//! * `homeContent`：先 GET `/api/live/getRecommend?page=1&size=20` 取首页推荐，
//!   再 GET `/api/live/getAllAreas` 取 5 组「地区类型 × 地区」；分类为 5 个
//!   硬编码平台：`推荐="?"`、`斗鱼="ByPlatform?platform=douyu&"`、
//!   `哔哩哔哩=…bilibili&`、`虎牙=…huya&`、`网易CC=…cc&`；filters 每组键
//!   `typeName{i}`、值形如 `{typeName}={areaName}`（首项 `=all`）。
//! * `categoryContent`：统计 13 个 `typeName{i}` 筛选中「已选」的个数 `i`：
//!   - `i == 1`：`/api/live/getRecommendByAreaAll?areaType={areaType}&area={area}&page={pg}`；
//!   - 否则：`/api/live/getRecommend{tid}page={pg}&size=20`（`{tid}` 即分类 id，
//!     `?` → `getRecommend?page=…`，`ByPlatform?platform=douyu&` →
//!     `getRecommendByPlatform?platform=douyu&page=…`）。
//!   行取 `data[]` 的 `platForm/roomId/ownerName/ownerHeadPic/categoryName`，
//!   id = `platform&roomId`；`i==1` 时仅保留 `platForm` 与分类平台一致的行，
//!   `pagecount` 为 50（`i==1` 时为 Integer.MAX_VALUE）。
//! * `detailContent`：id `platform&roomId` → `/api/live/getRoomInfo?platform=&roomId=`
//!   取 `roomName/roomPic/ownerName/roomId/platForm/isLive/online/categoryName`；
//!   再 GET `/api/live/getRealUrl?platform=&roomId=`，按 `OD/HD/SD/LD/FD` 顺序
//!   取非空清晰度，线路名 `YJ1211`，单集 `原画$url` 等（原类 `清晣` 错字保留）。
//! * `searchContent`：`/api/live/search?platform=all&keyWords={enc}&isLive=0`，
//!   `data[]` 取 `nickName/headPic/cateName/platform/roomId`，最多 20 条。
//! * `playerContent`：`{"header":{…},"parse":1,"playUrl":"","url":str2}`。
//!
//! 有意偏差：
//! 1. 上游 `live.yj1211.work` 现已 NXDOMAIN（`ENOTFOUND`），无法真站验证 →
//!    只做离线 fixture + mock，不提供 `#[ignore]` 真站用例。
//! 2. 原类 13 个 `typeName{i}` 筛选依赖宿主 filters UI；桌面分类模型无 filters，
//!    故恒走 `i==0` 的「不筛选」分支（`getRecommend{tid}`），`i==1` 的地区聚合
//!    接口仅保留实现与单测，不在正常链路触发。
//! 3. 原类失败重试为「3 次、每次 sleep 1s」；这里改为 2 次连续重试、不退避，
//!    避免阻塞（错误仍以 `catvod.result.parse` / 网络错误上抛）。
//! 4. ext 覆盖基址是测试钩子（原类无 init / 不用 ext），生产为空即默认基址。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_Yj1211";

/// 原类默认基址（写死）。
const DEFAULT_BASE: &str = "http://live.yj1211.work";

/// 原类 `A()` 的桌面 UA（YaBrowser 22.7 / Chrome 102）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.134 YaBrowser/22.7.1.755 (beta) Yowser/2.5 Safari/537.36";

/// 首页 5 个硬编码平台分类（`type_name` / `type_id`）。
const HOME_CATEGORIES: [(&str, &str); 5] = [
    ("推荐", "?"),
    ("斗鱼", "ByPlatform?platform=douyu&"),
    ("哔哩哔哩", "ByPlatform?platform=bilibili&"),
    ("虎牙", "ByPlatform?platform=huya&"),
    ("网易CC", "ByPlatform?platform=cc&"),
];

/// `getRealUrl` 的清晰度键 → 单集标题（原类顺序 OD/HD/SD/LD/FD，「清晣」为原错字）。
const STREAMS: [(&str, &str); 5] = [
    ("OD", "原画"),
    ("HD", "超清"),
    ("SD", "高清"),
    ("LD", "清晣"),
    ("FD", "流畅"),
];

/// 搜索最多返回条数（原类 `min(len, 20)`）。
const SEARCH_LIMIT: usize = 20;

/* ============================================================
 * 基础工具
 * ============================================================ */

pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

/// 基址：ext 首个 `http(s)` 段覆盖（测试钩子），否则默认基址。
fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    for token in ext.split(|c: char| c.is_whitespace() || c == ';' || c == ',') {
        let token = token.trim();
        let candidate = if token.starts_with("http://") || token.starts_with("https://") {
            token
        } else if let Some((_, rest)) = token.split_once('=') {
            let rest = rest.trim();
            if rest.starts_with("http://") || rest.starts_with("https://") {
                rest
            } else {
                continue;
            }
        } else {
            continue;
        };
        return candidate.trim_end_matches('/').to_string();
    }
    DEFAULT_BASE.to_string()
}

/// 请求头：仅桌面 UA。
fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

/// `{base}{path}`，base 去尾斜杠、path 以 `/` 开头。
fn api_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// 原类 `Bn.K0(url, headers)`：GET 文本；空响应视为失败并重试一次。
fn fetch(cfg: &TvConfig, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    let h = headers();
    let mut last_err = None;
    for _ in 0..2 {
        match get_text(&client, url, &h, 2) {
            Ok(text) if !text.trim().is_empty() => return Ok(text),
            Ok(_) => last_err = None,
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => Err(e),
        None => Err(CatvodError::new(ErrorClass::Parse, &Site::default(), format!("接口返回空响应：{url}")).into_core()),
    }
}

/// GET + JSON 解析。
fn get_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let text = fetch(cfg, url)?;
    serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("JSON 解析失败（{url}）：{e}")))
}

/// URL 编码（与 `URLEncoder.encode` 对齐：空格 → `+`）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 分类 id → 平台名（原类 `switch` 五个分支；未知 → 空串）。
pub fn platform_of(type_id: &str) -> &'static str {
    match type_id {
        "?" => "all",
        "ByPlatform?platform=douyu&" => "douyu",
        "ByPlatform?platform=bilibili&" => "bilibili",
        "ByPlatform?platform=huya&" => "huya",
        "ByPlatform?platform=cc&" => "cc",
        _ => "",
    }
}

/* ============================================================
 * 解析
 * ============================================================ */

/// 5 个硬编码平台分类（`homeContent` 不依赖网络）。
pub fn parse_categories() -> Vec<Category> {
    HOME_CATEGORIES
        .iter()
        .map(|(name, id)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 分类列表 URL（`i == 0` 分支：`getRecommend{tid}page={pg}&size=20`）。
pub fn category_url(base: &str, type_id: &str, page: u32) -> String {
    api_url(
        base,
        &format!("/api/live/getRecommend{type_id}page={page}&size=20"),
    )
}

/// 地区聚合 URL（原类 `i == 1` 分支；`area_type=area` 形如 `类型=地区`）。
pub fn area_url(base: &str, area_type: &str, area: &str, page: u32) -> String {
    api_url(
        base,
        &format!(
            "/api/live/getRecommendByAreaAll?areaType={}&area={}&page={page}",
            urlencode(area_type),
            urlencode(area)
        ),
    )
}

/// `data[]` 房间行 → 列表项。
pub fn parse_room_list(value: &Value) -> Vec<VideoItem> {
    let Some(rows) = value.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let platform = row.get("platForm").and_then(Value::as_str).unwrap_or_default();
            let room_id = row.get("roomId").and_then(Value::as_str).unwrap_or_default();
            if platform.is_empty() || room_id.is_empty() {
                return None;
            }
            Some(VideoItem {
                id: format!("{platform}&{room_id}"),
                name: row
                    .get("ownerName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                cover: row
                    .get("ownerHeadPic")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                remark: row
                    .get("categoryName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

/// `data[]` 搜索结果 → 列表项（最多 20 条）。
pub fn parse_search_results(value: &Value) -> Vec<VideoItem> {
    let Some(rows) = value.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .take(SEARCH_LIMIT)
        .filter_map(|row| {
            let platform = row.get("platform").and_then(Value::as_str).unwrap_or_default();
            let room_id = row.get("roomId").and_then(Value::as_str).unwrap_or_default();
            if platform.is_empty() || room_id.is_empty() {
                return None;
            }
            Some(VideoItem {
                id: format!("{platform}&{room_id}"),
                name: row
                    .get("nickName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                cover: row.get("headPic").and_then(Value::as_str).map(str::to_string),
                remark: row
                    .get("cateName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

/// id `platform&roomId` 拆两段。
pub fn split_room_id(id: &str) -> Option<(&str, &str)> {
    id.split_once('&').filter(|(p, r)| !p.is_empty() && !r.is_empty())
}

/// `getRealUrl` 的 `data` → 剧集组（按 OD/HD/SD/LD/FD 顺序，跳过空值）。
pub fn parse_streams(value: &Value) -> Vec<Episode> {
    let Some(data) = value.get("data") else {
        return Vec::new();
    };
    STREAMS
        .iter()
        .filter_map(|(key, title)| {
            let url = data.get(*key).and_then(Value::as_str).unwrap_or_default();
            if url.is_empty() {
                return None;
            }
            Some(Episode {
                title: (*title).to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

/// 房间详情 → `VideoDetail`。
pub fn build_detail(id: &str, info: &Value, urls: &Value) -> VideoDetail {
    let data = info.get("data").unwrap_or(&Value::Null);
    let get = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or_default();
    let is_live = get("isLive");
    let type_name = if is_live.is_empty() { "录播" } else { "正在直播中" };
    let desc = format!(
        "{type_name}｜{}｜{}{}｜观看人数:{}",
        get("categoryName"),
        get("ownerName"),
        if get("roomId").is_empty() { String::new() } else { format!(" RoomID:{}", get("roomId")) },
        get("online")
    );
    let mut episodes = parse_streams(urls);
    if episodes.is_empty() {
        episodes.push(Episode {
            title: "NoStream".to_string(),
            url: "nolink".to_string(),
        });
    }
    VideoDetail {
        id: id.to_string(),
        name: get("roomName").to_string(),
        cover: Some(get("roomPic").to_string()).filter(|s| !s.is_empty()),
        desc: Some(desc),
        area: Some(get("platForm").to_string()).filter(|s| !s.is_empty()),
        director: Some(format!("{} RoomID:{}", get("ownerName"), get("roomId"))),
        actor: Some(format!("观看人数:{}", get("online"))),
        episodes: vec![EpisodeGroup {
            group: "YJ1211".to_string(),
            items: episodes,
        }],
        ..Default::default()
    }
}

/* ============================================================
 * 入口
 * ============================================================ */

pub fn categories(_cfg: &TvConfig, _site: &Site) -> Vec<Category> {
    parse_categories()
}

/// 分类列表：`getRecommend{tid}page={pg}&size=20`。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
    _filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = category_url(&base, type_id, page);
    let value = get_json(cfg, site, &url)?;
    Ok(parse_room_list(&value))
}

/// 房间详情：`getRoomInfo` + `getRealUrl`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let (platform, room) = split_room_id(id)
        .ok_or_else(|| parse_err(site, format!("详情 id 需为「platform&roomId」，实际：{id}")))?;
    let info_url = api_url(
        &base,
        &format!("/api/live/getRoomInfo?platform={platform}&roomId={room}"),
    );
    let urls_url = api_url(
        &base,
        &format!("/api/live/getRealUrl?platform={platform}&roomId={room}"),
    );
    let info = get_json(cfg, site, &info_url)?;
    let urls = get_json(cfg, site, &urls_url)?;
    Ok(build_detail(id, &info, &urls))
}

/// 搜索：`search?platform=all&keyWords={enc}&isLive=0`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let url = api_url(
        &base,
        &format!(
            "/api/live/search?platform=all&keyWords={}&isLive=0",
            urlencode(keyword)
        ),
    );
    let value = get_json(cfg, site, &url)?;
    Ok(parse_search_results(&value))
}

/// 播放：原类 `{"header":…,"parse":1,"playUrl":"","url":str2}`。
pub fn player_content(_cfg: &TvConfig, _site: &Site, _flag: &str, id: &str) -> Result<Value> {
    Ok(json!({
        "header": headers(),
        "parse": 1,
        "playUrl": "",
        "url": id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;

    fn site_with_ext(ext: Option<&str>) -> Site {
        Site {
            key: "s".into(),
            name: "Yj1211".into(),
            api: API.into(),
            ext: ext.map(|s| Json::String(s.to_string())),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Yj1211"));
        assert!(handles("CSP_YJ1211"));
        assert!(!handles("csp_Yj1212"));
    }

    #[test]
    fn base_defaults_and_ext_override() {
        assert_eq!(base_of(&site_with_ext(None)), DEFAULT_BASE);
        assert_eq!(base_of(&site_with_ext(Some(""))), DEFAULT_BASE);
        assert_eq!(
            base_of(&site_with_ext(Some("mock=http://127.0.0.1:9/mock"))),
            "http://127.0.0.1:9/mock"
        );
    }

    #[test]
    fn categories_are_five_platforms() {
        let cats = parse_categories();
        assert_eq!(cats.len(), 5);
        assert_eq!(cats[0].type_name, "推荐");
        assert_eq!(cats[0].type_id, "?");
        assert_eq!(cats[4].type_name, "网易CC");
        assert_eq!(cats[4].type_id, "ByPlatform?platform=cc&");
    }

    #[test]
    fn platform_of_maps_known_platforms() {
        assert_eq!(platform_of("?"), "all");
        assert_eq!(platform_of("ByPlatform?platform=douyu&"), "douyu");
        assert_eq!(platform_of("ByPlatform?platform=bilibili&"), "bilibili");
        assert_eq!(platform_of("ByPlatform?platform=huya&"), "huya");
        assert_eq!(platform_of("ByPlatform?platform=cc&"), "cc");
        assert_eq!(platform_of("other"), "");
    }

    #[test]
    fn category_url_matches_original_templates() {
        let base = "http://live.yj1211.work";
        assert_eq!(
            category_url(base, "?", 2),
            "http://live.yj1211.work/api/live/getRecommend?page=2&size=20"
        );
        assert_eq!(
            category_url(base, "ByPlatform?platform=douyu&", 1),
            "http://live.yj1211.work/api/live/getRecommendByPlatform?platform=douyu&page=1&size=20"
        );
    }

    #[test]
    fn area_url_encodes_chinese() {
        assert_eq!(
            area_url("http://h", "类型", "全部", 3),
            "http://h/api/live/getRecommendByAreaAll?areaType=%E7%B1%BB%E5%9E%8B&area=%E5%85%A8%E9%83%A8&page=3"
        );
    }

    #[test]
    fn parse_room_list_builds_ids_and_skips_incomplete() {
        let v: Json = serde_json::from_str(
            r#"{"data":[
                {"platForm":"douyu","roomId":"1","ownerName":"A","ownerHeadPic":"http://p/a.jpg","categoryName":"英雄联盟"},
                {"platForm":"huya","roomId":"2","ownerName":"B"},
                {"platForm":"","roomId":"3","ownerName":"C"}
            ]}"#,
        )
        .unwrap();
        let rows = parse_room_list(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "douyu&1");
        assert_eq!(rows[0].name, "A");
        assert_eq!(rows[0].cover.as_deref(), Some("http://p/a.jpg"));
        assert_eq!(rows[0].remark.as_deref(), Some("英雄联盟"));
        assert_eq!(rows[1].id, "huya&2");
        assert_eq!(rows[1].cover, None);
    }

    #[test]
    fn parse_search_results_limits_and_maps_fields() {
        let mut data = Vec::new();
        for i in 0..25 {
            data.push(json!({"platform":"cc","roomId":format!("r{i}"),"nickName":format!("N{i}"),"headPic":"http://p","cateName":"C"}));
        }
        let v = json!({"data": data});
        let rows = parse_search_results(&v);
        assert_eq!(rows.len(), SEARCH_LIMIT);
        assert_eq!(rows[0].id, "cc&r0");
        assert_eq!(rows[0].name, "N0");
        assert_eq!(rows[0].remark.as_deref(), Some("C"));
    }

    #[test]
    fn split_room_id_requires_two_parts() {
        assert_eq!(split_room_id("douyu&123"), Some(("douyu", "123")));
        assert_eq!(split_room_id("douyu"), None);
        assert_eq!(split_room_id("&123"), None);
        assert_eq!(split_room_id("douyu&"), None);
    }

    #[test]
    fn parse_streams_keeps_original_order() {
        let v = json!({"data": {"FD":"f.m3u8","OD":"o.m3u8","SD":"s.m3u8","LD":""}});
        let eps = parse_streams(&v);
        assert_eq!(eps.len(), 3);
        assert_eq!(eps[0].title, "原画");
        assert_eq!(eps[0].url, "o.m3u8");
        assert_eq!(eps[1].title, "高清");
        assert_eq!(eps[2].title, "流畅");
    }

    #[test]
    fn build_detail_defaults_and_desc() {
        let info = json!({"data":{"roomName":"R","roomPic":"http://p/r.jpg","ownerName":"O","roomId":"9","platForm":"douyu","isLive":"1","online":"88","categoryName":"C"}});
        let urls = json!({"data":{"HD":"h.m3u8"}});
        let d = build_detail("douyu&9", &info, &urls);
        assert_eq!(d.id, "douyu&9");
        assert_eq!(d.name, "R");
        assert_eq!(d.cover.as_deref(), Some("http://p/r.jpg"));
        assert_eq!(d.area.as_deref(), Some("douyu"));
        assert!(d.desc.as_deref().unwrap().contains("正在直播中"));
        assert!(d.desc.as_deref().unwrap().contains("观看人数:88"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "YJ1211");
        assert_eq!(d.episodes[0].items[0].title, "超清");

        let empty = build_detail("douyu&9", &json!({"data":{}}), &json!({"data":{}}));
        assert_eq!(empty.episodes[0].items[0].title, "NoStream");
        assert_eq!(empty.episodes[0].items[0].url, "nolink");
    }

    #[test]
    fn player_json_is_parse_one_with_headers() {
        let cfg = TvConfig::default();
        let site = site_with_ext(None);
        let v = player_content(&cfg, &site, "YJ1211", "http://x/a.m3u8").unwrap();
        assert_eq!(v["parse"], 1);
        assert_eq!(v["url"], "http://x/a.m3u8");
        assert_eq!(v["playUrl"], "");
        assert!(v["header"]["User-Agent"].as_str().unwrap().contains("Chrome/102"));
    }
}
