//! 弹幕获取与 ASS 生成（B 站优先）
//!
//! 链路：页面 URL / 标题 → B 站 bvid → view 接口取 cid → 弹幕 XML → ASS 字幕文件。
//! ASS 以“外挂字幕”的方式交给 mpv（`sub-add`），因此复用 `player_add_subtitle` 链路，
//! 不额外引入渲染层。
//!
//! 注：TVBox 生态里弹幕源通常是一个 JS 接口；这里先实现最常用、无需签名的 B 站路径，
//! 其它站点/接口后续可按 `TvConfig.danmaku` 模板扩展。

use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::error::{CoreError, Result};

/// 请求 B 站接口用的浏览器 UA（不带 UA 会被风控直接拒绝）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const BILI_REFERER: &str = "https://www.bilibili.com/";
/// 单集最多生成多少条弹幕（避免 ASS 体积过大拖慢 mpv）。
const MAX_ITEMS: usize = 4000;

/// 一条弹幕。
#[derive(Debug, Clone, PartialEq)]
pub struct DanmakuItem {
    /// 出现时间（毫秒）。
    pub time_ms: i64,
    /// B 站弹幕模式：1-3 滚动、4 底部、5 顶部。
    pub mode: i64,
    /// RGB 颜色（0xRRGGBB）。
    pub color: u32,
    /// 文本（已做 XML 实体反转义）。
    pub text: String,
}

/// B 站视频标识：BV 号或 av 号。
#[derive(Debug, Clone, PartialEq)]
pub enum BiliVideoId {
    Bv(String),
    Av(String),
}

/// 从任意文本/URL 里提取 B 站视频标识（BV 优先，其次 av）。
pub fn extract_bili_id(input: &str) -> Option<BiliVideoId> {
    let bv = Regex::new(r"(?i)(BV[0-9A-Za-z]{10})").ok()?;
    if let Some(m) = bv.captures(input) {
        return Some(BiliVideoId::Bv(m[1].to_string()));
    }
    let av = Regex::new(r"(?i)av(\d{1,12})(?:$|[/?#&])").ok()?;
    if let Some(m) = av.captures(input) {
        return Some(BiliVideoId::Av(m[1].to_string()));
    }
    None
}

/// 解析 B 站弹幕 XML（`<d p="time,mode,size,color,...">文本</d>`）。
pub fn parse_bili_xml(xml: &str) -> Vec<DanmakuItem> {
    let Ok(re) = Regex::new(r#"(?s)<d\s+p="([^"]*)"[^>]*>(.*?)</d>"#) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for cap in re.captures_iter(xml) {
        let attr = &cap[1];
        let raw_text = unescape_xml(&cap[2]);
        let mut fields = attr.split(',');
        let Some(time) = fields.next().and_then(|v| v.trim().parse::<f64>().ok()) else {
            continue;
        };
        let mode = fields
            .next()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(1);
        let _size = fields.next();
        let color = fields
            .next()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0x00FF_FFFF);
        let text = raw_text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        items.push(DanmakuItem {
            time_ms: (time * 1000.0).round() as i64,
            mode,
            color: color & 0x00FF_FFFF,
            text,
        });
    }
    items.sort_by_key(|item| item.time_ms);
    items.truncate(MAX_ITEMS);
    items
}

/// 把弹幕列表生成 ASS 字幕文本（纯函数，便于单测）。
pub fn to_ass(items: &[DanmakuItem], width: u32, height: u32) -> String {
    let width = width.max(640);
    let height = height.max(360);
    let mut out = String::with_capacity(items.len() * 96 + 512);
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {width}\n"));
    out.push_str(&format!("PlayResY: {height}\n"));
    out.push_str("WrapStyle: 2\n");
    out.push_str("ScaledBorderAndShadow: yes\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    out.push_str("Style: Danmaku,Microsoft YaHei,42,&H00FFFFFF,&H00FFFFFF,&H00000000,&H64000000,0,0,0,0,100,100,0,0,1,2,1,7,0,0,0,1\n\n");
    out.push_str("[Events]\n");
    out.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");

    let lanes = 12i64;
    let lane_height = (height as i64 - 200).max(lanes * 24) / lanes;
    for (index, item) in items.iter().enumerate() {
        let start = item.time_ms.max(0);
        let (effect, duration_ms) = match item.mode {
            4 => {
                let y = height as i64 - 80;
                (format!(r"{{\an2\pos({},{} )}}", width / 2, y), 5000)
            }
            5 => {
                let y = 60;
                (format!(r"{{\an8\pos({},{} )}}", width / 2, y), 5000)
            }
            _ => {
                let lane = (index as i64) % lanes;
                let y = 60 + lane * lane_height;
                (
                    format!(r"{{\move({},{},-{},{} )}}", width, y, width / 2, y),
                    8000,
                )
            }
        };
        let color = ass_color(item.color);
        let text = escape_ass_text(&item.text);
        out.push_str(&format!(
            "Dialogue: 0,{},{},Danmaku,,0,0,0,,{{\\c{color}}}{effect}{text}\n",
            ass_time(start),
            ass_time(start + duration_ms)
        ));
    }
    out
}

/// 按页面 URL 拉取 B 站弹幕；非 B 站页面返回空列表（由调用方决定是否按标题兜底）。
pub fn fetch_bilibili(page_url: &str) -> Result<Vec<DanmakuItem>> {
    let Some(id) = extract_bili_id(page_url) else {
        return Ok(Vec::new());
    };
    let client = bili_client()?;
    let cid = bili_cid(&client, &id)?;
    fetch_dm_list(&client, cid)
}

/// 按标题搜索 B 站视频并取第一条的弹幕（无搜索权限/查无结果时返回空列表）。
pub fn fetch_by_title(title: &str) -> Result<Vec<DanmakuItem>> {
    let keyword = url::form_urlencoded::byte_serialize(title.trim().as_bytes()).collect::<String>();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let client = bili_client()?;
    let url = format!(
        "https://api.bilibili.com/x/web-interface/search/type?search_type=video&page=1&keyword={keyword}"
    );
    let value: Value = client
        .get(url)
        .header(reqwest::header::REFERER, BILI_REFERER)
        .send()?
        .error_for_status()?
        .json()?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        return Ok(Vec::new());
    }
    let Some(bvid) = value
        .pointer("/data/result/0/bvid")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(Vec::new());
    };
    let cid = bili_cid(&client, &BiliVideoId::Bv(bvid))?;
    fetch_dm_list(&client, cid)
}

/* ---------------- 内部实现 ---------------- */

fn bili_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(UA)
        .build()?)
}

fn bili_cid(client: &reqwest::blocking::Client, id: &BiliVideoId) -> Result<i64> {
    let query = match id {
        BiliVideoId::Bv(bvid) => format!("bvid={bvid}"),
        BiliVideoId::Av(aid) => format!("aid={aid}"),
    };
    let url = format!("https://api.bilibili.com/x/web-interface/view?{query}");
    let value: Value = client
        .get(url)
        .header(reqwest::header::REFERER, BILI_REFERER)
        .send()?
        .error_for_status()?
        .json()?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("B 站返回错误")
            .to_string();
        return Err(CoreError::Api { code, msg });
    }
    let cid = value
        .pointer("/data/cid")
        .and_then(Value::as_i64)
        .or_else(|| value.pointer("/data/pages/0/cid").and_then(Value::as_i64))
        .ok_or_else(|| CoreError::Config("B 站视频没有可用的 cid".to_string()))?;
    Ok(cid)
}

fn fetch_dm_list(client: &reqwest::blocking::Client, cid: i64) -> Result<Vec<DanmakuItem>> {
    let url = format!("https://api.bilibili.com/x/v1/dm/list.so?oid={cid}");
    // 该接口返回 `Content-Encoding: deflate`，reqwest 的 deflate 特性会自动解压。
    let xml = client
        .get(url)
        .header(reqwest::header::REFERER, BILI_REFERER)
        .header(reqwest::header::ACCEPT, "text/xml,application/xml,*/*")
        .send()?
        .error_for_status()?
        .text()?;
    Ok(parse_bili_xml(&xml))
}

fn unescape_xml(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn ass_color(rgb: u32) -> String {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    format!("&H00{b:02X}{g:02X}{r:02X}")
}

fn ass_time(ms: i64) -> String {
    let ms = ms.max(0);
    let total_seconds = ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    let centis = (ms % 1000) / 10;
    format!("{hours}:{minutes:02}:{seconds:02}.{centis:02}")
}

fn escape_ass_text(text: &str) -> String {
    text.replace('\\', "＼")
        .replace('{', "｛")
        .replace('}', "｝")
        .replace('\n', " ")
        .replace('\r', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bvid_from_page_url() {
        assert_eq!(
            extract_bili_id("https://www.bilibili.com/video/BV1GJ411x7h7/?spm_id_from=333"),
            Some(BiliVideoId::Bv("BV1GJ411x7h7".to_string()))
        );
        assert_eq!(
            extract_bili_id("视频 BV1xx411c7mD 的弹幕"),
            Some(BiliVideoId::Bv("BV1xx411c7mD".to_string()))
        );
        assert_eq!(extract_bili_id("https://example.com/a.m3u8"), None);
    }

    #[test]
    fn parses_bili_xml_items() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><i><chatid>1</chatid><d p="12.34,1,25,16711680,0,0,0,0">第一条 &amp; 弹幕</d><d p="0.5,5,25,16777215,0,0,0,0">顶部</d><d p=""></d></i>"#;
        let items = parse_bili_xml(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].time_ms, 500);
        assert_eq!(items[0].mode, 5);
        assert_eq!(items[1].time_ms, 12340);
        assert_eq!(items[1].color, 0x00FF_0000);
        assert_eq!(items[1].text, "第一条 & 弹幕");
    }

    #[test]
    fn ass_contains_events_and_escapes_override_braces() {
        let items = vec![DanmakuItem {
            time_ms: 1000,
            mode: 1,
            color: 0x00FF_FFFF,
            text: "带{花括号}的弹幕".to_string(),
        }];
        let ass = to_ass(&items, 1920, 1080);
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("PlayResX: 1920"));
        assert!(ass.contains("Dialogue: 0,0:00:01.00,"));
        assert!(ass.contains("带｛花括号｝的弹幕"));
        assert!(!ass.contains("{花括号}"));
    }
}
