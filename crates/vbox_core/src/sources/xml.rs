//! type=0 源适配器：XML 格式（苹果CMS XML 接口）
//!
//! 对齐 jun 版 TVBoxOS 的 type=0 分支（`SourceViewModel` + `AbsXml`/`Movie` 的
//! XStream 注解），接口约定：
//!
//! - 分类：GET `{api}`（部分改版接口要求 `{api}?ac=list`），解析
//!   `<class><ty id="1">电影</ty><ty id="2" name="电视剧"/></class>`；
//! - 列表：`{api}?ac=list&t={typeId}&pg={page}`；经典 TVBox/苹果CMS XML 部署实际用
//!   `ac=videolist`，解析失败会自动回退一次（见 [`list_videos`]）；
//! - 详情：`{api}?ac=detail&ids={id}`，经典部署同样回退 `ac=videolist`；
//! - 搜索：`{api}?wd={keyword}`（type=0 不带 `ac`，与参考实现一致）；
//! - 剧集：`<dl><dd flag="线路">第01集$http://...#第02集$http://...</dd></dl>`，
//!   一个 `<dd>` 是一条线路；线路内条目按 `#` 分集，每项按**第一个** `$` 分
//!   「名称 / 地址」，没有名称时用「第N集」（N 为该线路内序号）。
//!
//! 采集站的 XML 脏数据很多，这里统一走 quick-xml 的宽松模式：允许裸 `&`、
//! 允许标签不配对；单条记录字段缺失只留空值，不 panic。响应不是 XML（例如接口
//! 网关返回 HTML/JSON）时返回带地址的明确错误，方便上层提示换源。

use std::collections::HashMap;

use quick_xml::events::{BytesCData, BytesRef, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

/* ============================================================
 * 公共入口（由 source.rs / sources::mod.rs 按 kind=0 路由）
 * ============================================================ */

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    // 订阅里的 categories 只是白名单/排序（字符串写法没有 id）；
    // 真实 type_id 必须来自接口 <class><ty>，否则列表请求会带中文名而拿不到内容。
    let preset = &site.categories;
    let client = http::client_for(&cfg.http)?;
    let headers = site_extra_headers(site);
    let api = site.api.trim();
    if api.is_empty() {
        return if preset.is_empty() {
            Err(CoreError::Config(format!("站点「{}」的 api 为空", site.name)))
        } else {
            Ok(preset.clone())
        };
    }

    let mut api_categories: Vec<Category> = Vec::new();
    let mut last_err: Option<CoreError> = None;

    // 参考实现直接 GET api（不带参数）；少数改版接口要求 ac=list，直连失败再补一次
    match fetch_categories(&client, api, &headers, cfg) {
        Ok(categories) if !categories.is_empty() => api_categories = categories,
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("XML 分类接口直连失败: {e}");
            last_err = Some(e);
        }
    }
    if api_categories.is_empty() {
        let with_ac_list = build_api(api, &[("ac", "list")]);
        match fetch_categories(&client, &with_ac_list, &headers, cfg) {
            Ok(categories) if !categories.is_empty() => api_categories = categories,
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
                tracing::warn!("XML 分类接口不可用，回退站点预置分类: {e}");
            }
            return Ok(preset.clone());
        }
        return Err(last_err.unwrap_or_else(|| {
            CoreError::Config(format!("XML 分类接口未返回 <class><ty>（api={api}）"))
        }));
    }

    Ok(crate::sources::merge_preset_categories(preset, api_categories))
}

pub fn list_videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let client = http::client_for(&cfg.http)?;
    let headers = site_extra_headers(site);
    let page = page.to_string();
    // 经典苹果CMS XML 的列表接口是 ac=videolist；ac=list 只返回 class，
    // 会表现为「分类有、内容空」，因此以 videolist 为主、list 为备。
    let primary = build_api(&site.api, &[("ac", "videolist"), ("t", cat), ("pg", &page)]);
    let fallback = build_api(&site.api, &[("ac", "list"), ("t", cat), ("pg", &page)]);

    match fetch_videos(&client, &primary, &headers, cfg) {
        Ok(items) => {
            if !items.is_empty() || page_num(&page) > 1 {
                return Ok(items);
            }
            match fetch_videos(&client, &fallback, &headers, cfg) {
                Ok(legacy) if !legacy.is_empty() => {
                    tracing::warn!("XML 列表 ac=videolist 为空，已回退 ac=list");
                    Ok(legacy)
                }
                _ => Ok(items),
            }
        }
        Err(primary_err) => match fetch_videos(&client, &fallback, &headers, cfg) {
            Ok(items) => {
                tracing::warn!("XML 列表 ac=videolist 不可用，已回退 ac=list: {primary_err}");
                Ok(items)
            }
            Err(fallback_err) => Err(CoreError::Config(format!(
                "XML 列表接口失败（ac=videolist: {primary_err}；ac=list: {fallback_err}）"
            ))),
        },
    }
}

fn page_num(page: &str) -> u32 {
    page.trim().parse().unwrap_or(1)
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let client = http::client_for(&cfg.http)?;
    let headers = site_extra_headers(site);
    let primary = build_api(&site.api, &[("ac", "detail"), ("ids", id)]);
    let fallback = build_api(&site.api, &[("ac", "videolist"), ("ids", id)]);

    match fetch_detail(&client, &primary, &headers, cfg) {
        Ok(detail) => Ok(detail),
        Err(primary_err) => match fetch_detail(&client, &fallback, &headers, cfg) {
            Ok(detail) => {
                tracing::warn!("XML 详情 ac=detail 不可用，已回退 ac=videolist: {primary_err}");
                Ok(detail)
            }
            Err(fallback_err) => Err(CoreError::Config(format!(
                "XML 详情接口失败（id={id}；ac=detail: {primary_err}；ac=videolist: {fallback_err}）"
            ))),
        },
    }
}

/// 搜索：type=0 只带 `wd`（与 jun 版 `SourceViewModel.getSearch` 的 type0 分支一致）
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let client = http::client_for(&cfg.http)?;
    let headers = site_extra_headers(site);
    let url = build_api(&site.api, &[("wd", keyword)]);
    fetch_videos(&client, &url, &headers, cfg)
}

/* ============================================================
 * HTTP 小工具（与 cms.rs 保持同一语义；不跨模块引用其私有函数）
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

/// 拼接 API 地址；参数统一做表单编码（中文关键词、带 `&` 的 URL 都必须编码）。
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

fn fetch_categories(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &HashMap<String, String>,
    cfg: &TvConfig,
) -> Result<Vec<Category>> {
    let text = http::get_text(client, url, headers, cfg.http.retry)?;
    let parsed = parse_xml(&text)?;
    if !parsed.saw_element {
        return Err(CoreError::Config(format!(
            "分类接口返回内容不是 XML（url={url}）"
        )));
    }
    Ok(parsed.categories)
}

fn fetch_videos(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &HashMap<String, String>,
    cfg: &TvConfig,
) -> Result<Vec<VideoItem>> {
    let text = http::get_text(client, url, headers, cfg.http.retry)?;
    let parsed = parse_xml(&text)?;
    // JSON/HTML 之类的响应可能也能被宽松解析出文本事件，但没有 <list> 就说明拿错了接口
    if !parsed.saw_list {
        return Err(CoreError::Config(format!(
            "XML 列表接口未返回 <list>（url={url}）"
        )));
    }
    Ok(parsed.videos.iter().filter_map(to_video_item).collect())
}

fn fetch_detail(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &HashMap<String, String>,
    cfg: &TvConfig,
) -> Result<VideoDetail> {
    let text = http::get_text(client, url, headers, cfg.http.retry)?;
    let parsed = parse_xml(&text)?;
    if !parsed.saw_list {
        return Err(CoreError::Config(format!(
            "XML 详情接口未返回 <list>（url={url}）"
        )));
    }
    let raw = parsed
        .videos
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Config(format!("XML 详情为空（url={url}）")))?;
    to_detail(raw)
        .ok_or_else(|| CoreError::Config(format!("XML 详情缺少 id/name（url={url}）")))
}

/* ============================================================
 * XML 解析（quick-xml 事件流，单 pass 同时收集 class/video）
 * ============================================================ */

/// 解析过程中的原始 `<video>`：字段名 → 文本；`<dl>` 下每条 `<dd>` 单独保存。
#[derive(Default)]
struct RawVideo {
    fields: HashMap<String, String>,
    /// `(flag, 剧集串)`，一个 `<dd>` 一条线路
    lines: Vec<(String, String)>,
}

impl RawVideo {
    fn field(&self, key: &str) -> String {
        self.fields
            .get(key)
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    }

    fn opt_field(&self, key: &str) -> Option<String> {
        let value = self.field(key);
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }
}

struct ParsedXml {
    videos: Vec<RawVideo>,
    categories: Vec<Category>,
    /// 是否出现过任意 XML 元素：用于识别 JSON/HTML 等非 XML 响应
    saw_element: bool,
    /// 是否出现过 `<list>`：列表/详情响应的最小判据
    saw_list: bool,
}

struct FieldCapture {
    name: String,
    depth: usize,
    buf: String,
}

struct DdCapture {
    depth: usize,
    flag: String,
    buf: String,
}

struct TyCapture {
    id: String,
    attr_name: String,
    depth: usize,
    text: String,
}

fn parse_xml(xml: &str) -> Result<ParsedXml> {
    let mut reader = Reader::from_str(xml);
    {
        // 宽松模式：采集站常见未转义的 `&`、未闭合/不匹配的标签
        let config = reader.config_mut();
        config.allow_dangling_amp = true;
        config.allow_unmatched_ends = true;
        config.check_end_names = false;
    }

    let mut parsed = ParsedXml {
        videos: Vec::new(),
        categories: Vec::new(),
        saw_element: false,
        saw_list: false,
    };
    let mut depth: usize = 0;
    let mut video_depth: Option<usize> = None;
    let mut video: Option<RawVideo> = None;
    let mut field: Option<FieldCapture> = None;
    let mut dd: Option<DdCapture> = None;
    let mut ty: Option<TyCapture> = None;

    loop {
        let event = reader.read_event().map_err(|e| {
            CoreError::Config(format!(
                "XML 解析失败（位置 {}）: {e}",
                reader.buffer_position()
            ))
        })?;
        match event {
            Event::Start(e) => {
                depth += 1;
                parsed.saw_element = true;
                let name = local_name(e.name().as_ref());
                if name == "list" {
                    parsed.saw_list = true;
                }
                if video_depth.is_some() {
                    if name == "video" {
                        // 防御：异常嵌套时重置当前记录
                        video = Some(RawVideo::default());
                        video_depth = Some(depth);
                        field = None;
                        dd = None;
                    } else if name == "dd" {
                        dd = Some(DdCapture {
                            depth,
                            flag: attr_value(&e, "flag").unwrap_or_default(),
                            buf: String::new(),
                        });
                    } else if dd.is_none()
                        && field.is_none()
                        && video_depth == Some(depth.saturating_sub(1))
                        && is_video_field(&name)
                    {
                        field = Some(FieldCapture {
                            name: name.to_string(),
                            depth,
                            buf: String::new(),
                        });
                    }
                } else if name == "video" {
                    video = Some(RawVideo::default());
                    video_depth = Some(depth);
                } else if name == "ty" {
                    ty = Some(TyCapture {
                        id: attr_value(&e, "id")
                            .or_else(|| attr_value(&e, "type_id"))
                            .unwrap_or_default(),
                        attr_name: attr_value(&e, "name").unwrap_or_default(),
                        depth,
                        text: String::new(),
                    });
                }
            }
            Event::Empty(e) => {
                parsed.saw_element = true;
                let name = local_name(e.name().as_ref());
                if name == "list" {
                    parsed.saw_list = true;
                }
                if video_depth.is_none() && name == "ty" {
                    // 兼容 `<ty id="1" name="电影"/>`
                    let id = attr_value(&e, "id")
                        .or_else(|| attr_value(&e, "type_id"))
                        .unwrap_or_default();
                    let name_attr = attr_value(&e, "name").unwrap_or_default();
                    push_category(&mut parsed.categories, &id, &name_attr);
                } else if video_depth.is_some() && name == "dd" {
                    // 兼容空剧集线路（只有 flag）
                    if let Some(v) = video.as_mut() {
                        v.lines
                            .push((attr_value(&e, "flag").unwrap_or_default(), String::new()));
                    }
                }
            }
            Event::Text(t) => append_text(&mut field, &mut dd, &mut ty, &decode_text(&t)),
            Event::CData(c) => append_text(&mut field, &mut dd, &mut ty, &decode_cdata(&c)),
            Event::GeneralRef(r) => append_text(&mut field, &mut dd, &mut ty, &resolve_ref(&r)),
            Event::End(e) => {
                let name = local_name(e.name().as_ref());
                if let Some(vd) = video_depth {
                    if dd.as_ref().is_some_and(|d| d.depth == depth) {
                        if let Some(d) = dd.take() {
                            if let Some(v) = video.as_mut() {
                                v.lines.push((d.flag.trim().to_string(), d.buf));
                            }
                        }
                    } else if field
                        .as_ref()
                        .is_some_and(|f| f.depth == depth && f.name == name)
                    {
                        if let Some(f) = field.take() {
                            if let Some(v) = video.as_mut() {
                                v.fields.insert(f.name, f.buf);
                            }
                        }
                    }
                    if name == "video" && depth == vd {
                        if let Some(v) = video.take() {
                            parsed.videos.push(v);
                        }
                        video_depth = None;
                        field = None;
                        dd = None;
                    }
                } else if let Some(t) = &ty {
                    if t.depth == depth && name == "ty" {
                        let t = ty.take().expect("ty 捕获应存在");
                        let type_name = if !t.text.trim().is_empty() {
                            t.text.trim().to_string()
                        } else {
                            t.attr_name.trim().to_string()
                        };
                        push_category(&mut parsed.categories, &t.id, &type_name);
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            // Decl / Comment / PI / DocType 等与源数据映射无关，直接忽略
            _ => {}
        }
    }

    Ok(parsed)
}

fn push_category(out: &mut Vec<Category>, id: &str, name: &str) {
    let id = id.trim();
    let name = name.trim();
    if id.is_empty() && name.is_empty() {
        return;
    }
    out.push(Category {
        type_id: id.to_string(),
        type_name: name.to_string(),
    });
}

fn append_text(
    field: &mut Option<FieldCapture>,
    dd: &mut Option<DdCapture>,
    ty: &mut Option<TyCapture>,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    if let Some(d) = dd.as_mut() {
        d.buf.push_str(text);
    } else if let Some(f) = field.as_mut() {
        f.buf.push_str(text);
    } else if let Some(t) = ty.as_mut() {
        t.text.push_str(text);
    }
}

fn is_video_field(name: &str) -> bool {
    matches!(
        name,
        "id" | "name"
            | "pic"
            | "note"
            | "des"
            | "year"
            | "area"
            | "director"
            | "actor"
            | "lang"
            | "type"
            | "state"
            | "tag"
            | "last"
            | "action"
    )
}

/// 去掉可能的命名空间前缀（`ns:video` → `video`）
///
/// 返回 `String` 而不是 `&str`：`BytesStart::name()` 返回的 `QName` 是临时值，
/// 直接借用会在语句结束时触发 E0716，这里统一转成拥有所有权的短字符串。
fn local_name(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .unwrap_or_default()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn attr_value(e: &BytesStart, key: &str) -> Option<String> {
    for attr in e.attributes().with_checks(false).flatten() {
        if local_name(attr.key.as_ref()) != key {
            continue;
        }
        // 裸 `&` 等非法转义时 unescape 会失败，退回原始字节
        return Some(match attr.unescape_value() {
            Ok(v) => v.into_owned(),
            Err(_) => String::from_utf8_lossy(&attr.value).into_owned(),
        });
    }
    None
}

fn decode_text(text: &BytesText) -> String {
    let Ok(raw) = text.decode() else {
        return String::new();
    };
    // 0.38 的 Reader 会把 `&amp;` 拆成 GeneralRef 事件，这里再兜底一次完整反转义
    match quick_xml::escape::unescape(&raw) {
        Ok(v) => v.into_owned(),
        Err(_) => raw.to_string(),
    }
}

fn decode_cdata(cdata: &BytesCData) -> String {
    cdata.decode().map(|v| v.into_owned()).unwrap_or_default()
}

/// 解析 `GeneralRef`（`&amp;` / `&#123;` / `&#x7b;`），未知实体原样保留
fn resolve_ref(reference: &BytesRef) -> String {
    let name = reference
        .decode()
        .map(|v| v.into_owned())
        .unwrap_or_default();
    match name.as_str() {
        "amp" => "&".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "quot" => "\"".to_string(),
        "apos" => "'".to_string(),
        _ => {
            let code =
                if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = name.strip_prefix('#') {
                    dec.parse::<u32>().ok()
                } else {
                    None
                };
            match code.and_then(char::from_u32) {
                Some(c) => c.to_string(),
                None => format!("&{name};"),
            }
        }
    }
}

/* ============================================================
 * 原始记录 → 模型
 * ============================================================ */

fn to_video_item(raw: &RawVideo) -> Option<VideoItem> {
    let id = raw.field("id");
    let name = raw.field("name");
    // 没有 id 的条目点开必然失败（详情按 id 请求），列表阶段就丢弃，
    // 避免用户看到一张点不动、只会报「详情为空」的卡片。
    if id.is_empty() {
        return None;
    }
    Some(VideoItem {
        id,
        name,
        cover: raw.opt_field("pic"),
        remark: raw.opt_field("note"),
    })
}

fn to_detail(raw: RawVideo) -> Option<VideoDetail> {
    let id = raw.field("id");
    let name = raw.field("name");
    let id = if id.is_empty() { name.clone() } else { id };
    let name = if name.is_empty() { id.clone() } else { name };
    if id.is_empty() {
        return None;
    }
    Some(VideoDetail {
        id,
        name,
        cover: raw.opt_field("pic"),
        desc: raw.opt_field("des"),
        year: raw.opt_field("year"),
        area: raw.opt_field("area"),
        director: raw.opt_field("director"),
        actor: raw.opt_field("actor"),
        episodes: episodes_from_lines(&raw.lines),
    })
}

/// `<dl><dd flag="线路">名称$地址#名称$地址</dd></dl>` → 多线路多集
fn episodes_from_lines(lines: &[(String, String)]) -> Vec<EpisodeGroup> {
    let mut groups = Vec::new();
    for (flag, body) in lines {
        let items = episodes_from_body(body);
        if items.is_empty() {
            continue;
        }
        let wanted = flag.trim();
        let group = if wanted.is_empty() {
            // 序号只按“已产出的线路”计，空体 dd 不占号，避免 线路1/线路3 跳号。
            format!("线路{}", groups.len() + 1)
        } else {
            wanted.to_string()
        };
        groups.push(EpisodeGroup { group, items });
    }
    groups
}

fn episodes_from_body(body: &str) -> Vec<Episode> {
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
            // 没有 `$` 时整串按地址处理，标题用默认序号
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
    fn list_rows_without_id_are_skipped_and_detail_name_falls_back_to_id() {
        let list = r#"<rss version="2.0"><list><video><id>1</id><name>A</name></video><video><name>无名</name></video></list></rss>"#;
        let parsed = parse_xml(list).unwrap();
        let items: Vec<_> = parsed.videos.iter().filter_map(to_video_item).collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "1");

        let detail = r#"<rss version="2.0"><list><video><id>7</id><dl><dd flag="L">第1集$https://v/1.m3u8</dd></dl></video></list></rss>"#;
        let parsed = parse_xml(detail).unwrap();
        let d = to_detail(parsed.videos.into_iter().next().unwrap()).unwrap();
        assert_eq!(d.id, "7");
        assert_eq!(d.name, "7");
        assert_eq!(d.episodes.len(), 1);

        let empty = r#"<rss version="2.0"><list><video><pic>x</pic></video></list></rss>"#;
        let parsed = parse_xml(empty).unwrap();
        assert!(to_detail(parsed.videos.into_iter().next().unwrap()).is_none());
    }

    #[test]
    fn empty_line_does_not_consume_fallback_group_number() {
        let lines = vec![
            ("A".to_string(), "1$http://v/1.m3u8".to_string()),
            (String::new(), String::new()),
            (String::new(), "2$http://v/2.m3u8".to_string()),
        ];
        let groups = episodes_from_lines(&lines);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "A");
        assert_eq!(groups[1].group, "线路2");
        assert_eq!(groups[1].items[0].title, "2");
    }

    #[test]
    fn parse_categories_text_and_attr_name() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
            <rss version="5.1">
              <class>
                <ty id="1">电影</ty>
                <ty id="2" name="电视剧"/>
                <ty id="3"><![CDATA[动漫]]></ty>
              </class>
            </rss>"#;
        let parsed = parse_xml(xml).unwrap();
        assert_eq!(parsed.categories.len(), 3);
        assert_eq!(parsed.categories[0].type_id, "1");
        assert_eq!(parsed.categories[0].type_name, "电影");
        assert_eq!(parsed.categories[1].type_name, "电视剧");
        assert_eq!(parsed.categories[2].type_name, "动漫");
    }

    #[test]
    fn parse_detail_episodes_and_entities() {
        let xml = r#"<rss><list><video>
            <id>42</id>
            <name><![CDATA[庆余年 & 番外]]></name>
            <des>普通 &amp; 文本</des>
            <dl>
              <dd flag="m3u8">第1集$http://v/1.m3u8#第2集$http://v/2.m3u8</dd>
              <dd flag="mp4">http://v/1.mp4</dd>
            </dl>
          </video></list></rss>"#;
        let parsed = parse_xml(xml).unwrap();
        assert!(parsed.saw_list);
        assert_eq!(parsed.videos.len(), 1);
        let detail = to_detail(parsed.videos.into_iter().next().unwrap()).unwrap();
        assert_eq!(detail.id, "42");
        assert_eq!(detail.name, "庆余年 & 番外");
        assert_eq!(detail.desc.as_deref(), Some("普通 & 文本"));
        assert_eq!(detail.episodes.len(), 2);
        assert_eq!(detail.episodes[0].group, "m3u8");
        assert_eq!(detail.episodes[0].items[1].title, "第2集");
        assert_eq!(detail.episodes[0].items[1].url, "http://v/2.m3u8");
        // 无名称条目用「第N集」
        assert_eq!(detail.episodes[1].items[0].title, "第1集");
        assert_eq!(detail.episodes[1].items[0].url, "http://v/1.mp4");
    }

    #[test]
    fn non_xml_payload_is_detected() {
        let parsed = parse_xml(r#"{"code":1,"list":[]}"#).unwrap();
        assert!(!parsed.saw_element);
        assert!(!parsed.saw_list);
    }

    #[test]
    fn dangling_amp_is_tolerated() {
        let xml = r#"<rss><list><video><id>1</id><name>A & B</name></video></list></rss>"#;
        let parsed = parse_xml(xml).unwrap();
        assert_eq!(parsed.videos.len(), 1);
        assert_eq!(parsed.videos[0].field("name"), "A & B");
    }
}
