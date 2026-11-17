//! `csp_TTian`（天天影视 / 追剧 · App API v2）原生实现。
//!
//! 来源：`_rea/decompiled/dffec63f-pg/sources/com/github/catvod/spider/TTian.java`
//! （642 行；`detailContent` 经 `jadx --show-bad-code --single-class` 恢复为
//! `_rea/tmp/TTian.recovered.java`（842 行），字符串经 XOR 解码器还原）。
//! 原类 `extends Spider`，直接对 App API v2 发 **multipart/form-data** 请求：
//! * `init`：ext 按 `$$$` 切分 —— `[0]` 基址；`[1]` 长度 >7 时作为播放代理（`this.d`）；
//!   `[2] == "1"` 开启弹幕（`this.c`）；
//! * 签名：`sign = lower(md5(KEY + 秒级时间戳))`，`KEY = kj5649ertj84ks89r4jh8s45hf84hjfds04k`
//!   （`J.d` 解出 36 字节密钥后拼接时间戳，`C0097B.k` 为 MD5 hex 小写）；
//! * 端点（均 POST）：
//!   - 首页分类 `/v2/type/top_type` → `data.list[].type_id/type_name`
//!   - 首页推荐 `/v2/type/tj_vod` → `data.cai[].vod_*`
//!   - 分类列表 `/v2/home/type_search`（字段 sign/timestamp/limit=12/type_id/page）
//!     → `data.list[]`，输出 `page/pagecount:100/limit:12/total:1200`
//!   - 详情 `/v2/home/vod_details`（字段 sign/timestamp/vod_id）
//!     → `data`（vod_* + `vod_play_list[].{title,parse_urls,urls}`）
//!   - 搜索 `/v2/home/search`（字段 sign/timestamp/keyword）
//! * 详情线路：`parse_urls` 0 个 → 标记 `"no"`；1 个 → 取第 1 个；>1 → 取第 2 个，
//!   剧集 `name`/`url` 组合为 `url|flag`；播放时 flag 长度 <3 视为直链，
//!   否则请求 `flag + url + "&sign=" + sign` 换取真实播放地址（m3u8 兜底直链，
//!   已知站点域名则 `parse:1`）。
//!
//! 显式偏差（均有原因，避免伪造上游行为）：
//! 1. **上游已死**：三个 ext 主机（`http://op.ysdqjs.cn`、`http://app.kzjtv.com`）
//!    2026-11 探测全部不可达（fetch failed / 8s 超时，https 同），因此本家族只能 mock 验收，
//!    真站用例以 `#[ignore]` 记录且标注上游不可达；
//! 2. 原实现把播放地址包装成 App 内置本地代理
//!    （`http://127.0.0.1:10079/p/0/<proxy>/<urlenc>`，`C0124a.u`），桌面端无该代理进程，
//!    本实现直接返回上游地址；`[1]` 代理段仅解析保留、不参与播放；
//! 3. 弹幕（`C0105h.h`）依赖宿主 WebView/App 缓存，不实现；
//! 4. 首页 `filters` 硬编码 JSON（4 大类 类型/地区/年份）依赖 App 的筛选交互，
//!    桌面 dispatch 的 `list_videos` 无 filters 参数，故分类页固定按「全部」请求。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 画像里的 api id。
pub const API: &str = "csp_TTian";
/// 请求头 UA（原类 `a()` 返回的固定值）。
pub const UA: &str = "okhttp-okgo/jeasonlzy";
/// 签名密钥（36 字节，原类每个 helper 的字节数组解码结果一致）。
pub const SIGN_KEY: &str = "kj5649ertj84ks89r4jh8s45hf84hjfds04k";
/// 分类列表接口的固定 boundary。
pub const FIXED_BOUNDARY: &str = "4b8a0b47-84bc-4b6e-b252-db25b4b9abd2";
/// 随机 boundary 前缀（`formUpload`/`formUploadDetail`/`formUploadSearch`）。
const DIO_PREFIX: &str = "--dio-boundary-";
/// ext 缺失时的兜底基址（与原类「ext[0] 即基址」一致，取一个历史站点的域名）。
pub const DEFAULT_BASE: &str = "http://op.ysdqjs.cn";
/// 单个线路最多收集的剧集数（防御异常响应）。
const MAX_EPISODES: usize = 500;
/// `data.list` 为空时的兜底分类（原类 `homeContent(z)` 里 filters 的 4 个 type id）。
pub const CAT_FALLBACK: [(&str, &str); 4] = [
    ("1", "电影"),
    ("2", "电视剧"),
    ("4", "动漫"),
    ("3", "综艺"),
];
/// `playerContent` 里 `C0097B.N` 的「已知视频站点」判据（命中则 `parse:1`）。
const VIDEO_SITES: [&str; 11] = [
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

pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn play_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::PlayParse, site, detail).into_core()
}

fn net_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Network, site, detail).into_core()
}

/// ext 按 `$$$` 切分并 trim（原类 `init` 的 `split("\\$\\$\\$")`）。
fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

/// 基址：ext 第 1 段（原类 `this.a = split[0]`）。
pub fn base_of(site: &Site) -> String {
    let segs = ext_segments(site);
    let base = segs
        .first()
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_default();
    if base.is_empty() {
        DEFAULT_BASE.to_string()
    } else {
        base
    }
}

/// 播放代理：ext 第 2 段长度 >7 时生效（原类 `split[1].length() > 7`）。
pub fn proxy_of(site: &Site) -> Option<String> {
    ext_segments(site)
        .get(1)
        .filter(|seg| seg.len() > 7 && (seg.starts_with("http://") || seg.starts_with("https://")))
        .cloned()
}

/// 弹幕开关：ext 第 3 段为 `"1"`（原类 `this.c`，本实现只保留解析、不实现弹幕）。
pub fn danmaku_enabled(site: &Site) -> bool {
    ext_segments(site).get(2).map(|s| s == "1").unwrap_or(false)
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

fn header_map(extra: &HashMap<String, String>) -> reqwest::header::HeaderMap {
    let mut hm = reqwest::header::HeaderMap::new();
    for (k, v) in extra {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            hm.insert(name, value);
        }
    }
    hm
}

fn timestamp_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input.as_bytes()))
}

/// `sign = lower(md5(KEY + 秒级时间戳))`（原类四个 form 方法共用的签名）。
pub fn sign_for(ts: &str) -> String {
    md5_hex(&format!("{SIGN_KEY}{ts}"))
}

/// 10 位随机数字 boundary 后缀（原类 `getRandomNickname(10)`）。
fn random_digits() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 + d.as_secs())
        .unwrap_or(0);
    let mut state = nanos ^ 0x9e37_79b9_7f4a_7c15;
    let mut out = String::with_capacity(10);
    for _ in 0..10 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push(char::from(b'0' + ((state >> 33) % 10) as u8));
    }
    out
}

/// `formUpload` 请求的 boundary 值（客户端把它写进 content-type 的 boundary 参数）。
pub fn dio_boundary() -> String {
    format!("{DIO_PREFIX}{}", random_digits())
}

fn delimiter(boundary: &str) -> String {
    format!("--{boundary}")
}

/// `formUpload(url)`（无业务字段，仅 sign/timestamp）的 multipart body。
pub fn multipart_upload(boundary: &str, sign: &str, ts: &str) -> String {
    let d = delimiter(boundary);
    // 原类先写一个 name="" 的空字段，再写 sign / timestamp，最后以 `delimiter + "--"` 收尾。
    format!(
        "{d}\r\nContent-Disposition: form-data; name=\"\"\r\n\r\n\r\n{d}\
\r\nContent-Disposition: form-data; name=\"sign\"\r\n\r\n{sign}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"timestamp\"\r\n\r\n{ts}\
\r\n{d}--\r\n\r\n"
    )
}

/// `formUploadCate(url, typeId, page)` 的 multipart body（固定 boundary）。
pub fn multipart_cate(boundary: &str, sign: &str, ts: &str, type_id: &str, page: &str) -> String {
    let d = delimiter(boundary);
    format!(
        "{d}\r\nContent-Disposition: form-data; name=\"sign\"\r\n\r\n{sign}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"timestamp\"\r\n\r\n{ts}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"limit\"\r\n\r\n12\
\r\n{d}\r\nContent-Disposition: form-data; name=\"type_id\"\r\n\r\n{type_id}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"page\"\r\n\r\n{page}\
\r\n{d}--\r\n\r\n"
    )
}

/// `formUploadDetail(url, vodId)` 的 multipart body。
pub fn multipart_detail(boundary: &str, sign: &str, ts: &str, vod_id: &str) -> String {
    let d = delimiter(boundary);
    format!(
        "{d}\r\nContent-Disposition: form-data; name=\"vod_id\"\r\n\r\n{vod_id}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"sign\"\r\n\r\n{sign}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"timestamp\"\r\n\r\n{ts}\
\r\n{d}--\r\n\r\n"
    )
}

/// `formUploadSearch(url, keyword)` 的 multipart body。
pub fn multipart_search(boundary: &str, sign: &str, ts: &str, keyword: &str) -> String {
    let d = delimiter(boundary);
    format!(
        "{d}\r\nContent-Disposition: form-data; name=\"keyword\"\r\n\r\n{keyword}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"sign\"\r\n\r\n{sign}\
\r\n{d}\r\nContent-Disposition: form-data; name=\"timestamp\"\r\n\r\n{ts}\
\r\n{d}--\r\n\r\n"
    )
}

fn post_multipart(
    cfg: &TvConfig,
    base: &str,
    boundary: &str,
    body: &str,
) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let url = base;
    let resp = client
        .post(url)
        .headers(header_map(&headers()))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body.to_string())
        .send()
        .map_err(|e| net_err(&Site::default(), format!("请求失败 {url}: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| net_err(&Site::default(), format!("读取响应失败 {url}: {e}")))?;
    if !status.is_success() {
        return Err(net_err(
            &Site::default(),
            format!("HTTP {} -> {url}", status.as_u16()),
        ));
    }
    if text.trim() == "解析失败" {
        return Err(parse_err(&Site::default(), "上游返回「解析失败」"));
    }
    Ok(text)
}

fn post_json(cfg: &TvConfig, url: &str, boundary: &str, body: &str) -> Result<Value> {
    let text = post_multipart(cfg, url, boundary, body)?;
    serde_json::from_str(&text).map_err(|e| parse_err(&Site::default(), format!("响应不是 JSON: {e}")))
}

/// 无业务字段的 formUpload 调用（首页分类 / 推荐）。
fn upload_json(cfg: &TvConfig, base: &str, path: &str) -> Result<Value> {
    let boundary = dio_boundary();
    let ts = timestamp_secs();
    let sign = sign_for(&ts);
    let body = multipart_upload(&boundary, &sign, &ts);
    post_json(cfg, &format!("{base}{path}"), &boundary, &body)
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn pic_of(item: &Value) -> Option<String> {
    let pic = text_of(item.get("vod_pic"));
    if pic.is_empty() {
        let thumb = text_of(item.get("vod_pic_thumb"));
        if thumb.is_empty() {
            None
        } else {
            Some(thumb)
        }
    } else {
        Some(pic)
    }
}

/// 列表/搜索条目 → `VideoItem`（原类三处循环共用 `vod_id/vod_name/vod_pic|thumb/vod_remarks`）。
pub fn item_from(item: &Value) -> VideoItem {
    let remark = text_of(item.get("vod_remarks"));
    VideoItem {
        id: text_of(item.get("vod_id")),
        name: text_of(item.get("vod_name")),
        cover: pic_of(item),
        remark: if remark.is_empty() { None } else { Some(remark) },
    }
}

pub fn items_from(value: &Value) -> Vec<VideoItem> {
    value
        .as_array()
        .map(|arr| arr.iter().map(item_from).collect())
        .unwrap_or_default()
}

/// 首页分类：`POST {base}/v2/type/top_type`（formUpload）→ `data.list[]`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let base = base_of(site);
    let value = upload_json(cfg, &base, "/v2/type/top_type")?;
    Ok(categories_from_value(&value))
}

/// `top_type` 响应 → 分类（空则回退 `CAT_FALLBACK`）。
pub fn categories_from_value(value: &Value) -> Vec<Category> {
    let list = value
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<Category> = list
        .iter()
        .filter_map(|item| {
            let type_id = text_of(item.get("type_id"));
            let type_name = text_of(item.get("type_name"));
            if type_id.is_empty() || type_name.is_empty() {
                None
            } else {
                Some(Category {
                    type_id,
                    type_name,
                })
            }
        })
        .collect();
    if out.is_empty() {
        // 上游返回空（或已死）时回退到原类 filters 的 4 个 type id，保持首页可用。
        out = CAT_FALLBACK
            .iter()
            .map(|(id, name)| Category {
                type_id: (*id).to_string(),
                type_name: (*name).to_string(),
            })
            .collect();
    }
    out
}

/// 首页推荐：`POST {base}/v2/type/tj_vod`（formUpload）→ `data.cai[]`。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let value = upload_json(cfg, &base, "/v2/type/tj_vod")?;
    let list = value
        .get("data")
        .and_then(|d| d.get("cai"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items_from(&Value::Array(list)))
}

/// 分类列表：`POST {base}/v2/home/type_search`（formUploadCate，limit 固定 12）。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    type_id: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let type_id = type_id.trim();
    if type_id.is_empty() {
        return Err(parse_err(site, "分类 id 为空"));
    }
    let ts = timestamp_secs();
    let sign = sign_for(&ts);
    let body = multipart_cate(
        FIXED_BOUNDARY,
        &sign,
        &ts,
        type_id,
        &page.to_string(),
    );
    let value = post_json(
        cfg,
        &format!("{base}/v2/home/type_search"),
        FIXED_BOUNDARY,
        &body,
    )?;
    let list = value
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items_from(&Value::Array(list)))
}

/// 详情：`POST {base}/v2/home/vod_details`（formUploadDetail）→ `data`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let base = base_of(site);
    let ts = timestamp_secs();
    let sign = sign_for(&ts);
    let boundary = dio_boundary();
    let body = multipart_detail(&boundary, &sign, &ts, id);
    let value = post_json(
        cfg,
        &format!("{base}/v2/home/vod_details"),
        &boundary,
        &body,
    )?;
    let data = value
        .get("data")
        .cloned()
        .ok_or_else(|| parse_err(site, "详情响应缺少 data"))?;
    Ok(detail_from_data(&data))
}

/// `vod_details` 的 `data` 节点 → `VideoDetail`（拆分出来便于单测，不触网）。
pub fn detail_from_data(data: &Value) -> VideoDetail {
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    if let Some(lines) = data.get("vod_play_list").and_then(|v| v.as_array()) {
        for (idx, line) in lines.iter().enumerate() {
            let title = text_of(line.get("title"));
            let flag = play_flag(line);
            let mut items: Vec<Episode> = Vec::new();
            if let Some(urls) = line.get("urls").and_then(|v| v.as_array()) {
                for url in urls {
                    let play_url = text_of(url.get("url"));
                    if play_url.is_empty() {
                        continue;
                    }
                    items.push(Episode {
                        title: text_of(url.get("name")),
                        url: format!("{play_url}|{flag}"),
                    });
                    if items.len() >= MAX_EPISODES {
                        break;
                    }
                }
            }
            if items.is_empty() {
                continue;
            }
            groups.push(EpisodeGroup {
                group: if title.is_empty() {
                    format!("线路{}", idx + 1)
                } else {
                    title
                },
                items,
            });
        }
    }
    let desc = text_of(data.get("vod_content"));
    let year = text_of(data.get("vod_year"));
    let area = text_of(data.get("vod_area"));
    let director = text_of(data.get("vod_director"));
    let actor = text_of(data.get("vod_actor"));
    let type_name = text_of(data.get("vod_class"));
    VideoDetail {
        id: text_of(data.get("vod_id")),
        name: text_of(data.get("vod_name")),
        cover: pic_of(data),
        desc: if desc.is_empty() {
            None
        } else {
            Some(if type_name.is_empty() {
                desc
            } else {
                format!("[{type_name}] {desc}")
            })
        },
        year: if year.is_empty() { None } else { Some(year) },
        area: if area.is_empty() { None } else { Some(area) },
        director: if director.is_empty() {
            None
        } else {
            Some(director)
        },
        actor: if actor.is_empty() { None } else { Some(actor) },
        episodes: groups,
    }
}

/// 原类 detailContent 的线路标记：`parse_urls` 0 个 → `"no"`；1 个 → 第 1 个；否则第 2 个。
fn play_flag(line: &Value) -> String {
    let arr = line
        .get("parse_urls")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    match arr.len() {
        0 => "no".to_string(),
        1 => text_of(arr.first()),
        _ => text_of(arr.get(1)),
    }
}

/// 搜索：`POST {base}/v2/home/search`（formUploadSearch）→ `data.list[]`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err(parse_err(site, "搜索关键词为空"));
    }
    let base = base_of(site);
    let ts = timestamp_secs();
    let sign = sign_for(&ts);
    let boundary = dio_boundary();
    let body = multipart_search(&boundary, &sign, &ts, keyword);
    let value = post_json(cfg, &format!("{base}/v2/home/search"), &boundary, &body)?;
    let list = value
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items_from(&Value::Array(list)))
}

fn is_video_site(url: &str) -> bool {
    VIDEO_SITES.iter().any(|domain| url.contains(domain))
}

/// 播放：id 形如 `url|flag`；flag 长度 <3 直接直链，否则用 flag+url+sign 换真实地址。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(play_err(site, "播放 id 为空"));
    }
    let (url, flag) = match id.split_once('|') {
        Some((u, f)) => (u.trim(), f.trim()),
        None => (id, ""),
    };
    if url.is_empty() {
        return Err(play_err(site, "播放地址为空"));
    }
    if flag.len() < 3 {
        return Ok(json!({
            "parse": 0,
            "url": url,
            "header": { "User-Agent": UA },
        }));
    }
    // 原类对 flag 是解析站 URL 的线路：GET flag + url + "&sign=" + md5(KEY+ts)。
    let ts = timestamp_secs();
    let target = format!("{flag}{url}&sign={}", sign_for(&ts));
    let client = crate::http::client_for(&cfg.http)?;
    if let Ok(text) = crate::http::get_text(&client, &target, &headers(), 2) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            let resolved = text_of(value.get("url"));
            if !resolved.is_empty() {
                return Ok(json!({
                    "parse": 0,
                    "url": resolved,
                    "header": { "User-Agent": UA },
                }));
            }
        }
    }
    if url.contains(".m3u8") {
        return Ok(json!({
            "parse": 0,
            "url": url,
            "header": { "User-Agent": UA },
        }));
    }
    if is_video_site(url) {
        return Ok(json!({
            "parse": 1,
            "url": url,
            "header": { "User-Agent": UA },
        }));
    }
    Err(play_err(site, format!("无法解析播放地址：{url}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "ttian".into();
        s.name = "天天影视".into();
        s.api = API.into();
        s.ext = Some(json!(ext));
        s
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_TTian"));
        assert!(handles("csp_ttian"));
        assert!(!handles("csp_TtianV2"));
        assert!(!handles("csp_XiaoZhang"));
    }

    #[test]
    fn ext_splits_base_proxy_and_danmaku() {
        let s = site("http://op.ysdqjs.cn$$$http://127.0.0.1:10079$$$1");
        assert_eq!(base_of(&s), "http://op.ysdqjs.cn");
        assert_eq!(
            proxy_of(&s).as_deref(),
            Some("http://127.0.0.1:10079")
        );
        assert!(danmaku_enabled(&s));

        let plain = site("http://app.kzjtv.com$$$null");
        assert_eq!(base_of(&plain), "http://app.kzjtv.com");
        assert!(proxy_of(&plain).is_none());
        assert!(!danmaku_enabled(&plain));

        let empty = Site::default();
        assert_eq!(base_of(&empty), DEFAULT_BASE);
    }

    #[test]
    fn sign_is_lowercase_md5_of_key_and_timestamp() {
        // 用外部 md5sum 预先算好的向量：md5("kj…key" + "1700000000")
        assert_eq!(
            sign_for("1700000000"),
            "f4558a0199ba11fdb60e77394b9e2695"
        );
        assert_eq!(sign_for("1700000000").len(), 32);
        assert!(sign_for("1700000001").chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn upload_body_has_random_boundary_sign_and_timestamp() {
        let b = dio_boundary();
        assert_eq!(b.len(), "--dio-boundary-".len() + 10);
        let body = multipart_upload(&b, "abc123", "1700000000");
        let delim = format!("--{b}");
        assert!(body.starts_with(&delim), "body 应以分隔符开头：{body}");
        assert!(body.contains("name=\"sign\""));
        assert!(body.contains("\r\n\r\nabc123"));
        assert!(body.contains("name=\"timestamp\""));
        assert!(body.contains("\r\n\r\n1700000000"));
        assert!(body.ends_with("--\r\n\r\n"), "body 尾部不符：{body:?}");
    }

    #[test]
    fn cate_body_uses_fixed_boundary_and_limit_twelve() {
        let body = multipart_cate(FIXED_BOUNDARY, "sig", "1700", "2", "3");
        assert!(body.starts_with(&format!("--{FIXED_BOUNDARY}")));
        assert!(body.contains("name=\"limit\""));
        assert!(body.contains("\r\n\r\n12"));
        assert!(body.contains("name=\"type_id\""));
        assert!(body.contains("\r\n\r\n2"));
        assert!(body.contains("name=\"page\""));
        assert!(body.contains("\r\n\r\n3"));
        assert!(body.ends_with(&format!("--{FIXED_BOUNDARY}--\r\n\r\n")));
    }

    #[test]
    fn detail_and_search_bodies_carry_id_and_keyword() {
        let b = "--dio-boundary-1234567890";
        let detail = multipart_detail(b, "sig", "1700", "42");
        assert!(detail.contains("name=\"vod_id\""));
        assert!(detail.contains("\r\n\r\n42"));
        assert!(detail.contains("name=\"sign\""));

        let search = multipart_search(b, "sig", "1700", "妈妈");
        assert!(search.contains("name=\"keyword\""));
        assert!(search.contains("\r\n\r\n妈妈"));
        assert!(search.contains("name=\"timestamp\""));
    }

    #[test]
    fn items_map_poster_fallback_and_remark() {
        let value = json!({"data":{"list":[
            {"vod_id":11,"vod_name":"甲","vod_pic":"","vod_pic_thumb":"https://img/1.jpg","vod_remarks":"更新12集"},
            {"vod_id":"12","vod_name":"乙","vod_pic":"https://img/2.jpg","vod_pic_thumb":"https://img/2t.jpg"},
            {"vod_name":"缺 id","vod_pic":"https://img/3.jpg"}
        ]}});
        let list = value.get("data").and_then(|d| d.get("list")).cloned().unwrap();
        let items = items_from(&list);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "11");
        assert_eq!(items[0].cover.as_deref(), Some("https://img/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新12集"));
        assert_eq!(items[1].cover.as_deref(), Some("https://img/2.jpg"));
        assert!(items[1].remark.is_none());
        assert!(items[2].id.is_empty(), "缺 vod_id 时应为空：{:?}", items[2].id);
    }

    #[test]
    fn detail_maps_play_lines_and_parse_url_flags() {
        let data = json!({
            "vod_id": 42,
            "vod_name": "测试剧",
            "vod_pic": "https://img/cover.jpg",
            "vod_class": "国产剧",
            "vod_year": "2024",
            "vod_area": "大陆",
            "vod_remarks": "全12集",
            "vod_director": "张三",
            "vod_actor": "李四",
            "vod_content": "简介",
            "vod_play_list": [
                {"title": "线路一", "parse_urls": [], "urls": [
                    {"name": "第1集", "url": "https://v.qq.com/x/1"},
                    {"name": "第2集", "url": "https://v.qq.com/x/2"}
                ]},
                {"title": "线路二", "parse_urls": ["https://jx.example.com/?url="], "urls": [
                    {"name": "第1集", "url": "https://v.qq.com/y/1"}
                ]},
                {"title": "线路三", "parse_urls": ["a", "b"], "urls": [
                    {"name": "第1集", "url": "https://v.qq.com/z/1"}
                ]}
            ]
        });
        let d = detail_from_data(&data);
        assert_eq!(d.id, "42");
        assert_eq!(d.name, "测试剧");
        assert_eq!(d.cover.as_deref(), Some("https://img/cover.jpg"));
        assert_eq!(d.year.as_deref(), Some("2024"));
        assert_eq!(d.director.as_deref(), Some("张三"));
        assert!(d.desc.as_deref().unwrap_or("").contains("[国产剧]"));
        assert_eq!(d.episodes.len(), 3);
        assert_eq!(d.episodes[0].group, "线路一");
        assert_eq!(d.episodes[0].items[0].url, "https://v.qq.com/x/1|no");
        assert_eq!(
            d.episodes[1].items[0].url,
            "https://v.qq.com/y/1|https://jx.example.com/?url="
        );
        assert_eq!(d.episodes[2].items[0].url, "https://v.qq.com/z/1|b");
    }

    #[test]
    fn categories_fall_back_to_four_when_upstream_empty() {
        let empty = json!({"data":{"list":[]}});
        let cats = categories_from_value(&empty);
        assert_eq!(cats.len(), 4);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");

        let value = json!({"data":{"list":[{"type_id":"5","type_name":"纪录片"}]}});
        let cats = categories_from_value(&value);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "5");
    }

    #[test]
    fn player_passes_through_direct_urls() {
        let s = site("http://op.ysdqjs.cn");
        let cfg = TvConfig::default();
        let v = player_content(&cfg, &s, "", "https://v.qq.com/x/1|no").unwrap();
        assert_eq!(v.get("parse").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(
            v.get("url").and_then(|x| x.as_str()),
            Some("https://v.qq.com/x/1")
        );

        // flag 长度 >=3 且 URL 是已知站点：不打网络，先判定外层分支不可测；
        // 这里只验证无 `|` 的纯 m3u8 直链同样直出。
        let v = player_content(&cfg, &s, "", "https://cdn.example.com/a.m3u8").unwrap();
        assert_eq!(v.get("parse").and_then(|x| x.as_i64()), Some(0));
    }

    #[test]
    fn known_video_sites_are_recognised() {
        for url in [
            "https://v.qq.com/x/cover/abc.html",
            "https://www.iqiyi.com/v_abc.html",
            "https://www.bilibili.com/video/BV1xx",
            "https://www.mgtv.com/b/123/456.html",
        ] {
            assert!(is_video_site(url), "应识别为视频站：{url}");
        }
        assert!(!is_video_site("https://pan.quark.cn/s/abc"));
        // 注意：`le.com` 是 `example.com` 的子串（原类同样是 contains 判定），故负例用 example.org。
        assert!(!is_video_site("https://example.org/a.mp4"));
    }
}
