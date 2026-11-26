//! TVBox 标准 XPath 规则引擎（`csp_XPath` 家族）。
//!
//! 来源（jar 反编译证据，样本见 `jvm/samples/`，反编译产物归档于仓库外）：
//! * 类：`com.github.catvod.spider.XPath`（262 行）。
//! * `detailContent` 曾被反编译器跳过（922 指令）→ 用 `--show-bad-code` 恢复后逐字段还原。
//! * 规则模型：`com.github.catvod.parser.merge.O.a`，JSON 键名固定（`homeUrl` /
//!   `cateNode` / `cateUrl` / `dtUrl` / `playUrl` / `searchUrl` …），每个选择器
//!   键 `X` 可另带 `XR` 正则键做二次提取（`matcher.group(1).trim()`，失败回退原串）。
//! * 节点选取：`com.github.catvod.parser.merge.I0.a`（jar 内嵌 XPath 引擎），
//!   语义 = 以当前节点为上下文求值 XPath 1.0 表达式，取节点集。
//!
//! 桌面端实现：
//! * 用 `scraper` 解析 HTML → 序列化为良构 XML（自闭合 void 元素、去 script/style）
//!   → `sxd-document` + `sxd-xpath` 求值；
//! * 规则片段以 `/` 开头时按相对路径处理（`./text()`、`./@href`、`./parent::*…`），
//!   与 Java 端「以选中节点为上下文」的语义一致；
//! * 有意偏差：不做 `$$$` 第二段的 proxy 改写（桌面端无「本地代理前缀」概念，
//!   `getProxyDownloadUrl` 退化为恒等），在家族文档里记录。

use std::collections::HashMap;

use regex::Regex;
use scraper::Html;
use serde_json::{Map, Value};
use sxd_document::dom;
pub use sxd_xpath::nodeset::Node as NodeRef;
use sxd_xpath::{Context, Factory, Value as XPathValue};

/// 生成规则对象所需的 HTML 最大长度（防御异常大页面）。
const MAX_HTML: usize = 8 * 1024 * 1024;

/// 把 HTML 转成良构 XML 文本，供 `sxd-document` 解析。
pub fn html_to_xml(html: &str) -> String {
    let mut raw = html.to_string();
    if raw.len() > MAX_HTML {
        raw.truncate(MAX_HTML);
    }
    for re in [r"(?is)<script\b[^>]*>.*?</script>", r"(?is)<style\b[^>]*>.*?</style>"] {
        if let Ok(re) = Regex::new(re) {
            raw = re.replace_all(&raw, "").into_owned();
        }
    }
    let parsed = Html::parse_document(&raw);
    let serialized = parsed.html();
    sanitize_xml(&serialized)
}

/// 把 html5ever 序列化结果修补成 XML：去注释/doctype、自闭合 void 元素、转义裸 `&`。
fn sanitize_xml(html: &str) -> String {
    let mut out = html.to_string();
    for (pat, rep) in [
        (r"(?is)<!--.*?-->", ""),
        (r"(?is)<!DOCTYPE[^>]*>", ""),
        (r"(?is)<!\[CDATA\[", ""),
    ] {
        if let Ok(re) = Regex::new(pat) {
            out = re.replace_all(&out, rep).into_owned();
        }
    }
    out = rewrite_tags(&out);
    // 兜底：把不构成实体的裸 `&` 转义（html5ever 已处理，这里是防御性修补）。
    let amp = Regex::new(r"&(?!(?:#[0-9]+|#x[0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]*);)");
    if let Ok(re) = amp {
        out = re.replace_all(&out, "&amp;").into_owned();
    }
    out
}

/// 逐标签重写：保留合法标签、丢弃畸形标签、把 void 元素统一成 `<br/>`。
/// 扫描 `<...>` 时跟踪引号，属性值里的 `>` 不会截断标签。
fn rewrite_tags(html: &str) -> String {
    const VOID: &[&str] = &[
        "br", "hr", "img", "input", "meta", "link", "area", "base", "col", "embed", "param",
        "source", "track", "wbr",
    ];
    let Ok(strict) = Regex::new(
        r#"^</?[A-Za-z][A-Za-z0-9_.:-]*(\s+[A-Za-z_:][A-Za-z0-9_.:-]*\s*=\s*"[^"]*")*\s*/?>$"#,
    ) else {
        return html.to_string();
    };
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    let mut text_start = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let next = bytes.get(i + 1).copied().unwrap_or(0);
            if next.is_ascii_alphabetic() || next == b'/' {
                if let Some(end) = scan_tag_end(bytes, i) {
                    out.push_str(&html[text_start..i]);
                    let tag = &html[i..=end];
                    if strict.is_match(tag) {
                        if next.is_ascii_alphabetic() {
                            let name: String = tag[1..]
                                .chars()
                                .take_while(|c| c.is_ascii_alphanumeric())
                                .collect::<String>()
                                .to_ascii_lowercase();
                            if VOID.contains(&name.as_str()) {
                                let inner = tag[1..tag.len() - 1].trim().trim_end_matches('/').trim_end();
                                out.push('<');
                                out.push_str(inner);
                                out.push_str("/>");
                                i = end + 1;
                                text_start = i;
                                continue;
                            }
                        }
                        out.push_str(tag);
                    }
                    i = end + 1;
                    text_start = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&html[text_start..]);
    out
}

/// 找标签结束的 `>`；引号内的 `>` 不算结束。若引号一直未闭合，
/// 遇到「下一个非空白字符是 `<` 或已到结尾」的 `>` 也收束，避免吞掉后续标签。
fn scan_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut j = start + 1;
    while j < bytes.len() {
        let c = bytes[j];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == b'"' || c == b'\'' {
            quote = Some(c);
        } else if c == b'>' {
            if quote.is_none() {
                return Some(j);
            }
            let next = (j + 1..bytes.len()).find(|&k| !bytes[k].is_ascii_whitespace());
            if next.map_or(true, |k| bytes[k] == b'<') {
                return Some(j);
            }
        }
        j += 1;
    }
    None
}

/// 已解析的 HTML 页面（持有 sxd 文档 arena）。
pub struct Page {
    package: sxd_document::Package,
}

impl Page {
    /// 解析 HTML；失败返回 `None`（调用方转 `catvod.result.parse`）。
    pub fn new(html: &str) -> Option<Page> {
        let xml = html_to_xml(html);
        sxd_document::parser::parse(&xml)
            .ok()
            .map(|package| Page { package })
    }

    fn document(&self) -> dom::Document<'_> {
        self.package.as_document()
    }

    /// 从文档根节点按表达式取节点集（绝对路径语义）。
    pub fn select_root(&self, expr: &str) -> Vec<NodeRef<'_>> {
        self.select(self.document().root().into(), expr)
    }

    /// 以 `ctx` 为上下文求值，返回节点集（按文档顺序）。
    pub fn select<'d>(&'d self, ctx: NodeRef<'d>, expr: &str) -> Vec<NodeRef<'d>> {
        let expr = normalize_expr(expr);
        if expr.is_empty() {
            return Vec::new();
        }
        let Some(xpath) = Factory::new().build(&expr).ok().flatten() else {
            return Vec::new();
        };
        let context = Context::new();
        match xpath.evaluate(&context, ctx) {
            Ok(XPathValue::Nodeset(nodes)) => nodes.document_order(),
            _ => Vec::new(),
        }
    }

    /// 取第一个命中节点。
    pub fn first<'d>(&'d self, ctx: NodeRef<'d>, expr: &str) -> Option<NodeRef<'d>> {
        self.select(ctx, expr).into_iter().next()
    }
}

/// 规则片段（`/text()`、`/@href`）按相对路径求值；`//x` 表示后代任意层级。
fn normalize_expr(expr: &str) -> String {
    let e = expr.trim();
    if e.is_empty() {
        String::new()
    } else if e.starts_with('/') {
        format!(".{e}")
    } else {
        e.to_string()
    }
}

/// 节点字符串值：属性取值、文本节点取文本、元素取全部后代文本。
pub fn node_text(node: NodeRef<'_>) -> String {
    node.string_value()
}

/// 规则 JSON（`O.a.b(json)` 的桌面替身）。
#[derive(Debug, Clone, Default)]
pub struct Rule {
    raw: Map<String, Value>,
}

impl Rule {
    /// 解析规则 JSON 文本。
    pub fn from_str(text: &str) -> Option<Rule> {
        let value: Value = serde_json::from_str(text.trim()).ok()?;
        let raw = value.as_object()?.clone();
        Some(Rule { raw })
    }

    /// 取字符串键（沿用 Java 端 `.trim()` 语义）。
    pub fn s(&self, key: &str) -> String {
        match self.raw.get(key) {
            Some(Value::String(text)) => text.trim().to_string(),
            Some(Value::Number(n)) => n.to_string().trim().to_string(),
            _ => String::new(),
        }
    }

    /// 非空字符串键。
    pub fn opt(&self, key: &str) -> Option<String> {
        let text = self.s(key);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// 对象型键（如 `filter` / `cateManual`）。
    pub fn obj(&self, key: &str) -> Option<&Map<String, Value>> {
        self.raw.get(key).and_then(|v| v.as_object())
    }

    /// 键值均为字符串的映射（`cateManual`：分类名 → 分类 id）。
    pub fn str_map(&self, key: &str) -> Vec<(String, String)> {
        let Some(obj) = self.obj(key) else { return Vec::new() };
        obj.iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.trim().to_string(), v.trim().to_string())))
            .collect()
    }

    /// 应用 `<key>R` 正则（`matcher.group(1).trim()`；未配/未命中回退原串）。
    pub fn apply(&self, key: &str, input: &str) -> String {
        let pattern = self.s(&format!("{key}R"));
        if pattern.is_empty() {
            return input.to_string();
        }
        match Regex::new(&pattern) {
            Ok(re) => match re.captures(input) {
                Some(caps) => caps
                    .get(1)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_else(|| input.to_string()),
                None => input.to_string(),
            },
            Err(_) => input.to_string(),
        }
    }

    /// 是否存在该键（用于「可选字段是否配置」判断）。
    pub fn has(&self, key: &str) -> bool {
        !self.s(key).is_empty()
    }
}

/// 把规则的完整 HTML 结果解析成页面；解析失败返回家族统一错误。
pub fn parse_page(html: &str) -> Option<Page> {
    Page::new(html)
}

/// 便捷：HashMap<String,String> 与规则 `filter` 的桥接（家族层用）。
pub type Filters = HashMap<String, String>;