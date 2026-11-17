//! `csp_FirstAid` 原生实现（有来医生急救视频 `m.youlai.cn`，HTML 站点）。
//!
//! 语义来自静态反编译：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/FirstAid.java`
//! （117 行，去混淆后可直接读）。类 `extends Spider`，只用公开宿主常量 `C0338a.a(...)`，
//! 不依赖 Android API，可完全重写：
//!
//! 1. `init`：基址写死 `https://m.youlai.cn`（`ext` 未使用）。
//! 2. `homeContent`：8 个写死分类，`type_id` 形如 `jijiu|0`…`jijiu|7`（`|` 后是该分类
//!    在 `/jijiu` 页面中的**区块下标**），名称依次为
//!    急救技能 / 家庭生活 / 急危重症 / 常见损伤 / 动物致伤 / 海洋急救 / 中毒急救 / 意外事故。
//! 3. `categoryContent(typeId, page)`：请求 `{base}/{cate}`（如 `/jijiu`），
//!    取第 `idx` 个 `.jj-title-li` 区块内的 `li.list-br3` 行（`a[href]` 的文本为标题）；
//!    封面取第 `idx` 个 `img.block100` 的 `src`（同一分类共用一张题图）。**不分页**
//!    （原实现也忽略 `page`），`pagecount` 未输出。
//! 4. `detailContent(id)`：`id` 是列表给出的绝对详情地址（`/video/article/399920.html` 等）。
//!    原实现从 DOM 抓 `.video-title.h1-title` / `.video-cover.list-flex-in img` /
//!    `span.doc-name` / `.img-text-con` / `#video source`；现网已改版为 SSR 页面，
//!    这些选择器全部失效，数据改为内嵌 JSON `window.__INITIAL_DATA__` 的
//!    `videoDetailData` 节点（`title`/`lit_pic`/`content`/`media_url`/`doctor.name`）。
//!    因此本实现**优先解析内嵌 JSON**，同时保留旧选择器作兜底（站点回滚时仍可用）。
//! 5. `searchContent(word)`：`/cse/search?q={word}`，行 `.search-video-li.list-br2`，
//!    标题 `h5.line-clamp1`，封面 `dt.logo-bg img[src]`（`//` 开头补 `https:`）。
//! 6. `playerContent(flag, id)`：`id` 即直链 mp4（`media_url`），原样返回 + UA 头。
//!
//! 相对原实现的有意偏差（站点改版所致，均在本文件注释中标记）：
//! - 详情主数据源从 DOM 选择器改为 `window.__INITIAL_DATA__` JSON（旧选择器保留兜底）；
//! - `vod_actor` 取 `videoDetailData.doctor.name`（旧版取 `span.doc-name`，该节点已消失）；
//! - 分类行标题取 `a` 的完整文本（旧版 `C0116a.d("a", li)` 语义相同，只是不再截断）。

use std::collections::HashMap;
use std::sync::OnceLock;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};

use crate::error::Result;
use crate::html_rule;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_FirstAid";

/// 站点写死基址（`FirstAid.java` 字段 `a`）。
const DEFAULT_BASE: &str = "https://m.youlai.cn";

/// 安卓端 UA（`FirstAid.a()`）。
const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 内嵌数据标记。
const INITIAL_DATA_MARKER: &str = "window.__INITIAL_DATA__=";

/// 播放线路名（原实现 `vod_play_from = "Qile"`）。
const PLAY_FROM: &str = "Qile";

/// 8 个写死分类：`(type_id, type_name)`，`|` 后为页面区块下标。
const CATEGORIES: [(&str, &str); 8] = [
    ("jijiu|0", "急救技能"),
    ("jijiu|1", "家庭生活"),
    ("jijiu|2", "急危重症"),
    ("jijiu|3", "常见损伤"),
    ("jijiu|4", "动物致伤"),
    ("jijiu|5", "海洋急救"),
    ("jijiu|6", "中毒急救"),
    ("jijiu|7", "意外事故"),
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn base_of(site: &Site) -> String {
    let ext = site.ext_string();
    let trimmed = ext.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        DEFAULT_BASE.to_string()
    }
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

macro_rules! selector {
    ($fn_name:ident, $css:expr) => {
        fn $fn_name() -> &'static Selector {
            static SEL: OnceLock<Selector> = OnceLock::new();
            SEL.get_or_init(|| Selector::parse($css).expect(concat!("selector ", $css)))
        }
    };
}

selector!(block_sel, "img.block100");
selector!(title_block_sel, ".jj-title-li");
selector!(row_sel, "li.list-br3");
selector!(anchor_sel, "a");
selector!(search_row_sel, ".search-video-li.list-br2");
selector!(search_name_sel, "h5.line-clamp1");
selector!(search_img_sel, "dt.logo-bg img");
selector!(legacy_title_sel, ".video-title.h1-title");
selector!(legacy_cover_sel, ".video-cover.list-flex-in img");
selector!(legacy_doc_sel, "span.doc-name");
selector!(legacy_desc_sel, ".img-text-con");
selector!(legacy_source_sel, "#video source");
selector!(legacy_video_sel, "#video");
selector!(h1_sel, "h1");

/// 分类（原 `homeContent` 的写死表）。
pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

/// 列表（`/jijiu` 页面的第 `idx` 个区块）。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let (cate, idx) = split_cat(cat);
    let client = crate::http::client_for(&cfg.http)?;
    let url = format!("{base}/{cate}");
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    parse_by_block(site, &html, &base, idx)
}

/// 详情（优先内嵌 JSON，旧 CSS 选择器兜底）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        html_rule::join_url(&base, id)
    };
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    let mut detail = if let Some(data) = initial_data(&html) {
        parse_json_detail(site, &base, &url, &data)
    } else {
        parse_legacy_detail(site, &base, &url, &html)
    }?;
    detail.id = id.to_string();
    Ok(detail)
}

/// 搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let client = crate::http::client_for(&cfg.http)?;
    let encoded = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/cse/search?q={encoded}");
    let html = crate::http::get_text(&client, &url, &headers(), cfg.http.retry)?;
    Ok(parse_search(&html, &base))
}

/// 播放：`id` 即 mp4 直链。
pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site);
    let url = if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else {
        html_rule::join_url(&base, id)
    };
    Ok(json!({
        "parse": 0,
        "url": url,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * 解析
 * ============================================================ */

fn split_cat(cat: &str) -> (&str, usize) {
    match cat.rsplit_once('|') {
        Some((cate, idx)) => (cate, idx.trim().parse::<usize>().unwrap_or(0)),
        None => (cat, 0),
    }
}

/// 按区块下标解析列表行。
fn parse_by_block(site: &Site, html: &str, base: &str, idx: usize) -> Result<Vec<VideoItem>> {
    let doc = Html::parse_document(html);
    let blocks: Vec<_> = doc.select(title_block_sel()).collect();
    let Some(block) = blocks.get(idx) else {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            format!(
                "列表页只有 {} 个分类区块，索引 {} 越界（页面结构可能已变）",
                blocks.len(),
                idx
            ),
        )
        .into_core());
    };

    let cover = doc
        .select(block_sel())
        .nth(idx)
        .and_then(|img| img.value().attr("src"))
        .map(|src| abs_image(src, base))
        .filter(|s| !s.is_empty());

    let mut out = Vec::new();
    for row in block.select(row_sel()) {
        let Some(anchor) = row.select(anchor_sel()).next() else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let name = text_of(&anchor);
        if name.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id: html_rule::join_url(base, href),
            name,
            cover: cover.clone(),
            remark: None,
        });
    }
    Ok(out)
}

/// 搜索结果行。
fn parse_search(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    for row in doc.select(search_row_sel()) {
        let Some(anchor) = row.select(anchor_sel()).next() else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let name = row
            .select(search_name_sel())
            .next()
            .map(|el| text_of(&el))
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let cover = row
            .select(search_img_sel())
            .next()
            .and_then(|img| img.value().attr("src"))
            .map(|src| abs_image(src, base))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: html_rule::join_url(base, href),
            name,
            cover,
            remark: None,
        });
    }
    out
}

/// 提取 `window.__INITIAL_DATA__={...}` 里的 JSON（括号配对 + 字符串感知）。
fn initial_data(html: &str) -> Option<Value> {
    let marker = html.find(INITIAL_DATA_MARKER)?;
    let after = &html[marker + INITIAL_DATA_MARKER.len()..];
    let start = after.find('{')?;
    let bytes = after.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    serde_json::from_str(&after[start..end]).ok()
}

/// 从内嵌 JSON 构造详情。
fn parse_json_detail(site: &Site, base: &str, url: &str, root: &Value) -> Result<VideoDetail> {
    let data = root.get("videoDetailData").unwrap_or(root);
    let name = str_field(data, "title").unwrap_or_default();
    let cover = str_field(data, "lit_pic").filter(|s| !s.is_empty());
    let play_url = str_field(data, "media_url")
        .filter(|s| !s.is_empty())
        .or_else(|| str_field(data, "alivod_reference_url").filter(|s| !s.is_empty()));
    let actor = data
        .get("doctor")
        .and_then(|d| d.get("name"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let desc = str_field(data, "content")
        .or_else(|| str_field(data, "description"))
        .map(|raw| html_rule::strip_tags(&raw))
        .filter(|s| !s.is_empty());
    let year = str_field(data, "format_add_time")
        .filter(|s| s.len() >= 4 && s[..4].chars().all(|c| c.is_ascii_digit()))
        .map(|s| s[..4].to_string());

    let mut groups = Vec::new();
    if let Some(play) = play_url {
        let title = if name.is_empty() { "播放".to_string() } else { name.clone() };
        groups.push(EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items: vec![Episode { title, url: play }],
        });
    }

    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情内嵌数据既没有标题也没有播放地址（页面结构可能已变）",
        )
        .into_core());
    }

    let _ = (base, url);
    Ok(VideoDetail {
        id: String::new(),
        name,
        cover,
        desc,
        year,
        area: Some("中国".to_string()),
        director: None,
        actor,
        episodes: groups,
    })
}

/// 旧模板 DOM 兜底（站点回滚或 JSON 缺失时）。
fn parse_legacy_detail(site: &Site, base: &str, _url: &str, html: &str) -> Result<VideoDetail> {
    let doc = Html::parse_document(html);
    let name = doc
        .select(legacy_title_sel())
        .next()
        .map(|el| text_of(&el))
        .or_else(|| doc.select(h1_sel()).next().map(|el| text_of(&el)))
        .unwrap_or_default();
    let cover = doc
        .select(legacy_cover_sel())
        .next()
        .and_then(|img| img.value().attr("src"))
        .map(|src| abs_image(src, base));
    let actor = doc.select(legacy_doc_sel()).next().map(|el| text_of(&el)).filter(|s| !s.is_empty());
    let desc = doc
        .select(legacy_desc_sel())
        .next()
        .map(|el| text_of(&el))
        .filter(|s| !s.is_empty());
    let play = doc
        .select(legacy_source_sel())
        .next()
        .and_then(|el| el.value().attr("src"))
        .or_else(|| {
            doc.select(legacy_video_sel())
                .next()
                .and_then(|el| el.value().attr("src"))
        })
        .map(|s| abs_image(s, base))
        .filter(|s| !s.is_empty());

    let mut groups = Vec::new();
    if let Some(play) = play {
        groups.push(EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items: vec![Episode {
                title: if name.is_empty() { "播放".to_string() } else { name.clone() },
                url: play,
            }],
        });
    }

    if name.is_empty() && groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页既无标题也无播放地址（站点改版或非 FirstAid 模板？）",
        )
        .into_core());
    }
    let _ = base;
    Ok(VideoDetail {
        id: String::new(),
        name,
        cover,
        desc,
        year: None,
        area: Some("中国".to_string()),
        director: None,
        actor,
        episodes: groups,
    })
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `//host/x` / 相对地址补全。
fn abs_image(src: &str, base: &str) -> String {
    let s = src.trim();
    if s.starts_with("//") {
        format!("https:{s}")
    } else if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if s.is_empty() {
        String::new()
    } else {
        html_rule::join_url(base, s)
    }
}

fn text_of(el: &ElementRef<'_>) -> String {
    let mut out = String::new();
    for t in el.text() {
        out.push_str(t);
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            key: "firstaid".to_string(),
            name: "有来急救".to_string(),
            api: API.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_variants() {
        assert!(handles("csp_FirstAid"));
        assert!(handles(" Csp_FirstAid "));
        assert!(!handles("csp_FirstAidExtra"));
        assert_eq!(categories().len(), 8);
        assert_eq!(categories()[0].type_id, "jijiu|0");
        assert_eq!(categories()[7].type_name, "意外事故");
    }

    #[test]
    fn list_by_block_index() {
        let html = r#"
        <img class="block100" src="https://img.test/cat0.png">
        <div class="jj-title-li">
          <ul class="list-flex-br">
            <li class="list-br3"><a href="/jijiu/article/a.html"><div class="line-clamp1">人工呼吸</div></a></li>
            <li class="list-br3"><a href="/jijiu/article/b.html"><div class="line-clamp1">婴儿心脏骤停</div></a></li>
          </ul>
        </div>
        <img class="block100" src="/img/cat1.png">
        <div class="jj-title-li">
          <ul><li class="list-br3"><a href="/jijiu/article/c.html">家庭骨折</a></li></ul>
        </div>"#;
        let rows = parse_by_block(&site(), html, "https://m.youlai.cn", 1).expect("块 1");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "家庭骨折");
        assert_eq!(rows[0].id, "https://m.youlai.cn/jijiu/article/c.html");
        assert_eq!(rows[0].cover.as_deref(), Some("https://m.youlai.cn/img/cat1.png"));

        let zero = parse_by_block(&site(), html, "https://m.youlai.cn", 0).expect("块 0");
        assert_eq!(zero.len(), 2);
        assert_eq!(zero[0].cover.as_deref(), Some("https://img.test/cat0.png"));

        let err = parse_by_block(&site(), html, "https://m.youlai.cn", 5).unwrap_err();
        assert!(format!("{err:?}").contains("越界"));
    }

    #[test]
    fn detail_from_initial_data() {
        let html = r#"<html><body><script>window.__INITIAL_DATA__={"videoDetailData":{"id":"399920","title":"中暑如何处理","lit_pic":"https://file.test/a.jpeg","content":"<p>中暑根据程度不同。</p><br/>2、重症中暑","media_url":"https://vod.test/a-sd.mp4","format_add_time":"2023-03-21 00:00:05","doctor":{"name":"吕有凯"}},"seo":{}};</script></body></html>"#;
        let data = initial_data(html).expect("内嵌 JSON");
        let d = parse_json_detail(&site(), "https://m.youlai.cn", "", &data).expect("详情");
        assert_eq!(d.name, "中暑如何处理");
        assert_eq!(d.cover.as_deref(), Some("https://file.test/a.jpeg"));
        assert_eq!(d.actor.as_deref(), Some("吕有凯"));
        assert_eq!(d.year.as_deref(), Some("2023"));
        assert_eq!(d.area.as_deref(), Some("中国"));
        assert!(d.desc.as_deref().unwrap().contains("中暑根据程度不同"));
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "Qile");
        assert_eq!(d.episodes[0].items[0].url, "https://vod.test/a-sd.mp4");
    }

    #[test]
    fn legacy_detail_fallback() {
        let html = r#"
        <h1 class="video-title h1-title">心肺复苏</h1>
        <div class="video-cover list-flex-in"><img src="//file.test/b.jpeg"></div>
        <span class="doc-name">张三</span>
        <div class="img-text-con">按压深度 5 厘米</div>
        <video id="video"><source src="https://vod.test/b.mp4"></video>"#;
        let d = parse_legacy_detail(&site(), "https://m.youlai.cn", "", html).expect("旧模板");
        assert_eq!(d.name, "心肺复苏");
        assert_eq!(d.cover.as_deref(), Some("https://file.test/b.jpeg"));
        assert_eq!(d.actor.as_deref(), Some("张三"));
        assert_eq!(d.episodes[0].items[0].url, "https://vod.test/b.mp4");
    }

    #[test]
    fn search_rows() {
        let html = r#"
        <div class="search-video-li list-br2">
          <a href="/video/article/399920.html">
            <dl>
              <dt class="logo-bg"><img src="//file.test/c.jpeg"></dt>
              <dd><h5 class="line-clamp1">中暑如何处理</h5></dd>
            </dl>
          </a>
        </div>"#;
        let rows = parse_search(html, "https://m.youlai.cn");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "中暑如何处理");
        assert_eq!(rows[0].id, "https://m.youlai.cn/video/article/399920.html");
        assert_eq!(rows[0].cover.as_deref(), Some("https://file.test/c.jpeg"));
    }

    #[test]
    fn split_cat_parses_index() {
        assert_eq!(split_cat("jijiu|3"), ("jijiu", 3));
        assert_eq!(split_cat("jijiu"), ("jijiu", 0));
    }
}
