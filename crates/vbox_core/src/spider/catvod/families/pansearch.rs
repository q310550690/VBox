//! `csp_PanSearch` 原生实现（首个家族垂直切片）。
//!
//! 语义来自静态反编译（REA，证据 `ev_78181a92…cb933`）：
//! 1. GET `https://www.pansearch.me/` → HTML 里的 `script#__NEXT_DATA__` → `buildId`；
//! 2. GET `https://www.pansearch.me/_next/data/{buildId}/search.json?keyword=<enc>&pan=aliyundrive`
//!    头部 `x-nextjs-data: 1`、`referer: https://www.pansearch.me/`；
//! 3. 取 `pageProps.data.data[]`：`content`（HTML 片段）、`image`、`time`；
//!    标题 = `content` 第一行去标签，链接 = 片段内第一个 `<a href>`。
//!
//! 该类只重写 `init` + `searchContent`（继承 `Ali`），因此：
//! - 无首页/分类；搜索以外的入口返回明确错误（不伪造数据）；
//! - 结果是网盘分享页，不是可播放直链。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::Result;
use crate::model::{Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_PanSearch";
const HOME_URL: &str = "https://www.pansearch.me/";

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn next_data_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<script[^>]*\bid\s*=\s*"__NEXT_DATA__"[^>]*>(.*?)</script>"#)
            .expect("__NEXT_DATA__ 正则")
    })
}

fn href_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<a[^>]*\bhref\s*=\s*"([^"]+)""#).expect("href 正则"))
}

fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)</?[^>]+>").expect("标签正则"))
}

/// 搜索（公开入口，使用真实站点）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    search_with_base(cfg, site, keyword, HOME_URL)
}

/// 搜索（可指定 base，测试用 mock server）。
pub fn search_with_base(
    cfg: &TvConfig,
    site: &Site,
    keyword: &str,
    base: &str,
) -> Result<Vec<VideoItem>> {
    let client = crate::http::client_for(&cfg.http)?;
    let base = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };

    let home = crate::http::get_text(&client, &base, &HashMap::new(), 1)?;
    let build_id = extract_build_id(&home).ok_or_else(|| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            "首页缺少 script#__NEXT_DATA__ 或 buildId（站点改版？）",
        )
        .into_core()
    })?;

    let keyword = percent_encoding::utf8_percent_encode(
        keyword,
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let url = format!("{base}_next/data/{build_id}/search.json?keyword={keyword}&pan=aliyundrive");
    let mut headers = HashMap::new();
    headers.insert("x-nextjs-data".to_string(), "1".to_string());
    headers.insert("referer".to_string(), base.clone());
    let body = crate::http::get_text(&client, &url, &headers, 1)?;

    let value: Value = serde_json::from_str(&body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("搜索响应不是合法 JSON：{e}"),
        )
        .into_core()
    })?;
    Ok(items(&value))
}

/// 从首页 HTML 提取 Next.js 的 `buildId`。
pub fn extract_build_id(html: &str) -> Option<String> {
    let captured = next_data_re().captures(html)?;
    let raw = captured.get(1)?.as_str().trim();
    let value: Value = serde_json::from_str(raw).ok()?;
    value
        .get("buildId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// `pageProps.data.data[]` → 视频卡片。
pub fn items(value: &Value) -> Vec<VideoItem> {
    let Some(list) = value
        .pointer("/pageProps/data/data")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.trim().is_empty() {
                return None;
            }
            // 标题：第一行去标签（原实现 `content.split("\\n")[0].replaceAll("</?[^>]+>","")`）
            let first_line = content.lines().next().unwrap_or("");
            let title = tag_re().replace_all(first_line, "").trim().to_string();
            if title.is_empty() {
                return None;
            }
            // 链接：片段内第一个 <a href>
            let href = href_re()
                .captures(content)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
                .filter(|h| !h.is_empty())?;
            Some(VideoItem {
                id: href,
                name: title,
                cover: item
                    .get("image")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                remark: item
                    .get("time")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            })
        })
        .collect()
}

/// 该家族没有分类（Android 端继承默认空实现）。
pub fn categories() -> Vec<crate::model::Category> {
    Vec::new()
}

/// 该家族没有分类列表（只有搜索）。
pub fn videos() -> Vec<VideoItem> {
    Vec::new()
}

/// 网盘搜索结果没有详情/剧集。
pub fn detail(_cfg: &TvConfig, site: &Site, _id: &str) -> Result<VideoDetail> {
    Err(CatvodError::new(
        ErrorClass::HostApi,
        site,
        "PanSearch 只提供网盘搜索结果，没有 detailContent 契约",
    )
    .into_core())
}

/// 网盘分享页不是可播放直链。
pub fn player_content(_cfg: &TvConfig, site: &Site) -> Result<Value> {
    Err(CatvodError::new(
        ErrorClass::HostApi,
        site,
        "PanSearch 返回网盘分享链接，不支持直接播放",
    )
    .into_core())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_build_id_from_next_data_script() {
        let html = r#"<html><head>
            <script id="__NEXT_DATA__" type="application/json">{"props":{},"buildId":"bid-123"}</script>
        </head></html>"#;
        assert_eq!(extract_build_id(html).as_deref(), Some("bid-123"));
        assert!(extract_build_id("<html></html>").is_none());
    }

    #[test]
    fn parses_items_from_page_props() {
        let value: Value = serde_json::from_str(
            r#"{"pageProps":{"data":{"data":[
              {"content":"<a href=\"https://pan.quark.cn/s/1\">资源一</a>\n磁力: xxx","image":"i1.jpg","time":"2024-01-01"},
              {"content":"<a href=\"https://pan.baidu.com/s/2\">资源二</a>","image":"","time":""},
              {"content":"no link here","image":"","time":""},
              {"content":"","image":"","time":""}
            ]}}}"#,
        )
        .unwrap();
        let items = items(&value);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "https://pan.quark.cn/s/1");
        assert_eq!(items[0].name, "资源一");
        assert_eq!(items[0].cover.as_deref(), Some("i1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("2024-01-01"));
        assert_eq!(items[1].name, "资源二");
        assert!(items[1].cover.is_none());
    }

    #[test]
    fn handles_only_pansearch_api() {
        assert!(handles("csp_PanSearch"));
        assert!(handles(" csp_pansearch "));
        assert!(!handles("csp_Bili"));
    }
}
