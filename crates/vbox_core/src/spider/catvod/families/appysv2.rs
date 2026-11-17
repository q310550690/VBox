//! `csp_AppYsV2` 原生实现：通用「API v2」CMS 适配器。
//!
//! 反编译依据：`_rea/decompiled/ddaf13d7-0206/sources/com/github/catvod/spider/AppYsV2.java`
//! （1004 行、0 skip；同协议另有 1010/1083/1098 行变体）。原类 `extends Spider`，
//! 不依赖任何网盘引擎/WebView，是「按 ext 基址形态切换路由与 JSON 结构」的通用适配器：
//!
//! ```text
//! ext   : "{base}###{flag}"（原类用 "###" 切分，不是 "$$$"）
//! 形态  : api.php/app 或 xgapp -> App；含 ".vod" -> Vod；其余 -> Generic（api.php/**\/vod）
//! 列表  : App: {base}video?tid={cat}&class=&area=&lang=&year=&limit=18&pg={page}
//!         Vod: {base}?type={cat}&class=&area=&lang=&year=&by=&limit=18&page={page}
//!         Gen: {base}?ac=list&class={cat}&page={page}&area=&type=&start=
//! 分类  : App: {base}nav?token=   Vod: {base}/types（iopenyun: /list?type）  Gen: 写死
//! 详情  : App: {base}video_detail?id=   Vod: {base}/detail?vod_id=   Gen: 空（列表即详情）
//! 搜索  : App: {base}search?text={kw}&pg=   Vod: {base}?wd={kw}&page=   Gen: {base}?ac=list&zm=
//! 首页  : App: {base}index_video?token=   Vod: {base}/vodPhbAll   Gen: 列表第一页
//! ```
//!
//! 与原实现的有意偏差（记档）：
//! 1. `parse_api` 解析链（`RC.jw` 提取播放地址）未移植：非直链时按原类兜底返回
//!    `parse:1, jx:"1"`（桌面端由宿主播放器/JX 处理）；
//! 2. 首页 filters（`type_extend` 的 分类/地区/语言/年份 筛选）未接入 dispatch
//!    （`sources::spider::list_videos` 无 filters 参数），分类页按「全部」请求；
//! 3. 原类对 `iopenyun` / `dijiaxia` / `1010dy` 等主机的硬编码基址改为「按 ext 基址」
//!    （这些主机的详情/搜索路径由形态规则推导，不再写死域名）；
//! 4. 分类兜底只保留 6 个通用分类（电影/连续剧/综艺/动漫/4K/体育）；
//! 5. 列表 item 字段同时兼容 `vod_*` 与 `nextlink/title/pic/state` 两套（同原类）。

use std::collections::HashMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_AppYsV2";
/// 旧版同名类（AppYs）与 V2 共用同一套 `api.php/app` 协议与端点，
/// 仅 ext 通常是裸基址（没有 `###flag`）、播放器解析表更旧。
pub const API_LEGACY: &str = "csp_AppYs";

const UA_DART14: &str = "Dart/2.14 (dart:io)";
const UA_DART15: &str = "Dart/2.15 (dart:io)";
const UA_OKHTTP: &str = "okhttp/4.1.0";
const UA_DALVIK: &str = "Dalvik/2.1.0";

/// ext 分隔符：原类用 `###` 拆成 [base, flag]。
const EXT_SEP: &str = "###";

/// 通用形态的兜底分类（与原类 `P()` 中写死的第一段一致）。
const GENERIC_CATEGORIES: [(&str, &str); 6] = [
    ("movie", "电影"),
    ("tvplay", "连续剧"),
    ("tvshow", "综艺"),
    ("comic", "动漫"),
    ("movie_4k", "4K"),
    ("tiyu", "体育"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `api.php/app` 或 `xgapp` 形态。
    App,
    /// 含 `.vod` 的苹果 CMS 形态。
    Vod,
    /// `api.php/.../vod` 的通用形态。
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ext {
    pub base: String,
    pub flag: Option<String>,
}

/// 判断 api 是否归本家族。
pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API) || api.eq_ignore_ascii_case(API_LEGACY)
}

/// 解析 ext：`{base}###{flag}`；base 末尾补 `/` 之外保持原样（沿用原类拼接习惯）。
pub fn parse_ext(site: &Site) -> Result<Ext> {
    let raw = site.ext_string();
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(parse_err(site, "ext 为空，无法确定 API 基址"));
    }
    let mut parts = raw.split(EXT_SEP);
    let base = parts.next().unwrap_or("").trim().to_string();
    if base.is_empty() {
        return Err(parse_err(site, "ext 缺少基址"));
    }
    let flag = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(Ext { base, flag })
}

/// 基址形态判定。
pub fn classify(base: &str) -> Kind {
    if base.contains("api.php/app") || base.contains("xgapp") {
        return Kind::App;
    }
    if base.contains(".vod") {
        return Kind::Vod;
    }
    Kind::Generic
}

/// 原类 `j()` 的 UA 选择逻辑（按基址特征；保留主机名录的语义但收敛为关键字）。
pub fn user_agent(base: &str) -> &'static str {
    if base.contains("vod.9e03.com") {
        return "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/103.0.0.0 Mobile Safari/537.36";
    }
    if base.contains("api.php/app") || base.contains("xgapp") || base.contains("freekan") {
        return UA_DART14;
    }
    if ["zsb", "fkxs", "xays", "xcys", "szys", "dxys", "ytys", "qnys"]
        .iter()
        .any(|k| base.contains(k))
    {
        return UA_DART15;
    }
    if base.contains(".vod") {
        return UA_OKHTTP;
    }
    UA_DALVIK
}

pub fn headers(base: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), user_agent(base).to_string());
    map
}

fn fetch_text(cfg: &TvConfig, site: &Site, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let hdrs = headers(base);
    crate::http::get_text(&client, url, &hdrs, 2).map_err(|e| {
        CatvodError::new(ErrorClass::HostApi, site, format!("请求失败: {url}: {e}")).into_core()
    })
}

fn parse_err(site: &Site, msg: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, msg).into_core()
}

fn host_err(site: &Site, msg: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, msg).into_core()
}

fn encode(text: &str) -> String {
    utf8_percent_encode(text, NON_ALPHANUMERIC).to_string()
}

/// 分类源（与原类 `l()` 等价）。
pub fn nav_url(base: &str) -> Option<String> {
    match classify(base) {
        Kind::App => Some(format!("{base}nav?token=")),
        Kind::Vod => {
            if base.contains("iopenyun.com") {
                Some(format!("{base}/list?type"))
            } else {
                Some(format!("{base}/types"))
            }
        }
        Kind::Generic => None,
    }
}

/// 首页视频源（与原类 `PG()` 等价）。
pub fn home_video_url(base: &str) -> Option<String> {
    match classify(base) {
        Kind::App => Some(format!("{base}index_video?token=")),
        Kind::Vod => Some(format!("{base}/vodPhbAll")),
        Kind::Generic => None,
    }
}

/// 分类页 URL（与原类 `v(base)+cat+Fa(base)` 等价；筛选一律留空）。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    match classify(base) {
        Kind::App => format!(
            "{base}video?tid={cat}&class=&area=&lang=&year=&limit=18&pg={page}"
        ),
        Kind::Vod => format!(
            "{base}?type={cat}&class=&area=&lang=&year=&by=&limit=18&page={page}"
        ),
        Kind::Generic => format!("{base}?ac=list&class={cat}&page={page}&area=&type=&start="),
    }
}

/// 详情 URL（与原类 `K()` 等价）。
pub fn detail_url(base: &str, id: &str) -> Option<String> {
    match classify(base) {
        Kind::App => Some(format!("{base}video_detail?id={id}")),
        Kind::Vod => {
            if base.contains("iopenyun.com") {
                Some(format!("{base}/detailID?vod_id={id}"))
            } else {
                Some(format!("{base}/detail?vod_id={id}"))
            }
        }
        Kind::Generic => None,
    }
}

/// 搜索 URL（与原类 `a()` 等价）。
pub fn search_url(base: &str, keyword: &str) -> Option<String> {
    let kw = encode(keyword);
    match classify(base) {
        Kind::App => Some(format!("{base}search?text={kw}&pg=")),
        Kind::Vod => {
            if base.contains("iopenyun.com") {
                Some(format!("{base}/list?wd={kw}&page="))
            } else {
                Some(format!("{base}?wd={kw}&page="))
            }
        }
        Kind::Generic => {
            let mut buf = String::new();
            buf.push_str(base);
            // 各主机对关键字参数名不一致，与原类一致：只有少数主机用 wd，其余用 zm
            if base.contains("api.php/") {
                buf.push_str("?ac=list&zm=");
            } else {
                return None;
            }
            buf.push_str(&kw);
            buf.push_str("&page=");
            Some(buf)
        }
    }
}

fn array_of(value: &Value) -> Option<Vec<Value>> {
    if let Some(list) = value.get("list").and_then(Value::as_array) {
        return Some(list.clone());
    }
    if let Some(list) = value
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("list"))
        .and_then(Value::as_array)
    {
        return Some(list.clone());
    }
    value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
}

fn str_field(item: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = item.get(*key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
        if let Some(number) = item.get(*key).and_then(Value::as_i64) {
            return number.to_string();
        }
    }
    String::new()
}

fn join_array(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| match v {
                Value::String(s) => s.trim().to_string(),
                other => other.to_string().trim_matches('"').to_string(),
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// 列表条目（兼容 `vod_*` 与 `nextlink/title/pic/state` 两套字段）。
pub fn parse_items(value: &Value) -> Vec<VideoItem> {
    let Some(list) = array_of(value) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in list {
        let id = str_field(&entry, &["vod_id", "nextlink", "id"]);
        let name = str_field(&entry, &["vod_name", "title", "name"]);
        if id.is_empty() || name.is_empty() {
            continue;
        }
        let cover = str_field(&entry, &["vod_pic", "pic", "img_url", "cover"]);
        let remark = str_field(&entry, &["vod_remarks", "state", "notes"]);
        items.push(VideoItem {
            id,
            name,
            cover: (!cover.is_empty()).then_some(cover),
            remark: (!remark.is_empty()).then_some(remark),
        });
    }
    items
}

/// 分页数（`totalpage` / `pagecount` / `data.total`+`data.limit`）。
pub fn parse_page_count(value: &Value) -> Option<u32> {
    for key in ["totalpage", "pagecount"] {
        if let Some(n) = value.get(key).and_then(Value::as_i64) {
            return Some(n.max(1) as u32);
        }
    }
    let data = value.get("data")?;
    let total = data.get("total").and_then(Value::as_i64)?;
    let limit = data.get("limit").and_then(Value::as_i64).filter(|n| *n > 0)?;
    let pages = if total % limit == 0 { total / limit } else { total / limit + 1 };
    Some(pages.max(1) as u32)
}

/// 分类项（`list` / `data.list` / `data[]`，字段 `type_id/type_name` 或 `id/name`）。
pub fn parse_categories(value: &Value) -> Vec<Category> {
    let Some(list) = array_of(value) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in list {
        let id = str_field(&entry, &["type_id", "id", "tid"]);
        let name = str_field(&entry, &["type_name", "name", "title"]);
        if id.is_empty() || name.is_empty() {
            continue;
        }
        out.push(Category { type_id: id, type_name: name });
    }
    out
}

fn episodes_from_pairs(pairs: &[(String, String)]) -> Vec<EpisodeGroup> {
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for (line, raw) in pairs {
        let line = if line.trim().is_empty() { "默认" } else { line.trim() };
        let idx = match groups.iter().position(|g: &EpisodeGroup| g.group == line) {
            Some(idx) => idx,
            None => {
                groups.push(EpisodeGroup { group: line.to_string(), items: Vec::new() });
                groups.len() - 1
            }
        };
        for (n, chunk) in raw.split('#').enumerate() {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            let (title, url) = match chunk.split_once('$') {
                Some((title, url)) => (title.trim().to_string(), url.trim().to_string()),
                None => (format!("第{}集", n + 1), chunk.to_string()),
            };
            if url.is_empty() {
                continue;
            }
            groups[idx].items.push(Episode { title, url });
        }
    }
    groups.retain(|g| !g.items.is_empty());
    groups
}

fn some_if(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

fn first_non_empty(candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .find(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
}

fn fill_common(detail: &mut VideoDetail, node: &Value) {
    detail.name = str_field(node, &["vod_name", "title"]);
    detail.cover = some_if(str_field(node, &["vod_pic", "img_url", "pic"]));
    detail.desc = some_if(str_field(node, &["vod_content", "intro"]));
    detail.year = some_if(str_field(node, &["vod_year", "pubtime"]));
    detail.area = first_non_empty(&[str_field(node, &["vod_area"]), join_array(node.get("area"))]);
    detail.director = first_non_empty(&[
        str_field(node, &["vod_director"]),
        join_array(node.get("director")),
    ]);
    detail.actor = first_non_empty(&[
        str_field(node, &["vod_actor"]),
        join_array(node.get("actor")),
    ]);
}

/// 详情解析（App/Vod/Generic 三种结构）。
pub fn parse_detail(value: &Value, kind: Kind, id: &str) -> VideoDetail {
    let mut detail = VideoDetail { id: id.to_string(), ..Default::default() };
    let mut pairs: Vec<(String, String)> = Vec::new();
    match kind {
        Kind::App => {
            if let Some(node) = value.get("data") {
                fill_common(&mut detail, node);
                if let Some(players) = node.get("vod_url_with_player").and_then(Value::as_array) {
                    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
                    for player in players {
                        let line =
                            str_field(player, &["code", "name"]).replace(' ', "");
                        let url = str_field(player, &["url"]);
                        if line.is_empty() || url.is_empty() {
                            continue;
                        }
                        grouped.entry(line).or_default().push(url);
                    }
                    pairs = grouped
                        .into_iter()
                        .map(|(line, urls)| (line, urls.join("#")))
                        .collect();
                }
            }
        }
        Kind::Vod => {
            if let Some(node) = value.get("data") {
                fill_common(&mut detail, node);
                if let Some(play_list) = node.get("vod_play_list").and_then(Value::as_array) {
                    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
                    for entry in play_list {
                        let info = entry.get("player_info").cloned().unwrap_or(Value::Null);
                        let mut line = str_field(&info, &["from"]);
                        if line.is_empty() {
                            line = str_field(&info, &["show"]);
                        }
                        let url = str_field(entry, &["url"]);
                        if line.is_empty() || url.is_empty() {
                            continue;
                        }
                        grouped.entry(line).or_default().push(url);
                    }
                    pairs = grouped
                        .into_iter()
                        .map(|(line, urls)| (line, urls.join("#")))
                        .collect();
                }
            }
        }
        Kind::Generic => {
            fill_common(&mut detail, value);
            if let Some(videolist) = value.get("videolist").and_then(Value::as_object) {
                for (line, raw) in videolist {
                    let Some(list) = raw.as_array() else { continue };
                    let mut chunks = Vec::new();
                    for (n, entry) in list.iter().enumerate() {
                        let title = str_field(entry, &["title", "name"]);
                        let mut url = str_field(entry, &["url"]);
                        if let Some(pos) = url.find("url=") {
                            url = url[pos + 4..].to_string();
                        }
                        if url.is_empty() {
                            continue;
                        }
                        let name =
                            if title.is_empty() { format!("第{}集", n + 1) } else { title };
                        chunks.push(format!("{name}${url}"));
                    }
                    if !chunks.is_empty() {
                        pairs.push((line.clone(), chunks.join("#")));
                    }
                }
            }
        }
    }
    // 组顺序稳定化：原实现用 HashMap 收集线路名，顺序不稳定；按线路名排序便于断言与展示
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    detail.episodes = episodes_from_pairs(&pairs);
    detail
}

fn video_ext_regex() -> &'static Regex {
    static CELL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(
            r"(?i)\.(m3u8|mp4|flv|mkv|avi|ts|mov|wmv|rmvb|webm|m4v|mpd|mp3|m4a|m4s)(\?|#|$)",
        )
        .expect("video ext regex")
    })
}

/// 是否直链视频地址（与原类 `RC.Pd()` 的用途对齐）。
pub fn is_video_url(url: &str) -> bool {
    video_ext_regex().is_match(url)
}

/// 分类：App 走 nav、Vod 走 types、Generic 走写死分类。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let ext = parse_ext(site)?;
    let base = ext.base.as_str();
    if let Some(url) = nav_url(base) {
        let text = fetch_text(cfg, site, base, &url)?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| parse_err(site, format!("分类响应不是 JSON: {e}")))?;
        let cats = parse_categories(&value);
        if cats.is_empty() {
            return Err(parse_err(site, "分类响应为空"));
        }
        return Ok(cats);
    }
    Ok(GENERIC_CATEGORIES
        .iter()
        .map(|(id, name)| Category { type_id: (*id).to_string(), type_name: (*name).to_string() })
        .collect())
}

/// 分类列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let ext = parse_ext(site)?;
    let base = ext.base.as_str();
    let url = list_url(base, cat, page);
    let text = fetch_text(cfg, site, base, &url)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("列表响应不是 JSON: {e}")))?;
    Ok(parse_items(&value))
}

/// 详情。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let ext = parse_ext(site)?;
    let base = ext.base.as_str();
    let kind = classify(base);
    let Some(url) = detail_url(base, id) else {
        return Err(host_err(site, "该形态（Generic）没有独立详情接口"));
    };
    let text = fetch_text(cfg, site, base, &url)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("详情响应不是 JSON: {e}")))?;
    let detail = parse_detail(&value, kind, id);
    if detail.name.is_empty() {
        return Err(parse_err(site, "详情缺少标题"));
    }
    Ok(detail)
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let ext = parse_ext(site)?;
    let base = ext.base.as_str();
    let Some(prefix) = search_url(base, keyword) else {
        return Ok(Vec::new());
    };
    let text = fetch_text(cfg, site, base, &prefix)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("搜索响应不是 JSON: {e}")))?;
    Ok(parse_items(&value))
}

/// 播放：直链直接 `parse:0`；否则与原类兜底一致，交给 JX（`parse:1, jx:"1"`）。
pub fn player_content(
    _cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    let url = id.trim();
    if url.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    if is_video_url(url) {
        return Ok(serde_json::json!({
            "url": url,
            "parse": 0,
            "jx": 0,
            "playUrl": "",
            "header": { "User-Agent": UA_DALVIK },
        }));
    }
    Ok(serde_json::json!({
        "url": url,
        "parse": 1,
        "jx": "1",
        "playUrl": "",
        "header": { "User-Agent": user_agent("") },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        let mut site = Site::default();
        site.key = "appysv2".to_string();
        site.name = "AppYsV2".to_string();
        site.api = API.to_string();
        site.ext = Some(serde_json::json!(ext));
        site
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_AppYsV2"));
        assert!(handles("CSP_APPYSV2"));
        assert!(handles("csp_AppYs"));
        assert!(handles("CSP_APPYS"));
        assert!(!handles("csp_AppYsV3"));
    }

    #[test]
    fn parse_ext_splits_triple_hash() {
        let ext = parse_ext(&site("http://a.test/api.php/app/###nftv")).unwrap();
        assert_eq!(ext.base, "http://a.test/api.php/app/");
        assert_eq!(ext.flag.as_deref(), Some("nftv"));
        let ext = parse_ext(&site("http://a.test/api.php/v1.vod")).unwrap();
        assert_eq!(ext.flag, None);
        assert!(parse_ext(&site("")).is_err());
    }

    #[test]
    fn classify_three_shapes() {
        assert_eq!(classify("http://x/api.php/app/"), Kind::App);
        assert_eq!(classify("http://x/xgapp/vod/"), Kind::App);
        assert_eq!(classify("http://x/api.php/v1.vod"), Kind::Vod);
        assert_eq!(classify("http://x/api.php/v1/vod"), Kind::Generic);
    }

    #[test]
    fn user_agent_matches_original_rules() {
        assert!(user_agent("http://x/api.php/app/").starts_with("Dart/2.14"));
        assert_eq!(user_agent("http://x/api.php/v1.vod"), "okhttp/4.1.0");
        assert_eq!(user_agent("http://x/api.php/v1/vod"), "Dalvik/2.1.0");
    }

    #[test]
    fn list_urls_per_shape() {
        assert_eq!(
            list_url("http://x/api.php/app/", "1", 2),
            "http://x/api.php/app/video?tid=1&class=&area=&lang=&year=&limit=18&pg=2"
        );
        assert_eq!(
            list_url("http://x/api.php/v1.vod", "2", 3),
            "http://x/api.php/v1.vod?type=2&class=&area=&lang=&year=&by=&limit=18&page=3"
        );
        assert_eq!(
            list_url("http://x/api.php/v1/vod", "3", 1),
            "http://x/api.php/v1/vod?ac=list&class=3&page=1&area=&type=&start="
        );
    }

    #[test]
    fn detail_and_search_urls() {
        assert_eq!(
            detail_url("http://x/api.php/app/", "9").unwrap(),
            "http://x/api.php/app/video_detail?id=9"
        );
        assert_eq!(
            detail_url("http://x/api.php/v1.vod", "9").unwrap(),
            "http://x/api.php/v1.vod/detail?vod_id=9"
        );
        assert!(detail_url("http://x/api.php/v1/vod", "9").is_none());
        assert_eq!(
            search_url("http://x/api.php/app/", "海 贼").unwrap(),
            "http://x/api.php/app/search?text=%E6%B5%B7%20%E8%B4%BC&pg="
        );
        assert_eq!(
            search_url("http://x/api.php/v1.vod", "a").unwrap(),
            "http://x/api.php/v1.vod?wd=a&page="
        );
    }

    #[test]
    fn parse_items_both_field_sets() {
        let value = serde_json::json!({
            "list": [
                {"vod_id": 1, "vod_name": "A", "vod_pic": "p", "vod_remarks": "HD"},
                {"nextlink": "2", "title": "B", "pic": "q", "state": "更新"}
            ]
        });
        let items = parse_items(&value);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].name, "B");
        assert_eq!(items[1].remark.as_deref(), Some("更新"));
    }

    #[test]
    fn parse_page_count_variants() {
        let v = serde_json::json!({"totalpage": 5});
        assert_eq!(parse_page_count(&v), Some(5));
        let v = serde_json::json!({"data": {"total": 21, "limit": 10}});
        assert_eq!(parse_page_count(&v), Some(3));
        assert_eq!(parse_page_count(&serde_json::json!({})), None);
    }

    #[test]
    fn parse_categories_fields() {
        let v = serde_json::json!({"data": [{"type_id": "1", "type_name": "电影"}]});
        let cats = parse_categories(&v);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "1");
    }

    #[test]
    fn parse_detail_app_shape() {
        let value = serde_json::json!({
            "data": {
                "vod_name": "剧", "vod_pic": "c", "vod_content": "简介",
                "vod_url_with_player": [
                    {"code": "qq", "url": "https://v.qq.com/1.m3u8"},
                    {"code": "qq", "url": "https://v.qq.com/2.m3u8"},
                    {"name": "youku", "url": "https://v.youku.com/a.m3u8"}
                ]
            }
        });
        let d = parse_detail(&value, Kind::App, "42");
        assert_eq!(d.id, "42");
        assert_eq!(d.name, "剧");
        assert_eq!(d.episodes.len(), 2);
        assert_eq!(d.episodes[0].items.len(), 2);
    }

    #[test]
    fn parse_detail_vod_and_generic_shapes() {
        let vod = serde_json::json!({
            "data": {
                "vod_name": "V", "vod_play_list": [
                    {"player_info": {"from": "l1"}, "url": "第1集$https://a/1.m3u8#第2集$https://a/2.m3u8"}
                ]
            }
        });
        let d = parse_detail(&vod, Kind::Vod, "1");
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].items.len(), 2);

        let generic = serde_json::json!({
            "vod_id": "g1", "title": "G", "img_url": "c", "intro": "i",
            "videolist": {"line": [{"title": "正片", "url": "https://p/1.m3u8"}]}
        });
        let d = parse_detail(&generic, Kind::Generic, "g1");
        assert_eq!(d.name, "G");
        assert_eq!(d.episodes[0].items[0].title, "正片");
    }

    #[test]
    fn player_prefers_direct_video() {
        let cfg = TvConfig::default();
        let s = site("http://x/api.php/app/");
        let direct = player_content(&cfg, &s, "", "https://cdn/a.m3u8").unwrap();
        assert_eq!(direct["parse"], 0);
        let fallback = player_content(&cfg, &s, "", "https://v.qq.com/x/cover/abc.html").unwrap();
        assert_eq!(fallback["parse"], 1);
        assert!(player_content(&cfg, &s, "", "  ").is_err());
    }
}
