//! 苹果CMS（type=1）源站适配
//!
//! 接口约定（苹果CMS v10 常见）：
//! - 分类：从 `ac=list` 响应的 `class` 字段取；也可来自站点预置 categories
//! - 列表：`{api}?ac=list&t={typeId}&pg={page}`
//! - 详情：`{api}?ac=detail&ids={id}`
//! - 搜索：`{api}?ac=list&wd={keyword}`
//!
//! 关键坑：`vod_play_url` 的原始格式与 TVBox 订阅格式不同，见 `parse_episodes`。

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::*;
use serde::Deserialize;
use std::collections::HashMap;

/// 苹果CMS 接口里 id / typeId 常混用数字和字符串（如 `"type_id":1` vs `"type_id":"1"`）。
/// 统一收成 String，避免 serde 因类型不匹配报错。
fn string_or_number<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Str(String),
        Num(u64),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Str(s) => Ok(s),
        Raw::Num(n) => Ok(n.to_string()),
    }
}

/// 必填文本字段的宽松读法：缺失/null/结构值 → 空串，数字/布尔转文本
fn text_default<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(text_opt(deserializer)?.unwrap_or_default())
}

/// 可选文本字段：兼容数字/布尔（苹果CMS 各站对 vod_year/vod_remarks 的类型并不统一）
fn text_opt<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    })
}

/// code 字段：部分站点返回字符串（"1"），部分返回数字；无法解析时按成功处理
fn code_lenient<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(1),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(1),
        _ => 1,
    })
}

/// 顶层数组字段：缺失或 null 都按空数组（部分接口空页返回 list: null）
fn vec_or_empty<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// 拼接完整 API 地址（处理 api 末尾是否带 ?）
fn build_api(api: &str, params: &[(&str, &str)]) -> String {
    // TVBox 订阅里 api 字段常带首尾空白（如末尾空格被 URL 编码成 %20 导致请求失败），先 trim
    let api = api.trim();
    if params.is_empty() {
        return api.to_string();
    }
    // 与 type=0/type=4 相同：统一表单编码，避免搜索词里的空格/&/# 破坏查询串
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

/// 取单站级额外请求头（如 ext.header / ext.referer / ext.cookie）
fn site_extra_headers(site: &Site) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(ext) = &site.ext {
        if let Some(obj) = ext.as_object() {
            for key in ["referer", "cookie", "user-agent"] {
                if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
                    map.insert(key.to_string(), v.to_string());
                }
            }
        }
    }
    map
}

/* ---------- 内部响应结构（只取需要的字段） ---------- */

#[derive(serde::Deserialize)]
struct CmsList {
    /// 部分搜索/精简接口不返回 code；按成功处理（TVBox Gson 的默认行为）
    #[serde(default = "default_code", deserialize_with = "code_lenient")]
    code: i64,
    #[serde(default, deserialize_with = "text_default")]
    msg: String,
    #[serde(default, deserialize_with = "vec_or_empty")]
    list: Vec<CmsVideo>,
    #[serde(default, deserialize_with = "vec_or_empty")]
    class: Vec<CmsClass>,
}

fn default_code() -> i64 {
    1
}

#[derive(serde::Deserialize)]
struct CmsClass {
    #[serde(default, deserialize_with = "text_default")]
    type_id: String,
    #[serde(default, deserialize_with = "text_default")]
    type_name: String,
}

#[derive(serde::Deserialize, Default)]
struct CmsVideo {
    #[serde(default, deserialize_with = "text_default")]
    vod_id: String,
    #[serde(default, deserialize_with = "text_default")]
    vod_name: String,
    #[serde(default, deserialize_with = "text_opt")]
    vod_pic: Option<String>,
    /// 部分接口只给缩略图/幻灯片图，`vod_pic` 为空时按 TVBox 的习惯兜底
    #[serde(default, deserialize_with = "text_opt")]
    vod_pic_thumb: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_pic_slide: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_remarks: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_content: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_year: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_area: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_director: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_actor: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_play_from: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_play_url: Option<String>,
    #[serde(default, deserialize_with = "text_opt")]
    vod_play_note: Option<String>,
}

/* ---------- 对外实现 ---------- */

/// 封面兜底：`vod_pic` → `vod_pic_thumb` → `vod_pic_slide`
///
/// 苹果CMS 的 `ac=list` 摘要接口**不返回** `vod_pic`（TVBox 的列表接口用的是
/// `ac=videolist`），这是「TVBox 有封面、VBox 没有」的直接原因之一；这里再做一层
/// 字段兜底，兼容只给缩略图的接口。协议相对地址补 https，避免 WebView 自定义
/// 协议下解析成 `tauri://...`。
fn cover_of(v: &CmsVideo) -> Option<String> {
    for candidate in [&v.vod_pic, &v.vod_pic_thumb, &v.vod_pic_slide] {
        let Some(raw) = candidate.as_deref().map(str::trim) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        return Some(if let Some(rest) = raw.strip_prefix("//") {
            format!("https://{rest}")
        } else {
            raw.to_string()
        });
    }
    None
}

/// 发一次苹果CMS JSON 请求；只负责解包，不校验 `code`
fn request_cms(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &HashMap<String, String>,
    cfg: &TvConfig,
) -> Result<CmsList> {
    let text = http::get_text(client, url, headers, cfg.http.retry)?;
    serde_json::from_str(&text)
        .map_err(|e| CoreError::Config(format!("接口返回不是 JSON（url={url}）: {e}")))
}

fn ensure_code_ok(resp: &CmsList) -> Result<()> {
    if resp.code != 1 {
        return Err(CoreError::Api {
            code: resp.code,
            msg: resp.msg.clone(),
        });
    }
    Ok(())
}

/// 取分类：以接口 `class` 的真实 `type_id` 为准
///
/// 顺序对齐 TVBox：先直接 GET api（很多 api 本身已带 `?ac=list`），
/// 拿不到 class 再补一次 `ac=list&pg=1`。接口不可用时回退到站点预置分类，
/// 保证离线/接口抖动时分类栏还在。
pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let preset = &site.categories;
    let api = site.api.trim();
    if api.is_empty() {
        return if preset.is_empty() {
            Err(CoreError::Config(format!("站点「{}」的 api 为空", site.name)))
        } else {
            Ok(preset.clone())
        };
    }
    let client = http::client_for(&cfg.http)?;
    let headers = site_extra_headers(site);

    let mut api_categories: Vec<Category> = Vec::new();
    let mut last_err: Option<CoreError> = None;

    match request_cms(&client, api, &headers, cfg)
        .and_then(|resp| {
            ensure_code_ok(&resp)?;
            Ok(resp.class)
        }) {
        Ok(class) if !class.is_empty() => {
            api_categories = class
                .into_iter()
                .map(|c| Category::new(c.type_id, c.type_name))
                .collect();
        }
        Ok(_) => {}
        Err(e) => last_err = Some(e),
    }

    if api_categories.is_empty() {
        let url = build_api(api, &[("ac", "list"), ("pg", "1")]);
        match request_cms(&client, &url, &headers, cfg).and_then(|resp| {
            ensure_code_ok(&resp)?;
            Ok(resp.class)
        }) {
            Ok(class) if !class.is_empty() => {
                api_categories = class
                    .into_iter()
                    .map(|c| Category::new(c.type_id, c.type_name))
                    .collect();
            }
            Ok(_) => {}
            Err(e) => {
                if last_err.is_none() {
                    last_err = Some(e);
                }
            }
        }
    }

    if api_categories.is_empty() {
        if !preset.is_empty() {
            if let Some(e) = &last_err {
                tracing::warn!("分类接口不可用，回退站点预置分类: {e}");
            }
            return Ok(preset.clone());
        }
        return Err(last_err.unwrap_or_else(|| {
            CoreError::Config(format!("分类接口未返回 class（api={api}）"))
        }));
    }

    Ok(crate::sources::merge_preset_categories(preset, api_categories))
}

/// 取某个分类的一页列表
///
/// TVBox 的 type=1 列表接口是 `ac=videolist`（返回 `vod_pic` 等完整字段）；
/// 老接口只认 `ac=list`，因此 videolist 失败或首页为空时回退一次。
pub fn list_videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let client = http::client_for(&cfg.http)?;
    let headers = site_extra_headers(site);
    let page_str = page.to_string();
    let primary = build_api(
        &site.api,
        &[("ac", "videolist"), ("t", cat), ("pg", &page_str)],
    );
    let fallback = build_api(&site.api, &[("ac", "list"), ("t", cat), ("pg", &page_str)]);

    match fetch_video_list(&client, &primary, &headers, cfg) {
        Ok(items) => {
            // 第 1 页为空才回退：第 2 页以后为空就是正常翻到末页
            if !items.is_empty() || page > 1 {
                return Ok(items);
            }
            match fetch_video_list(&client, &fallback, &headers, cfg) {
                Ok(legacy) if !legacy.is_empty() => {
                    tracing::warn!("列表 ac=videolist 为空，已回退 ac=list");
                    Ok(legacy)
                }
                _ => Ok(items),
            }
        }
        Err(primary_err) => match fetch_video_list(&client, &fallback, &headers, cfg) {
            Ok(items) => {
                tracing::warn!("列表 ac=videolist 失败，已回退 ac=list: {primary_err}");
                Ok(items)
            }
            Err(fallback_err) => Err(CoreError::Config(format!(
                "列表接口失败（ac=videolist: {primary_err}；ac=list: {fallback_err}）"
            ))),
        },
    }
}

/// 把原始列表映射为条目：空 id 跳过，空名称回退 id（Gson 语义下缺失字段不应让整页失败）
fn to_items(list: Vec<CmsVideo>) -> Vec<VideoItem> {
    list.into_iter()
        .filter_map(|v| {
            let id = v.vod_id.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let name = if v.vod_name.trim().is_empty() {
                id.clone()
            } else {
                v.vod_name.clone()
            };
            Some(VideoItem {
                id,
                name,
                cover: cover_of(&v),
                remark: v.vod_remarks,
            })
        })
        .collect()
}

fn fetch_video_list(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &HashMap<String, String>,
    cfg: &TvConfig,
) -> Result<Vec<VideoItem>> {
    let resp = request_cms(client, url, headers, cfg)?;
    ensure_code_ok(&resp)?;
    Ok(to_items(resp.list))
}

/// 搜索（type0/1 共用；type1 追加 `ac=detail`，与 jun 的 SourceViewModel 一致）
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let client = http::client_for(&cfg.http)?;
    let params: &[(&str, &str)] = if site.kind == 1 {
        &[("wd", keyword), ("ac", "detail")]
    } else {
        &[("wd", keyword)]
    };
    let url = build_api(&site.api, params);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let resp: CmsList = serde_json::from_str(&text)
        .map_err(|e| CoreError::Config(format!("搜索接口解析失败: {e}")))?;
    if resp.code != 1 && resp.code != 0 {
        return Err(CoreError::Api {
            code: resp.code,
            msg: resp.msg,
        });
    }
    Ok(to_items(resp.list))
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let client = http::client_for(&cfg.http)?;
    let url = build_api(&site.api, &[("ac", "detail"), ("ids", id)]);
    let text = http::get_text(&client, &url, &site_extra_headers(site), cfg.http.retry)?;
    let resp: CmsList = serde_json::from_str(&text)
        .map_err(|e| CoreError::Config(format!("详情接口解析失败: {}", e)))?;
    if resp.code != 1 {
        return Err(CoreError::Api {
            code: resp.code,
            msg: resp.msg,
        });
    }
    let v = resp
        .list
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Config(format!("详情为空: id={}", id)))?;

    detail_from_video(v)
}

fn detail_from_video(v: CmsVideo) -> Result<VideoDetail> {
    let mut detail_id = v.vod_id.trim().to_string();
    let mut name = v.vod_name.trim().to_string();
    if name.is_empty() {
        name = detail_id.clone();
    }
    if detail_id.is_empty() {
        detail_id = name.clone();
    }
    if detail_id.is_empty() {
        return Err(CoreError::Config(
            "详情缺少 id/name（苹果CMS 返回了空记录）".to_string(),
        ));
    }
    let episodes = parse_episodes(
        v.vod_play_from.as_deref().unwrap_or(""),
        v.vod_play_url.as_deref().unwrap_or(""),
        v.vod_play_note.as_deref().unwrap_or(""),
    );
    let cover = cover_of(&v);
    Ok(VideoDetail {
        id: detail_id,
        name,
        cover,
        desc: v.vod_content,
        year: v.vod_year,
        area: v.vod_area,
        director: v.vod_director,
        actor: v.vod_actor,
        episodes,
    })
}

/* ---------- vod_play_url 解析（重点） ---------- */

/// 解析苹果CMS 原始播放字段为"多线路 + 多集"
///
/// 苹果CMS v10（`ac=detail` / `ac=videolist`）的真实格式：
/// - `vod_play_from`：多条线路用 `$$$` 分隔（如 `hhyun$$$hhm3u8`）；
/// - `vod_play_url`：**线路之间**用 `$$$` 分隔，**线路内部**每集是「名称$地址」，
///   集与集之间用 `#` 分隔，例如
///   `全集完结$https://play.hhuus.com/play/bYEQJMOb$$$全集完结$https://play.hhuus.com/play/bYEQJMOb/index.m3u8`
///
/// 旧实现按 `$` 切分并把每个片段都当 URL，会把「全集完结」这种集名当成播放地址，
/// 详情页因此多出无效剧集；这里改为「先分线路，再 `#` 分集，首个 `$` 分名称/地址」。
fn parse_episodes(play_from: &str, play_url: &str, _play_note: &str) -> Vec<EpisodeGroup> {
    let play_url = play_url.trim();
    if play_url.is_empty() {
        return vec![];
    }

    let line_names: Vec<&str> = if play_from.trim().is_empty() {
        Vec::new()
    } else {
        play_from.split("$$$").map(str::trim).collect()
    };
    let bodies: Vec<&str> = play_url.split("$$$").collect();

    // 多线路：`$$$` 两侧一一对应
    if bodies.len() > 1 {
        // 空体不占线路号：`$$$` 后的第一条有效线路是「线路1」，不是「线路2」。
        // 有显式线路名时仍按原下标取（名字与位置一一对应），只有回退命名按已产出条数计。
        let mut groups: Vec<EpisodeGroup> = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            let items = split_episodes_in_group(body);
            if items.is_empty() {
                continue;
            }
            let group = line_names
                .get(i)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("线路{}", groups.len() + 1));
            groups.push(EpisodeGroup { group, items });
        }
        return groups;
    }

    // 单线路：整段就是「名称$地址#名称$地址」
    let items = split_episodes_in_group(bodies[0]);
    if items.is_empty() {
        return vec![];
    }
    let group = line_names
        .first()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "默认".to_string());
    vec![EpisodeGroup { group, items }]
}

/// 线路内拆分各集：`#` 分隔每集，每集首个 `$` 前是集名、之后是地址
///
/// 没有 `$` 的条目按「只有地址」处理（生成「第N集」）；集名为空时同样编号。
fn split_episodes_in_group(body: &str) -> Vec<Episode> {
    body.split('#')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .filter_map(|(i, entry)| {
            let (title, url) = match entry.find('$') {
                Some(pos) => {
                    let title = entry[..pos].trim();
                    let url = entry[pos + 1..].trim();
                    let title = if title.is_empty() {
                        format!("第{}集", i + 1)
                    } else {
                        title.to_string()
                    };
                    (title, url)
                }
                None => (format!("第{}集", i + 1), entry),
            };
            if url.is_empty() {
                None
            } else {
                Some(Episode {
                    title,
                    url: url.to_string(),
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_names_default_to_id_and_empty_ids_are_skipped() {
        let json = r#"{"code":1,"class":[{"type_id":1}],"list":[{"vod_id":7},{"vod_id":"","vod_name":"x"},{"vod_id":8,"vod_name":2024}]}"#;
        let resp: CmsList = serde_json::from_str(json).unwrap();
        assert_eq!(resp.class[0].type_id, "1");
        assert_eq!(resp.class[0].type_name, "");
        let items = to_items(resp.list);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "7");
        assert_eq!(items[0].name, "7");
        assert_eq!(items[1].id, "8");
        assert_eq!(items[1].name, "2024");
    }

    #[test]
    fn build_api_form_encodes_keywords_and_keeps_existing_query() {
        let url = build_api("https://a/api.php/provide/vod", &[("wd", "海 贼&x")]);
        assert_eq!(
            url,
            "https://a/api.php/provide/vod?wd=%E6%B5%B7+%E8%B4%BC%26x"
        );
        let url = build_api("https://a/api.php/provide/vod?", &[("ac", "list")]);
        assert_eq!(url, "https://a/api.php/provide/vod?ac=list");
        let url = build_api("https://a/api.php/provide/vod?key=1", &[("ac", "list")]);
        assert_eq!(url, "https://a/api.php/provide/vod?key=1&ac=list");
    }

    #[test]
    fn leading_empty_line_does_not_consume_group_number() {
        let groups = parse_episodes("", "$$$第1集$https://v/1.m3u8", "");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "线路1");
        assert_eq!(groups[0].items[0].title, "第1集");
    }

    #[test]
    fn detail_id_falls_back_to_name_and_empty_record_is_rejected() {
        let json = r#"{"code":1,"list":[{"vod_name":"有名无 id"}]}"#;
        let resp: CmsList = serde_json::from_str(json).unwrap();
        let d = detail_from_video(resp.list.into_iter().next().unwrap()).unwrap();
        assert_eq!(d.id, "有名无 id");
        assert_eq!(d.name, "有名无 id");

        let json = r#"{"code":1,"list":[{"vod_pic":"c"}]}"#;
        let resp: CmsList = serde_json::from_str(json).unwrap();
        let err = detail_from_video(resp.list.into_iter().next().unwrap()).unwrap_err();
        assert!(format!("{err}").contains("缺少 id/name"));
    }

    #[test]
    fn null_containers_and_msg_are_tolerated() {
        let json = r#"{"code":1,"msg":null,"list":null,"class":null}"#;
        let resp: CmsList = serde_json::from_str(json).unwrap();
        assert_eq!(resp.code, 1);
        assert!(resp.msg.is_empty());
        assert!(resp.list.is_empty());
        assert!(resp.class.is_empty());
    }

    #[test]
    fn tolerates_numeric_optional_fields_and_string_code() {
        let json = r#"{"code":"1","list":[{"vod_id":7,"vod_name":"n","vod_year":2024,"vod_remarks":2024,"vod_play_from":"l","vod_play_url":"1$u"}]}"#;
        let resp: CmsList = serde_json::from_str(json).unwrap();
        assert_eq!(resp.code, 1);
        let v = &resp.list[0];
        assert_eq!(v.vod_year.as_deref(), Some("2024"));
        assert_eq!(v.vod_remarks.as_deref(), Some("2024"));
        assert_eq!(v.vod_play_from.as_deref(), Some("l"));
    }

    #[test]
    fn test_parse_cms_real_lines() {
        // 真实「豪华资源」详情：两条线路，各一集，集名相同、地址不同
        let groups = parse_episodes(
            "hhyun$$$hhm3u8",
            "全集完结$https://play.hhuus.com/play/bYEQJMOb$$$全集完结$https://play.hhuus.com/play/bYEQJMOb/index.m3u8",
            "",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "hhyun");
        assert_eq!(groups[0].items.len(), 1);
        assert_eq!(groups[0].items[0].title, "全集完结");
        assert_eq!(groups[0].items[0].url, "https://play.hhuus.com/play/bYEQJMOb");
        assert_eq!(groups[1].group, "hhm3u8");
        assert_eq!(groups[1].items.len(), 1);
        assert_eq!(
            groups[1].items[0].url,
            "https://play.hhuus.com/play/bYEQJMOb/index.m3u8"
        );
    }

    #[test]
    fn test_parse_cms_multi_episode_single_line() {
        let groups = parse_episodes(
            "线路1",
            "第01集$https://a/1.m3u8#第02集$https://a/2.m3u8",
            "",
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "线路1");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[1].title, "第02集");
        assert_eq!(groups[0].items[1].url, "https://a/2.m3u8");
    }

    #[test]
    fn test_parse_cms_address_only() {
        let groups = parse_episodes("", "https://a/1.m3u8#https://a/2.m3u8", "");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "默认");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[0].title, "第1集");
        assert_eq!(groups[0].items[0].url, "https://a/1.m3u8");
    }

    #[test]
    fn test_parse_cms_empty() {
        assert!(parse_episodes("", "", "").is_empty());
        assert!(parse_episodes("线路1", "$$$", "").is_empty());
    }

    #[test]
    fn test_cover_fallback_and_protocol_relative() {
        let mut v = CmsVideo::default();
        assert_eq!(cover_of(&v), None);
        v.vod_pic_thumb = Some("  ".into());
        v.vod_pic_slide = Some("//img.example.com/a.jpg".into());
        assert_eq!(
            cover_of(&v).as_deref(),
            Some("https://img.example.com/a.jpg")
        );
        v.vod_pic = Some("https://img.example.com/b.jpg".into());
        assert_eq!(
            cover_of(&v).as_deref(),
            Some("https://img.example.com/b.jpg")
        );
    }
}
