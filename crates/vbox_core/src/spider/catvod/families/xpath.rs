//! `csp_XPath`：TVBox 标准 XPath 规则引擎（规则来自 `ext`）。
//!
//! 来源（REA 反编译证据）：
//! * `com.github.catvod.spider.XPath`，`_rea/decompiled/0f047745-FxzHm/deobf/
//!   com/github/catvod/spider/XPath.java`（262 行）；`detailContent` 被 jadx 跳过
//!   （922 指令），用 `--show-bad-code` 恢复到 `_rea/tmp/r138/XPath.bad.java`（698 行）。
//! * 规则模型 `com.github.catvod.parser.merge.O.a`（477 行）与节点选取
//!   `com.github.catvod.parser.merge.I0.a`（jar 内嵌 XPath 引擎）的语义已还原在
//!   `crates/vbox_core/src/spider/catvod/xpath_rule.rs` 的文档里。
//!
//! `ext` 约定（原类 `init`）：
//! - 单段：规则 JSON 文本，或 `http(s)://…` 规则地址（惰性拉取）；
//! - 两段 `规则$$$proxy`：第二段是本地代理前缀，桌面端无 `Proxy` 概念 → 忽略（偏差 1）。
//!
//! 规则键（全部可选，缺失=不启用对应步骤）：
//! `ua`、`homeUrl`、`cateNode/cateName/cateNameR/cateId/cateIdR/cateManual/filter`、
//! `homeVodNode/homeVodName/homeVodId/homeVodImg/homeVodMark`、
//! `cateUrl/cateVodNode/cateVodName/cateVodId/cateVodImg/cateVodMark`、
//! `dtUrl/dtNode/dtName/dtImg/dtCate/dtYear/dtArea/dtMark/dtActor/dtDirector/dtDesc`、
//! `dtFromNode/dtFromName/dtUrlNode/dtUrlSubNode/dtUrlId/dtUrlName`、
//! `playUrl/playUa/playReferer/searchUrl/scVodNode/scVodName/scVodId/scVodImg/scVodMark`。
//!
//! 有意偏差（记录在计划文档附录 EN）：
//! 1. `$$$` 第二段的 proxy 前缀被忽略（桌面端无本地代理）。
//! 2. 宿主分类接口不携带筛选参数（`Category` 只有 id/name），因此 `filter` 规则与
//!    `videos` 的 `filter` 入参暂不消费（原类可把筛选透传到 `cateUrl` 之外）。
//! 3. 原类 `categoryContent` 恒 `pagecount=1`（不翻页），这里保留同样行为。
//! 4. 原类 `cateVodMark` 复用了 `cateVodNameR` 的正则（反编译可见的原类 bug），保留。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};
use super::super::xpath_rule::{node_text, parse_page, NodeRef, Page, Rule};

pub const API: &str = "csp_XPath";
/// 规则未配置 `ua` 时的兜底请求头（原类 `b()` 内置 Chrome/122）。
const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 是否属于本家族（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn headers(rule: &Rule) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert(
        "User-Agent".to_string(),
        rule.opt("ua").unwrap_or_else(|| DEFAULT_UA.to_string()),
    );
    map
}

fn fetch(cfg: &TvConfig, rule: &Rule, url: &str) -> Result<String> {
    let client = client_for(&cfg.http)?;
    get_text(&client, url, &headers(rule), 2)
}

/// 载入规则：`ext` 为规则 JSON 文本或规则 URL（`$$$proxy` 第二段忽略）。
pub fn load_rule(cfg: &TvConfig, site: &Site) -> Result<Rule> {
    let ext = site.ext_string();
    let ext = ext.trim();
    if ext.is_empty() {
        return Err(parse_err(site, "csp_XPath 的 ext 为空：需要规则 JSON 或其 URL"));
    }
    let spec = match ext.split_once("$$$") {
        Some((head, _proxy)) => head.trim(),
        None => ext,
    };
    let text = if spec.starts_with("http") {
        fetch(cfg, &Rule::default(), spec)?
    } else {
        spec.to_string()
    };
    Rule::from_str(&text).ok_or_else(|| {
        parse_err(
            site,
            format!("csp_XPath 规则 JSON 解析失败（{} 字节）", text.len()),
        )
    })
}

/// 相对地址补全（等价于 Java `URI.resolve` 的常见情形）：
/// 绝对地址原样；`//host/x` 补 https；`/x` 挂到 origin；`x/y` 挂到基址目录。
fn join_url(base: &str, url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if url.starts_with("//") {
        return format!("https:{url}");
    }
    let base = base.trim();
    if url.starts_with('/') {
        if let Some(offset) = base.find("://").map(|index| index + 3) {
            let rest = &base[offset..];
            let end = rest.find('/').map(|pos| offset + pos).unwrap_or(base.len());
            return format!("{}{}", &base[..end], url);
        }
        return format!("{}{}", base.trim_end_matches('/'), url);
    }
    let dir = match base.rfind('/') {
        Some(pos) if pos > base.find("://").unwrap_or(0) + 2 => &base[..pos + 1],
        _ => {
            if base.is_empty() {
                ""
            } else {
                base
            }
        }
    };
    if dir.is_empty() {
        url.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{url}")
    } else {
        format!("{dir}/{url}")
    }
}

/// 只做最小编码（空格、`&`、`#`、`?`、`=`、`+`、`%` 等交给 `%XX`）。
fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 模板占位符替换（`{cateId}` / `{catePg}` / `{wd}` / `{vid}` / `{playUrl}`）。
fn apply_tpl(tpl: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = tpl.to_string();
    for (key, value) in pairs {
        out = out.replace(key, value);
    }
    out
}

/// 取子节点的字符串值：先按 `<key>` 选择器取第一个节点，再套 `<key>R` 正则。
fn node_value(page: &Page, node: NodeRef<'_>, rule: &Rule, key: &str) -> String {
    let selector = rule.s(key);
    if selector.is_empty() {
        return String::new();
    }
    let raw = page
        .first(node, &selector)
        .map(node_text)
        .unwrap_or_default();
    rule.apply(key, raw.trim())
}

/// 把相对封面补成绝对地址（基址优先 `homeUrl`，其次当前页 URL）。
fn cover_url(rule: &Rule, page_url: &str, raw: &str) -> String {
    let base = rule.opt("homeUrl").unwrap_or_else(|| page_url.to_string());
    join_url(&base, raw)
}

fn item_of(page: &Page, node: NodeRef<'_>, rule: &Rule, prefix: &str, page_url: &str) -> VideoItem {
    let id = node_value(page, node, rule, &format!("{prefix}Id"));
    let name = node_value(page, node, rule, &format!("{prefix}Name"));
    let img = node_value(page, node, rule, &format!("{prefix}Img"));
    let mark = node_value(page, node, rule, &format!("{prefix}Mark"));
    VideoItem {
        id,
        name,
        cover: {
            let cover = cover_url(rule, page_url, &img);
            if cover.is_empty() { None } else { Some(cover) }
        },
        remark: if mark.is_empty() { None } else { Some(mark) },
    }
}

/// 解析条目列表（首页推荐 / 分类 / HTML 搜索共用）。
fn parse_vod_list(html: &str, rule: &Rule, prefix: &str, page_url: &str) -> Vec<VideoItem> {
    let selector = rule.s(&format!("{prefix}Node"));
    if selector.is_empty() {
        return Vec::new();
    }
    let Some(page) = parse_page(html) else { return Vec::new() };
    page.select_root(&selector)
        .into_iter()
        .map(|node| item_of(&page, node, rule, prefix, page_url))
        .filter(|item| !item.name.is_empty() || !item.id.is_empty())
        .collect()
}

pub fn parse_categories(html: &str, rule: &Rule) -> Vec<Category> {
    if rule.obj("cateManual").is_some() {
        let manual = rule.str_map("cateManual");
        if !manual.is_empty() {
            return manual
                .into_iter()
                .map(|(name, type_id)| Category { type_id, type_name: name })
                .collect();
        }
    }
    let selector = rule.s("cateNode");
    if selector.is_empty() {
        return Vec::new();
    }
    let Some(page) = parse_page(html) else { return Vec::new() };
    page
        .select_root(&selector)
        .into_iter()
        .filter_map(|node| {
            let name = node_value(&page, node, rule, "cateName");
            let id = node_value(&page, node, rule, "cateId");
            if id.is_empty() {
                return None;
            }
            Some(Category {
                type_id: id.clone(),
                type_name: if name.is_empty() { id } else { name },
            })
        })
        .collect()
}

/// 分类列表（`cateManual` 优先，否则拉 `homeUrl` 解析 `cateNode`）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let rule = load_rule(cfg, site)?;
    if rule.obj("cateManual").is_some() {
        let list = rule.str_map("cateManual");
        if !list.is_empty() {
            return Ok(list
                .into_iter()
                .map(|(name, type_id)| Category { type_id, type_name: name })
                .collect());
        }
    }
    let home = rule.opt("homeUrl").unwrap_or_default();
    let html = if home.is_empty() { String::new() } else { fetch(cfg, &rule, &home)? };
    Ok(parse_categories(&html, &rule))
}

/// 分类内容：`cateUrl` 模板 + `{cateId}`/`{catePg}`。
pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    cat: &str,
    page: u32,
    _filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    let rule = load_rule(cfg, site)?;
    let tpl = rule.s("cateUrl");
    if tpl.is_empty() {
        return Ok(Vec::new());
    }
    let page_text = page.max(1).to_string();
    let url = apply_tpl(&tpl, &[("{cateId}", cat), ("{catePg}", &page_text)]);
    let html = fetch(cfg, &rule, &url)?;
    Ok(parse_vod_list(&html, &rule, "cateVod", &url))
}

fn opt_value(
    page: &Page,
    node: NodeRef<'_>,
    rule: &Rule,
    key: &str,
) -> Option<String> {
    let value = node_value(page, node, rule, key);
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 详情：`dtUrl` → `dtNode`，可选字段按规则存在才取。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let rule = load_rule(cfg, site)?;
    let tpl = rule.s("dtUrl");
    if tpl.is_empty() {
        return Err(parse_err(site, "csp_XPath 规则缺少 dtUrl"));
    }
    let url = apply_tpl(&tpl, &[("{vid}", id)]);
    let html = fetch(cfg, &rule, &url)?;
    let Some(page) = parse_page(&html) else {
        return Err(parse_err(site, "csp_XPath 详情页 HTML 解析失败"));
    };
    let node_sel = rule.s("dtNode");
    if node_sel.is_empty() {
        return Err(parse_err(site, "csp_XPath 规则缺少 dtNode"));
    }
    let Some(node) = page.select_root(&node_sel).into_iter().next() else {
        return Err(parse_err(site, "csp_XPath 详情页未匹配 dtNode"));
    };

    let name = node_value(&page, node, &rule, "dtName");
    let img = node_value(&page, node, &rule, "dtImg");
    let mut detail = VideoDetail {
        id: id.to_string(),
        name: if name.is_empty() { id.to_string() } else { name },
        ..Default::default()
    };
    let cover = cover_url(&rule, &url, &img);
    if !cover.is_empty() {
        detail.cover = Some(cover);
    }
    detail.desc = opt_value(&page, node, &rule, "dtDesc");
    detail.year = opt_value(&page, node, &rule, "dtYear");
    detail.area = opt_value(&page, node, &rule, "dtArea");
    detail.director = opt_value(&page, node, &rule, "dtDirector");
    detail.actor = opt_value(&page, node, &rule, "dtActor");

    let from_sel = rule.s("dtFromNode");
    let from_names: Vec<String> = if from_sel.is_empty() {
        Vec::new()
    } else {
        page.select_root(&from_sel)
            .into_iter()
            .map(|n| node_value(&page, n, &rule, "dtFromName"))
            .collect()
    };

    let url_node_sel = rule.s("dtUrlNode");
    let sub_sel = rule.s("dtUrlSubNode");
    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    if !url_node_sel.is_empty() {
        for (index, line) in page.select_root(&url_node_sel).into_iter().enumerate() {
            let subs = if sub_sel.is_empty() {
                Vec::new()
            } else {
                page.select(line, &sub_sel)
            };
            let mut items: Vec<Episode> = Vec::new();
            for sub in subs {
                let ep_name = node_value(&page, sub, &rule, "dtUrlName");
                let ep_id = node_value(&page, sub, &rule, "dtUrlId");
                if ep_id.is_empty() {
                    continue;
                }
                items.push(Episode {
                    title: if ep_name.is_empty() { ep_id.clone() } else { ep_name },
                    url: ep_id,
                });
            }
            if items.is_empty() {
                continue;
            }
            let group = match from_names.get(index) {
                Some(name) if !name.is_empty() => name.clone(),
                _ => format!("线路{}", episodes.len() + 1),
            };
            episodes.push(EpisodeGroup { group, items });
        }
    }
    detail.episodes = episodes;
    Ok(detail)
}

/// `json:` 前缀的搜索规则：按 `>` 分段逐层取 JSON 数组。
fn parse_json_search(body: &str, rule: &Rule, path: &str) -> Vec<VideoItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut current = &root;
    for part in path.split('>').map(str::trim).filter(|p| !p.is_empty()) {
        let Some(next) = current.get(part) else {
            return Vec::new();
        };
        current = next;
    }
    let Some(rows) = current.as_array() else {
        return Vec::new();
    };
    let home = rule.opt("homeUrl").unwrap_or_default();
    rows.iter()
        .filter_map(|row| {
            let field = |key: &str| -> String {
                row.get(key)
                    .and_then(|v| v.as_str())
                    .map(|raw| rule.apply(key, raw.trim()))
                    .unwrap_or_default()
            };
            let id = field("scVodId");
            let name = field("scVodName");
            if id.is_empty() && name.is_empty() {
                return None;
            }
            let img = field("scVodImg");
            let cover = cover_url(rule, &home, &img);
            let mark = field("scVodMark");
            Some(VideoItem {
                id,
                name,
                cover: if cover.is_empty() { None } else { Some(cover) },
                remark: if mark.is_empty() { None } else { Some(mark) },
            })
        })
        .collect()
}

/// 搜索：HTML 列表，或 `scVodNode` 以 `json:` 开头时走 JSONPath。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let rule = load_rule(cfg, site)?;
    let tpl = rule.s("searchUrl");
    if tpl.is_empty() {
        return Ok(Vec::new());
    }
    let url = apply_tpl(&tpl, &[("{wd}", &urlencode(keyword))]);
    let body = fetch(cfg, &rule, &url)?;
    let selector = rule.s("scVodNode");
    if let Some(path) = selector.strip_prefix("json:") {
        return Ok(parse_json_search(&body, &rule, path));
    }
    Ok(parse_vod_list(&body, &rule, "scVod", &url))
}

/// 播放：`{playUrl}` 模板 + 规则里的 UA/Referer，恒 `parse: 0`。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    let rule = load_rule(cfg, site)?;
    let tpl = rule.s("playUrl");
    let url = if tpl.is_empty() {
        id.to_string()
    } else {
        apply_tpl(&tpl, &[("{playUrl}", id)])
    };
    let mut header = serde_json::Map::new();
    if let Some(ua) = rule.opt("playUa") {
        header.insert("User-Agent".to_string(), Value::String(ua));
    }
    if let Some(referer) = rule.opt("playReferer") {
        header.insert("Referer".to_string(), Value::String(referer));
    }
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": Value::Object(header),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = r#"{
      "ua": "UA-Test/1.0",
      "homeUrl": "https://example.test",
      "cateNode": "//ul[@class='nav']/li/a",
      "cateName": "/text()",
      "cateId": "/@href",
      "cateIdR": "/list-(\\d+)\\.html",
      "homeVodNode": "//div[@class='item']",
      "homeVodName": "/a[@class='title']/@title",
      "homeVodId": "/a[@class='title']/@href",
      "homeVodIdR": "/v/(\\w+)\\.html",
      "homeVodImg": "/a/@data-bg",
      "homeVodMark": "/span[@class='desc']/text()",
      "cateUrl": "https://example.test/list-{cateId}-{catePg}.html",
      "cateVodNode": "//ul[@class='vlist']/li",
      "cateVodName": "/a/text()",
      "cateVodId": "/a/@href",
      "cateVodIdR": "/v/(\\w+)\\.html",
      "cateVodImg": "/img/@src",
      "cateVodMark": "/span/text()",
      "dtUrl": "https://example.test/v/{vid}.html",
      "dtNode": "//div[@class='content']",
      "dtName": "/h1/text()",
      "dtImg": "/div[@class='cover']/img/@src",
      "dtYear": "/p[@class='year']/text()",
      "dtDesc": "/div[@class='desc']/text()",
      "dtFromNode": "//ul[@class='froms']/li",
      "dtFromName": "/text()",
      "dtUrlNode": "//ul[@class='lists']",
      "dtUrlSubNode": "/li/a",
      "dtUrlId": "/@href",
      "dtUrlIdR": "/p/(\\S+)\\.html",
      "dtUrlName": "/text()",
      "playUrl": "https://example.test/p/{playUrl}.html",
      "playUa": "UA-Play/2.0",
      "playReferer": "https://example.test/",
      "searchUrl": "https://example.test/s?wd={wd}",
      "scVodNode": "//div[@class='result']",
      "scVodName": "/a/text()",
      "scVodId": "/a/@href",
      "scVodIdR": "/v/(\\w+)\\.html",
      "scVodImg": "/img/@src",
      "scVodMark": "/span/text()"
    }"#;

    fn rule() -> Rule {
        Rule::from_str(RULE).expect("rule")
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_XPath"));
        assert!(handles("CSP_XPATH"));
        assert!(!handles("csp_Nmys"));
    }

    #[test]
    fn categories_follow_cate_manual() {
        let r = Rule::from_str(r#"{"cateManual": {"电影": "1", "电视剧": "2"}}"#).unwrap();
        let categories = parse_categories("", &r);
        assert_eq!(categories.len(), 2);
        assert_eq!(categories[0].type_id, "1");
        assert_eq!(categories[0].type_name, "电影");
    }

    #[test]
    fn categories_from_nav_nodes() {
        let html = r#"<html><body><ul class="nav">
          <li><a href="/list-3.html">动作片</a></li>
          <li><a href="/list-4.html">喜剧片</a></li>
        </ul></body></html>"#;
        let categories = parse_categories(html, &rule());
        assert_eq!(categories.len(), 2);
        assert_eq!(categories[0].type_id, "3");
        assert_eq!(categories[0].type_name, "动作片");
    }

    #[test]
    fn vod_list_parses_items_and_joins_cover() {
        let html = r#"<html><body><ul class="vlist">
          <li><a href="/v/abc.html">第一部</a><img src="/img/a.jpg"/><span>更新至 8 集</span></li>
        </ul></body></html>"#;
        let items = parse_vod_list(html, &rule(), "cateVod", "https://example.test/list-1-1.html");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "abc");
        assert_eq!(items[0].name, "第一部");
        assert_eq!(items[0].cover.as_deref(), Some("https://example.test/img/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新至 8 集"));
    }

    #[test]
    fn detail_reads_optional_fields_and_two_lines() {
        let html = r#"<html><body><div class="content">
          <h1>测试剧</h1>
          <div class="cover"><img src="/cover.jpg"/></div>
          <p class="year">2024</p>
          <div class="desc">简介文本</div>
          <ul class="froms"><li>线路一</li><li>线路二</li></ul>
          <ul class="lists">
            <li><a href="/p/aaa.html">第1集</a><a href="/p/bbb.html">第2集</a></li>
          </ul>
          <ul class="lists">
            <li><a href="/p/ccc.html">第3集</a></li>
          </ul>
        </div></body></html>"#;
        let page = parse_page(html).expect("page");
        let rule = rule();
        let node = page.select_root("//div[@class='content']").remove(0);
        assert_eq!(node_value(&page, node, &rule, "dtName"), "测试剧");
        assert_eq!(
            cover_url(&rule, "https://example.test/v/1.html", &node_value(&page, node, &rule, "dtImg")),
            "https://example.test/cover.jpg"
        );
        assert_eq!(opt_value(&page, node, &rule, "dtYear").as_deref(), Some("2024"));
        assert_eq!(opt_value(&page, node, &rule, "dtActor"), None);
    }

    #[test]
    fn search_html_list_parses() {
        let html = r#"<html><body><div class="result">
          <a href="/v/xyz.html">命中</a><img src="/s.jpg"/><span>备注</span>
        </div></body></html>"#;
        let items = parse_vod_list(html, &rule(), "scVod", "https://example.test/s?wd=x");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "xyz");
        assert_eq!(items[0].name, "命中");
    }

    #[test]
    fn search_json_path_reads_rows() {
        let rule = rule();
        let body = r#"{"data":{"list":[{"scVodId":"v1","scVodName":"甲"},{"scVodId":"v2","scVodName":"乙"}]}}"#;
        let items = parse_json_search(body, &rule, "data>list");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "v1");
        assert_eq!(items[1].name, "乙");
        assert!(parse_json_search("not json", &rule, "data>list").is_empty());
    }

    #[test]
    fn url_helpers_behave() {
        assert_eq!(urlencode("斗 罗&x"), "%E6%96%97%20%E7%BD%97%26x");
        assert_eq!(join_url("https://a.test/", "/x"), "https://a.test/x");
        assert_eq!(join_url("https://a.test/", "//c.test/x"), "https://c.test/x");
        assert_eq!(join_url("https://a.test", "y.jpg"), "https://a.test/y.jpg");
        assert_eq!(
            apply_tpl("https://x/list-{cateId}-{catePg}.html", &[("{cateId}", "1"), ("{catePg}", "2")]),
            "https://x/list-1-2.html"
        );
    }
    #[test]
    fn page_tolerates_unquoted_quote_in_meta_attribute() {
        // dm84 详情页真实片段：description 的属性值里有未转义的 `"`。
        // html5ever 会把后半段序列化成「中文属性名」，sxd 在 "=" 处报 XML 错误；
        // sanitize_xml 必须只丢弃这两个畸形标签，保留正文标题与选择器。
        let html = r#"<html><head><meta name="description" content="觉醒的"废物"林家大少爷" /><meta charset="utf-8"></head><body><div class="content"><h1 class="v_title">标题</h1><div class="desc">简介</div></div></body></html>"#;
        let page = parse_page(html).expect("page");
        let titles = page.select_root("//h1[@class='v_title']");
        assert_eq!(titles.len(), 1);
        assert_eq!(node_text(titles[0]), "标题");
        let descs = page.select_root("//div[@class='desc']");
        assert_eq!(descs.len(), 1);
        assert_eq!(node_text(descs[0]), "简介");
    }

    #[test]
    fn sanitizer_keeps_gt_in_quoted_attr_and_drops_malformed_tags() {
        // 回归：上一版用 `<[^>]*>` 扫标签，属性值里合法的 `>` 会把开标签截断后整段丢弃。
        // 现在按引号状态找标签结束，只丢弃真正畸形的标签。
        let html = r#"<html><body>
<div class="ok" data-x="a>b"><span class="t">甲</span></div>
<meta name="description" content="觉醒的"废物"林家大少爷" />
<p class="raw">a & b</p>
<div class="ok2"><script>var x = "<div class='fake'>";</script><b>乙</b></div>
</body></html>"#;
        let page = parse_page(html).expect("page");
        assert_eq!(page.select_root("//div[@class='ok']").len(), 1);
        assert_eq!(page.select_root("//span[@class='t']").len(), 1);
        assert_eq!(node_text(page.select_root("//span[@class='t']")[0]), "甲");
        assert_eq!(page.select_root("//p[@class='raw']").len(), 1, "裸 & 不应导致标签被丢");
        assert_eq!(page.select_root("//div[@class='ok2']").len(), 1);
        assert_eq!(page.select_root("//div[@class='fake']").len(), 0, "script 内容必须删除");
        assert_eq!(page.select_root("//meta[@name='description']").len(), 0);
    }

}
