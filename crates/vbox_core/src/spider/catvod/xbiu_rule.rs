//! XBiu 自动规则引擎的通用层（第 151 轮 W1）。
//!
//! 对应 Android 类 `com.github.catvod.spider.XBiu`（`extends Spider`）的
//! 静态工具部分：`HtmlNodeHlper` / `Utils` 的 HTML 定位原语，以及规则文档
//! 的加载与归一化（`rK()`）。规则引擎复用本模块，保证「同一份 Java 语义、
//! 单一 Rust 实现」。
//!
//! 设计要点：
//! - Java 用 `JSONObject` + 大量 `optXxx/getXxx`，这里统一用 `serde_json::Value`
//!   并按 Java 的「缺键 = 默认值 / 异常」语义做返回；
//! - 所有下标函数都按 Java 的 `String.indexOf/charAt` 语义（字符下标按 UTF-16
//!   处理会与 Rust 的字节下标不同；XBiu 的 HTML 基本都是 ASCII 标签，故按字节
//!   下标实现，文档里记录这一偏差）。

use serde_json::{Map, Value};

/// `HtmlNodeHlper.TU`：视为「无闭合」的标签前缀。
pub const NOISE_TAGS: [&str; 4] = ["img", "br", "meta", "!--"];

/// `HtmlNodeHlper.trimHtmlString(s, "")`。
pub fn trim_html_string(html: &str) -> String {
    trim_html_string_with(html, "")
}

/// `HtmlNodeHlper.trimHtmlString(s, repl)`。
pub fn trim_html_string_with(html: &str, repl: &str) -> String {
    let flat = html.replace("\r\n", "").replace('\n', "");
    let mut out = String::with_capacity(flat.len());
    let mut in_tag = false;
    for ch in flat.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push_str(repl);
            }
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.replace("&nbsp;", "").replace("&emsp;", "").trim().to_string()
}

/// `HtmlNodeHlper.isPairedHtmlTag(html, pos)`：是否为需要闭合的普通标签。
pub fn is_paired_html_tag(html: &str, pos: usize) -> bool {
    let start = pos.min(html.len());
    let mut end = (start + 10).min(html.len());
    while end > start && !html.is_char_boundary(end) {
        end -= 1;
    }
    let head = &html[start..end];
    if !NOISE_TAGS.iter().any(|tag| head.contains(tag)) {
        return true;
    }
    let bytes = html.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            return i > 0 && bytes[i - 1] == b'/';
        }
        i += 1;
    }
    false
}

/// `HtmlNodeHlper.findUpNodes(html, from, levels)`：从 `from` 向左找 `levels`
/// 层祖先标签的起始下标（返回顺序：由内向外）。
pub fn find_up_nodes(html: &str, from: isize, levels: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if levels == 0 {
        if from >= 0 {
            out.push(from as usize);
        }
        return out;
    }
    if from < 0 {
        return out;
    }
    let bytes = html.as_bytes();
    let mut depth: isize = 0;
    let mut pos: isize = from;
    while pos >= 0 {
        let idx = pos as usize;
        let mut next = pos;
        match bytes[idx] {
            b'/' => {
                if idx + 1 < bytes.len() && bytes[idx + 1] == b'>' {
                    depth += 1;
                } else if idx > 0 && bytes[idx - 1] == b'<' {
                    depth += 1;
                    next = pos - 1;
                }
            }
            b'<' => {
                if depth == 0 {
                    out.push(idx);
                } else {
                    depth -= 1;
                    if is_paired_html_tag(html, idx) && depth < 0 {
                        depth = 0;
                    }
                }
            }
            _ => {}
        }
        if out.len() >= levels {
            break;
        }
        pos = next - 1;
    }
    out
}

/// `HtmlNodeHlper.getChildNodes(html)`：按顶层节点拆分。
pub fn get_child_nodes(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    if html.is_empty() || !html.starts_with('<') {
        return out;
    }
    let mut i = 1isize;
    while i >= 0 && (i as usize) < html.len() {
        let lt = match html[i as usize..].find('<') {
            Some(off) => i as usize + off,
            None => break,
        };
        let node = node_string(html, lt);
        if node.is_empty() {
            break;
        }
        out.push(node.to_string());
        i = lt as isize + node.len() as isize;
    }
    out
}

/// `HtmlNodeHlper.nodeString(html, pos)`：从 `pos` 开始取一个完整标签节点。
/// 返回 `""` 表示不是标签起点。
pub fn node_string(html: &str, pos: usize) -> &str {
    if pos >= html.len() || !html[pos..].starts_with('<') {
        return "";
    }
    let bytes = html.as_bytes();
    let mut depth: isize = 0;
    let mut i = pos;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'<' => {
                if bytes[i + 1] != b'/' && is_paired_html_tag(html, i) {
                    depth += 1;
                }
            }
            b'/' => {
                if bytes[i + 1] == b'>' {
                    depth -= 1;
                } else if i > 0 && bytes[i - 1] == b'<' {
                    // Java：'</' 只减深度，不跳过 '<'（下标照常 +1）。
                    depth -= 1;
                }
            }
            b'>' => {
                if depth == 0 {
                    return &html[pos..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    &html[pos..]
}

/// `Utils.getRegion(html, rule)`：按规则里的 `region: [start, end?]` 截取。
pub fn get_region(html: &str, rule: &Value) -> String {
    let arr = match rule.get("region").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => return html.to_string(),
    };
    let first = arr[0].as_str().unwrap_or("");
    let start = match html.find(first) {
        Some(v) => v,
        None => return html.to_string(),
    };
    if arr.len() <= 1 {
        return html[start..].to_string();
    }
    let second = arr[1].as_str().unwrap_or("");
    match html[start + first.len()..].find(second) {
        Some(off) => {
            let end = start + first.len() + off;
            html[start..end].to_string()
        }
        None => html[start..].to_string(),
    }
}

/// `Utils.findSubString(html, from, [start, end, left, right], fallback)`。
pub fn find_sub_string(html: &str, from: usize, spec: Option<&Value>, fallback: &str) -> String {
    let arr = match spec.and_then(|v| v.as_array()) {
        Some(a) if a.len() >= 2 => a,
        _ => return fallback.to_string(),
    };
    let first = arr[0].as_str().unwrap_or("");
    let second = arr[1].as_str().unwrap_or("");
    let left = arr.get(2).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as usize;
    let right = arr.get(3).and_then(|v| v.as_i64()).unwrap_or(0);
    let rel = match html[from.min(html.len())..].find(first) {
        Some(v) => v,
        None => return fallback.to_string(),
    };
    let after = from.min(html.len()) + rel + first.len();
    let rel2 = match html[after..].find(second) {
        Some(v) => v,
        None => return fallback.to_string(),
    };
    let stop = after + rel2;
    let from2 = (after + left).min(stop);
    let stop2 = if right >= 0 { (stop + right as usize).min(html.len()) } else { stop };
    trim_html_string(&html[from2..stop2])
}

/// `Utils.findBlockPos(prev, cur)`：在两个「向上节点」列表里找共同块的起点。
/// 对应 Java：取较短长度；长度 1 直接返回 `cur[0]`；否则逐项比较，遇到不同返回
/// `cur[i-1]`，全部相同返回 `cur[last]`。
pub fn find_block_pos(prev: &[usize], cur: &[usize]) -> Option<usize> {
    // Exact port of Java Utils.findBlockPos: return cur[i-1] for the first
    // equal index i (skip i=0), else cur[last].
    let n = prev.len().min(cur.len());
    if n == 0 {
        return cur.first().copied();
    }
    if n == 1 {
        return cur.first().copied();
    }
    for i in 0..n {
        if prev[i] != cur[i] {
            continue;
        }
        if i == 0 {
            continue;
        }
        return cur.get(i - 1).copied();
    }
    cur.get(n - 1).copied()
}

/// `Utils.getSubStringCount(html, pat)`。
pub fn sub_string_count(html: &str, pat: &str) -> usize {
    if pat.is_empty() {
        return 0;
    }
    html.match_indices(pat).count()
}

/// `Utils.getLookbackArray(rule)`：返回规则里第一个「lookback 数组」。
/// 判定：值必须是数组、长度 > 4、且第 5 项为 > 0 的整数。
pub fn get_lookback_array(rule: &Value) -> Option<Value> {
    let map = rule.as_object()?;
    for (_, value) in map {
        if value.is_array() && get_lookback_count(value) > 0 {
            return Some(value.clone());
        }
    }
    None
}

/// `Utils.getLookbackCount(arr)`：`arr[4]`，长度不足 5 返回 0。
pub fn get_lookback_count(arr: &Value) -> i64 {
    match arr.get(4).and_then(|v| v.as_i64()) {
        Some(n) => n,
        None => 0,
    }
}

/// `rK()` 产出的规则文档与派生字段。
#[derive(Debug, Clone)]
pub struct RuleDoc {
    /// 规则 JSON（对应 Java 静态字段 `a`）。
    pub rule: Value,
    /// 从规则 URL 取到的原始文本（对应 `TU`）。
    pub rule_text: String,
    /// 站点基址（`homeUrl`，已按占位符规则裁剪）。
    pub home_url: String,
    /// 分隔符集合（`this.r`）：由 `list.url` 里出现的 `/ . - _ ~` 组成。
    pub separators: String,
    /// 播放关键词（`play.keywords`）。
    pub keywords: Vec<String>,
}

/// XBiu 的 ext 三种形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtKind {
    /// 纯 JSON 规则文档。
    JsonRule,
    /// 以 http 开头的规则文档地址（无 `{cateId}`/`{catePg}` 占位符）。
    RuleUrl,
    /// 以 http 开头的站点页面地址（含 `{cateId}`/`{catePg}` 占位符）。
    SiteUrl,
}

/// 判定 ext 形态。
pub fn ext_kind(ext: &str) -> ExtKind {
    let ext = ext.trim();
    if !ext.starts_with("http") {
        return ExtKind::JsonRule;
    }
    if ext.contains("{cateId}") || ext.contains("{catePg}") {
        ExtKind::SiteUrl
    } else {
        ExtKind::RuleUrl
    }
}

/// 取 URL 的 path 部分（`scheme://authority` 之后），用于 `homeUrl` 裁剪。
fn url_path(url: &str) -> String {
    let after_scheme = match url.find("://") {
        Some(v) => v + 3,
        None => return String::new(),
    };
    match url[after_scheme..].find('/') {
        Some(v) => url[after_scheme + v..].to_string(),
        None => String::new(),
    }
}

/// `rK()`：把 ext（+ 可选 fetch 到的规则文本）归一化成 [`RuleDoc`]。
///
/// 与 Java 的差异（记录在文档里）：
/// - `jsonmerge`/`readRuleFromByUrl` 基类实现恒为空，等价于不做合并；
/// - Java 的 `getString` 缺键抛异常会中断整个归一化，这里改为跳过该步。
pub fn load_rule(ext: &str, fetched_rule: Option<&str>) -> Result<RuleDoc, String> {
    let ext = ext.trim();
    if ext.is_empty() {
        return Err("ext 为空".to_string());
    }
    let mut rule = match ext_kind(ext) {
        ExtKind::JsonRule => {
            serde_json::from_str::<Value>(ext).map_err(|e| format!("ext 不是合法 JSON 规则: {e}"))?
        }
        ExtKind::RuleUrl => {
            let text = fetched_rule.ok_or_else(|| "ext 为规则地址，但未取到规则文档".to_string())?;
            serde_json::from_str::<Value>(text).map_err(|e| format!("规则文档不是合法 JSON: {e}"))?
        }
        ExtKind::SiteUrl => {
            // readRuleFromByUrl 基类实现返回 "{}"；homeUrl 来自 ext 本身。
            let mut obj = Map::new();
            obj.insert("homeUrl".to_string(), Value::String(ext.to_string()));
            Value::Object(obj)
        }
    };
    if !rule.is_object() {
        return Err("规则文档顶层不是对象".to_string());
    }
    let rule_text = match ext_kind(ext) {
        ExtKind::JsonRule => serde_json::to_string(&rule).unwrap_or_default(),
        ExtKind::RuleUrl => fetched_rule.unwrap_or("{}").to_string(),
        ExtKind::SiteUrl => "{}".to_string(),
    };

    // 1) 把 "a.b" 扁平键折叠成嵌套对象（Java 的 rK 同名循环）。
    let flat_keys: Vec<String> = rule.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    for key in flat_keys {
        let parts: Vec<&str> = key.split('.').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            continue;
        }
        let (head, tail) = (parts[0].to_string(), parts[1].to_string());
        let obj = rule.as_object_mut().unwrap();
        if !obj.get(&head).map(|v| v.is_object()).unwrap_or(false) {
            obj.insert(head.clone(), Value::Object(Map::new()));
        }
        let value = obj.get(&key).cloned();
        let child = obj.get_mut(&head).unwrap().as_object_mut().unwrap();
        if !child.contains_key(&tail) {
            if let Some(v) = value {
                child.insert(tail, v);
            }
        }
    }

    // 2) header / ua。
    if rule.get("header").and_then(|v| v.as_object()).is_none() {
        rule.as_object_mut().unwrap().insert("header".to_string(), Value::Object(Map::new()));
    }
    if rule.get("ua").and_then(|v| v.as_str()).map(|ua| ua.eq_ignore_ascii_case("android")).unwrap_or(false) {
        rule.as_object_mut()
            .unwrap()
            .get_mut("header")
            .and_then(|v| v.as_object_mut())
            .map(|h| {
                h.insert(
                    "User-Agent".to_string(),
                    Value::String(
                        "Mozilla/5.0 (Linux; Android 8.1.0; ALP-AL00 Build/HUAWEIALP-AL00; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/63.0.3239.83 Mobile Safari/537.36 T7/10.13 baiduboxapp/10.13.0.11 (Baidu; P1 8.1.0)".to_string(),
                    ),
                );
            });
    }

    // 3) list / detail / playlist 占位对象。
    for key in ["list", "detail", "playlist"] {
        if rule.get(key).and_then(|v| v.as_object()).is_none() {
            rule.as_object_mut().unwrap().insert(key.to_string(), Value::Object(Map::new()));
        }
    }

    // 4) homeUrl 裁剪 + list.url 默认值。
    let mut home_url = rule.get("homeUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if home_url.contains("{cateId}") {
        let path = url_path(&home_url);
        let cut = if path.is_empty() { home_url.len() } else { home_url.find(&path).unwrap_or(home_url.len()) };
        let base = home_url[..cut].to_string();
        rule.as_object_mut().unwrap().insert("homeUrl".to_string(), Value::String(base.clone()));
        let list = rule.as_object_mut().unwrap().get_mut("list").and_then(|v| v.as_object_mut()).unwrap();
        if !list.contains_key("url") {
            list.insert("url".to_string(), Value::String(home_url.clone()));
        }
        home_url = base;
    }

    // 5) 分隔符集合（list.url 里出现的 / . - _ ~）与播放关键词。
    let list_url = rule
        .get("list")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut separators = String::new();
    for ch in ['/', '.', '-', '_', '~'] {
        if list_url.contains(ch) {
            separators.push(ch);
        }
    }
    let keywords = rule
        .get("play")
        .and_then(|v| v.get("keywords"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    Ok(RuleDoc { rule, rule_text, home_url, separators, keywords })
}

/// `addHttpPrefix(url)`：相对地址拼 `homeUrl`，绝对地址原样返回。
pub fn add_http_prefix(rule: &Value, url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    if url.starts_with("http") {
        return url.to_string();
    }
    let home = rule.get("homeUrl").and_then(|v| v.as_str()).unwrap_or("");
    let base = home.strip_suffix('/').unwrap_or(home);
    if url.starts_with('/') {
        format!("{base}{url}")
    } else {
        format!("{base}/{url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn node_string_returns_complete_nodes() {
        let html = r#"<div class="a"><img src="x"/><a href="u">标题</a></div><p>尾部"#;
        let a = html.find("<a href").unwrap();
        assert_eq!(node_string(html, a), r#"<a href="u">标题</a>"#);
        let img = html.find("<img").unwrap();
        assert_eq!(node_string(html, img), r#"<img src="x"/>"#);
        let div = html.find("<div").unwrap();
        assert_eq!(node_string(html, div), r#"<div class="a"><img src="x"/><a href="u">标题</a></div>"#);
        assert_eq!(node_string(html, 1), "");
    }

    #[test]
    fn is_paired_html_tag_follows_noise_tags() {
        assert!(!is_paired_html_tag("<img src=\"x\">", 0));
        assert!(is_paired_html_tag("<img src=\"x\"/>", 0));
        assert!(!is_paired_html_tag("<br>", 0));
        assert!(!is_paired_html_tag("<!-- c -->", 0));
        assert!(is_paired_html_tag("<div class=\"a\">", 0));
    }

    #[test]
    fn trim_html_string_strips_tags_and_collapses_space() {
        assert_eq!(trim_html_string("  标题 \r\n <span>第1集</span>&nbsp;"), "标题 第1集");
        assert_eq!(trim_html_string_with("<b>甲</b><i>乙</i>", " "), "甲 乙");
    }

    #[test]
    fn find_up_nodes_records_ancestors_outer_first() {
        let html = r#"<ul><li><a href="2">B</a></li></ul>"#;
        let from = html.find(r#"<a href="2""#).unwrap() as isize - 1;
        assert_eq!(find_up_nodes(html, from, 3), vec![4, 0]);
        assert_eq!(find_up_nodes(html, -1, 3), Vec::<usize>::new());
        assert_eq!(find_up_nodes(html, from, 0), vec![from as usize]);
    }

    #[test]
    fn get_region_honours_start_and_end_markers() {
        let html = "<x>前置</x><list>a<b/>c</list><y/>";
        assert_eq!(get_region(html, &json!({"region": ["<list>"]})), "<list>a<b/>c</list><y/>");
        assert_eq!(get_region(html, &json!({"region": ["<list>", "</list>"]})), "<list>a<b/>c");
        assert_eq!(get_region(html, &json!({"region": ["不存在"]})), html);
        assert_eq!(get_region(html, &json!({})), html);
    }

    #[test]
    fn find_sub_string_matches_java_bounds() {
        let html = r#"<a href="http://s/1.html" title="第1集">"#;
        let spec = json!(["href=\"", "\""]);
        assert_eq!(find_sub_string(html, 0, Some(&spec), ""), "http://s/1.html");
        assert_eq!(find_sub_string(html, 0, Some(&json!(["title=\"", "\""])), ""), "第1集");
        assert_eq!(find_sub_string(html, 0, Some(&json!(["缺", "\""])), "回退"), "回退");
        assert_eq!(find_sub_string(html, 0, None, "回退"), "回退");
    }

    #[test]
    fn find_block_pos_follows_java_semantics() {
        // Java: skip i=0, return cur[i-1] at the first equal ancestor,
        // otherwise cur[last].
        assert_eq!(find_block_pos(&[10, 5], &[10, 5]), Some(10));
        assert_eq!(find_block_pos(&[10, 5], &[9, 5]), Some(9));
        assert_eq!(find_block_pos(&[1, 2, 3], &[1, 2, 8]), Some(1));
        assert_eq!(find_block_pos(&[1, 9, 3], &[2, 8, 8]), Some(8));
        assert_eq!(find_block_pos(&[], &[7]), Some(7));
    }

    #[test]
    fn lookback_array_and_count() {
        let rule = json!({"cate": ["a", "b", "c", "d", 3], "vod_id": ["x", "y"]});
        let arr = get_lookback_array(&rule).unwrap();
        assert_eq!(get_lookback_count(&arr), 3);
        assert_eq!(get_lookback_count(&json!(["a"])), 0);
        assert!(get_lookback_array(&json!({"vod_id": ["x"]})).is_none());
    }

    #[test]
    fn sub_string_count_counts_non_overlapping() {
        assert_eq!(sub_string_count("abababa", "ab"), 3);
        assert_eq!(sub_string_count("abc", ""), 0);
    }

    #[test]
    fn ext_kind_is_tri_state() {
        assert_eq!(ext_kind("{\"a\":1}"), ExtKind::JsonRule);
        assert_eq!(ext_kind("http://x/rule.json"), ExtKind::RuleUrl);
        assert_eq!(ext_kind("http://x/type/{cateId}.html"), ExtKind::SiteUrl);
    }

    #[test]
    fn load_rule_flattens_and_derives_fields() {
        let ext = r#"{
          "homeUrl": "http://example.com/vodshow/{cateId}/page/{catePg}.html",
          "list.vod_id": ["href=\"(.*?)\""],
          "ua": "android",
          "play.keywords": ["m3u8", "mp4"]
        }"#;
        let doc = load_rule(ext, None).unwrap();
        assert_eq!(doc.rule["list"]["vod_id"][0], "href=\"(.*?)\"");
        assert_eq!(doc.home_url, "http://example.com");
        assert_eq!(doc.separators, "/.");
        assert_eq!(doc.keywords, vec!["m3u8", "mp4"]);
        assert!(doc.rule["header"]["User-Agent"].as_str().unwrap().contains("baiduboxapp"));
    }

    #[test]
    fn load_rule_fills_list_url_from_home_url() {
        let ext = r#"{"homeUrl": "http://site.com/type/{cateId}-{catePg}.html"}"#;
        let doc = load_rule(ext, None).unwrap();
        assert_eq!(doc.home_url, "http://site.com");
        assert_eq!(doc.rule["list"]["url"], "http://site.com/type/{cateId}-{catePg}.html");
    }

    #[test]
    fn load_rule_rule_url_needs_fetched_text() {
        assert!(load_rule("http://x/rule.json", None).is_err());
        let doc = load_rule("http://x/rule.json", Some("{\"homeUrl\":\"http://a.com\"}")).unwrap();
        assert_eq!(doc.home_url, "http://a.com");
        assert_eq!(doc.rule_text, "{\"homeUrl\":\"http://a.com\"}");
    }

    #[test]
    fn load_rule_site_url_is_empty_rule_with_home_url() {
        let doc = load_rule("http://site.com/list/{cateId}", None).unwrap();
        assert_eq!(doc.home_url, "http://site.com");
        assert!(doc.rule.as_object().unwrap().contains_key("list"));
    }

    #[test]
    fn add_http_prefix_joins_relative_urls() {
        let rule = json!({"homeUrl": "http://site.com"});
        assert_eq!(add_http_prefix(&rule, "/a.jpg"), "http://site.com/a.jpg");
        assert_eq!(add_http_prefix(&rule, "b.jpg"), "http://site.com/b.jpg");
        assert_eq!(add_http_prefix(&rule, "https://cdn/x.jpg"), "https://cdn/x.jpg");
        assert_eq!(add_http_prefix(&rule, ""), "");
    }

    #[test]
    fn get_child_nodes_splits_top_level() {
        let html = r#"<div><li>a</li><li>b</li></div>"#;
        assert_eq!(get_child_nodes(html), vec!["<li>a</li>", "<li>b</li>", "</div>"]);
    }
}
