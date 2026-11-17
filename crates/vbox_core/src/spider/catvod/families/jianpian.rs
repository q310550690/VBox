//! csp_Jianpian：荐片 App 的 JSON 接口（`http://api2.rinhome.com`）。
//!
//! 反编译来源：`0f047745-FxzHm` 版 `Jianpian.java`（147 行，jadx 0 skip）。
//! 协议要点：
//!
//! 1. 固定分类 5 个：`0=全部 / 1=电影 / 2=电视剧 / 3=动漫 / 4=综艺`（原实现写死）。
//! 2. 列表：`GET /api/crumb/list?area=0&category_id={cat}&page={page}&type=0&limit=24&sort=hot&year=0`。
//! 3. 搜索：`GET /api/video/search?page=1&key={URLEncoder.encode(kw)}`。
//! 4. 详情：`GET /api/node/detail?channel=wandoujia&token=&id={id}`，响应 `{data:{…}}`。
//! 5. 播放：直接回放分集地址（`btbo_downlist[].val`，`ftp` 前缀替换成 `tvbox-xg:ftp`），
//!    带 `User-Agent: jianpian-android/360` 与 `JPAUTH`。
//!
//! 站点 `ext` 在原始配置里是「首页推荐 JSON 的地址」（可为相对路径），本实现只在
//! `ext` 第一段是 http(s) 时把它当作 API 基址（便于 mock/镜像），否则用内置基址。
//!
//! 上游可达性（2026-11-11 实测）：`api2.rinhome.com` 全部路径返回 nginx 404，
//! 因此当前只能 L-B（mock）验收。

use std::collections::HashMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Jianpian";

const DEFAULT_BASE: &str = "http://api2.rinhome.com";
const UA: &str = "jianpian-android/360";
const JPAUTH: &str = "y261ow7kF2dtzlxh1GS9EB8nbTxNmaK/QQIAjctlKiEv";
/// 原实现的播放线路名。
const PLAY_FROM: &str = "Jianpian";
/// 原实现给海报 URL 追加的 catvod 请求头标记；VBox 侧只保留纯 URL。
const POSTER_HEADER_MARK: &str = "@Referer=www.jianpianapp.com@User-Agent=jianpian-version362";

const CATEGORIES: [(&str, &str); 5] = [
    ("0", "全部"),
    ("1", "电影"),
    ("2", "电视剧"),
    ("3", "动漫"),
    ("4", "综艺"),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 站点 `ext` 的第一段如果是 http(s) 就当作基址，否则回退内置基址。
fn base_of(site: &Site) -> String {
    let raw = site.ext_string();
    let first = raw.split("$$$").next().unwrap_or_default().trim();
    let base = first.trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        base.to_string()
    } else {
        DEFAULT_BASE.to_string()
    }
}

fn headers() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map.insert("JPAUTH".to_string(), JPAUTH.to_string());
    map
}

fn headers_value() -> Value {
    json!({ "User-Agent": UA, "JPAUTH": JPAUTH })
}

fn fetch_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let client = crate::http::client_for(&cfg.http)?;
    let body = crate::http::get_text(&client, url, &headers(), cfg.http.retry)?;
    serde_json::from_str(&body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Jianpian 响应不是合法 JSON: {e}"),
        )
        .into_core()
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

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let cat = if cat.trim().is_empty() { "0" } else { cat.trim() };
    let url = format!(
        "{}/api/crumb/list?area=0&category_id={cat}&page={page}&type=0&limit=24&sort=hot&year=0",
        base_of(site)
    );
    let json = fetch_json(cfg, site, &url)?;
    Ok(parse_list(&json))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{}/api/video/search?page=1&key={kw}", base_of(site));
    let json = fetch_json(cfg, site, &url)?;
    Ok(parse_list(&json))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let url = format!(
        "{}/api/node/detail?channel=wandoujia&token=&id={}",
        base_of(site),
        utf8_percent_encode(id, NON_ALPHANUMERIC)
    );
    let json = fetch_json(cfg, site, &url)?;
    parse_detail(&json, site)
}

pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let _ = (cfg, site);
    Ok(json!({
        "parse": 0,
        "url": id,
        "header": headers_value(),
        "playUrl": "",
    }))
}

/* ------------------------------------------------------------------ */
/* 解析                                                                */
/* ------------------------------------------------------------------ */

/// `F.b` 模型的取值：JSON 里可能是字符串，也可能是 `{"title"|"name": …}`。
pub fn value_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.trim().to_string(),
        Value::Object(map) => map
            .get("title")
            .or_else(|| map.get("name"))
            .or_else(|| map.get("val"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// 去掉原实现追加的 catvod 请求头标记（`@Referer=…@User-Agent=…`）。
pub fn clean_poster(raw: &str) -> String {
    let cut = raw.find(POSTER_HEADER_MARK).unwrap_or(raw.len());
    raw[..cut].trim().to_string()
}

fn list_rows(json: &Value) -> Vec<Value> {
    json.get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// 解析列表/搜索响应：`data[]` → `VideoItem`。
pub fn parse_list(json: &Value) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for row in list_rows(json) {
        let id = row
            .get("jump_id")
            .or_else(|| row.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let title = row
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if id.is_empty() || title.is_empty() {
            continue;
        }
        let poster = row
            .get("thumbnail")
            .or_else(|| row.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let mut remark = row
            .get("mask")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if remark.is_empty() {
            remark = value_str(row.get("playlist").unwrap_or(&Value::Null));
        }
        let cover = clean_poster(poster);
        out.push(VideoItem {
            id,
            name: title,
            cover: if cover.is_empty() { None } else { Some(cover) },
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/// 解析详情响应：`{data:{…}}` → `VideoDetail`（单一线路 `Jianpian`）。
pub fn parse_detail(json: &Value, site: &Site) -> Result<VideoDetail> {
    let data = json.get("data").unwrap_or(&Value::Null);
    if !data.is_object() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_Jianpian 详情响应缺少 data 对象",
        )
        .into_core());
    }
    let id = data
        .get("jump_id")
        .or_else(|| data.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let name = data
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_Jianpian 详情缺少 title",
        )
        .into_core());
    }
    let poster = data
        .get("thumbnail")
        .or_else(|| data.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let cover = clean_poster(poster);

    let joined = |key: &str| -> String {
        data.get(key)
            .and_then(|v| v.as_array())
            .map(|rows| {
                rows.iter()
                    .map(value_str)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    };

    let desc = data
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .replace('\u{3000}', "")
        .trim()
        .to_string();

    let mut episodes = Vec::new();
    if let Some(rows) = data.get("btbo_downlist").and_then(|v| v.as_array()) {
        for (idx, row) in rows.iter().enumerate() {
            let raw = value_str(row);
            if raw.is_empty() {
                continue;
            }
            let url = raw.replace("ftp", "tvbox-xg:ftp");
            let (title, url) = split_episode(idx, &url);
            episodes.push(Episode { title, url });
        }
    }
    let mut groups = Vec::new();
    if !episodes.is_empty() {
        groups.push(EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items: episodes,
        });
    }

    Ok(VideoDetail {
        id,
        name,
        cover: if cover.is_empty() { None } else { Some(cover) },
        desc: Some(desc).filter(|s| !s.is_empty()),
        year: non_empty(value_str(data.get("year").unwrap_or(&Value::Null))),
        area: non_empty(value_str(data.get("area").unwrap_or(&Value::Null))),
        actor: non_empty(joined("actors")),
        director: non_empty(joined("directors")),
        episodes: groups,
    })
}

fn split_episode(idx: usize, raw: &str) -> (String, String) {
    match raw.split_once('$') {
        Some((title, url)) if !title.trim().is_empty() => (title.trim().to_string(), url.trim().to_string()),
        _ => (format!("第{}集", idx + 1), raw.trim().to_string()),
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_only_jianpian() {
        assert!(handles("csp_Jianpian"));
        assert!(handles(" CSP_JIANPIAN "));
        assert!(!handles("csp_Bili"));
    }

    #[test]
    fn value_reads_string_and_object() {
        assert_eq!(value_str(&json!(" 电影 ")), "电影");
        assert_eq!(value_str(&json!({"title": "2021"})), "2021");
        assert_eq!(value_str(&json!({"name": "张三"})), "张三");
        assert_eq!(value_str(&Value::Null), "");
    }

    #[test]
    fn poster_mark_is_stripped() {
        let raw = format!("https://img.test/a.jpg{POSTER_HEADER_MARK}");
        assert_eq!(clean_poster(&raw), "https://img.test/a.jpg");
        assert_eq!(clean_poster("https://img.test/b.jpg"), "https://img.test/b.jpg");
    }

    #[test]
    fn parse_list_rows() {
        let json = json!({
            "data": [
                {"jump_id": "42", "title": "流浪地球", "thumbnail": format!("https://img.test/1.jpg{POSTER_HEADER_MARK}"), "mask": "已完结"},
                {"id": "43", "title": "满江红", "path": "https://img.test/2.jpg", "playlist": {"title": "更新至10集"}},
                {"title": "缺 id"}
            ]
        });
        let items = parse_list(&json);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "42");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.test/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("已完结"));
        assert_eq!(items[1].id, "43");
        assert_eq!(items[1].remark.as_deref(), Some("更新至10集"));
    }

    #[test]
    fn parse_detail_maps_all_fields() {
        let json = json!({
            "data": {
                "jump_id": "42",
                "title": "流浪地球",
                "thumbnail": format!("https://img.test/1.jpg{POSTER_HEADER_MARK}"),
                "description": "太阳即将毁灭　人类开启流浪计划",
                "year": {"title": "2021"},
                "area": {"title": "大陆"},
                "actors": [{"name": "吴京"}, {"name": "刘德华"}],
                "directors": [{"name": "郭帆"}],
                "btbo_downlist": [{"val": "https://cdn.test/1.m3u8"}, {"val": "名称$https://cdn.test/2.m3u8"}]
            }
        });
        let detail = parse_detail(&json, &site()).expect("detail");
        assert_eq!(detail.id, "42");
        assert_eq!(detail.name, "流浪地球");
        assert_eq!(detail.cover.as_deref(), Some("https://img.test/1.jpg"));
        assert_eq!(detail.year.as_deref(), Some("2021"));
        assert_eq!(detail.area.as_deref(), Some("大陆"));
        assert_eq!(detail.actor.as_deref(), Some("吴京 刘德华"));
        assert_eq!(detail.director.as_deref(), Some("郭帆"));
        assert_eq!(detail.desc.as_deref(), Some("太阳即将毁灭人类开启流浪计划"));
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, "Jianpian");
        assert_eq!(detail.episodes[0].items.len(), 2);
        assert_eq!(detail.episodes[0].items[0].title, "第1集");
        assert_eq!(detail.episodes[0].items[1].title, "名称");
        assert_eq!(detail.episodes[0].items[1].url, "https://cdn.test/2.m3u8");
    }

    #[test]
    fn parse_detail_rejects_empty_payload() {
        let err = parse_detail(&json!({"data": null}), &site()).expect_err("应报解析错");
        let msg = format!("{err:?}");
        assert!(msg.contains("catvod.result.parse"), "实际: {msg}");
    }

    #[test]
    fn ftp_links_are_rewritten() {
        let json = json!({"data": {"title": "x", "btbo_downlist": [{"val": "ftp://a/1.mkv"}]}});
        let detail = parse_detail(&json, &site()).expect("detail");
        assert_eq!(detail.episodes[0].items[0].url, "tvbox-xg:ftp://a/1.mkv");
    }
}
