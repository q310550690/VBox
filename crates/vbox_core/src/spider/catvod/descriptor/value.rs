//! 取值后处理与模板渲染（白名单指令集，见计划 §5.3）。
//!
//! 这里只实现**声明式**变换：没有条件表达式、没有循环、没有脚本。每个函数都能
//! 单独单测，描述里出现的每个指令都能指回一段手写实现做等价性对照。

use std::collections::{BTreeMap, BTreeSet};

use percent_encoding::{percent_decode_str, percent_encode, AsciiSet, NON_ALPHANUMERIC};

use super::schema::FieldSpec;
use super::schema::Encode;

/// 变量表（请求级 + 描述级）。
#[derive(Debug, Clone, Default)]
pub struct Vars {
    map: BTreeMap<String, String>,
}

impl Vars {
    /// 新建。
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入。
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.map.insert(key.to_string(), value.into());
    }

    /// 读取。
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    /// 以字符串取（缺省空）。
    pub fn text(&self, key: &str) -> String {
        self.map.get(key).cloned().unwrap_or_default()
    }

    /// 数值取（缺省 0）。
    pub fn number(&self, key: &str) -> u64 {
        self.map
            .get(key)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    }

    /// 全部键。
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }
}

/// `-_.~` 之外全部编码（LiteApple 口径的 URL 编码）。
const URL_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// 按口径编码。
pub fn encode(value: &str, mode: Encode) -> String {
    match mode {
        Encode::None => value.to_string(),
        Encode::Alnum => percent_encode(value.as_bytes(), NON_ALPHANUMERIC).to_string(),
        Encode::Url => percent_encode(value.as_bytes(), URL_SET).to_string(),
    }
}

/// 百分号解码（非法序列原样保留）。
pub fn percent_decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// 折叠空白为单个空格并去首尾。
pub fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 过滤图标字体私用区（U+E000–U+F8FF）与 NBSP，再折叠空白（Ysj `clean_text`）。
pub fn clean_private_use(value: &str) -> String {
    let filtered: String = value
        .chars()
        .filter(|c| !('\u{e000}'..='\u{f8ff}').contains(c) && *c != '\u{a0}')
        .collect();
    collapse(&filtered)
}

/// 渲染 `{name}` / `{name:mod}`；`mod` ∈ `raw` / `alnum` / `url`。
///
/// 未知占位符渲染为空串：描述加载期由 [`unknown_placeholders`] 拦下拼写错误，
/// 运行期不再 panic。
pub fn render(template: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push('{');
            rest = after;
            continue;
        };
        let inner = &after[..end];
        let (name, mode) = match inner.split_once(':') {
            Some((n, m)) => (n, m),
            None => (inner, "raw"),
        };
        let value = vars.text(name.trim());
        match mode {
            "raw" => out.push_str(&value),
            "alnum" => out.push_str(&encode(&value, Encode::Alnum)),
            "url" => out.push_str(&encode(&value, Encode::Url)),
            _ => out.push_str(&value),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// 提取模板里的占位符名（去重、保序），用于加载期校验。
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let inner = &after[..end];
        let name = inner.split_once(':').map(|(n, _)| n).unwrap_or(inner).trim();
        if !name.is_empty() && seen.insert(name.to_string()) {
            out.push(name.to_string());
        }
        rest = &after[end + 1..];
    }
    out
}

/// 模板里不在 `known` 中的占位符。
pub fn unknown_placeholders(template: &str, known: &BTreeSet<String>) -> Vec<String> {
    placeholders(template)
        .into_iter()
        .filter(|p| !known.contains(p))
        .collect()
}

/// 按 `cut` 中的任一字符截断（`None` 不截断）。
pub fn cut_at(value: &str, cut: Option<&str>) -> String {
    match cut {
        Some(chars) => value
            .find(|c: char| chars.contains(c))
            .map(|idx| value[..idx].to_string())
            .unwrap_or_else(|| value.to_string()),
        None => value.to_string(),
    }
}

/// 在扁平文本里找 `{label}：` 并取到截断字符前的值（Ysj 主演/导演口径）。
pub fn labeled(flat: &str, label: &str, cut: Option<&str>) -> Option<String> {
    let marker = format!("{label}：");
    let pos = flat.find(&marker)?;
    let rest = &flat[pos + marker.len()..];
    let value = cut_at(rest.trim_start(), cut).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 对字段原始值执行后处理链；空结果返回 `None`。
pub fn apply(spec: &FieldSpec, raw: String, base: &str, vars: &Vars) -> Option<String> {
    let mut s = raw;
    if spec.trim {
        s = s.trim().to_string();
    }
    for (from, to) in &spec.replace {
        if !from.is_empty() {
            s = s.replace(from, to);
        }
    }
    if let Some(prefix) = &spec.strip_prefix {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    if let Some(suffix) = &spec.strip_suffix {
        if let Some(rest) = s.strip_suffix(suffix) {
            s = rest.to_string();
        }
    }
    if let Some(index) = spec.segment {
        let sep = spec.split_on.as_deref().unwrap_or("/");
        let parts: Vec<&str> = s.split(sep).collect();
        let picked = parts
            .get(index)
            .map(|v| v.to_string())
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                if spec.tail_non_empty {
                    parts
                        .iter()
                        .rev()
                        .find(|v| !v.trim().is_empty())
                        .map(|v| v.to_string())
                } else {
                    None
                }
            });
        s = picked.unwrap_or_default();
    }
    if spec.percent_decode {
        s = percent_decode(&s);
    }
    if spec.clean_private_use {
        s = clean_private_use(&s);
    } else if spec.collapse {
        s = collapse(&s);
    }
    if spec.trim {
        s = s.trim().to_string();
    }
    if spec.join_base && !s.is_empty() {
        s = crate::html_rule::join_url(base, &s);
    }
    if s.is_empty() {
        if let Some(default) = &spec.default {
            s = render(default, vars);
        }
    }
    if !s.is_empty() {
        if let Some(prefix) = &spec.prefix {
            s = format!("{}{s}", render(prefix, vars));
        }
        if let Some(suffix) = &spec.suffix {
            s = format!("{s}{}", render(suffix, vars));
        }
    }
    if s.is_empty() && spec.non_empty {
        return None;
    }
    if s.is_empty() && !spec.keep_empty {
        return None;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::new();
        v.set("id", "a b/c");
        v.set("page", "2");
        v
    }

    #[test]
    fn render_modes() {
        let v = vars();
        assert_eq!(render("x={id}", &v), "x=a b/c");
        assert_eq!(render("x={id:url}", &v), "x=a%20b%2Fc");
        assert_eq!(render("x={id:alnum}", &v), "x=a%20b%2Fc");
        assert_eq!(render("{page}", &v), "2");
        assert_eq!(render("{missing}", &v), "");
    }

    #[test]
    fn url_encoding_keeps_unreserved() {
        assert_eq!(encode("a-b_c.d~e f", Encode::Url), "a-b_c.d~e%20f");
    }

    #[test]
    fn private_use_filtered() {
        assert_eq!(clean_private_use("a\u{e000}b\u{a0} c"), "ab c");
    }

    #[test]
    fn labeled_cuts() {
        assert_eq!(
            labeled("主演：张三 / 李四", "主演", Some("/\n")),
            Some("张三".to_string())
        );
        assert_eq!(labeled("导演：王五", "导演", Some("/")), Some("王五".into()));
        assert_eq!(labeled("没有", "导演", None), None);
    }

    #[test]
    fn placeholders_found() {
        assert_eq!(
            placeholders("{a}/{b:url}/{a}"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn apply_chain() {
        let mut spec = FieldSpec::default();
        spec.trim = true;
        spec.strip_prefix = Some("Pref:".into());
        spec.segment = Some(1);
        let out = apply(&spec, " Pref:x/y/z ".to_string(), "https://b", &Vars::new());
        assert_eq!(out, Some("y".to_string()));
    }

    #[test]
    fn apply_default_when_empty() {
        let mut spec = FieldSpec::default();
        spec.default = Some("{page}".into());
        let mut v = Vars::new();
        v.set("page", "9");
        assert_eq!(apply(&spec, String::new(), "", &v), Some("9".to_string()));
    }
}
