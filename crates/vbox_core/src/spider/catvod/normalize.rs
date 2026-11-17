//! catvod 契约层：结果归一化（计划 §5）。
//!
//! jar spider 返回的是**未加类型的 JSON**（宿主用 `JSONObject` + `$$$` 拼接字符串）。
//! 本模块把那些结构翻译成 VBox 的强类型模型，纯函数、无 IO、可单元测试：
//!
//! | catvod 字段 | 归一化目标 |
//! | --- | --- |
//! | `class` (`type_id`/`type_name`) | `Vec<Category>` |
//! | `list` (`vod_id`/`vod_name`/`vod_pic`/`vod_remarks`) | `Vec<VideoItem>` |
//! | `list[0]` + `vod_play_from` / `vod_play_url` | `VideoDetail`（`$$$` 分线路、`#` 分集、`$` 标题/地址） |
//! | `playerContent` 的 `url`/`parse`/`jx`/`header` | `PlayResult` |
//!
//! 关键约定（与 Android 宿主一致）：
//! - `vod_id` 可能是数字或字符串 → 统一字符串；
//! - `vod_play_url` 的线路数可能与 `vod_play_from` 不一致时，补齐 `线路 N`；
//! - `playerContent.parse=1` 表示需要 WebView 嗅探 → `sniff=true`；
//! - `playerContent.jx=1` 表示走聚合解析 → `jx_from`；
//! - `header` 既可能是 `k=v&k2=v2` 字符串，也可能是对象。

use std::collections::HashMap;

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, PlayResult, VideoDetail, VideoItem};

/// 数组字段是否存在（诊断用）。
pub fn array_field(value: &Value, key: &str) -> bool {
    value.get(key).map(|v| v.is_array()).unwrap_or(false)
}

fn as_i64(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok().or_else(|| {
            // "3.0" 这类浮点字符串
            s.trim().parse::<f64>().ok().map(|f| f as i64)
        }),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn as_str(v: Option<&Value>) -> Option<String> {
    let v = v?;
    match v {
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn as_id(v: Option<&Value>) -> Option<String> {
    let v = v?;
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `class` → 分类列表（兼容 `type_id` 为数字）。
pub fn categories(value: &Value) -> Vec<Category> {
    let Some(items) = value.get("class").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = as_id(item.get("type_id"))?;
            let name = as_str(item.get("type_name")).unwrap_or_else(|| id.clone());
            Some(Category {
                type_id: id,
                type_name: name,
            })
        })
        .collect()
}

/// `list` → 视频卡片列表。
pub fn videos(value: &Value) -> Vec<VideoItem> {
    let Some(items) = value.get("list").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = as_id(item.get("vod_id"))?;
            let name = as_str(item.get("vod_name")).unwrap_or_else(|| id.clone());
            Some(VideoItem {
                id,
                name,
                cover: as_str(item.get("vod_pic")),
                remark: as_str(item.get("vod_remarks")),
            })
        })
        .collect()
}

/// `detailContent` → 详情（取 `list[0]`）。
pub fn detail(value: &Value) -> Result<VideoDetail> {
    let item = value
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| {
            CoreError::Config("catvod.result.parse：detailContent 缺少 list[0]".to_string())
        })?;

    let id = as_id(item.get("vod_id"))
        .ok_or_else(|| CoreError::Config("catvod.result.parse：详情缺少 vod_id".to_string()))?;
    let name = as_str(item.get("vod_name")).unwrap_or_else(|| id.clone());

    let from: Vec<String> = as_str(item.get("vod_play_from"))
        .map(|s| {
            s.split("$$$")
                .map(|x| x.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    let play_url = as_str(item.get("vod_play_url")).unwrap_or_default();
    let episodes = episodes(&play_url, &from);

    Ok(VideoDetail {
        id,
        name,
        cover: as_str(item.get("vod_pic")),
        desc: as_str(item.get("vod_content")).or_else(|| as_str(item.get("vod_blurb"))),
        year: as_str(item.get("vod_year")),
        area: as_str(item.get("vod_area")),
        director: as_str(item.get("vod_director")),
        actor: as_str(item.get("vod_actor")),
        episodes,
    })
}

/// `vod_play_url` → 分集分组。
///
/// 格式：`线路1集1名$地址#集2名$地址$$$线路2…`；无标题时用 `第N集`。
pub fn episodes(play_url: &str, from: &[String]) -> Vec<EpisodeGroup> {
    if play_url.trim().is_empty() {
        return Vec::new();
    }
    play_url
        .split("$$$")
        .enumerate()
        .filter_map(|(index, group)| {
            let items: Vec<Episode> = group
                .split('#')
                .filter_map(|entry| {
                    let entry = entry.trim();
                    if entry.is_empty() {
                        return None;
                    }
                    let (title, url) = match entry.split_once('$') {
                        Some((t, u)) => (t.trim(), u.trim()),
                        None => ("", entry),
                    };
                    if url.is_empty() {
                        return None;
                    }
                    Some(Episode {
                        title: if title.is_empty() {
                            // 由调用方按序号补名；这里给空，normalize 后统一补
                            String::new()
                        } else {
                            title.to_string()
                        },
                        url: url.to_string(),
                    })
                })
                .enumerate()
                .map(|(i, mut ep)| {
                    if ep.title.is_empty() {
                        ep.title = format!("第{}集", i + 1);
                    }
                    ep
                })
                .collect::<Vec<_>>();
            if items.is_empty() {
                return None;
            }
            let group_name = from
                .get(index)
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("线路{}", index + 1));
            Some(EpisodeGroup {
                group: group_name,
                items,
            })
        })
        .collect()
}

/// `playerContent` → 播放结果。
pub fn play(value: &Value) -> Result<PlayResult> {
    let url = as_str(value.get("url"))
        .ok_or_else(|| CoreError::Config("catvod.result.parse：playerContent 缺少 url".to_string()))?;
    let parse = as_i64(value.get("parse")).unwrap_or(0) != 0 || value.get("parse") == Some(&Value::Bool(true));
    let jx = as_i64(value.get("jx")).unwrap_or(0);
    let mut headers = parse_headers(value.get("header"));

    // url 方向标记（catvod 约定）
    let sniff = parse;
    if url.starts_with("proxy://") {
        // 本地代理路由，由调用方补 host/port；这里保留原始路由
        headers.insert("X-VBox-Route".to_string(), url.clone());
    }

    Ok(PlayResult {
        url,
        headers,
        subtitles: Vec::new(),
        sniff,
        parse_name: if jx > 0 {
            Some("catvod-jx".to_string())
        } else {
            None
        },
        jx_from: if jx > 0 {
            Some(format!("jx={jx}"))
        } else {
            None
        },
        trace: Vec::new(),
    })
}

/// `header` 字段：`k=v&k2=v2` 或 `{"k":"v"}`。
pub fn parse_headers(value: Option<&Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(value) = value else { return out };
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if let Some(v) = as_str(Some(v)) {
                    out.insert(k.clone(), v);
                }
            }
        }
        Value::String(s) => {
            let t = s.trim();
            // 不少 jar 把 header 序列化成 JSON 字符串（JSONObject.toString()），
            // 直接按 & 切会得到空表；先尝试 JSON 对象，失败再回退 k=v&k2=v2。
            if t.starts_with('{') {
                if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(t) {
                    for (k, v) in &map {
                        if let Some(v) = as_str(Some(v)) {
                            out.insert(k.clone(), v);
                        }
                    }
                    return out;
                }
            }
            for pair in t.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    let k = k.trim();
                    if !k.is_empty() {
                        out.insert(k.to_string(), v.trim().to_string());
                    }
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_categories_with_numeric_ids() {
        let v: Value = serde_json::from_str(
            r#"{"class":[{"type_id":1,"type_name":"电影"},{"type_id":"2","type_name":"剧集"}]}"#,
        )
        .unwrap();
        let cats = categories(&v);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[1].type_name, "剧集");
    }

    #[test]
    fn normalizes_video_list() {
        let v: Value = serde_json::from_str(
            r#"{"list":[{"vod_id":7,"vod_name":"A","vod_pic":"p.jpg","vod_remarks":"更新至3集"}]}"#,
        )
        .unwrap();
        let items = videos(&v);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "7");
        assert_eq!(items[0].remark.as_deref(), Some("更新至3集"));
    }

    #[test]
    fn splits_episodes_into_groups() {
        let groups = episodes("第1集$http://a/1.m3u8#第2集$http://a/2.m3u8$$$线路二$http://b/1", &["line1".into(), "line2".into()]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "line1");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[1].url, "http://a/2.m3u8");
        assert_eq!(groups[1].group, "line2");
        assert_eq!(groups[1].items[0].title, "线路二");
    }

    #[test]
    fn fills_missing_group_and_episode_names() {
        let groups = episodes("$http://a/1#http://a/2", &[]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "线路1");
        assert_eq!(groups[0].items[0].title, "第1集");
        assert_eq!(groups[0].items[1].title, "第2集");
        assert_eq!(groups[0].items[1].url, "http://a/2");
    }

    #[test]
    fn keeps_group_position_when_play_from_has_empty_segment() {
        // vod_play_from 中间的空段不能把后续线路名整体前移：
        // L1$$$（空）$$$L3 必须映射到第 1/2/3 条线路。
        let v: Value = serde_json::from_str(
            r#"{"list":[{"vod_id":"1","vod_play_from":"L1$$$$$$L3",
                "vod_play_url":"1$http://a/1$$$1$http://b/1$$$1$http://c/1"}]}"#,
        )
        .unwrap();
        let d = detail(&v).unwrap();
        assert_eq!(d.episodes.len(), 3);
        assert_eq!(d.episodes[0].group, "L1");
        assert_eq!(d.episodes[1].group, "线路2");
        assert_eq!(d.episodes[2].group, "L3");
    }

    #[test]
    fn normalizes_detail_with_strings() {
        let v: Value = serde_json::from_str(
            r#"{"list":[{"vod_id":"42","vod_name":"X","vod_pic":"c.jpg","vod_content":"简介",
                "vod_year":"2024","vod_area":"大陆","vod_director":"D","vod_actor":"A",
                "vod_play_from":"l1$$$l2","vod_play_url":"1$http://a/1#2$http://a/2$$$1$http://b/1"}]}"#,
        )
        .unwrap();
        let d = detail(&v).unwrap();
        assert_eq!(d.id, "42");
        assert_eq!(d.episodes.len(), 2);
        assert_eq!(d.episodes[0].group, "l1");
        assert_eq!(d.episodes[1].items[0].url, "http://b/1");
    }

    #[test]
    fn normalizes_play_with_sniff_and_headers() {
        let v: Value = serde_json::from_str(
            r#"{"url":"https://v/x.m3u8","parse":1,"jx":1,"header":"Referer=https://a/&UA=x"}"#,
        )
        .unwrap();
        let p = play(&v).unwrap();
        assert!(p.sniff);
        assert_eq!(p.headers.get("Referer").map(String::as_str), Some("https://a/"));
        assert_eq!(p.jx_from.as_deref(), Some("jx=1"));
    }

    #[test]
    fn play_header_json_string_is_parsed() {
        let v: Value = serde_json::from_str(
            r#"{"url":"u","header":"{\"User-Agent\":\"UA\",\"Referer\":\"https://a/\"}"}"#,
        )
        .unwrap();
        let p = play(&v).unwrap();
        assert_eq!(p.headers.get("User-Agent").map(String::as_str), Some("UA"));
        assert_eq!(p.headers.get("Referer").map(String::as_str), Some("https://a/"));
    }

    #[test]
    fn play_header_object_is_supported() {
        let v: Value = serde_json::from_str(r#"{"url":"u","header":{"Cookie":"a=b"}}"#).unwrap();
        let p = play(&v).unwrap();
        assert!(!p.sniff);
        assert_eq!(p.headers.get("Cookie").map(String::as_str), Some("a=b"));
    }
}
