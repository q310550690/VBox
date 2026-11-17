//! EPG 节目单解析（XMLTV 格式）
//!
//! 直播源可附组级 EPG 地址（`LiveSource.epg`）或 m3u 头部 `x-tvg-url`。XMLTV 的节目形态：
//!
//! ```xml
//! <tv>
//!   <programme start="20261028120000 +0800" stop="20261028130000 +0800" channel="cctv1">
//!     <title lang="zh">新闻联播</title>
//!   </programme>
//! </tv>
//! ```
//!
//! 这里只取 VBox 直播页需要的四元组：频道（XMLTV `channel`，通常等于 m3u 的 `tvg-id`）、
//! 标题、开始/结束时间（保留 XMLTV 原始字符串，交给前端按需展示）。
//!
//! EPG 地址支持 `{name}` / `{date}` 模板（参考 `LivePlayActivity.buildEpgUrl`）：由
//! [`render_epg_url`] 对频道名做 URL 编码后替换；`date` 由调用方按本地时区（参考实现用
//! GMT+8）格式化为 `yyyy-MM-dd` 传入。没有模板的地址原样请求。

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::Rules;

/// 单个节目
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EpgProgram {
    /// XMLTV `channel` 属性（台标 id，一般与 m3u 的 `tvg-id` 对应）
    pub channel: String,
    pub title: String,
    /// XMLTV 时间原串，如 `20261028120000 +0800`
    pub start: String,
    pub end: String,
}

/// 拉取并解析某 EPG 地址（XMLTV）。
///
/// 使用 [`crate::http`] 的统一规则（UA / 超时 / 重试）。URL 模板（`{name}`/`{date}`）
/// 需调用方先用 [`render_epg_url`] 渲染好再传入。
pub fn fetch_epg(epg_url: &str) -> Result<Vec<EpgProgram>> {
    fetch_epg_inner(epg_url).map_err(|e| crate::sources::map_aux_error("epg", e))
}

fn fetch_epg_inner(epg_url: &str) -> Result<Vec<EpgProgram>> {
    let epg_url = epg_url.trim();
    if epg_url.is_empty() {
        return Err(CoreError::Config("EPG 地址为空".into()));
    }
    if !epg_url.starts_with("http://") && !epg_url.starts_with("https://") {
        return Err(CoreError::Config(format!(
            "EPG 地址仅支持 http(s): {epg_url}"
        )));
    }

    let rules = Rules::default();
    let client = http::client_for(&rules)?;
    let text = http::get_text(&client, epg_url, &HashMap::new(), rules.retry)?;
    parse_epg_xml(&text)
}

/// 解析 XMLTV 文本，返回 `<programme>` 列表。
///
/// 容错：没有 `<title>` 的节目跳过；`<title>` 内的实体（`&amp;` 等）与 CDATA 会还原；
/// 同一节目有多个 `<title>`（多语言）时取第一个。
pub fn parse_epg_xml(xml: &str) -> Result<Vec<EpgProgram>> {
    let xml = xml.trim_start_matches('\u{feff}');
    if xml.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut reader = Reader::from_str(xml);
    // 不要 trim_text：`&amp;` 会被拆成 Text + GeneralRef + Text，逐段 trim 会吃掉
    // “新闻联播 & 天气预报”里的空格。标题最终统一 trim。

    let mut programs: Vec<EpgProgram> = Vec::new();
    let mut current: Option<EpgProgram> = None;
    let mut in_title = false;
    let mut title_done = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => match start.local_name().as_ref() {
                b"programme" => {
                    current = Some(EpgProgram {
                        channel: xml_attr(&start, b"channel"),
                        title: String::new(),
                        start: xml_attr(&start, b"start"),
                        end: xml_attr(&start, b"stop"),
                    });
                    in_title = false;
                    title_done = false;
                }
                b"title" if current.is_some() && !title_done => in_title = true,
                _ => {}
            },
            Ok(Event::Empty(_)) => {
                // <programme .../> 之类的空节目没有标题，直接忽略
            }
            Ok(Event::Text(text)) => {
                if in_title {
                    if let Some(program) = current.as_mut() {
                        let raw = text
                            .decode()
                            .map_err(|e| CoreError::Config(format!("EPG XML 文本编码错误: {e}")))?;
                        let decoded = quick_xml::escape::unescape(&raw)
                            .map_err(|e| CoreError::Config(format!("EPG XML 转义错误: {e}")))?;
                        program.title.push_str(&decoded);
                    }
                }
            }
            Ok(Event::CData(text)) => {
                if in_title {
                    if let Some(program) = current.as_mut() {
                        program
                            .title
                            .push_str(&String::from_utf8_lossy(&text.into_inner()));
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                // Text 里拆出来的实体引用：&#65; 或 &amp; 之类
                if in_title {
                    if let Some(program) = current.as_mut() {
                        match reference.resolve_char_ref() {
                            Ok(Some(ch)) => program.title.push(ch),
                            Ok(None) => {
                                let name = reference.decode().map_err(|e| {
                                    CoreError::Config(format!("EPG XML 实体编码错误: {e}"))
                                })?;
                                if let Some(value) =
                                    quick_xml::escape::resolve_predefined_entity(&name)
                                {
                                    program.title.push_str(value);
                                }
                            }
                            Err(e) => {
                                return Err(CoreError::Config(format!("EPG XML 字符引用错误: {e}")))
                            }
                        }
                    }
                }
            }
            Ok(Event::End(end)) => match end.local_name().as_ref() {
                b"title" => {
                    in_title = false;
                    title_done = true;
                }
                b"programme" => {
                    if let Some(mut program) = current.take() {
                        program.title = program.title.trim().to_string();
                        if !program.title.is_empty() {
                            programs.push(program);
                        }
                    }
                    in_title = false;
                    title_done = false;
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(CoreError::Config(format!(
                    "EPG XML 解析失败（字节位置 {}）: {e}",
                    reader.error_position()
                )))
            }
            _ => {}
        }
    }

    // 容忍缺少 </programme> 的脏 XML：把最后一个节目收下
    if let Some(mut program) = current.take() {
        program.title = program.title.trim().to_string();
        if !program.title.is_empty() {
            programs.push(program);
        }
    }

    Ok(programs)
}

/// 渲染 EPG URL 模板（参考 `LivePlayActivity.buildEpgUrl`）：
/// - `{name}` → URL 编码后的频道名（空格编码为 `%20`）；
/// - `{date}` → 调用方传入的 `yyyy-MM-dd`；
/// - 不含占位符的地址原样返回。
pub fn render_epg_url(template: &str, channel_name: &str, date_ymd: &str) -> String {
    let encoded = encode_query_value(channel_name);
    template
        .replace("{name}", &encoded)
        .replace("{date}", date_ymd)
}

/// 按 application/x-www-form-urlencoded 规则编码，并把空格统一成 `%20`
/// （等价 Java `URLEncoder.encode(v, "UTF-8").replace("+", "%20")`）。
fn encode_query_value(value: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    // URLEncoder 不转义 - _ . *，这里保持一致（其余非字母数字全部 %XX）
    const QUERY_SET: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'*');
    utf8_percent_encode(value, QUERY_SET).to_string()
}

/// 读取 XML 属性并还原实体；缺失/非法时返回空串
fn xml_attr(element: &BytesStart<'_>, key: &[u8]) -> String {
    match element.try_get_attribute(key) {
        Ok(Some(attr)) => match attr.unescape_value() {
            Ok(value) => value.trim().to_string(),
            Err(_) => String::from_utf8_lossy(&attr.value).trim().to_string(),
        },
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <programme start="20261028120000 +0800" stop="20261028130000 +0800" channel="cctv1">
    <title lang="zh">新闻联播 &amp; 天气预报</title>
    <title lang="en">News</title>
  </programme>
  <programme start="20261028130000 +0800" stop="20261028140000 +0800" channel="cctv2">
    <title><![CDATA[电视剧 <A>]]></title>
  </programme>
  <programme start="20261028140000 +0800" stop="20261028150000 +0800" channel="empty"/>
</tv>"#;

    #[test]
    fn parses_xmltv_programmes() {
        let programs = parse_epg_xml(XML).unwrap();
        assert_eq!(programs.len(), 2);
        assert_eq!(programs[0].channel, "cctv1");
        assert_eq!(programs[0].title, "新闻联播 & 天气预报");
        assert_eq!(programs[0].start, "20261028120000 +0800");
        assert_eq!(programs[0].end, "20261028130000 +0800");
        assert_eq!(programs[1].channel, "cctv2");
        assert_eq!(programs[1].title, "电视剧 <A>");
    }

    #[test]
    fn empty_and_malformed_xml() {
        assert!(parse_epg_xml("").unwrap().is_empty());
        assert!(parse_epg_xml("<tv></tv>").unwrap().is_empty());
        assert!(parse_epg_xml("<tv><programme></tv>").is_err());
    }

    #[test]
    fn renders_epg_url_template() {
        let url = render_epg_url(
            "http://epg.example.com/api?ch={name}&date={date}",
            "CCTV-1 综合",
            "2026-10-28",
        );
        assert_eq!(
            url,
            "http://epg.example.com/api?ch=CCTV-1%20%E7%BB%BC%E5%90%88&date=2026-10-28"
        );
        assert_eq!(
            render_epg_url("http://epg.example.com/all.xml", "CCTV1", "2026-10-28"),
            "http://epg.example.com/all.xml"
        );
    }
}
