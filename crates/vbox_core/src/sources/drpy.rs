//! drpy/drpy2 源适配：把规则交给 Node sidecar，Rust 负责解析 TVBox JSON。
//!
//! drpy 站点典型配置：
//! `{ "api": "./lib/drpy2.min.js", "ext": "./js/rule.js", "type": 3 }`
//! `api` 是 drpy2 引擎，`ext` 是规则脚本；Node 宿主加载两者后调用
//! `init(rule)` / `home` / `category` / `detail` / `play` / `search`。

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::node;

pub fn is_drpy(site: &Site) -> bool {
    node::is_drpy_site(&site.api, &site.ext_string())
}

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let value = call(cfg, site, "home", vec![Value::Bool(true)])?;
    Ok(parse_classes(&value))
}

/// drpy 的 class 既可能是对象数组（type_id/type_name），也可能是字符串数组。
/// 旧实现只认对象，字符串形态会静默返回 0 个分类。
fn parse_classes(value: &Value) -> Vec<Category> {
    let Some(items) = value.get("class").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if let Some(text) = item.as_str() {
                if text.is_empty() {
                    return None;
                }
                return Some(Category::new(text, text));
            }
            let id = item
                .get("type_id")
                .map(|v| v.text())
                .filter(|s| !s.is_empty())?;
            let name = item
                .get("type_name")
                .map(|v| v.text())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| id.clone());
            Some(Category::new(id, name))
        })
        .collect()
}

pub fn list_videos(cfg: &TvConfig, site: &Site, tid: &str, page: u32) -> Result<Vec<VideoItem>> {
    let value = call(
        cfg,
        site,
        "category",
        vec![
            Value::String(tid.to_string()),
            Value::String(page.to_string()),
            Value::Bool(true),
            Value::Object(Default::default()),
        ],
    )?;
    Ok(parse_video_list(&value))
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let value = call(cfg, site, "detail", vec![Value::String(id.to_string())])?;
    let item = value
        .get("list")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| value.clone());
    let mut detail = VideoDetail {
        id: id.to_string(),
        name: str_field(&item, "vod_name").unwrap_or_else(|| site.name.clone()),
        cover: str_field(&item, "vod_pic"),
        desc: str_field(&item, "vod_content").or_else(|| str_field(&item, "vod_blurb")),
        year: str_field(&item, "vod_year"),
        area: str_field(&item, "vod_area"),
        director: str_field(&item, "vod_director"),
        actor: str_field(&item, "vod_actor"),
        episodes: Vec::new(),
    };
    detail.episodes = parse_episodes(&item);
    Ok(detail)
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    let value = call(
        cfg,
        site,
        "search",
        vec![Value::String(keyword.to_string()), Value::Bool(quick)],
    )?;
    Ok(parse_video_list(&value))
}

pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip: &[String],
) -> Result<Value> {
    let flags = Value::Array(vip.iter().cloned().map(Value::String).collect());
    call(
        cfg,
        site,
        "play",
        vec![
            Value::String(flag.to_string()),
            Value::String(id.to_string()),
            flags,
        ],
    )
}

fn call(cfg: &TvConfig, site: &Site, method: &str, args: Vec<Value>) -> Result<Value> {
    let ext = site.ext_string();
    if ext.trim().is_empty() {
        return Err(CoreError::Config(format!(
            "drpy 站点「{}」缺少 ext 规则脚本",
            site.name
        )));
    }
    node::call(cfg, &site.api, &ext, method, &args)
}

fn parse_video_list(value: &Value) -> Vec<VideoItem> {
    let Some(items) = value.get("list").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item
                .get("vod_id")
                .map(JsonText::text)
                .filter(|s| !s.is_empty())?;
            let name = str_field(item, "vod_name").unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            Some(VideoItem {
                id,
                name,
                cover: str_field(item, "vod_pic"),
                remark: str_field(item, "vod_remarks"),
            })
        })
        .collect()
}

fn parse_episodes(item: &Value) -> Vec<EpisodeGroup> {
    // 1) 标准 TVBox：vod_play_from / vod_play_url
    if let (Some(from), Some(urls)) = (
        item.get("vod_play_from").and_then(Value::as_str),
        item.get("vod_play_url").and_then(Value::as_str),
    ) {
        let groups: Vec<&str> = from.split("$$$").collect();
        let play_groups: Vec<&str> = urls.split("$$$").collect();
        let mut out = Vec::new();
        for (idx, play) in play_groups.iter().enumerate() {
            // vod_play_from 允许中间空段（"L1$$$L3"）：空名不能整体前移，
            // 否则后续线路名错位；缺失/空名统一回退到与集数位置一致的线路名。
            let name = groups
                .get(idx)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("线路{}", idx + 1));
            let items: Vec<Episode> = play
                .split('#')
                .filter_map(|ep| {
                    let (title, url) = ep.split_once('$')?;
                    if url.trim().is_empty() {
                        return None;
                    }
                    Some(Episode {
                        title: title.trim().to_string(),
                        url: url.trim().to_string(),
                    })
                })
                .collect();
            if !items.is_empty() {
                out.push(EpisodeGroup {
                    group: name.to_string(),
                    items,
                });
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    // 2) 自定义 episodes 数组
    let mut out = Vec::new();
    if let Some(groups) = item.get("episodes").and_then(Value::as_array) {
        for group in groups {
            let name = group
                .get("group")
                .or_else(|| group.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("默认");
            let items: Vec<Episode> = group
                .get("items")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|ep| {
                            let title = ep
                                .get("title")
                                .map(|v| v.text())
                                .unwrap_or_default();
                            // drpy 的集数 url 既可能是字符串，也可能是数字 id；
                            // 只认 as_str 会把数字 id 静默丢掉，整组变空。
                            let url = ep
                                .get("url")
                                .map(|v| v.text())
                                .filter(|s| !s.is_empty())?;
                            Some(Episode {
                                title,
                                url,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !items.is_empty() {
                out.push(EpisodeGroup {
                    group: name.to_string(),
                    items,
                });
            }
        }
    }
    out
}

fn str_field(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .map(JsonText::text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

trait JsonText {
    fn text(&self) -> String;
}

impl JsonText for Value {
    fn text(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn play_from_empty_middle_segment_keeps_positions() {
        let item = json!({
            "vod_play_from": "L1$$$$$$L3",
            "vod_play_url": "A$1#B$2$$$C$3$$$D$4"
        });
        let groups = parse_episodes(&item);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].group, "L1");
        assert_eq!(groups[1].group, "线路2");
        assert_eq!(groups[2].group, "L3");
        assert_eq!(groups[1].items[0].title, "C");
        assert_eq!(groups[1].items[0].url, "3");
    }

    #[test]
    fn episode_numeric_url_and_title_are_stringified() {
        let item = json!({
            "episodes": [{
                "group": "A",
                "items": [
                    {"title": 1, "url": 123},
                    {"title": "第二集", "url": "u2"},
                    {"title": "空", "url": ""}
                ]
            }]
        });
        let groups = parse_episodes(&item);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[0].title, "1");
        assert_eq!(groups[0].items[0].url, "123");
        assert_eq!(groups[0].items[1].url, "u2");
    }

    #[test]
    fn class_object_array_is_parsed() {
        let value = json!({"class": [
            {"type_id": "1", "type_name": "电影"},
            {"type_id": 2, "type_name": 3}
        ]});
        let out = parse_classes(&value);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].type_id, "1");
        assert_eq!(out[0].type_name, "电影");
        assert_eq!(out[1].type_id, "2");
        assert_eq!(out[1].type_name, "3");
    }

    #[test]
    fn class_string_array_uses_value_as_both_fields() {
        let value = json!({"class": ["电影", "", "电视剧"]});
        let out = parse_classes(&value);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].type_id, "电影");
        assert_eq!(out[0].type_name, "电影");
        assert_eq!(out[1].type_id, "电视剧");
    }

    #[test]
    fn play_from_missing_names_fall_back_by_position() {
        let item = json!({
            "vod_play_from": "L1",
            "vod_play_url": "A$1$$$B$2"
        });
        let groups = parse_episodes(&item);
        assert_eq!(groups[0].group, "L1");
        assert_eq!(groups[1].group, "线路2");
    }
}
