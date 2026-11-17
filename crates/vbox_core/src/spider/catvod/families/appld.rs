//! `csp_AppLd` 原生实现（「率率」App：单站 JSON API + `lvdou+` AES 播放串）。
//!
//! ## 来源（REA 反编译证据）
//!
//! * jar `69dadb9a-Yoursmile.jar` 的 `com.github.catvod.spider.AppLd`：jadx 只还原
//!   1174 行前身（decompile 缺口多），第 126 轮改走 dex2jar 2.4 + Vineflower 1.10.1
//!   单类 → `_rea/tmp/appmao/sel-AppLd/AppLd.java`（1174 行、0 失败）。
//! * 字符串用 d2j 短数组 XOR 编码，`_rea/tmp/appmao/dec2.mjs` 解出 61/61 条
//!   （607 shorts、0 miss）→ `AppLd.decoded.java`。
//! * ext：`Base64 → RC4("fantaiying666") → 每字节 -1 → JSON`，与
//!   `crate::spider::catvod::appto` 完全同构（复用 `decode_ext_value`）。
//!
//! ## 协议（逐段还原）
//!
//! * ext JSON：`server`（API 基址）与 `mackey`（`key|iv`，AES-128 的 key/iv）。
//! * 头：仅 `User-Agent: Chrome/109`。
//! * 分类：`GET {server}nav?token=` → `{"list":[{type_id,type_name}]}`。
//! * 首页列表：`GET {server}index_video?token=` → `{"list":[{"vlist":[…]}]}` 展平。
//! * 分类列表：`GET {server}video?tid={id}&class=&area=&lang=&year=&limit=18&pg={page}`
//!   → `{"list":[vod…],"pagecount":N}`。
//! * 详情：`GET {server}video_detail?id={id}` → `{"data":{vod_*, vod_url_with_player[]}}`；
//!   `vod_url_with_player[]` 每项 `{name|code,url}` 是一条线路、一个播放串。
//! * 搜索：`GET {server}search?text={kw}&pg=` → `{"list":[vod…]}`。
//! * 播放：id 以 `lvdou+` 开头时 → 去前缀、Base64 解码、AES/CBC/PKCS5 解密
//!   （key/iv 来自 mackey）得到真实 url；`isVideoFormat(url)` 为真 → `parse=0`，
//!   否则 → `{"parse":1,"jx":"1","url":…}`。
//!
//! ## 有意偏差（原类自带问题或桌面端边界）
//!
//! 1. 上游 `http://103.85.85.197:8899/api.php/app/` 在 2026-11 已超时/不可达，
//!    本家族只能以 mock 夹具验收（无真站 `#[ignore]` 用例）。
//! 2. 原类在所有线路名后追加常量「饭太硬祝大家新年快乐🧧」并用 `$$` 拼接
//!    `vod_play_from`/`vod_play_url`（TVBox 标准是 `$$$`）；本实现改用真实
//!    `name`/`code` 作线路名、`$$$` 语义由结构化 `episodes` 表达，属修复性偏差。
//! 3. 原类每条线路只给一个播放串、未带剧集标题；本实现用线路名作该集标题。
//! 4. 线路上限 `MAX_EPISODES`、详情线路去重顺序与原类一致（首次出现顺序）。

use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::client_for;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::appto::{aes_cbc_decrypt, decode_ext_value};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

/// 家族标识（`profiles.json` 的 api 名）。
pub const API: &str = "csp_AppLd";
/// 原类 `getHeaders()` 的 UA。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
/// 原类输出 `limit` 字段的固定值。
pub const PAGE_LIMIT: i64 = 90;
/// 加密播放串前缀。
pub const PLAY_PREFIX: &str = "lvdou+";
/// 单线路最多保留的剧集数。
const MAX_EPISODES: usize = 500;

/// 是否由本家族处理。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// ext：`server` + 可选 `mackey`。
struct Ext {
    server: String,
    mackey: Option<String>,
}

fn ext_of(site: &Site) -> Result<Ext> {
    let raw = site.ext_string();
    if raw.trim().is_empty() {
        return Err(parse_err(site, "ext 为空：AppLd 需要 lvdou 配置串"));
    }
    let value = decode_ext_value(&raw)
        .or_else(|_| serde_json::from_str::<Value>(raw.trim()).map_err(|e| e.to_string()))
        .map_err(|e| parse_err(site, format!("ext 解码失败: {e}")))?;
    let server = value
        .get("server")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| parse_err(site, "AppLd ext 缺少 server"))?
        .to_string();
    let mackey = value
        .get("mackey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(Ext { server, mackey })
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

fn base_url(ext: &Ext) -> String {
    // 原类 `yq` 字段以 `/` 结尾，端点直接拼接；这里统一补一个尾斜杠。
    format!("{}/", ext.server.trim_end_matches('/'))
}

fn get_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let client = client_for(&cfg.http)?;
    let text = crate::http::get_text(&client, url, &headers(), 2)?;
    serde_json::from_str(&text).map_err(|e| parse_err(site, format!("响应不是 JSON（{url}）: {e}")))
}

fn s_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn id_field(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn item_from(v: &Value) -> Option<VideoItem> {
    let id = id_field(v, "vod_id")?;
    let name = s_field(v, "vod_name")?.to_string();
    Some(VideoItem {
        id,
        name,
        cover: s_field(v, "vod_pic").map(str::to_string),
        remark: s_field(v, "vod_remarks").map(str::to_string),
    })
}

fn items_from(value: &Value) -> Vec<VideoItem> {
    value
        .get("list")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(item_from).collect())
        .unwrap_or_default()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 去 HTML 标签并还原常见实体（原类 `stripHtml` 的等价实现）。
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

/// 分类：`nav?token=`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let ext = ext_of(site)?;
    let url = format!("{}nav?token=", base_url(&ext));
    let value = get_json(cfg, site, &url)?;
    let mut out = Vec::new();
    if let Some(arr) = value.get("list").and_then(Value::as_array) {
        for it in arr {
            let id = id_field(it, "type_id");
            let name = s_field(it, "type_name");
            if let (Some(id), Some(name)) = (id, name) {
                out.push(Category {
                    type_id: id,
                    type_name: name.to_string(),
                });
            }
        }
    }
    if out.is_empty() {
        return Err(parse_err(site, "首页分类为空：AppLd nav 没有可识别分类"));
    }
    Ok(out)
}

/// 首页推荐：`index_video?token=`，展平 `list[].vlist[]`。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let url = format!("{}index_video?token=", base_url(&ext));
    let value = get_json(cfg, site, &url)?;
    let mut out = Vec::new();
    if let Some(arr) = value.get("list").and_then(Value::as_array) {
        for group in arr {
            if let Some(vlist) = group.get("vlist").and_then(Value::as_array) {
                for it in vlist {
                    if let Some(item) = item_from(it) {
                        out.push(item);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 分类列表：`video?tid=&…&pg=`。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let url = format!(
        "{}video?tid={}&class=&area=&lang=&year=&limit=18&pg={}",
        base_url(&ext),
        urlencode(type_id),
        page.max(1)
    );
    let value = get_json(cfg, site, &url)?;
    let items = items_from(&value);
    if items.is_empty() {
        return Err(parse_err(site, "分类列表为空：AppLd video 没有可识别条目"));
    }
    Ok(items)
}

/// 搜索：`search?text=&pg=`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let url = format!("{}search?text={}&pg=", base_url(&ext), urlencode(keyword));
    let value = get_json(cfg, site, &url)?;
    Ok(items_from(&value))
}

/// 详情：`video_detail?id=`；`vod_url_with_player[]` 的每一项是一条线路 + 一个播放串。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    if id.trim().is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let ext = ext_of(site)?;
    let url = format!("{}video_detail?id={}", base_url(&ext), urlencode(id));
    let value = get_json(cfg, site, &url)?;
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let name = s_field(&data, "vod_name")
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.trim().to_string());

    let mut groups: Vec<EpisodeGroup> = Vec::new();
    if let Some(arr) = data.get("vod_url_with_player").and_then(Value::as_array) {
        for (idx, entry) in arr.iter().enumerate() {
            let label = s_field(entry, "name")
                .or_else(|| s_field(entry, "code"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("线路{}", idx + 1));
            let Some(play_url) = s_field(entry, "url") else {
                continue;
            };
            if let Some(group) = groups.iter_mut().find(|g| g.group == label) {
                if group.items.len() < MAX_EPISODES {
                    group.items.push(Episode {
                        title: label.clone(),
                        url: play_url.to_string(),
                    });
                }
            } else {
                groups.push(EpisodeGroup {
                    group: label.clone(),
                    items: vec![Episode {
                        title: label.clone(),
                        url: play_url.to_string(),
                    }],
                });
            }
        }
    }
    if groups.is_empty() {
        return Err(parse_err(site, "详情没有可播放线路：vod_url_with_player 为空"));
    }

    let desc = s_field(&data, "vod_content")
        .map(strip_html)
        .filter(|s| !s.is_empty());
    Ok(VideoDetail {
        id: id.trim().to_string(),
        name,
        cover: s_field(&data, "vod_pic").map(str::to_string),
        desc,
        year: s_field(&data, "vod_year").map(str::to_string),
        area: s_field(&data, "vod_area").map(str::to_string),
        director: s_field(&data, "vod_director").map(str::to_string),
        actor: s_field(&data, "vod_actor").map(str::to_string),
        episodes: groups,
    })
}

/// 播放：`lvdou+` 前缀的 AES 串解密；视频格式直出 `parse=0`，否则交给嗅探器。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let _ = (cfg, flag);
    let ext = ext_of(site)?;
    let url = decode_play_id(site, &ext, id)?;
    if is_video_format(&url) {
        Ok(json!({
            "parse": 0,
            "url": url,
            "playUrl": "",
            "header": { "User-Agent": UA }
        }))
    } else {
        Ok(json!({ "parse": 1, "jx": "1", "url": url }))
    }
}

fn decode_play_id(site: &Site, ext: &Ext, id: &str) -> Result<String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let Some(rest) = id.strip_prefix(PLAY_PREFIX) else {
        return Ok(id.to_string());
    };
    let mackey = ext
        .mackey
        .as_deref()
        .ok_or_else(|| parse_err(site, "播放串是加密的，但 ext 缺少 mackey"))?;
    let (key, iv) = mackey
        .split_once('|')
        .ok_or_else(|| parse_err(site, "AppLd mackey 需要 key|iv 形式"))?;
    let cipher = B64
        .decode(rest.trim())
        .map_err(|e| parse_err(site, format!("播放串不是合法 Base64: {e}")))?;
    let plain = aes_cbc_decrypt(key.as_bytes(), iv.as_bytes(), &cipher)
        .map_err(|e| parse_err(site, format!("播放串 AES 解密失败: {e}")))?;
    String::from_utf8(plain)
        .map(|s| s.trim().to_string())
        .map_err(|e| parse_err(site, format!("播放串解密后不是 UTF-8: {e}")))
}

fn is_video_format(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    [".m3u8", ".mp4", ".flv", ".mkv", ".avi", ".ts", ".mov", ".m4v", ".mpegurl"]
        .iter()
        .any(|ext| lower.contains(ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::catvod::appto::rc4;

    fn site(ext: Value) -> Site {
        Site {
            key: "appld".to_string(),
            name: "率率".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(ext),
            ..Default::default()
        }
    }

    fn plain_site() -> Site {
        site(json!({ "server": "http://127.0.0.1:9/api.php/app/", "mackey": "yingshicherwQ4Zi|PHRBxdt7PppocRJ5" }))
    }

    fn encode_ext(raw: &str) -> String {
        let mut plain = Vec::with_capacity(raw.len());
        for b in raw.as_bytes() {
            plain.push(b.wrapping_add(1));
        }
        let mixed = rc4(b"fantaiying666", &plain);
        B64.encode(&mixed)
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_AppLd"));
        assert!(handles(" csp_appld "));
        assert!(!handles("csp_AppSK"));
    }

    #[test]
    fn parses_raw_json_ext() {
        let ext = ext_of(&plain_site()).expect("ext");
        assert_eq!(ext.server, "http://127.0.0.1:9/api.php/app/");
        assert_eq!(
            ext.mackey.as_deref(),
            Some("yingshicherwQ4Zi|PHRBxdt7PppocRJ5")
        );
    }

    #[test]
    fn decodes_encoded_ext() {
        let raw = r#"{"server":"http://a.b/api.php/app/","mackey":"k|v"}"#;
        let ext = ext_of(&site(json!(encode_ext(raw)))).expect("ext");
        assert_eq!(ext.server, "http://a.b/api.php/app/");
        assert_eq!(ext.mackey.as_deref(), Some("k|v"));
    }

    #[test]
    fn rejects_ext_without_server() {
        assert!(ext_of(&site(json!({ "mackey": "k|v" }))).is_err());
    }

    #[test]
    fn strips_html_and_entities() {
        assert_eq!(
            strip_html("<p>第一行&nbsp;内容</p><br/>第二行 &amp; 更多"),
            "第一行 内容第二行 & 更多"
        );
    }

    #[test]
    fn player_passes_plain_urls_through() {
        let out = player_content(&TvConfig::default(), &plain_site(), "", "https://cdn/a.m3u8")
            .expect("player");
        assert_eq!(out["parse"], 0);
        assert_eq!(out["url"], "https://cdn/a.m3u8");
    }

    #[test]
    fn player_requires_mackey_for_encrypted_id() {
        let s = site(json!({ "server": "http://a/" }));
        assert!(player_content(&TvConfig::default(), &s, "", "lvdou+AAAA").is_err());
    }

    #[test]
    fn player_decodes_aes_play_id() {
        use aes::Aes128;
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        let key = b"yingshicherwQ4Zi";
        let iv = b"PHRBxdt7PppocRJ5";
        let plain = b"https://cdn.example.com/play/index.m3u8";
        let enc = cbc::Encryptor::<Aes128>::new_from_slices(key, iv).expect("encryptor");
        let mut buf = vec![0u8; plain.len() + 16];
        buf[..plain.len()].copy_from_slice(plain);
        let cipher = enc
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .expect("encrypt")
            .to_vec();
        let id = format!("{PLAY_PREFIX}{}", B64.encode(&cipher));
        let out = player_content(&TvConfig::default(), &plain_site(), "率率", &id).expect("player");
        assert_eq!(out["parse"], 0);
        assert_eq!(out["url"], "https://cdn.example.com/play/index.m3u8");
    }

    #[test]
    fn non_video_url_is_flagged_for_parsing() {
        let out = player_content(
            &TvConfig::default(),
            &plain_site(),
            "",
            "https://page.example.com/watch/1",
        )
        .expect("player");
        assert_eq!(out["parse"], 1);
        assert_eq!(out["jx"], "1");
    }
}
