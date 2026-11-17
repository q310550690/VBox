//! 描述解释器：把 [`Descriptor`] 解释成 5 个入口的取数行为。
//!
//! 约束（见 `docs/type3兼容/type3-翻译层与引擎工作包.md` §4）：
//!
//! - 引擎只做白名单内的动作：拼 URL、取 JSON 路径、取 HTML 选择器、做字段清洗；
//!   不做任意代码执行。
//! - 断言语义必须与对应手写家族逐条对齐（同一份 mock 下输出一致），见 `tests/catvod_descriptor_*`。
//! - 错误文案由描述给出，缺失时使用通用文案；错误分类统一为 `Parse`。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Map, Value};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

use super::path;
use super::schema::{
    CatEp, Descriptor, DetailEp, DetailFields, Filter, GroupsHtml, GroupsJson, HeaderMode,
    ItemFields, Kind, ListEp, PlaySpec, ProxySpec, ReqRule, ReqTarget, Requirement, SearchEp,
    Segment,
};
use super::value::{self, Vars};

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// 一个入口的请求描述视图（让 `url_for` 同时服务 4 种端点结构）。
struct ReqView<'a> {
    path: &'a str,
    url: Option<&'a str>,
    rules: &'a [ReqRule],
    segments: &'a [Segment],
    params: &'a [super::schema::Param],
}

macro_rules! req_view {
    ($ep:expr) => {
        ReqView {
            path: &$ep.path,
            url: $ep.url.as_deref(),
            rules: &$ep.rules,
            segments: &$ep.segments,
            params: &$ep.params,
        }
    };
}

/// 一次入口调用的运行期状态。
struct Runtime<'a> {
    desc: &'a Descriptor,
    cfg: &'a TvConfig,
    site: &'a Site,
    base: String,
    vars: Vars,
    retries: u32,
}

impl<'a> Runtime<'a> {
    fn new(desc: &'a Descriptor, cfg: &'a TvConfig, site: &'a Site) -> Result<Self> {
        let base = base_of(desc, site);
        let mut vars = Vars::new();
        vars.set("base", &base);
        for decl in &desc.vars {
            eval_var(decl, site, &mut vars);
        }
        let retries = desc.retries.unwrap_or(cfg.http.retry);
        Ok(Self { desc, cfg, site, base, vars, retries })
    }

    fn headers(&self) -> BTreeMap<String, String> {
        render_map(&self.desc.headers, &self.vars)
    }

    fn fetch(&self, url: &str) -> Result<String> {
        let client = http::client_for(&self.cfg.http)?;
        let headers: HashMap<String, String> = self.headers().into_iter().collect();
        http::get_text(&client, url, &headers, self.retries)
    }

    /// 按端点声明的 HTTP 方法取文本。
    fn fetch_req(
        &self,
        url: &str,
        method: super::schema::HttpMethod,
        body: &[super::schema::BodyParam],
        vars: &Vars,
    ) -> Result<String> {
        use super::schema::{BodyKind, HttpMethod};
        if method == HttpMethod::Get {
            return self.fetch(url);
        }
        let client = http::client_for(&self.cfg.http)?;
        let headers: HashMap<String, String> = self.headers().into_iter().collect();
        match method {
            HttpMethod::PostJson => {
                let mut map = serde_json::Map::new();
                for field in body {
                    let raw = value::render(&field.value, vars);
                    let json = match field.kind {
                        BodyKind::Str => Value::String(raw),
                        BodyKind::Number => serde_json::from_str(&raw)
                            .ok()
                            .filter(Value::is_number)
                            .unwrap_or(Value::String(raw)),
                        BodyKind::Bool => Value::Bool(matches!(raw.trim(), "true" | "1")),
                    };
                    map.insert(field.key.clone(), json);
                }
                let body = Value::Object(map).to_string();
                http::post_json_text(&client, url, &headers, &body)
            }
            HttpMethod::PostForm => {
                // 表单值由 HTTP 库统一百分号编码，此处只做模板渲染，避免二次编码。
                let mut form = HashMap::new();
                for field in body {
                    form.insert(field.key.clone(), value::render(&field.value, vars));
                }
                http::post_form_text(&client, url, &headers, &form)
            }
            HttpMethod::Get => unreachable!("已在上面短路"),
        }
    }

    fn parse_json(&self, url: &str, text: &str) -> Result<Value> {
        match serde_json::from_str(text) {
            Ok(v) => Ok(v),
            Err(e) => {
                let mut vars = self.vars.clone();
                vars.set("error", e.to_string());
                vars.set("url", url);
                let tpl = self
                    .desc
                    .json_error
                    .as_deref()
                    .unwrap_or("描述 JSON 解析失败: {error}; url={url}");
                Err(parse_err(self.site, value::render(tpl, &vars)))
            }
        }
    }

    fn url_for(&self, req: ReqView<'_>, vars: &Vars) -> String {
        if let Some(tpl) = req.url {
            return value::render(tpl, vars);
        }
        let id = vars.text("id");
        for rule in req.rules {
            if cond_hit(
                &id,
                &rule.prefixes,
                &rule.suffixes,
                rule.contains.as_deref(),
                rule.contains_ignore_case.as_deref(),
            ) {
                return value::render(&rule.url, vars);
            }
        }
        let mut url = format!("{}{}", self.base, value::render(req.path, vars));
        for seg in req.segments {
            if let Some(name) = &seg.if_var {
                if vars.text(name).trim().is_empty() {
                    continue;
                }
            }
            if let Some(gt) = &seg.if_gt {
                if vars.number(&gt.var) <= gt.value {
                    continue;
                }
            }
            url.push_str(&value::render(&seg.template, vars));
        }
        if !req.params.is_empty() {
            url.push('?');
            let mut first = true;
            for p in req.params {
                if !first {
                    url.push('&');
                }
                first = false;
                url.push_str(&p.key);
                url.push('=');
                let rendered = value::render(&p.value, vars);
                url.push_str(&value::encode(&rendered, p.encode));
            }
        }
        url
    }
}

/// `ext` 分段：CatVod 的 `ext` 同时存在 `$$$`（多段配置）与 `#;|`（多 token）两种写法，
/// 基址提取与 `from_ext` 都必须按两种分隔符一起切，否则 `base$$$1` 会被当成完整基址。
fn ext_tokens(ext: &str) -> impl Iterator<Item = &str> {
    ext.split("$$$")
        .flat_map(|part| part.split(['#', ';', '|']))
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// 基址：`ext` 覆盖策略。
fn base_of(desc: &Descriptor, site: &Site) -> String {
    let ext = site.ext_string();
    match desc.base.ext {
        super::schema::ExtMode::None => desc.base.default.clone(),
        super::schema::ExtMode::Whole => {
            // 手写家族普遍按 `$$$` 取第一段（首段即基址），`whole` 也要遵守。
            let candidate = ext.split("$$$").next().unwrap_or("").trim();
            if candidate.starts_with("http://") || candidate.starts_with("https://") {
                candidate.trim_end_matches('/').to_string()
            } else {
                desc.base.default.clone()
            }
        }
        super::schema::ExtMode::Token => {
            for t in ext_tokens(&ext) {
                if t.starts_with("http://") || t.starts_with("https://") {
                    return t.trim_end_matches('/').to_string();
                }
            }
            desc.base.default.clone()
        }
    }
}

/// 解释一条变量声明。
fn eval_var(decl: &super::schema::VarDecl, site: &Site, vars: &mut Vars) {
    let mut value: Option<String> = decl.literal.clone();
    if let Some(prefix) = &decl.from_ext {
        let ext = site.ext_string();
        for t in ext_tokens(&ext) {
            if let Some(rest) = t.strip_prefix(prefix.as_str()) {
                let rest = rest.trim();
                if rest.starts_with("http://") || rest.starts_with("https://") {
                    value = Some(rest.trim_end_matches('/').to_string());
                }
            }
        }
    }
    if let Some(op) = decl.apply {
        match op {
            super::schema::VarOp::UnixTs => {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                value = Some(ts.to_string());
            }
            super::schema::VarOp::UnixTsMs => {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                value = Some(ts.to_string());
            }
            super::schema::VarOp::Md5Hex => {
                let seed = value::render(decl.template.as_deref().unwrap_or(""), vars);
                let digest = format!("{:x}", md5::compute(seed.as_bytes()));
                let sliced = match decl.slice {
                    Some([start, end]) if start <= end && end <= digest.len() => {
                        digest[start..end].to_string()
                    }
                    Some([start, end]) => digest.get(start..end).unwrap_or("").to_string(),
                    None => digest,
                };
                value = Some(sliced);
            }
        }
    }
    if value.as_deref().map(str::is_empty).unwrap_or(true) {
        if let Some(fallback) = &decl.default {
            value = Some(fallback.clone());
        }
    }
    vars.set(&decl.name, value.unwrap_or_default());
}

fn render_map(map: &BTreeMap<String, String>, vars: &Vars) -> BTreeMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), value::render(v, vars)))
        .collect()
}

/// 已声明条件全部成立才算命中（AND 语义）。
fn cond_hit(
    value: &str,
    prefixes: &[String],
    suffixes: &[String],
    contains: Option<&str>,
    contains_ignore_case: Option<&str>,
) -> bool {
    if !prefixes.is_empty() && !prefixes.iter().any(|p| value.starts_with(p.as_str())) {
        return false;
    }
    if !suffixes.is_empty() && !suffixes.iter().any(|s| value.ends_with(s.as_str())) {
        return false;
    }
    if let Some(c) = contains {
        if !value.contains(c) {
            return false;
        }
    }
    if let Some(c) = contains_ignore_case {
        if !value.to_ascii_lowercase().contains(&c.to_ascii_lowercase()) {
            return false;
        }
    }
    true
}

/* ============================================================
 * JSON 取值
 * ============================================================ */

fn json_field(root: &Value, spec: &super::schema::FieldSpec, base: &str, vars: &Vars) -> Option<String> {
    let raw = match spec.path.as_deref() {
        Some(p) => match &spec.join {
            // 数组拼接：`TextUtils.join(" ", list)`。
            Some(sep) => {
                let parts: Vec<String> = path::walk(root, &path::parse(p).unwrap_or_default())
                    .into_iter()
                    .filter_map(path::scalar)
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(sep))
                }
            }
            None => path::get(root, p).and_then(path::scalar),
        },
        None => None,
    };
    if let Some(text) = raw {
        if let Some(v) = value::apply(spec, text, base, vars) {
            return Some(v);
        }
    }
    if let Some(fallback) = &spec.fallback {
        return json_field(root, fallback, base, vars);
    }
    if spec.default.is_some() {
        return value::apply(spec, String::new(), base, vars);
    }
    None
}

/// 行集合：取第一个非空路径；若最后一步没有 `[]` 而结果恰为一个数组，展开它。
fn json_rows<'a>(root: &'a Value, selectors: &[String]) -> Vec<&'a Value> {
    for sel in selectors {
        let Ok(steps) = path::parse(sel) else { continue };
        let mut found = path::walk(root, &steps);
        if found.len() == 1 {
            if let Some(arr) = found[0].as_array() {
                found = arr.iter().collect();
            }
        }
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

/* ============================================================
 * HTML 取值
 * ============================================================ */

enum Scope2<'a> {
    Doc(&'a Html),
    Row(ElementRef<'a>),
}

impl<'a> Scope2<'a> {
    fn find(&self, selectors: &[String]) -> Option<ElementRef<'a>> {
        for sel in selectors {
            let Ok(selector) = Selector::parse(sel) else { continue };
            let found = match self {
                Scope2::Doc(doc) => doc.select(&selector).next(),
                Scope2::Row(row) => row.select(&selector).next(),
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
}

fn text_of(el: &ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

fn html_field(scope: &Scope2<'_>, spec: &super::schema::FieldSpec, base: &str, vars: &Vars) -> Option<String> {
    let el = if spec.self_el == Some(true) {
        match scope {
            Scope2::Row(row) => Some(*row),
            Scope2::Doc(_) => None,
        }
    } else {
        scope.find(&spec.sel)
    };
    let raw = match el {
        Some(el) => match &spec.attr {
            Some(attr) => el.value().attr(attr).unwrap_or("").trim().to_string(),
            None => text_of(&el),
        },
        None => String::new(),
    };
    if let Some(v) = value::apply(spec, raw, base, vars) {
        return Some(v);
    }
    if let Some(fallback) = &spec.fallback {
        return html_field(scope, fallback, base, vars);
    }
    if spec.default.is_some() {
        return value::apply(spec, String::new(), base, vars);
    }
    None
}

fn html_rows<'a>(doc: &'a Html, selectors: &[String]) -> Vec<ElementRef<'a>> {
    for sel in selectors {
        let Ok(selector) = Selector::parse(sel) else { continue };
        let rows: Vec<_> = doc.select(&selector).collect();
        if !rows.is_empty() {
            return rows;
        }
    }
    Vec::new()
}

/* ============================================================
 * 行过滤
 * ============================================================ */

/// 行过滤：返回 `true` 表示丢弃该行。
///
/// * `equals` / `exclude_contains` / `if_empty` 是排除条件（命中即丢）；
/// * `contains` 是保留条件（不命中即丢），用于「搜索结果只留名称匹配项」这类过滤。
fn row_filtered(field: &str, vars: &Vars, filters: &[Filter], values: &[(&str, Option<&str>)]) -> bool {
    for filter in filters {
        if filter.field != field {
            continue;
        }
        let current = values
            .iter()
            .find(|(name, _)| *name == filter.field)
            .and_then(|(_, v)| *v)
            .unwrap_or("");
        if let Some(eq) = &filter.equals {
            if current.trim() == eq.trim() {
                return true;
            }
        }
        if let Some(contains) = &filter.contains {
            let needle = value::render(contains, vars);
            if !current.contains(&needle) {
                return true;
            }
        }
        if let Some(excludes) = &filter.exclude_contains {
            let needle = value::render(excludes, vars);
            if current.contains(&needle) {
                return true;
            }
        }
        if filter.if_empty == Some(true) && current.trim().is_empty() {
            return true;
        }
    }
    false
}

/* ============================================================
 * 五个入口
 * ============================================================ */

/// 分类。
/// 描述层入口枚举（`family_fallback` 判定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Categories,
    List,
    Detail,
    Search,
    Play,
}

/// 该入口是否声明「仍走手写家族」。
///
/// 迁移期允许一个描述只接管部分入口：其余入口写 `family_fallback: true`，
/// 由分发层回落到手写实现。`search` 端点整体缺省时同样视为手写。
pub fn delegates(desc: &Descriptor, entry: Entry) -> bool {
    match entry {
        Entry::Categories => desc.endpoints.categories.family_fallback,
        Entry::List => desc.endpoints.list.family_fallback,
        Entry::Detail => desc.endpoints.detail.family_fallback,
        Entry::Search => desc
            .endpoints
            .search
            .as_ref()
            .map(|ep| ep.family_fallback)
            .unwrap_or(true),
        Entry::Play => desc.endpoints.play.family_fallback,
    }
}

pub fn categories(desc: &Descriptor, cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let ep: &CatEp = &desc.endpoints.categories;
    // 静态分类：不联网、不求值变量，直接返回（App 类固定导航）。
    if let Some(list) = &ep.static_categories {
        let _ = (cfg, site);
        return Ok(list
            .iter()
            .map(|cat| Category {
                type_id: cat.id.clone(),
                type_name: cat.name.clone(),
            })
            .collect());
    }
    let rt = Runtime::new(desc, cfg, site)?;
    let url = rt.url_for(req_view!(ep), &rt.vars);
    let text = match rt.fetch_req(&url, ep.method, &ep.body, &rt.vars) {
        Ok(t) => t,
        Err(e) => {
            if ep.on_error_empty {
                return Ok(Vec::new());
            }
            return Err(e);
        }
    };
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    match desc.kind {
        Kind::Json => {
            let value = rt.parse_json(&url, &text)?;
            for row in json_rows(&value, &ep.rows) {
                let id = json_field(row, &ep.fields.id, &rt.base, &rt.vars);
                let name = json_field(row, &ep.fields.name, &rt.base, &rt.vars);
                let values = [("id", id.as_deref()), ("name", name.as_deref())];
                let filtered = ["id", "name"]
                    .iter()
                    .any(|f| row_filtered(f, &rt.vars, &ep.filters, &values));
                if filtered {
                    continue;
                }
                let (Some(id), Some(name)) = (id, name) else { continue };
                if ep.dedupe && !seen.insert(id.clone()) {
                    continue;
                }
                out.push(Category { type_id: id, type_name: name });
            }
        }
        Kind::Html => {
            let doc = Html::parse_document(&text);
            for row in html_rows(&doc, &ep.rows) {
                let scope = Scope2::Row(row);
                let id = html_field(&scope, &ep.fields.id, &rt.base, &rt.vars);
                let name = html_field(&scope, &ep.fields.name, &rt.base, &rt.vars);
                let values = [("id", id.as_deref()), ("name", name.as_deref())];
                let filtered = ["id", "name"]
                    .iter()
                    .any(|f| row_filtered(f, &rt.vars, &ep.filters, &values));
                if filtered {
                    continue;
                }
                let (Some(id), Some(name)) = (id, name) else { continue };
                if ep.dedupe && !seen.insert(id.clone()) {
                    continue;
                }
                out.push(Category { type_id: id, type_name: name });
            }
        }
    }
    Ok(out)
}

/// 列表。
pub fn videos(
    desc: &Descriptor,
    cfg: &TvConfig,
    site: &Site,
    cat: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    let rt = Runtime::new(desc, cfg, site)?;
    let ep: &ListEp = &desc.endpoints.list;
    let page = match ep.page_min {
        Some(min) => page.max(min),
        None => page,
    };
    let mut vars = rt.vars.clone();
    vars.set("cat", cat);
    vars.set("page", page.to_string());
    let url = rt.url_for(req_view!(ep), &vars);
    let text = rt.fetch_req(&url, ep.method, &ep.body, &vars)?;
    match desc.kind {
        Kind::Json => {
            let value = rt.parse_json(&url, &text)?;
            Ok(collect_items_json(&value, &ep.rows, &ep.fields, &ep.filters, &rt, &vars))
        }
        Kind::Html => {
            let doc = Html::parse_document(&text);
            Ok(collect_items_html(&doc, &ep.rows, &ep.fields, &ep.filters, &rt, &vars))
        }
    }
}

fn collect_items_json(
    value: &Value,
    rows: &[String],
    fields: &ItemFields,
    filters: &[Filter],
    rt: &Runtime<'_>,
    vars: &Vars,
) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for row in json_rows(value, rows) {
        let id = json_field(row, &fields.id, &rt.base, vars);
        let name = json_field(row, &fields.name, &rt.base, vars);
        let cover = fields
            .cover
            .as_ref()
            .and_then(|spec| json_field(row, spec, &rt.base, vars));
        let remark = fields
            .remark
            .as_ref()
            .and_then(|spec| json_field(row, spec, &rt.base, vars));
        let values = [
            ("id", id.as_deref()),
            ("name", name.as_deref()),
            ("cover", cover.as_deref()),
            ("remark", remark.as_deref()),
        ];
        let filtered = ["id", "name", "cover", "remark"]
            .iter()
            .any(|f| row_filtered(f, vars, filters, &values));
        if filtered {
            continue;
        }
        let (Some(id), Some(name)) = (id, name) else { continue };
        out.push(VideoItem { id, name, cover, remark });
    }
    out
}

fn collect_items_html(
    doc: &Html,
    rows: &[String],
    fields: &ItemFields,
    filters: &[Filter],
    rt: &Runtime<'_>,
    vars: &Vars,
) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for row in html_rows(doc, rows) {
        let scope = Scope2::Row(row);
        let id = html_field(&scope, &fields.id, &rt.base, vars);
        let name = html_field(&scope, &fields.name, &rt.base, vars);
        let cover = fields
            .cover
            .as_ref()
            .and_then(|spec| html_field(&scope, spec, &rt.base, vars));
        let remark = fields
            .remark
            .as_ref()
            .and_then(|spec| html_field(&scope, spec, &rt.base, vars));
        let values = [
            ("id", id.as_deref()),
            ("name", name.as_deref()),
            ("cover", cover.as_deref()),
            ("remark", remark.as_deref()),
        ];
        let filtered = ["id", "name", "cover", "remark"]
            .iter()
            .any(|f| row_filtered(f, vars, filters, &values));
        if filtered {
            continue;
        }
        let (Some(id), Some(name)) = (id, name) else { continue };
        out.push(VideoItem { id, name, cover, remark });
    }
    out
}

/// 搜索。
pub fn search(desc: &Descriptor, cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let rt = Runtime::new(desc, cfg, site)?;
    let Some(ep): Option<&SearchEp> = desc.endpoints.search.as_ref() else {
        return Ok(Vec::new());
    };
    let keyword = if ep.trim_keyword { keyword.trim() } else { keyword };
    if ep.trim_keyword && keyword.is_empty() {
        return Ok(Vec::new());
    }
    let mut vars = rt.vars.clone();
    vars.set("kw", keyword);
    let url = rt.url_for(req_view!(ep), &vars);
    let text = rt.fetch_req(&url, ep.method, &ep.body, &vars)?;
    match desc.kind {
        Kind::Json => {
            let value = rt.parse_json(&url, &text)?;
            Ok(collect_items_json(&value, &ep.rows, &ep.fields, &ep.filters, &rt, &vars))
        }
        Kind::Html => {
            if let Some(marker) = &ep.empty_marker {
                if text.contains(marker) {
                    return Ok(Vec::new());
                }
            }
            let doc = Html::parse_document(&text);
            Ok(collect_items_html(&doc, &ep.rows, &ep.fields, &ep.filters, &rt, &vars))
        }
    }
}

/* ============================================================
 * 详情
 * ============================================================ */

/// 详情。
pub fn detail(desc: &Descriptor, cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let rt = Runtime::new(desc, cfg, site)?;
    let ep: &DetailEp = &desc.endpoints.detail;
    let id_value = if ep.trim_id { id.trim() } else { id };
    if ep.trim_id && id_value.trim().is_empty() {
        if let Some(msg) = &ep.empty_id_error {
            return Err(parse_err(site, msg.clone()));
        }
    }
    let mut vars = rt.vars.clone();
    vars.set("id", id_value);
    let url = rt.url_for(req_view!(ep), &vars);
    let text = rt.fetch_req(&url, ep.method, &ep.body, &vars)?;
    let mut out = VideoDetail { id: id_value.to_string(), ..Default::default() };
    match desc.kind {
        Kind::Json => {
            let value = rt.parse_json(&url, &text)?;
            let null = Value::Null;
            let root = match &ep.root {
                Some(p) => path::get(&value, p).unwrap_or(&null),
                None => &value,
            };
            let root_missing = ep.root.is_some() && std::ptr::eq(root, &null);
            fill_detail_json(root, ep, &rt.base, &vars, &mut out);
            // 与 HTML 分支一致：`fields.desc` 优先，`desc_parts` 只是多段拼接的备选。
            out.desc = match &ep.fields.desc {
                Some(spec) => json_field(root, spec, &rt.base, &vars),
                None => detail_desc_json(root, &ep.desc_parts),
            };
            out.episodes = detail_groups_json(root, ep.groups.as_ref(), site, &rt.base, &vars)?;
            let compact = serde_json::to_string(&value).unwrap_or_else(|_| text.clone());
            check_requirements(ep, &out, root_missing, &compact, id_value, site)?;
        }
        Kind::Html => {
            let doc = Html::parse_document(&text);
            let scope = Scope2::Doc(&doc);
            fill_detail_html(&scope, Some(&doc), ep, &rt.base, &vars, &mut out);
            check_requirements(ep, &out, false, &text, id_value, site)?;
        }
    }
    Ok(out)
}

fn fill_detail_json(
    root: &Value,
    ep: &DetailEp,
    base: &str,
    vars: &Vars,
    out: &mut VideoDetail,
) {
    let fields: &DetailFields = &ep.fields;
    out.name = json_field(root, &fields.name, base, vars).unwrap_or_default();
    out.cover = fields.cover.as_ref().and_then(|s| json_field(root, s, base, vars));
    out.year = fields.year.as_ref().and_then(|s| json_field(root, s, base, vars));
    out.area = fields.area.as_ref().and_then(|s| json_field(root, s, base, vars));
    out.director = fields
        .director
        .as_ref()
        .and_then(|s| json_field(root, s, base, vars));
    out.actor = fields.actor.as_ref().and_then(|s| json_field(root, s, base, vars));
}

fn detail_desc_json(root: &Value, parts: &[super::schema::DescPart]) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        let raw = path::get(root, &part.from).and_then(path::scalar);
        let Some(raw) = raw else { continue };
        let mut text = if part.collapse { value::collapse(&raw) } else { raw.trim().to_string() };
        if text.is_empty() {
            continue;
        }
        if let Some(label) = &part.label {
            text = format!("{label}{text}");
        }
        out.push(text);
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

fn detail_groups_json(
    root: &Value,
    groups: Option<&Value>,
    site: &Site,
    base: &str,
    vars: &Vars,
) -> Result<Vec<EpisodeGroup>> {
    let Some(groups) = groups else { return Ok(Vec::new()) };
    let shape = super::schema::parse_groups_json(groups).map_err(|e| parse_err(site, e))?;
    let rows = json_rows(root, &shape.rows);
    let mut out = Vec::new();
    if let Some(fixed) = &shape.single_group {
        let mut items = Vec::new();
        for row in rows {
            push_episode_json(row, &shape, base, vars, &mut items);
        }
        if !items.is_empty() {
            out.push(EpisodeGroup { group: fixed.clone(), items });
        }
        return Ok(out);
    }
    let mut index = 0usize;
    for row in rows {
        index += 1;
        let name = shape
            .name
            .as_deref()
            .and_then(|p| path::get(row, p))
            .and_then(path::scalar)
            .unwrap_or_default();
        let mut vars = Vars::new();
        vars.set("index", index.to_string());
        let group = if name.trim().is_empty() {
            shape
                .name_fallback
                .as_deref()
                .map(|tpl| value::render(tpl, &vars))
                .unwrap_or_default()
        } else {
            name
        };
        let mut items = Vec::new();
        push_episode_json(row, &shape, base, &vars, &mut items);
        if items.is_empty() {
            continue;
        }
        out.push(EpisodeGroup { group, items });
    }
    Ok(out)
}

fn push_episode_json(
    row: &Value,
    shape: &GroupsJson,
    base: &str,
    vars: &Vars,
    items: &mut Vec<Episode>,
) {
    let rows: Vec<&Value> = match &shape.items {
        Some(p) => path::walk(row, &path::parse(p).unwrap_or_default()),
        // 缺省 `items` 表示行本身即条目（LiteApple `data.urls[]`）。
        None => vec![row],
    };
    for entry in rows {
        let url = shape
            .url
            .as_ref()
            .and_then(|spec| json_field(entry, spec, base, vars))
            .unwrap_or_default();
        if url.trim().is_empty() {
            continue;
        }
        let mut url = url;
        if let Some(prefix_if) = &shape.url_prefix_if_group {
            if shape
                .name
                .as_deref()
                .and_then(|p| path::get(row, p))
                .and_then(path::scalar)
                .as_deref()
                == Some(prefix_if.when.as_str())
            {
                if !url.starts_with(&prefix_if.prefix) {
                    url = format!("{}{}", prefix_if.prefix, url);
                }
            }
        }
        let mut title = shape
            .title
            .as_ref()
            .and_then(|spec| json_field(entry, spec, base, vars))
            .unwrap_or_default()
            .trim()
            .to_string();
        if title.is_empty() && shape.title_fallback_url {
            title = url.clone();
        }
        if title.is_empty() && shape.title_fallback_episode {
            title = format!("第{}集", items.len() + 1);
        }
        items.push(Episode { title, url });
    }
}

fn fill_detail_html(
    scope: &Scope2<'_>,
    doc: Option<&Html>,
    ep: &DetailEp,
    base: &str,
    vars: &Vars,
    out: &mut VideoDetail,
) {
    let fields: &DetailFields = &ep.fields;
    out.name = html_field(scope, &fields.name, base, vars).unwrap_or_default();
    out.cover = fields.cover.as_ref().and_then(|s| html_field(scope, s, base, vars));
    out.desc = fields.desc.as_ref().and_then(|s| html_field(scope, s, base, vars));
    out.year = fields.year.as_ref().and_then(|s| html_field(scope, s, base, vars));
    out.area = fields.area.as_ref().and_then(|s| html_field(scope, s, base, vars));
    out.director = fields
        .director
        .as_ref()
        .and_then(|s| html_field(scope, s, base, vars));
    out.actor = fields.actor.as_ref().and_then(|s| html_field(scope, s, base, vars));
    if let Some(doc) = doc {
        fill_meta_html(doc, ep, out);
        if let Some(groups) = parse_groups_html_opt(ep.groups.as_ref()) {
            out.episodes = detail_groups_html(doc, &groups, base);
        }
    }
}

fn parse_groups_html_opt(groups: Option<&Value>) -> Option<GroupsHtml> {
    let groups = groups?;
    super::schema::parse_groups_html(groups).ok()
}

/// 逐条 `li.data`：先按锚点 href 归类，再按扁平文本标签取值。
fn fill_meta_html(doc: &Html, ep: &DetailEp, out: &mut VideoDetail) {
    let Some(scope_sel) = &ep.meta_scope else { return };
    let Ok(selector) = Selector::parse(scope_sel) else { return };
    let anchor_sel = ep.meta_anchor.as_deref().unwrap_or("a");
    let Ok(anchor) = Selector::parse(anchor_sel) else { return };
    for block in doc.select(&selector) {
        for el in block.select(&anchor) {
            let href = el.value().attr("href").unwrap_or("");
            for meta in &ep.meta {
                let Some(needle) = &meta.href_contains else { continue };
                if href.contains(needle) {
                    if meta.first_only && detail_field_set(out, &meta.field) {
                        continue;
                    }
                    let value = text_of(&el);
                    if !value.is_empty() {
                        set_detail_field(out, &meta.field, value);
                    }
                }
            }
        }
        let flat = value::collapse(&text_of(&block));
        let mut labeled_hit = false;
        for meta in &ep.meta {
            let Some(label) = &meta.label else { continue };
            if labeled_hit {
                continue;
            }
            if meta.first_only && detail_field_set(out, &meta.field) {
                continue;
            }
            if let Some(v) = value::labeled(&flat, label, meta.cut.as_deref()) {
                set_detail_field(out, &meta.field, v);
                labeled_hit = true;
            }
        }
    }
}

fn detail_field_set(out: &VideoDetail, field: &str) -> bool {
    match field {
        "name" => !out.name.is_empty(),
        "cover" => out.cover.is_some(),
        "desc" => out.desc.is_some(),
        "year" => out.year.is_some(),
        "area" => out.area.is_some(),
        "director" => out.director.is_some(),
        "actor" => out.actor.is_some(),
        _ => false,
    }
}

fn set_detail_field(out: &mut VideoDetail, field: &str, value: String) {
    match field {
        "name" => out.name = value,
        "cover" => out.cover = Some(value),
        "desc" => out.desc = Some(value),
        "year" => out.year = Some(value),
        "area" => out.area = Some(value),
        "director" => out.director = Some(value),
        "actor" => out.actor = Some(value),
        _ => {}
    }
}

fn detail_groups_html(doc: &Html, shape: &GroupsHtml, base: &str) -> Vec<EpisodeGroup> {
    let mut tabs: Vec<String> = Vec::new();
    if let Ok(selector) = Selector::parse(&shape.tabs) {
        for el in doc.select(&selector) {
            tabs.push(text_of(&el));
        }
    }
    let mut out = Vec::new();
    let Ok(list_sel) = Selector::parse(&shape.lists) else { return out };
    let Ok(item_sel) = Selector::parse(&shape.item) else { return out };
    let url_attr = shape.url_attr.as_deref().unwrap_or("href");
    for (idx, list) in doc.select(&list_sel).enumerate() {
        let mut items = Vec::new();
        for el in list.select(&item_sel) {
            let mut url = el.value().attr(url_attr).unwrap_or("").trim().to_string();
            if url.is_empty() {
                continue;
            }
            if let Some(prefix) = &shape.url_strip_prefix {
                url = url.replace(prefix.as_str(), "");
            }
            if shape.url_join_base {
                url = crate::html_rule::join_url(base, &url);
            }
            let title = text_of(&el);
            items.push(Episode { title, url });
        }
        if items.is_empty() {
            continue;
        }
        let mut vars = Vars::new();
        vars.set("index", (idx + 1).to_string());
        let tab = tabs.get(idx).map(|s| value::clean_private_use(s)).unwrap_or_default();
        let group = if tab.trim().is_empty() {
            shape
                .name_fallback
                .as_deref()
                .map(|tpl| value::render(tpl, &vars))
                .unwrap_or_default()
        } else {
            value::collapse(&tab)
        };
        out.push(EpisodeGroup { group, items });
    }
    out
}

fn check_requirements(
    ep: &DetailEp,
    out: &VideoDetail,
    root_missing: bool,
    raw: &str,
    id: &str,
    site: &Site,
) -> Result<()> {
    for req in &ep.require {
        let failed = match req.target {
            ReqTarget::Root => root_missing,
            ReqTarget::Name => out.name.is_empty(),
            ReqTarget::Episodes => out.episodes.is_empty(),
            ReqTarget::NameAndEpisodes => out.name.is_empty() && out.episodes.is_empty(),
        };
        if failed {
            return Err(requirement_error(req, raw, id, site));
        }
    }
    Ok(())
}

fn requirement_error(req: &Requirement, raw: &str, id: &str, site: &Site) -> CoreError {
    let mut vars = Vars::new();
    vars.set("id", id);
    vars.set("raw", raw);
    vars.set("value", raw);
    parse_err(site, value::render(&req.error, &vars))
}

/* ============================================================
 * 播放
 * ============================================================ */

/// 播放。
pub fn player_content(
    desc: &Descriptor,
    cfg: &TvConfig,
    site: &Site,
    id: &str,
) -> Result<Value> {
    let rt = Runtime::new(desc, cfg, site)?;
    let ep: &PlaySpec = &desc.endpoints.play;
    let token = id.trim();
    if token.is_empty() {
        let msg = ep
            .empty_id_error
            .clone()
            .unwrap_or_else(|| "播放地址为空".to_string());
        return Err(parse_err(site, msg));
    }
    let mut vars = rt.vars.clone();
    vars.set("id", token);
    let mut chosen: Option<(String, u8, bool, BTreeMap<String, String>)> = None;
    for rule in &ep.rules {
        let hit = cond_hit(
            token,
            &rule.prefixes,
            &rule.suffixes,
            rule.contains.as_deref(),
            rule.contains_ignore_case.as_deref(),
        );
        if !hit {
            continue;
        }
        if !rule.platforms.is_empty() {
            match rule.platforms.iter().find(|p| token.contains(p.contains.as_str())) {
                Some(platform) => vars.set("platform.referer", &platform.referer),
                None => continue,
            }
        }
        let url = match &rule.url {
            Some(tpl) => value::render(tpl, &vars),
            None => value::render(ep.url.as_deref().unwrap_or("{id}"), &vars),
        };
        let header = render_map(&rule.header, &vars);
        chosen = Some((url, rule.parse.unwrap_or(ep.parse), rule.jx.unwrap_or(ep.jx), header));
        break;
    }
    let (url, parse, jx, header) = match chosen {
        Some(c) => c,
        None => match &ep.proxy {
            Some(proxy) => resolve_proxy(proxy, &rt, &vars, site)?,
            None => (
                value::render(ep.url.as_deref().unwrap_or("{id}"), &vars),
                ep.parse,
                ep.jx,
                render_map(&ep.header, &vars),
            ),
        },
    };
    let header = if ep.header_mode == HeaderMode::Descriptor {
        render_map(&desc.headers, &vars)
    } else {
        header
    };
    let mut map = Map::new();
    map.insert("parse".to_string(), json!(parse));
    if jx {
        map.insert("jx".to_string(), json!(0));
    }
    map.insert("playUrl".to_string(), json!(""));
    map.insert("url".to_string(), json!(url));
    if ep.header_as_json {
        let text = serde_json::to_string(&header).unwrap_or_else(|_| "{}".to_string());
        map.insert("header".to_string(), json!(text));
    } else {
        map.insert("header".to_string(), json!(header));
    }
    Ok(Value::Object(map))
}

fn resolve_proxy(
    proxy: &ProxySpec,
    rt: &Runtime<'_>,
    vars: &Vars,
    site: &Site,
) -> Result<(String, u8, bool, BTreeMap<String, String>)> {
    let url = value::render(&proxy.url, vars);
    let text = rt.fetch(&url)?;
    let value = rt.parse_json(&url, &text)?;
    let mut real = path::get(&value, &proxy.extract)
        .and_then(path::scalar)
        .unwrap_or_default();
    if proxy.join_base && !real.trim().is_empty() {
        real = crate::html_rule::join_url(&rt.base, &real);
    }
    if real.trim().is_empty() {
        return Err(parse_err(site, value::render(&proxy.error, vars)));
    }
    Ok((
        real,
        rt.desc.endpoints.play.parse,
        rt.desc.endpoints.play.jx,
        render_map(&proxy.header, vars),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rules;

    fn desc(text: &str) -> Descriptor {
        super::super::schema::parse(text).expect("descriptor")
    }

    fn site(api: &str, ext: &str) -> Site {
        Site { api: api.to_string(), ext: Some(Value::String(ext.to_string())), ..Default::default() }
    }

    fn cfg() -> TvConfig {
        TvConfig { http: Rules::default(), ..Default::default() }
    }

    #[test]
    fn cond_hit_and_semantics() {
        assert!(cond_hit("abc", &["a".into()], &[], None, None));
        assert!(!cond_hit("abc", &["a".into()], &["z".into()], None, None));
        assert!(cond_hit("ABC", &[], &[], None, Some("abc")));
        assert!(!cond_hit("abc", &[], &[], Some("x"), None));
    }

    #[test]
    fn url_for_rules_segments_and_params() {
        let d = desc(
            r#"{
              "schema": 1, "api": "csp_X", "kind": "json", "class": "X", "jar": "x.jar",
              "jar_sha256": "0000000000000000000000000000000000000000000000000000000000000001",
              "evidence": [{"method": "homeContent", "file": "X.java"}],
              "base": {"default": "https://b.example", "ext": "whole"},
              "vars": [],
              "headers": {"User-Agent": "UA"},
              "endpoints": {
                "categories": {"path": "/c", "rows": ["data[]"],
                  "fields": {"id": "type_id", "name": "type_name"}},
                "list": {"path": "/l", "page_min": 1, "rows": ["data[]"],
                  "fields": {"id": "id", "name": "name"},
                  "segments": [{"template": "/p{page}", "if_gt": {"var": "page", "value": 1}}],
                  "params": [{"key": "type", "value": "{cat}", "encode": "alnum"},
                             {"key": "p", "value": "{page}"}]},
                "detail": {"path": "/d",
                  "fields": {"name": "name"},
                  "segments": [{"template": "/{id}"}],
                  "rules": [{"prefixes": ["http://"], "url": "{id}"}]},
                "play": {"parse": 1, "url": "{base}/p/{id}", "header": {"User-Agent": "{ua}"}}
              }
            }"#,
        );
        let cfg = cfg();
        let site = site("csp_X", "https://override.example/");
        let rt = Runtime::new(&d, &cfg, &site).unwrap();
        assert_eq!(rt.base, "https://override.example");
        let mut vars = rt.vars.clone();
        vars.set("cat", "电影 1");
        vars.set("page", "3");
        let ep = &d.endpoints.list;
        let url = rt.url_for(req_view!(ep), &vars);
        assert_eq!(url, "https://override.example/l/p3?type=%E7%94%B5%E5%BD%B1%201&p=3");
        let ep = &d.endpoints.detail;
        vars.set("id", "http://abs.example/x");
        assert_eq!(rt.url_for(req_view!(ep), &vars), "http://abs.example/x");
        vars.set("id", "42");
        assert_eq!(rt.url_for(req_view!(ep), &vars), "https://override.example/d/42");
    }

    #[test]
    fn play_platform_and_proxy_priority() {
        let d = desc(
            r#"{
              "schema": 1, "api": "csp_X", "kind": "json", "class": "X", "jar": "x.jar",
              "jar_sha256": "0000000000000000000000000000000000000000000000000000000000000001",
              "evidence": [{"method": "playerContent", "file": "X.java"}],
              "base": {"default": "https://b.example"},
              "headers": {},
              "endpoints": {
                "categories": {"path": "/c", "rows": ["x"], "fields": {"id": "a", "name": "b"}},
                "list": {"path": "/l", "rows": ["x"], "fields": {"id": "a", "name": "b"}},
                "detail": {"path": "/d", "fields": {"name": "a"}},
                "play": {"parse": 0, "jx": true,
                  "url": "{base}/p/{id}",
                  "rules": [
                    {"prefixes": ["jx:"], "url": "{id}", "header": {}},
                    {"platforms": [{"contains": "youku", "referer": "https://www.youku.com"}],
                     "header": {"referer": "{platform.referer}", "User-Agent": "C"}}
                  ],
                  "proxy": {"url": "https://proxy.example/?url={id:alnum}",
                            "extract": "data.url", "header": {"User-Agent": "D"},
                            "error": "代理未返回 url: id={id}"}
                }
              }
            }"#,
        );
        let s = site("csp_X", "");
        let c = cfg();
        assert_eq!(
            player_content(&d, &c, &s, "jx:1").unwrap(),
            json!({"parse": 0, "jx": 0, "playUrl": "", "url": "jx:1", "header": {}})
        );
        assert_eq!(
            player_content(&d, &c, &s, "youku-9").unwrap(),
            json!({"parse": 0, "jx": 0, "playUrl": "", "url": "https://b.example/p/youku-9",
                   "header": {"referer": "https://www.youku.com", "User-Agent": "C"}})
        );
        assert!(player_content(&d, &c, &s, "  ").is_err());
    }
}
