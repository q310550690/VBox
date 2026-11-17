//! Jsoup 风格 HTML 规则引擎（对齐 TVBox `crawler/js/HtmlParser.java`）
//!
//! 提供 JS spider 的 `pdfh/pdfa/pd/pdfla` 以及原生规则引擎（XYQHiker/XBPQ）共用的
//! 选择器解释器。规则语法：
//! - `A&&B&&Text` / `A&&B&&Html` / `A&&B&&href`：链式选择，最后一段是取值方式
//! - `A B`：空格表示逐级 select
//! - `:eq(n)` / `:lt(n)` / `:gt(n)` / `:first` / `:last`：对当前结果集取下标
//! - `--排除选择器`：在结果中排除（当前实现做元素级过滤，嵌套排除能力有限）
//! - `Text` / `Html` / 属性名：取值方式
//!
//! 注意：Jsoup 的部分伪类（`:contains`、复杂 `:has`）不能 1:1 映射到 CSS；
//! 这里对 `:has()` 做降级处理，并在不支持时给出空结果而不是整段脚本崩溃。

use scraper::{ElementRef, Html, Selector};
use url::Url;

/// `pdfh`：从 HTML 中按规则取单值（等价 `Global.pdfh`）
pub fn pdfh(html: &str, rule: &str) -> String {
    pd(html, rule, "")
}

/// `pdfa`：从 HTML 中按规则取元素数组（返回 outerHTML，等价 `Global.pdfa`）
pub fn pdfa(html: &str, rule: &str) -> Vec<String> {
    let Some(doc) = parse_doc(html) else {
        return Vec::new();
    };
    let chain = parse_hiker_to_jq(rule, false);
    let elements = select_chain(&doc, &chain);
    elements.iter().map(outer_html).collect()
}

/// `pd`：带 base URL 的 `pdfh`（等价 `Global.pd`）
pub fn pd(html: &str, rule: &str, add_url: &str) -> String {
    let Some(doc) = parse_doc(html) else {
        return String::new();
    };
    let rule = rule.trim();
    if rule.is_empty() {
        return String::new();
    }
    if rule == "body&&Text" || rule == "Text" {
        return doc.root_element().text().collect::<String>();
    }
    if rule == "body&&Html" || rule == "Html" {
        return doc.root_element().html();
    }

    let (chain_rule, option) = match rule.rfind("&&") {
        Some(pos) => (&rule[..pos], rule[pos + 2..].trim()),
        None => (rule, ""),
    };
    let chain = parse_hiker_to_jq(chain_rule, true);
    let elements = select_chain(&doc, &chain);
    if elements.is_empty() {
        return String::new();
    }

    match option {
        "Text" | "text" => elements
            .iter()
            .map(|e| e.text().collect::<String>())
            .collect::<Vec<_>>()
            .join(""),
        "Html" | "html" => elements
            .iter()
            .map(|e| e.html())
            .collect::<Vec<_>>()
            .join(""),
        "" => elements.first().map(|e| outer_html(e)).unwrap_or_default(),
        attr => {
            let mut result = elements
                .first()
                .and_then(|e| e.value().attr(attr))
                .unwrap_or("")
                .to_string();
            if attr.eq_ignore_ascii_case("style") && result.contains("url(") {
                result = extract_css_url(&result).unwrap_or_default();
            }
            if !add_url.is_empty() && is_url_attr(attr) && !is_special_url(&result) {
                if result.contains("http") {
                    result = result[result.find("http").unwrap_or(0)..].to_string();
                } else {
                    result = join_url(add_url, &result);
                }
            }
            result
        }
    }
}

/// `pdfla`：列表规则 + 文本规则 + 链接规则（等价 `Global.pdfla`）
pub fn pdfla(html: &str, p1: &str, list_text: &str, list_url: &str, add_url: &str) -> Vec<String> {
    let Some(doc) = parse_doc(html) else {
        return Vec::new();
    };
    let chain = parse_hiker_to_jq(p1, false);
    let elements = select_chain(&doc, &chain);
    let mut out = Vec::with_capacity(elements.len());
    for element in &elements {
        let item_html = outer_html(element);
        let text = pd(&item_html, list_text, "").trim().to_string();
        let url = pd(&item_html, list_url, add_url).trim().to_string();
        out.push(format!("{text}${url}"));
    }
    out
}

/* ============================================================
 * 规则解析 / 选择
 * ============================================================ */

fn parse_doc(html: &str) -> Option<Html> {
    if html.trim().is_empty() {
        return None;
    }
    Some(Html::parse_document(html))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexOp {
    None,
    Eq(i64),
    Lt(i64),
    Gt(i64),
    First,
    Last,
}

/// 对齐 `HtmlParser.parseHikerToJq`：把 `A&&B` 变成 `A:eq(0) B[:eq(0)]`
fn parse_hiker_to_jq(rule: &str, first: bool) -> String {
    let parts: Vec<&str> = rule.split("&&").collect();
    if parts.len() > 1 {
        let mut new_parts = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            let safe_last = part.split_whitespace().last().unwrap_or("");
            if no_add_index(safe_last) || (!first && i + 1 == parts.len()) {
                new_parts.push(part.to_string());
            } else {
                new_parts.push(format!("{part}:eq(0)"));
            }
        }
        new_parts.join(" ")
    } else if !no_add_index(rule.split_whitespace().last().unwrap_or("")) && first {
        format!("{rule}:eq(0)")
    } else {
        rule.to_string()
    }
}

fn no_add_index(token: &str) -> bool {
    token.contains(":eq")
        || token.contains(":lt")
        || token.contains(":gt")
        || token.contains(":first")
        || token.contains(":last")
        || token == "body"
        || token.starts_with('#')
}

fn select_chain<'a>(doc: &'a Html, chain: &str) -> Vec<ElementRef<'a>> {
    let mut current: Vec<ElementRef> = Vec::new();
    for token in chain.split_whitespace() {
        if token.is_empty() {
            continue;
        }
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (base, op, excludes) = parse_token(token);
        let Some(selector) = compile_selector(&base) else {
            return Vec::new();
        };
        let mut selected: Vec<ElementRef> = if current.is_empty() {
            doc.select(&selector).collect()
        } else {
            current
                .iter()
                .flat_map(|root| root.select(&selector))
                .collect()
        };
        selected = apply_index(selected, &op);
        if !excludes.is_empty() {
            selected.retain(|element| {
                !excludes.iter().any(|ex| {
                    compile_selector(ex)
                        .map(|sel| element.select(&sel).next().is_some())
                        .unwrap_or(false)
                })
            });
        }
        if selected.is_empty() {
            return Vec::new();
        }
        current = selected;
    }
    current
}

fn parse_token(token: &str) -> (String, IndexOp, Vec<String>) {
    let mut parts = token.split("--");
    let head = parts.next().unwrap_or("").to_string();
    let excludes: Vec<String> = parts
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut base = head;
    let mut op = IndexOp::None;
    for marker in [":eq", ":lt", ":gt"] {
        if let Some(idx) = base.find(marker) {
            let rest = &base[idx + marker.len()..];
            if let Some(end) = rest.find(')') {
                let number = rest[..end].trim_start_matches('(').trim();
                let n = number.parse::<i64>().unwrap_or(0);
                base.truncate(idx);
                op = match marker {
                    ":eq" => IndexOp::Eq(n),
                    ":lt" => IndexOp::Lt(n),
                    _ => IndexOp::Gt(n),
                };
            }
            break;
        }
    }
    if op == IndexOp::None {
        if let Some(idx) = base.find(":first") {
            base.truncate(idx);
            op = IndexOp::First;
        } else if let Some(idx) = base.find(":last") {
            base.truncate(idx);
            op = IndexOp::Last;
        }
    }
    (base, op, excludes)
}

fn compile_selector(base: &str) -> Option<Selector> {
    let cleaned = strip_has(base);
    Selector::parse(&cleaned).ok()
}

/// scraper 不支持 `:has()`；降级为去掉该伪类，尽量保留主选择器。
fn strip_has(selector: &str) -> String {
    if !selector.contains(":has(") {
        return selector.to_string();
    }
    let mut out = String::with_capacity(selector.len());
    let chars: Vec<char> = selector.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&[':', 'h', 'a', 's', '(']) {
            let mut depth = 0i32;
            i += 5;
            while i < chars.len() {
                if chars[i] == '(' {
                    depth += 1;
                } else if chars[i] == ')' {
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                    depth -= 1;
                }
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out.trim().to_string()
}

fn apply_index<'a>(elements: Vec<ElementRef<'a>>, op: &IndexOp) -> Vec<ElementRef<'a>> {
    match op {
        IndexOp::None => elements,
        IndexOp::Eq(n) => {
            let len = elements.len() as i64;
            let idx = if *n < 0 { len + n } else { *n };
            if idx >= 0 && idx < len {
                elements.get(idx as usize).cloned().into_iter().collect()
            } else {
                Vec::new()
            }
        }
        IndexOp::Lt(n) => elements.into_iter().take((*n).max(0) as usize).collect(),
        IndexOp::Gt(n) => elements
            .into_iter()
            .skip((*n).max(0) as usize + 1)
            .collect(),
        IndexOp::First => elements.into_iter().take(1).collect(),
        IndexOp::Last => elements.into_iter().last().into_iter().collect(),
    }
}

fn outer_html(element: &ElementRef<'_>) -> String {
    let name = element.value().name();
    let mut out = String::new();
    out.push('<');
    out.push_str(name);
    for (key, value) in element.value().attrs() {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(&value.replace('"', "&quot;"));
        out.push('"');
    }
    out.push('>');
    out.push_str(&element.html());
    out.push_str("</");
    out.push_str(name);
    out.push('>');
    out
}

fn is_url_attr(attr: &str) -> bool {
    let lower = attr.to_ascii_lowercase();
    ["url", "src", "href", "-original", "-src", "-play"]
        .iter()
        .any(|key| lower.contains(key))
}

fn is_special_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["ftp:", "magnet:", "thunder:", "ws:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn extract_css_url(style: &str) -> Option<String> {
    let start = style.find("url(")? + 4;
    let end = style[start..].find(')')? + start;
    let value = style[start..end].trim().trim_matches(['"', '\'']);
    Some(value.to_string())
}

pub fn join_url(parent: &str, child: &str) -> String {
    if parent.trim().is_empty() {
        return child.to_string();
    }
    if child.starts_with("http://") || child.starts_with("https://") {
        return child.to_string();
    }
    Url::parse(parent)
        .ok()
        .and_then(|base| base.join(child).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| child.to_string())
}

/// 非 Jsoup 的 `A&&B` 截取规则：取 A 与 B 之间的文本并去标签。
pub fn extract_marker(html: &str, rule: &str) -> Option<String> {
    extract_between_raw(html, rule).map(|raw| strip_tags(&raw))
}

/// 与 `extract_marker` 相同，但保留原始文本（用于 JSON/脚本片段截取）。
pub fn extract_between_raw(html: &str, rule: &str) -> Option<String> {
    let (prefix, suffix) = rule.split_once("&&")?;
    if prefix.is_empty() || suffix.is_empty() {
        return None;
    }
    let start = html.find(prefix)? + prefix.len();
    let rest = &html[start..];
    let end = rest.find(suffix)?;
    Some(rest[..end].to_string())
}

/// 非 Jsoup 的块拆分规则：`A&&B` 表示每个块从 A 到 B。
pub fn split_blocks(html: &str, rule: &str) -> Vec<String> {
    let Some((start_marker, end_marker)) = rule.split_once("&&") else {
        return Vec::new();
    };
    if start_marker.is_empty() || end_marker.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < html.len() {
        let Some(start_rel) = html[cursor..].find(start_marker) else {
            break;
        };
        let start = cursor + start_rel;
        let after_start = start + start_marker.len();
        let Some(end_rel) = html[after_start..].find(end_marker) else {
            break;
        };
        let end = after_start + end_rel + end_marker.len();
        out.push(html[start..end].to_string());
        cursor = end;
    }
    out
}

pub fn strip_tags(raw: &str) -> String {
    let Ok(re) = regex::Regex::new(r"(?s)<[^>]*>") else {
        return raw.trim().to_string();
    };
    let text = re.replace_all(raw, " ").to_string();
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"
    <html><body>
      <ul class="stui-vodlist">
        <li><a class="pic" href="/detail/1.html" title="Movie A">
          <img class="lazyload" data-original="https://img.example.com/a.jpg"/>
          <span class="pic-text">HD</span></a></li>
        <li><a class="pic" href="/detail/2.html" title="Movie B">
          <img class="lazyload" data-original="https://img.example.com/b.jpg"/>
          <span class="pic-text">更新至10集</span></a></li>
      </ul>
      <div id="playlist">
        <h3>线路一</h3>
        <ul class="stui-content__playlist">
          <li><a href="/play/1-1-1.html">第1集</a></li>
          <li><a href="/play/1-1-2.html">第2集</a></li>
        </ul>
      </div>
    </body></html>"#;

    #[test]
    fn pdfh_text_and_attr() {
        assert_eq!(pdfh(HTML, "h3&&Text"), "线路一");
        assert_eq!(pdfh(HTML, "a.pic&&title"), "Movie A");
        assert_eq!(
            pdfh(HTML, "img.lazyload&&data-original"),
            "https://img.example.com/a.jpg"
        );
    }

    #[test]
    fn pdfh_url_join() {
        let out = pd(
            "<a href='/x/1.html'>A</a>",
            "a&&href",
            "https://www.example.com/list/",
        );
        assert_eq!(out, "https://www.example.com/x/1.html");
    }

    #[test]
    fn pdfa_chain() {
        let items = pdfa(HTML, "ul.stui-vodlist&&li");
        assert_eq!(items.len(), 2);
        let titles = pdfa(HTML, "ul.stui-vodlist&&li")
            .iter()
            .map(|h| pdfh(h, "a.pic&&title"))
            .collect::<Vec<_>>();
        assert_eq!(titles, vec!["Movie A", "Movie B"]);
    }

    #[test]
    fn pdfla_list() {
        let out = pdfla(
            HTML,
            "ul.stui-content__playlist&&li",
            "a&&Text",
            "a&&href",
            "https://www.example.com/",
        );
        assert_eq!(out[0], "第1集$https://www.example.com/play/1-1-1.html");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn eq_and_has_fallback() {
        assert_eq!(
            pdfh(HTML, "ul.stui-vodlist&&li:eq(1)&&a.pic&&title"),
            "Movie B"
        );
        // :has 降级不应崩溃
        let arr = pdfa(HTML, "body&&.stui-vodlist:has(a)");
        assert_eq!(arr.len(), 1);
    }
}
