//! type=4 HTTP 远程接口（把 Spider 协议跑在远端 API 上）
//!
//! 对齐 jun 版 TVBoxOS `SourceViewModel` 的 type=4 分支（报告 §5.2/§5.6）：
//! 远端接口的响应结构与 `AbsJson`/`AbsXml` 相同，仍然只有 `class[]` 与 `list[]`
//! 两种顶层数据，只是请求参数多了一个 `extend`（站点 `ext` 透传）。
//!
//! 请求约定：
//! - 分类：`{api}?filter=true[&extend=]` → `{class:[{type_id,type_name}]}`
//! - 列表：`{api}?ac=list&t={typeId}&pg={page}[&extend=]`
//! - 详情：`{api}?ac=detail&ids={id}[&extend=]`
//! - 搜索：`{api}?wd={keyword}&ac=detail&quick=false[&extend=]`
//! - 播放：`{api}?play={idOrUrl}&flag={flag}[&extend=]`
//!   → `{url, header, parse, jx, playUrl}`（`get_play` 原样返回，供
//!   `source::get_play` 后续接入归一化）
//!
//! 真实远端接口大多由 Gson 生成、字段经常缺一半，所以这里全部按「宽松解析」
//! 处理：字段别名兼容、数字/字符串混用、缺失字段留空，绝不 panic。`extend`
//! 若本身是 http(s) 地址，会先拉取成真实内容（参考 `getFixUrl`：空/>2500
//! 字符时保留原值）。

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

/// 远端接口单次允许的 `extend` 内容长度上限（对齐 `getFixUrl` 的 2500）
const EXTEND_MAX_CHARS: usize = 2500;

/* ============================================================
 * 公共入口
 * ============================================================ */

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let client = http::client_for(&cfg.http)?;
    let extend = extend_param(site, &client);
    let mut params: Vec<(&str, &str)> = vec![("filter", "true")];
    if !extend.is_empty() {
        params.push(("extend", extend.as_str()));
    }
    let url = build_api(&site.api, &params);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let value = parse_json_body(&text, &url)?;
    let Some(classes) = find_class_array(&value) else {
        // Gson 风格：字段缺失不报错，直接给空列表
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in classes {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let type_id = str_field(obj, &["type_id", "id", "tid"]).trim().to_string();
        let type_name = str_field(obj, &["type_name", "name"]).trim().to_string();
        if type_id.is_empty() && type_name.is_empty() {
            continue;
        }
        out.push(Category { type_id, type_name });
    }
    Ok(out)
}

pub fn list_videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let client = http::client_for(&cfg.http)?;
    let extend = extend_param(site, &client);
    let page = page.to_string();
    let mut params: Vec<(&str, &str)> = vec![("ac", "list"), ("t", cat), ("pg", page.as_str())];
    push_extend(&mut params, &extend);
    let url = build_api(&site.api, &params);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let value = parse_json_body(&text, &url)?;
    list_items(&value)
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let client = http::client_for(&cfg.http)?;
    let extend = extend_param(site, &client);
    let mut params: Vec<(&str, &str)> = vec![("ac", "detail"), ("ids", id)];
    push_extend(&mut params, &extend);
    let url = build_api(&site.api, &params);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let value = parse_json_body(&text, &url)?;

    if let Some(err) = api_error(&value) {
        return Err(err);
    }
    let obj = find_list_array(&value)
        .and_then(|items| items.first())
        .and_then(|item| item.as_object())
        .ok_or_else(|| CoreError::Config(format!("type=4 详情为空（id={id}，url={url}）")))?;

    detail_from_obj(obj, &url)
}

fn detail_from_obj(obj: &Map<String, Value>, url: &str) -> Result<VideoDetail> {
    let mut detail_id = str_field(obj, &["vod_id", "id"]).trim().to_string();
    let mut name = str_field(obj, &["vod_name", "name", "title"])
        .trim()
        .to_string();
    if name.is_empty() {
        name = detail_id.clone();
    }
    if detail_id.is_empty() {
        detail_id = name.clone();
    }
    if detail_id.is_empty() {
        return Err(CoreError::Config(format!(
            "type=4 详情缺少 id/name（url={url}）"
        )));
    }

    let episodes = parse_play_fields(
        &str_field(obj, &["vod_play_from", "play_from", "playFrom"]),
        &str_field(obj, &["vod_play_url", "play_url", "playUrl"]),
    );

    Ok(VideoDetail {
        id: detail_id,
        name,
        cover: opt_field(obj, &["vod_pic", "pic", "cover", "vod_pic_thumb"]),
        desc: opt_field(obj, &["vod_content", "content", "des", "desc", "vod_blurb"]),
        year: opt_field(obj, &["vod_year", "year"]),
        area: opt_field(obj, &["vod_area", "area"]),
        director: opt_field(obj, &["vod_director", "director"]),
        actor: opt_field(obj, &["vod_actor", "actor"]),
        episodes,
    })
}

/// 搜索：`wd` 原文交给 [`build_api`] 统一做表单编码，避免双重编码。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let client = http::client_for(&cfg.http)?;
    let extend = extend_param(site, &client);
    let mut params: Vec<(&str, &str)> = vec![("wd", keyword), ("ac", "detail"), ("quick", "false")];
    push_extend(&mut params, &extend);
    let url = build_api(&site.api, &params);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let value = parse_json_body(&text, &url)?;
    list_items(&value)
}

/// 播放信息：`{api}?play={idOrUrl}&flag={flag}[&extend=]`
///
/// 返回远端原始 JSON（约定字段 `url`/`header`/`parse`/`jx`/`playUrl`）。这里只做
/// 轻量归一：若根节点没有 `url` 但 `data` 对象里有，则展开 `data`。真正的
/// `PlayResult` 归一（去 `video://`、拼 `playUrl`、进解析链）交给
/// `source::get_play`，本函数保持「远端返回什么就给什么」。
pub fn get_play(cfg: &TvConfig, site: &Site, flag: &str, id_or_url: &str) -> Result<Value> {
    let client = http::client_for(&cfg.http)?;
    let extend = extend_param(site, &client);
    let mut params: Vec<(&str, &str)> = vec![("play", id_or_url), ("flag", flag)];
    push_extend(&mut params, &extend);
    let url = build_api(&site.api, &params);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let value = parse_json_body(&text, &url)?;
    if value.get("url").is_none() {
        if let Some(data) = value.get("data") {
            if data.get("url").is_some() {
                return Ok(data.clone());
            }
        }
    }
    Ok(value)
}

/* ============================================================
 * HTTP / extend 小工具
 * ============================================================ */

fn site_extra_headers(site: &Site) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(obj) = site.ext.as_ref().and_then(|v| v.as_object()) {
        for key in ["referer", "cookie", "user-agent"] {
            if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
                map.insert(key.to_string(), v.to_string());
            }
        }
    }
    map
}

fn build_api(api: &str, params: &[(&str, &str)]) -> String {
    let api = api.trim();
    if params.is_empty() {
        return api.to_string();
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(key, value);
    }
    let query = serializer.finish();
    let sep = if api.contains('?') {
        if api.ends_with('?') || api.ends_with('&') {
            ""
        } else {
            "&"
        }
    } else {
        "?"
    };
    format!("{api}{sep}{query}")
}

fn push_extend<'a>(params: &mut Vec<(&'a str, &'a str)>, extend: &'a str) {
    if !extend.is_empty() {
        params.push(("extend", extend));
    }
}

/// 站点 `ext` → `extend` 查询参数。
///
/// 与 `getFixUrl` 一致：`ext` 本身是 http(s) 时先抓取内容，成功且非空、长度
/// 不超过 2500 字符才替换；否则保留原值（拉取失败也只记日志，不阻断请求）。
fn extend_param(site: &Site, client: &reqwest::blocking::Client) -> String {
    let raw = site.ext_string();
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return raw;
    }
    if !(raw.starts_with("http://") || raw.starts_with("https://")) {
        return raw;
    }
    match http::get_text(client, &raw, &HashMap::new(), 0) {
        Ok(text) => {
            let text = text.trim();
            if text.is_empty() {
                return raw;
            }
            // 能压成紧凑 JSON 就压（与 tryMinifyJson 对齐）
            let text = match serde_json::from_str::<Value>(text) {
                Ok(value) => value.to_string(),
                Err(_) => text.to_string(),
            };
            if text.chars().count() > EXTEND_MAX_CHARS {
                raw
            } else {
                text
            }
        }
        Err(e) => {
            tracing::warn!("type=4 extend 拉取失败，使用原始值: {e}");
            raw
        }
    }
}

/* ============================================================
 * JSON 宽松解析
 * ============================================================ */

fn parse_json_body(text: &str, url: &str) -> Result<Value> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Err(CoreError::Config(format!(
            "type=4 接口返回空响应（url={url}）"
        )));
    }
    serde_json::from_str(text)
        .map_err(|e| CoreError::Config(format!("type=4 接口 JSON 解析失败（url={url}）: {e}")))
}

/// `{class:[...]}` / `{classes:[...]}` / `{data:{class:[...]}}` / `{data:[...]}` 都吃
fn find_class_array(value: &Value) -> Option<&Vec<Value>> {
    for key in ["class", "classes"] {
        if let Some(list) = value.get(key).and_then(Value::as_array) {
            return Some(list);
        }
        if let Some(list) = value
            .get("data")
            .and_then(|data| data.get(key))
            .and_then(Value::as_array)
        {
            return Some(list);
        }
    }
    let data = value.get("data").and_then(Value::as_array)?;
    // data 直接是分类数组时，用字段名判断，避免和列表响应混淆
    if data.iter().any(|item| {
        item.get("type_id").is_some()
            || item.get("type_name").is_some()
            || item.get("tid").is_some()
    }) {
        Some(data)
    } else {
        None
    }
}

/// `{list:[...]}` / `{data:{list:[...]}}` / `{data:[...]}` / 根数组 都吃
fn find_list_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(list) = value.get("list").and_then(Value::as_array) {
        return Some(list);
    }
    if let Some(list) = value
        .get("data")
        .and_then(|data| data.get("list"))
        .and_then(Value::as_array)
    {
        return Some(list);
    }
    if let Some(list) = value.get("data").and_then(Value::as_array) {
        return Some(list);
    }
    value.as_array()
}

fn api_error(value: &Value) -> Option<CoreError> {
    let obj = value.as_object()?;
    let code = obj.get("code").and_then(value_as_i64)?;
    // 兼容 1/0/200 三种成功码
    if matches!(code, 1 | 0 | 200) {
        return None;
    }
    Some(CoreError::Api {
        code,
        msg: str_field(obj, &["msg", "message", "error"]),
    })
}

fn value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        // 少数接口把 url 这类字段给成数组/对象，保留 JSON 文本，至少不丢信息
        other => other.to_string(),
    }
}

fn str_field(obj: &Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            let text = value_to_string(value);
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn opt_field(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    let text = str_field(obj, keys).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn to_video_item(obj: &Map<String, Value>) -> Option<VideoItem> {
    let id = str_field(obj, &["vod_id", "id"]).trim().to_string();
    let name = str_field(obj, &["vod_name", "name", "title"])
        .trim()
        .to_string();
    // 详情按 id 请求，空 id 的行点开必然失败；列表阶段丢弃（与 type=0/苹果CMS 一致）。
    if id.is_empty() {
        return None;
    }
    Some(VideoItem {
        id,
        name,
        cover: opt_field(
            obj,
            &["vod_pic", "pic", "cover", "vod_pic_thumb", "vod_pic_slide"],
        ),
        remark: opt_field(
            obj,
            &["vod_remarks", "remarks", "note", "vod_note", "vod_state"],
        ),
    })
}

fn list_items(value: &Value) -> Result<Vec<VideoItem>> {
    if let Some(items) = find_list_array(value) {
        return Ok(items
            .iter()
            .filter_map(|item| item.as_object())
            .filter_map(to_video_item)
            .collect());
    }
    if let Some(err) = api_error(value) {
        return Err(err);
    }
    Ok(Vec::new())
}

/* ============================================================
 * vod_play_from / vod_play_url 解析（AbsJson 语义）
 * ============================================================ */

/// 与 `AbsJson.toXmlVideo` + `absXml` 的拆分顺序一致：
/// 1) `vod_play_from` / `vod_play_url` 先按 `$$$` 分线路；
/// 2) 每条线路内按 `#` 分集，每项按**第一个** `$` 分「名称 / 地址」，无名称用「第N集」。
fn parse_play_fields(play_from: &str, play_url: &str) -> Vec<EpisodeGroup> {
    let play_url = play_url.trim();
    if play_url.is_empty() {
        return Vec::new();
    }
    if play_url.contains("$$$") {
        let bodies: Vec<&str> = play_url.split("$$$").collect();
        let names: Vec<&str> = play_from.split("$$$").map(str::trim).collect();
        // 空体不占线路号：有显式名字按原下标取，回退命名按已产出条数计。
        let mut groups = Vec::new();
        for (index, body) in bodies.iter().enumerate() {
            let items = parse_episode_body(body);
            if items.is_empty() {
                continue;
            }
            let group = names
                .get(index)
                .map(|name| name.to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("线路{}", groups.len() + 1));
            groups.push(EpisodeGroup { group, items });
        }
        return groups;
    }

    // 单线路：vod_play_from 通常就是一个线路名（可能为空）
    let group = play_from
        .trim()
        .split("$$$")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let group = if group.is_empty() {
        "默认".to_string()
    } else {
        group
    };
    let items = parse_episode_body(play_url);
    if items.is_empty() {
        Vec::new()
    } else {
        vec![EpisodeGroup { group, items }]
    }
}

fn parse_episode_body(body: &str) -> Vec<Episode> {
    let mut items = Vec::new();
    for (index, entry) in body
        .split('#')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        let (title, url) = match entry.find('$') {
            Some(pos) => {
                let title = entry[..pos].trim();
                let url = entry[pos + 1..].trim();
                let title = if title.is_empty() {
                    format!("第{}集", index + 1)
                } else {
                    title.to_string()
                };
                (title, url.to_string())
            }
            None => (format!("第{}集", index + 1), entry.to_string()),
        };
        if url.is_empty() {
            continue;
        }
        items.push(Episode { title, url });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_fields_absjson_style() {
        let groups = parse_play_fields(
            "线路一$$$线路二",
            "第1集$http://v/11.m3u8#第2集$http://v/12.m3u8$$$第1集$http://v/21.m3u8",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "线路一");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[1].title, "第2集");
        assert_eq!(groups[1].items[0].url, "http://v/21.m3u8");
    }

    #[test]
    fn leading_empty_line_does_not_consume_group_number() {
        let groups = parse_play_fields("", "$$$第1集$https://v/1.m3u8");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "线路1");
    }

    #[test]
    fn play_fields_single_line_without_name() {
        let groups = parse_play_fields("", "http://v/1.mp4#http://v/2.mp4");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "默认");
        assert_eq!(groups[0].items[0].title, "第1集");
        assert_eq!(groups[0].items[1].url, "http://v/2.mp4");
    }

    #[test]
    fn class_and_list_data_wrappers() {
        let value: Value =
            serde_json::from_str(r#"{"data":{"class":[{"type_id":1,"type_name":"电影"}]}}"#)
                .unwrap();
        assert_eq!(find_class_array(&value).unwrap().len(), 1);

        let value: Value =
            serde_json::from_str(r#"{"data":{"list":[{"vod_id":7,"vod_name":"A"}]}}"#).unwrap();
        assert_eq!(find_list_array(&value).unwrap().len(), 1);
        let item = to_video_item(find_list_array(&value).unwrap()[0].as_object().unwrap()).unwrap();
        assert_eq!(item.id, "7");
        assert_eq!(item.name, "A");
    }

    #[test]
    fn rows_without_id_are_skipped_and_detail_name_falls_back_to_id() {
        let value: Value =
            serde_json::from_str(r#"{"list":[{"vod_id":7,"vod_name":"A"},{"vod_name":"无名"}]}"#)
                .unwrap();
        let items = list_items(&value).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "7");

        let obj: Map<String, Value> =
            serde_json::from_str(r#"{"id":9,"pic":"c"}"#).unwrap();
        let detail = detail_from_obj(&obj, "u").unwrap();
        assert_eq!(detail.id, "9");
        assert_eq!(detail.name, "9");

        let empty: Map<String, Value> = serde_json::from_str(r#"{"pic":"c"}"#).unwrap();
        let err = detail_from_obj(&empty, "u").unwrap_err();
        assert!(format!("{err}").contains("缺少 id/name"));
    }

    #[test]
    fn missing_fields_do_not_panic() {
        let value: Value = serde_json::from_str(r#"{"code":1,"list":[{}]}"#).unwrap();
        assert!(list_items(&value).unwrap().is_empty());
        let value: Value = serde_json::from_str(r#"{"code":0,"msg":"ok"}"#).unwrap();
        assert!(list_items(&value).unwrap().is_empty());
    }

    #[test]
    fn error_code_maps_to_api_error() {
        let value: Value = serde_json::from_str(r#"{"code":404,"msg":"找不到"}"#).unwrap();
        assert!(matches!(
            api_error(&value),
            Some(CoreError::Api { code: 404, .. })
        ));
    }
}
