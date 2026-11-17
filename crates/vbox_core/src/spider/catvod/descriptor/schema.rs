//! 翻译层描述 schema（计划 `docs/计划-type3-翻译层与引擎工作包.md` §5）。
//!
//! 描述文件是**数据**，不是脚本：只声明协议形状（基址、请求、字段路径/选择器、
//! 取值后处理、播放分支），由 [`super::engine`] 通用解释。刻意不做图灵完备——
//! 遇到描述表达不了的分支，要么加一条白名单指令，要么保留手写实现（§5.4 插件口）。
//!
//! 设计要点：
//! 1. `deny_unknown_fields`：拼错的键直接报错，避免「字段写错但静默取空」；
//! 2. provenance 必填（`jar` / `jar_sha256` / `class` / `evidence`）：描述必须能追到反编译证据；
//! 3. `kind` 决定 `rows` / `fields` 的解释方式：`json` 走 JSON 路径，`html` 走 CSS 选择器；
//! 4. 所有模板统一用 `{name}` 占位（`{id}` / `{cat}` / `{page}` / `{kw}` / `{base}` /
//!    `{变量名}`），`{name:alnum}` 与 `{name:url}` 是两种百分号编码修饰。
//!
//! schema 版本 1。破坏性改动必须提升 [`SCHEMA_VERSION`] 并保留旧版解析。

use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
use serde::Deserialize;

/// 当前 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;

/// 描述类型：决定 `rows` / `fields` 的解释方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// 自建 JSON API：`rows` 是 JSON 路径（`data.list[]`），字段是相对路径。
    Json,
    /// 网页：`rows` 是 CSS 选择器，字段是 CSS 选择器 + 属性/文本。
    Html,
}

/// `ext` 覆盖基址的方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtMode {
    /// 不读 `ext`，始终用默认基址。
    #[default]
    None,
    /// `ext` 整串以 http(s) 开头时覆盖（Ysj）。
    Whole,
    /// `ext` 按 `#;|` 分段，第一段 http(s) 覆盖（Kkys / LiteApple）。
    Token,
}

/// 取值后处理：`apply` 的取值白名单。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarOp {
    /// 当前 Unix 秒。
    UnixTs,
    /// 当前 Unix 毫秒。
    UnixTsMs,
    /// 对 `template` 求值后取 MD5 十六进制，可选按 `slice` 截取。
    Md5Hex,
}

/// 百分号编码口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encode {
    /// 不编码（数字、固定串）。
    #[default]
    None,
    /// `NON_ALPHANUMERIC`：除字母数字外全编码（Kkys / Ysj 口径）。
    Alnum,
    /// 保留 `-_.~` 的 URL 编码（LiteApple 口径）。
    Url,
}

/// 顶层描述。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    /// 必须等于 [`SCHEMA_VERSION`]。
    pub schema: u32,
    /// TVBox api 名（如 `csp_Kkys`），大小写不敏感匹配。
    pub api: String,
    /// 协议类型。
    pub kind: Kind,
    /// 原类全名（provenance）。
    pub class: String,
    /// 证据 jar 文件名（provenance）。
    pub jar: String,
    /// 证据 jar SHA-256（小写 hex，64 位）。
    pub jar_sha256: String,
    /// 方法级证据：描述中的每个协议点应能指回反编译源。
    pub evidence: Vec<Evidence>,
    /// 基址策略。
    pub base: Base,
    /// 变量表（按声明顺序求值，可引用先前变量）。
    #[serde(default)]
    pub vars: Vec<VarDecl>,
    /// 随每个请求发送的请求头；值支持模板（如 `"{ts}"` / `"{hash}"`）。
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// 重试次数；缺省用 `cfg.http.retry`（Ysj 口径）。
    #[serde(default)]
    pub retries: Option<u32>,
    /// JSON 解析失败的报错模板（`{error}` / `{url}`）。
    #[serde(default)]
    pub json_error: Option<String>,
    /// 五入口。
    pub endpoints: Endpoints,
}

/// 一条方法级证据。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// TVBox 方法名（`homeContent` / `categoryContent` / ...）。
    pub method: String,
    /// 反编译源相对路径（仓库根为基准）。
    pub file: String,
    /// 行号区间，如 `"287-340"`。
    #[serde(default)]
    pub lines: Option<String>,
    /// 补充说明（如「jadx 未反编译，用 Vineflower 视图」）。
    #[serde(default)]
    pub note: Option<String>,
}

/// 基址策略。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base {
    /// 默认基址（无尾斜杠）。
    pub default: String,
    /// `ext` 覆盖方式。
    #[serde(default)]
    pub ext: ExtMode,
}

/// 变量声明（一个变量 = 字面量 / ext 段 / 计算值）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarDecl {
    /// 变量名，模板里写作 `{name}`。
    pub name: String,
    /// 字面量。
    #[serde(default)]
    pub literal: Option<String>,
    /// 从 `ext` 的 `prefix=` 段取值（如 `proxy=`）。
    #[serde(default)]
    pub from_ext: Option<String>,
    /// 计算方式。
    #[serde(default)]
    pub apply: Option<VarOp>,
    /// 计算模板（`md5_hex` 的种子）。
    #[serde(default)]
    pub template: Option<String>,
    /// 十六进制结果截取 `[start, end)`，按字符。
    #[serde(default)]
    pub slice: Option<[usize; 2]>,
    /// 兜底值。
    #[serde(default)]
    pub default: Option<String>,
}

/// 请求段：URL 规则、路径、可选段、查询参数。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Req {
    /// 路径模板（拼在基址后），可含 `{id}` / `{cat}` 等。
    #[serde(default)]
    pub path: String,
    /// 完整 URL 模板；给出时忽略 `path` / `segments`。
    #[serde(default)]
    pub url: Option<String>,
    /// 按条件改写 URL（命中第一条）。
    #[serde(default)]
    pub rules: Vec<ReqRule>,
    /// 追加到 `path` 后的可选段（按顺序判断条件）。
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// 查询参数（按顺序拼接，值支持模板）。
    #[serde(default)]
    pub params: Vec<Param>,
}

/// 一条 URL 改写规则；所有声明条件同时成立才命中。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReqRule {
    /// 任一前缀命中。
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// 任一后缀命中。
    #[serde(default)]
    pub suffixes: Vec<String>,
    /// 子串命中（大小写敏感）。
    #[serde(default)]
    pub contains: Option<String>,
    /// 子串命中（大小写不敏感）。
    #[serde(default)]
    pub contains_ignore_case: Option<String>,
    /// 命中后的完整 URL 模板。
    pub url: String,
}

/// 追加段：条件成立时把模板拼到路径后。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    /// 段模板。
    pub template: String,
    /// 变量非空时追加。
    #[serde(default)]
    pub if_var: Option<String>,
    /// 变量数值大于阈值时追加（如 `page > 1`）。
    #[serde(default)]
    pub if_gt: Option<IfGt>,
}

/// 数值比较条件。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IfGt {
    /// 变量名。
    pub var: String,
    /// 阈值。
    pub value: u64,
}

/// 查询参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// 参数名。
    pub key: String,
    /// 值模板。
    pub value: String,
    /// 值的编码口径。
    #[serde(default)]
    pub encode: Encode,
}

/// HTTP 方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    /// GET（缺省）。
    #[default]
    Get,
    /// POST JSON。
    PostJson,
    /// POST 表单。
    PostForm,
}

/// POST body 字段：JSON 数值/布尔需显式声明，其余按字符串。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyParam {
    /// 字段名。
    pub key: String,
    /// 值模板。
    pub value: String,
    /// JSON 类型（表单模式忽略）。
    #[serde(default)]
    pub kind: BodyKind,
}

/// body 字段的 JSON 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// 字符串（缺省）。
    #[default]
    Str,
    /// 数值（渲染结果按 JSON 数字解析，失败则退化为字符串）。
    Number,
    /// 布尔（`true`/`1` 为真）。
    Bool,
}

/// 字段取值规则（JSON 路径或 CSS 选择器 + 后处理链）。
#[derive(Debug, Clone)]
pub struct FieldSpec {
    /// JSON 路径（`kind=json`）。
    pub path: Option<String>,
    /// CSS 选择器（`kind=html`），按顺序取第一个有结果的。
    pub sel: Vec<String>,
    /// 属性名；缺省取文本。
    pub attr: Option<String>,
    /// 对容器元素本身取值（行元素 / 分类锚点）。
    pub self_el: Option<bool>,
    /// 结果为空时改用的规则。
    pub fallback: Option<Box<FieldSpec>>,
    /// 结果为空时的字面量。
    pub default: Option<String>,
    /// 去首尾空白（HTML 文本默认已去）。
    pub trim: bool,
    /// 空白折叠为单个空格。
    pub collapse: bool,
    /// 过滤图标字体私用区（U+E000–U+F8FF）与 NBSP，并折叠空白（Ysj `clean_text`）。
    pub clean_private_use: bool,
    /// 去前缀。
    pub strip_prefix: Option<String>,
    /// 去后缀。
    pub strip_suffix: Option<String>,
    /// 分段分隔符（缺省 `/`；`split_on: "$"` 配 `segment` 拆 `标题$url`）。
    pub split_on: Option<String>,
    /// 取分段第 n 段（0 基；分隔符见 `split_on`）。
    pub segment: Option<usize>,
    /// 值为数组时按该分隔符拼接（手写 `TextUtils.join(" ", actors)`）。
    pub join: Option<String>,
    /// 分段为空时回退到最后一段非空（Ysj `id_from_href`）。
    pub tail_non_empty: bool,
    /// 百分号解码。
    pub percent_decode: bool,
    /// 相对地址按基址补全。
    pub join_base: bool,
    /// 子串替换（按声明顺序）。
    pub replace: BTreeMap<String, String>,
    /// 结果非空时加前缀模板。
    pub prefix: Option<String>,
    /// 结果非空时加后缀模板。
    pub suffix: Option<String>,
    /// 空串转为 `None`（对应 `Option<String>` 字段）。
    pub non_empty: bool,
    /// 空串保留为值（对应手写里「字段可以空、行仍保留」的 `as_str`，如 Kkys `vod_name`）。
    pub keep_empty: bool,
}

/// 字符串简写（`"id": "vod_id"`）等价于 `{"path": "vod_id"}`；对象形式键名严格校验。
impl<'de> Deserialize<'de> for FieldSpec {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(de).map_err(de::Error::custom)?;
        match value {
            serde_json::Value::String(path) => Ok(FieldSpec { path: Some(path), ..Default::default() }),
            serde_json::Value::Object(map) => {
                let mut spec = FieldSpec::default();
                for (key, value) in map {
                    match key.as_str() {
                        "path" => spec.path = opt_string(value)?,
                        "sel" => spec.sel = string_vec(value)?,
                        "attr" => spec.attr = opt_string(value)?,
                        "self" => spec.self_el = opt_bool(value)?,
                        "fallback" => {
                            spec.fallback = Some(Box::new(
                                serde_json::from_value(value).map_err(de::Error::custom)?,
                            ))
                        }
                        "default" => spec.default = opt_string(value)?,
                        "trim" => spec.trim = as_bool(value)?,
                        "collapse" => spec.collapse = as_bool(value)?,
                        "clean_private_use" => spec.clean_private_use = as_bool(value)?,
                        "strip_prefix" => spec.strip_prefix = opt_string(value)?,
                        "strip_suffix" => spec.strip_suffix = opt_string(value)?,
                        "split_on" => spec.split_on = opt_string(value)?,
                        "segment" => spec.segment = Some(as_usize(value)?),
                        "join" => spec.join = opt_string(value)?,
                        "tail_non_empty" => spec.tail_non_empty = as_bool(value)?,
                        "percent_decode" => spec.percent_decode = as_bool(value)?,
                        "join_base" => spec.join_base = as_bool(value)?,
                        "replace" => {
                            spec.replace =
                                serde_json::from_value(value).map_err(de::Error::custom)?
                        }
                        "prefix" => spec.prefix = opt_string(value)?,
                        "suffix" => spec.suffix = opt_string(value)?,
                        "non_empty" => spec.non_empty = as_bool(value)?,
                        "keep_empty" => spec.keep_empty = as_bool(value)?,
                        other => {
                            return Err(de::Error::custom(format!("未知字段规则键 `{other}`")));
                        }
                    }
                }
                Ok(spec)
            }
            other => Err(de::Error::custom(format!(
                "字段规则必须是字符串或对象，实际为 {other}"
            ))),
        }
    }
}

fn opt_string<E: de::Error>(value: serde_json::Value) -> Result<Option<String>, E> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        other => Err(E::custom(format!("期望字符串，实际为 {other}"))),
    }
}

fn opt_bool<E: de::Error>(value: serde_json::Value) -> Result<Option<bool>, E> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Bool(b) => Ok(Some(b)),
        other => Err(E::custom(format!("期望布尔值，实际为 {other}"))),
    }
}

fn as_bool<E: de::Error>(value: serde_json::Value) -> Result<bool, E> {
    value
        .as_bool()
        .ok_or_else(|| E::custom(format!("期望布尔值，实际为 {value}")))
}

fn as_usize<E: de::Error>(value: serde_json::Value) -> Result<usize, E> {
    match value.as_u64() {
        Some(n) => Ok(n as usize),
        None => Err(E::custom(format!("期望非负整数，实际为 {value}"))),
    }
}

fn string_vec<E: de::Error>(value: serde_json::Value) -> Result<Vec<String>, E> {
    match value {
        serde_json::Value::String(s) => Ok(vec![s]),
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                serde_json::Value::String(s) => Ok(s),
                other => Err(E::custom(format!("选择器必须是字符串，实际为 {other}"))),
            })
            .collect(),
        other => Err(E::custom(format!("选择器必须是字符串或数组，实际为 {other}"))),
    }
}

impl FieldSpec {
    /// 是否声明了任何取值来源。
    pub fn has_source(&self) -> bool {
        self.path.is_some() || !self.sel.is_empty() || self.self_el.unwrap_or(false)
    }
}

impl Default for FieldSpec {
    fn default() -> Self {
        FieldSpec {
            path: None,
            sel: Vec::new(),
            attr: None,
            self_el: None,
            fallback: None,
            split_on: None,
            join: None,
            default: None,
            trim: false,
            collapse: false,
            clean_private_use: false,
            strip_prefix: None,
            strip_suffix: None,
            segment: None,
            tail_non_empty: false,
            percent_decode: false,
            join_base: false,
            replace: BTreeMap::new(),
            prefix: None,
            suffix: None,
            non_empty: false,
            keep_empty: false,
        }
    }
}

/// 分类行字段。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatFields {
    /// 分类 id。
    pub id: FieldSpec,
    /// 分类名。
    pub name: FieldSpec,
}

impl Default for CatFields {
    fn default() -> Self {
        CatFields {
            id: FieldSpec::default(),
            name: FieldSpec::default(),
        }
    }
}

/// 静态分类项（不联网固定返回，见 `CatEp::static_categories`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticCategory {
    /// 分类 id。
    pub id: String,
    /// 分类名。
    pub name: String,
}

/// 列表/搜索行字段。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemFields {
    /// 条目 id。
    pub id: FieldSpec,
    /// 条目名。
    pub name: FieldSpec,
    /// 封面。
    #[serde(default)]
    pub cover: Option<FieldSpec>,
    /// 备注。
    #[serde(default)]
    pub remark: Option<FieldSpec>,
}

/// 详情字段。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailFields {
    /// 标题。
    pub name: FieldSpec,
    /// 封面。
    #[serde(default)]
    pub cover: Option<FieldSpec>,
    /// 简介（单字段；与 `desc_parts` 二选一，`desc` 优先）。
    #[serde(default)]
    pub desc: Option<FieldSpec>,
    /// 年份。
    #[serde(default)]
    pub year: Option<FieldSpec>,
    /// 地区。
    #[serde(default)]
    pub area: Option<FieldSpec>,
    /// 导演。
    #[serde(default)]
    pub director: Option<FieldSpec>,
    /// 主演。
    #[serde(default)]
    pub actor: Option<FieldSpec>,
}

/// 简介分段：`label：value`，空值跳过。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescPart {
    /// 标签前缀（写成 `标签：`）。
    #[serde(default)]
    pub label: Option<String>,
    /// 取值路径。
    pub from: String,
    /// 空白折叠。
    #[serde(default)]
    pub collapse: bool,
}

/// 行过滤：字段满足条件时丢弃该行。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// 字段名（`id` / `name` / `cover` / `remark`）。
    pub field: String,
    /// 去空白后等于该值。
    #[serde(default)]
    pub equals: Option<String>,
    /// 包含该子串（值支持 `{kw}` 模板）。
    #[serde(default)]
    pub contains: Option<String>,
    /// 包含该子串时丢弃该行（值支持模板；Ysj 分类跳过 `.html` 链接）。
    #[serde(default)]
    pub exclude_contains: Option<String>,
    /// 去空白后为空。
    #[serde(default)]
    pub if_empty: Option<bool>,
}

/// 分类入口。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatEp {
    /// HTTP 方法（缺省 GET）。
    #[serde(default)]
    pub method: HttpMethod,
    /// POST body（`method != get` 时必填）。
    #[serde(default)]
    pub body: Vec<BodyParam>,
    /// 该入口仍由手写家族实现：描述存在时本入口不让描述层接管（迁移期降级）。
    #[serde(default)]
    pub family_fallback: bool,
    /// 路径模板。
    #[serde(default)]
    pub path: String,
    /// 完整 URL 模板。
    #[serde(default)]
    pub url: Option<String>,
    /// URL 改写规则。
    #[serde(default)]
    pub rules: Vec<ReqRule>,
    /// 可选路径段。
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// 查询参数。
    #[serde(default)]
    pub params: Vec<Param>,
    /// 行集合（JSON 路径数组，或 CSS 选择器）。`static_categories` 存在时可为空。
    #[serde(default, deserialize_with = "one_or_many")]
    pub rows: Vec<String>,
    /// 行字段。`static_categories` 存在时可为缺省。
    #[serde(default)]
    pub fields: CatFields,
    /// 静态分类：声明后不发起请求，直接返回该列表（App 类固定导航）。
    #[serde(default)]
    pub static_categories: Option<Vec<StaticCategory>>,
    /// 行过滤。
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// 按 id 去重（保留首个）。
    #[serde(default)]
    pub dedupe: bool,
    /// 请求/解析失败时返回空（LiteApple 口径）。
    #[serde(default)]
    pub on_error_empty: bool,
}

/// 列表入口。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListEp {
    /// HTTP 方法（缺省 GET）。
    #[serde(default)]
    pub method: HttpMethod,
    /// POST body（`method != get` 时必填）。
    #[serde(default)]
    pub body: Vec<BodyParam>,
    /// 该入口仍由手写家族实现：描述存在时本入口不让描述层接管（迁移期降级）。
    #[serde(default)]
    pub family_fallback: bool,
    /// 路径模板。
    #[serde(default)]
    pub path: String,
    /// 完整 URL 模板。
    #[serde(default)]
    pub url: Option<String>,
    /// URL 改写规则。
    #[serde(default)]
    pub rules: Vec<ReqRule>,
    /// 可选路径段。
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// 查询参数。
    #[serde(default)]
    pub params: Vec<Param>,
    /// 行集合（JSON 路径数组，或 CSS 选择器）。
    #[serde(deserialize_with = "one_or_many")]
    pub rows: Vec<String>,
    /// 行字段。
    pub fields: ItemFields,
    /// 行过滤。
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// 页码下限（Kkys `page.max(1)`）。
    #[serde(default)]
    pub page_min: Option<u32>,
}

/// 搜索入口。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchEp {
    /// HTTP 方法（缺省 GET）。
    #[serde(default)]
    pub method: HttpMethod,
    /// POST body（`method != get` 时必填）。
    #[serde(default)]
    pub body: Vec<BodyParam>,
    /// 该入口仍由手写家族实现：描述存在时本入口不让描述层接管（迁移期降级）。
    #[serde(default)]
    pub family_fallback: bool,
    /// 路径模板。
    #[serde(default)]
    pub path: String,
    /// 完整 URL 模板。
    #[serde(default)]
    pub url: Option<String>,
    /// URL 改写规则。
    #[serde(default)]
    pub rules: Vec<ReqRule>,
    /// 可选路径段。
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// 查询参数。
    #[serde(default)]
    pub params: Vec<Param>,
    /// 行集合（JSON 路径数组，或 CSS 选择器）。
    #[serde(deserialize_with = "one_or_many")]
    pub rows: Vec<String>,
    /// 行字段。
    pub fields: ItemFields,
    /// 行过滤。
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// 页面包含该串时视为空结果（Ysj「暂无相关视频」）。
    #[serde(default)]
    pub empty_marker: Option<String>,
    /// 关键词 `trim` 后为空则直接返回空结果、不发请求（Czsapp）。
    #[serde(default)]
    pub trim_keyword: bool,
}

/// 详情入口。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailEp {
    /// HTTP 方法（缺省 GET）。
    #[serde(default)]
    pub method: HttpMethod,
    /// POST body（`method != get` 时必填）。
    #[serde(default)]
    pub body: Vec<BodyParam>,
    /// 该入口仍由手写家族实现：描述存在时本入口不让描述层接管（迁移期降级）。
    #[serde(default)]
    pub family_fallback: bool,
    /// 路径模板。
    #[serde(default)]
    pub path: String,
    /// 完整 URL 模板。
    #[serde(default)]
    pub url: Option<String>,
    /// URL 改写规则。
    #[serde(default)]
    pub rules: Vec<ReqRule>,
    /// 可选路径段。
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// 查询参数。
    #[serde(default)]
    pub params: Vec<Param>,
    /// JSON 根路径（`data` / `data.video`）。
    #[serde(default)]
    pub root: Option<String>,
    /// 字段。
    pub fields: DetailFields,
    /// 简介分段（JSON kind）。
    #[serde(default)]
    pub desc_parts: Vec<DescPart>,
    /// 线路组（JSON 或 HTML 形状，按 `kind` 解析）。
    #[serde(default)]
    pub groups: Option<serde_json::Value>,
    /// HTML 元数据（`li.data` 里的年份/地区/主演/导演）。
    #[serde(default)]
    pub meta: Vec<MetaSpec>,
    /// 元数据遍历的容器选择器（如 `li.data`）。
    #[serde(default)]
    pub meta_scope: Option<String>,
    /// 元数据里的锚点选择器（缺省 `a`）。
    #[serde(default)]
    pub meta_anchor: Option<String>,
    /// 详情 id 为空时的报错。
    #[serde(default)]
    pub empty_id_error: Option<String>,
    /// 先对 id 去首尾空白（Kkys / LiteApple 口径）。
    #[serde(default)]
    pub trim_id: bool,
    /// 抽取完成后的硬性要求（按顺序检查）。
    #[serde(default)]
    pub require: Vec<Requirement>,
}

/// HTML 元数据规则。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaSpec {
    /// 目标字段：`year` / `area` / `director` / `actor`。
    pub field: String,
    /// 锚点 href 包含该串时取锚点文本。
    #[serde(default)]
    pub href_contains: Option<String>,
    /// 扁平文本里的标签前缀（写成 `主演`，匹配 `主演：`）。
    #[serde(default)]
    pub label: Option<String>,
    /// 截断字符集合（如 `"/\n"`）。
    #[serde(default)]
    pub cut: Option<String>,
    /// 已有值时不再覆盖（Ysj 主演锚点口径）。
    #[serde(default)]
    pub first_only: bool,
}

/// 详情硬性要求。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    /// 检查目标。
    pub target: ReqTarget,
    /// 不满足时的报错模板（`{id}` / `{raw}`）。
    pub error: String,
}

/// 要求目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReqTarget {
    /// JSON 根路径必须存在。
    Root,
    /// 标题非空。
    Name,
    /// 至少一组线路。
    Episodes,
    /// 标题与线路不能同时为空（Ysj 口径）。
    NameAndEpisodes,
}

/// 播放入口。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaySpec {
    /// 该入口仍由手写家族实现：描述存在时本入口不让描述层接管（迁移期降级）。
    #[serde(default)]
    pub family_fallback: bool,
    /// id 为空时的报错。
    #[serde(default)]
    pub empty_id_error: Option<String>,
    /// 输出是否带 `"jx": 0`（Kkys 口径）。
    #[serde(default)]
    pub jx: bool,
    /// 默认 `parse` 值。
    #[serde(default)]
    pub parse: u8,
    /// 顺序匹配的分支。
    #[serde(default)]
    pub rules: Vec<PlayRule>,
    /// 都不命中时的代理取直链。
    #[serde(default)]
    pub proxy: Option<ProxySpec>,
    /// 都不命中且无代理时的 URL 模板（缺省 `{id}`）。
    #[serde(default)]
    pub url: Option<String>,
    /// 兜底结果的请求头（`header_mode=rule` 时）。
    #[serde(default)]
    pub header: BTreeMap<String, String>,
    /// 结果 `header` 的来源：`rule`（分支自带）或 `descriptor`（描述请求头，LiteApple）。
    #[serde(default)]
    pub header_mode: HeaderMode,
    /// 结果 `header` 序列化为 JSON 字符串（LiteApple 口径）。
    #[serde(default)]
    pub header_as_json: bool,
}

/// 结果 `header` 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderMode {
    /// 分支自带请求头。
    #[default]
    Rule,
    /// 描述级请求头（动态签名头）。
    Descriptor,
}

/// 播放分支；所有声明条件同时成立才命中。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayRule {
    /// 任一前缀命中。
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// 任一后缀命中。
    #[serde(default)]
    pub suffixes: Vec<String>,
    /// 子串命中。
    #[serde(default)]
    pub contains: Option<String>,
    /// 子串命中（大小写不敏感）。
    #[serde(default)]
    pub contains_ignore_case: Option<String>,
    /// 平台表：id 命中 `contains` 时把 `referer` 注入模板变量 `{platform.referer}`。
    #[serde(default)]
    pub platforms: Vec<Platform>,
    /// 结果 URL 模板（缺省 `{id}`）。
    #[serde(default)]
    pub url: Option<String>,
    /// 覆盖 `parse`。
    #[serde(default)]
    pub parse: Option<u8>,
    /// 覆盖 `jx`。
    #[serde(default)]
    pub jx: Option<bool>,
    /// 该分支的请求头。
    #[serde(default)]
    pub header: BTreeMap<String, String>,
}

/// 平台关键词 → referer。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    /// id 包含该串即命中。
    pub contains: String,
    /// 注入 `{platform.referer}` 的值。
    pub referer: String,
}

/// 代理取直链。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxySpec {
    /// 代理 URL 模板。
    pub url: String,
    /// 从代理响应里取直链的 JSON 路径。
    pub extract: String,
    /// 结果请求头。
    #[serde(default)]
    pub header: BTreeMap<String, String>,
    /// 取不到直链时的报错模板。
    pub error: String,
    /// 代理 URL 为站内相对路径时按基址绝对化。
    #[serde(default)]
    pub join_base: bool,
}

/// 五入口集合。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoints {
    /// 分类。
    pub categories: CatEp,
    /// 列表。
    pub list: ListEp,
    /// 详情。
    pub detail: DetailEp,
    /// 搜索。
    #[serde(default)]
    pub search: Option<SearchEp>,
    /// 播放。
    pub play: PlaySpec,
}

/// JSON 线路组形状（`kind=json`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupsJson {
    /// 组行集合路径（`vod_play[]` / `urls[]`）。
    #[serde(deserialize_with = "one_or_many")]
    pub rows: Vec<String>,
    /// 组名字段。
    #[serde(default)]
    pub name: Option<String>,
    /// 组名为空时的回退模板（`线路{index}`，`{index}` 1 基）。
    #[serde(default)]
    pub name_fallback: Option<String>,
    /// 组内条目集合路径（`url[]`）；缺省表示行本身即条目（LiteApple `urls[]`）。
    #[serde(default)]
    pub items: Option<String>,
    /// 条目标题字段（可为字符串或字段规则）。
    #[serde(default)]
    pub title: Option<FieldSpec>,
    /// 条目 URL 字段（可为字符串或字段规则）。
    #[serde(default)]
    pub url: Option<FieldSpec>,
    /// 标题为空时用 URL 代替（LiteApple 口径）。
    #[serde(default)]
    pub title_fallback_url: bool,
    /// 标题为空时回退 `第{n}集`（`{n}` 为该线路内 1 基序号）。
    #[serde(default)]
    pub title_fallback_episode: bool,
    /// 组名等于 `when` 时给 URL 加前缀（Kkys `jp` 线路）。
    #[serde(default)]
    pub url_prefix_if_group: Option<PrefixIf>,
    /// 单线路：所有行都是条目，线路名固定（LiteApple）。
    #[serde(default)]
    pub single_group: Option<String>,
}

/// 组名条件前缀。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixIf {
    /// 组名（原始值）。
    pub when: String,
    /// 前缀。
    pub prefix: String,
}

/// HTML 线路组形状（`kind=html`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupsHtml {
    /// 线路名选择器集合（与 `lists` 按序配对）。
    pub tabs: String,
    /// 播放列表容器选择器集合。
    pub lists: String,
    /// 单集选择器（相对容器）。
    pub item: String,
    /// URL 属性名（缺省 `href`）。
    #[serde(default)]
    pub url_attr: Option<String>,
    /// URL 去前缀。
    #[serde(default)]
    pub url_strip_prefix: Option<String>,
    /// URL 为站内相对路径时按基址绝对化（`join_url(base, url)`）。
    #[serde(default)]
    pub url_join_base: bool,
    /// 线路名为空时的回退模板（`线路{index}`，`{index}` 1 基）。
    #[serde(default)]
    pub name_fallback: Option<String>,
}

/// 解析描述文本。
pub fn parse(text: &str) -> Result<Descriptor, String> {
    serde_json::from_str(text).map_err(|e| format!("描述 JSON 解析失败: {e}"))
}

/// 把字符串或字符串数组统一读成 `Vec<String>`。
pub fn one_or_many<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let value = OneOrMany::deserialize(d)?;
    let out = match value {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    };
    if out.iter().any(|s| s.trim().is_empty()) {
        return Err(de::Error::custom("字符串列表不能含空串"));
    }
    Ok(out)
}

/// 生成请求段公共字段（避免五处手抄）。
impl Descriptor {
    /// 校验 schema 与 provenance；返回全部问题（空 = 通过）。
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.schema != SCHEMA_VERSION {
            errs.push(format!(
                "schema 必须是 {SCHEMA_VERSION}，实际 {}",
                self.schema
            ));
        }
        if self.api.trim().is_empty() {
            errs.push("api 不能为空".to_string());
        }
        if self.class.trim().is_empty() {
            errs.push("class 不能为空".to_string());
        }
        if self.jar.trim().is_empty() {
            errs.push("jar 不能为空".to_string());
        }
        if !is_sha256(&self.jar_sha256) {
            errs.push(format!("jar_sha256 必须是 64 位小写 hex：{}", self.jar_sha256));
        }
        if self.evidence.is_empty() {
            errs.push("evidence 不能为空".to_string());
        }
        for (i, ev) in self.evidence.iter().enumerate() {
            if ev.method.trim().is_empty() {
                errs.push(format!("evidence[{i}].method 不能为空"));
            }
            if ev.file.trim().is_empty() {
                errs.push(format!("evidence[{i}].file 不能为空"));
            }
        }
        if !self.base.default.starts_with("http") {
            errs.push(format!("base.default 必须是 http(s) 地址：{}", self.base.default));
        }
        if self.headers.keys().any(|k| k.trim().is_empty()) {
            errs.push("headers 不能有空键".to_string());
        }
        for v in &self.vars {
            if v.name.trim().is_empty() {
                errs.push("vars.name 不能为空".to_string());
            }
            if v.apply.is_some() && v.literal.is_some() {
                errs.push(format!("变量 {} 不能同时声明 apply 与 literal", v.name));
            }
        }
        let e = &self.endpoints;
        let static_cats = e.categories.static_categories.as_deref();
        match static_cats {
            Some(list) => {
                if list.is_empty() {
                    errs.push("endpoints.categories.static_categories 不能为空数组".to_string());
                }
                for (i, cat) in list.iter().enumerate() {
                    if cat.id.trim().is_empty() {
                        errs.push(format!("static_categories[{i}].id 不能为空"));
                    }
                    if cat.name.trim().is_empty() {
                        errs.push(format!("static_categories[{i}].name 不能为空"));
                    }
                }
            }
            None => check_req(
                "categories",
                &e.categories.path,
                e.categories.url.is_some(),
                &mut errs,
            ),
        }
        for (name, method, body) in [
            ("categories", e.categories.method, e.categories.body.as_slice()),
            ("list", e.list.method, e.list.body.as_slice()),
            ("detail", e.detail.method, e.detail.body.as_slice()),
        ] {
            check_body(name, method, body, &mut errs);
        }
        if let Some(s) = &e.search {
            check_body("search", s.method, &s.body, &mut errs);
        }
        check_req("list", &e.list.path, e.list.url.is_some(), &mut errs);
        check_req("detail", &e.detail.path, e.detail.url.is_some(), &mut errs);
        if let Some(s) = &e.search {
            check_req("search", &s.path, s.url.is_some(), &mut errs);
        }
        if static_cats.is_none() && e.categories.rows.is_empty() {
            errs.push("endpoints.categories.rows 不能为空".to_string());
        }
        if e.list.rows.is_empty() {
            errs.push("endpoints.list.rows 不能为空".to_string());
        }
        if let Some(s) = &e.search {
            if s.rows.is_empty() {
                errs.push("endpoints.search.rows 不能为空".to_string());
            }
        }
        let mut all: Vec<(&str, &FieldSpec)> = Vec::new();
        if static_cats.is_none() {
            all.push(("categories.fields.id", &e.categories.fields.id));
            all.push(("categories.fields.name", &e.categories.fields.name));
        }
        all.push(("list.fields.id", &e.list.fields.id));
        all.push(("list.fields.name", &e.list.fields.name));
        all.push(("detail.fields.name", &e.detail.fields.name));
        if let Some(f) = &e.list.fields.cover {
            all.push(("list.fields.cover", f));
        }
        if let Some(f) = &e.list.fields.remark {
            all.push(("list.fields.remark", f));
        }
        if let Some(s) = &e.search {
            all.push(("search.fields.id", &s.fields.id));
            all.push(("search.fields.name", &s.fields.name));
            if let Some(f) = &s.fields.cover {
                all.push(("search.fields.cover", f));
            }
            if let Some(f) = &s.fields.remark {
                all.push(("search.fields.remark", f));
            }
        }
        for &(name, f) in &all {
            validate_field(self.kind, name, f, &mut errs);
        }
        if let Some(f) = &e.detail.fields.cover {
            validate_field(self.kind, "detail.fields.cover", f, &mut errs);
        }
        for (name, f) in [
            ("detail.fields.desc", &e.detail.fields.desc),
            ("detail.fields.year", &e.detail.fields.year),
            ("detail.fields.area", &e.detail.fields.area),
            ("detail.fields.director", &e.detail.fields.director),
            ("detail.fields.actor", &e.detail.fields.actor),
        ] {
            if let Some(f) = f {
                validate_field(self.kind, name, f, &mut errs);
            }
        }
        match self.kind {
            Kind::Json => {
                if !e.detail.meta.is_empty() {
                    errs.push("kind=json 不支持 detail.meta".to_string());
                }
                if !e.detail.desc_parts.is_empty() {
                    for (i, p) in e.detail.desc_parts.iter().enumerate() {
                        if p.from.trim().is_empty() {
                            errs.push(format!("desc_parts[{i}].from 不能为空"));
                        }
                    }
                }
                if let Some(g) = &e.detail.groups {
                    match parse_groups_json(g) {
                        Ok(g) => validate_groups_json(&g, &mut errs),
                        Err(err) => errs.push(format!("detail.groups: {err}")),
                    }
                }
            }
            Kind::Html => {
                if !e.detail.desc_parts.is_empty() {
                    errs.push("kind=html 不支持 detail.desc_parts（用 fields.desc）".to_string());
                }
                if !e.detail.meta.is_empty() && e.detail.meta_scope.is_none() {
                    errs.push("detail.meta 需要 detail.meta_scope".to_string());
                }
                for (i, m) in e.detail.meta.iter().enumerate() {
                    if !matches!(m.field.as_str(), "year" | "area" | "director" | "actor") {
                        errs.push(format!("detail.meta[{i}].field 非法：{}", m.field));
                    }
                    if m.href_contains.is_none() && m.label.is_none() {
                        errs.push(format!("detail.meta[{i}] 需要 href_contains 或 label"));
                    }
                }
                if let Some(g) = &e.detail.groups {
                    match parse_groups_html(g) {
                        Ok(_) => {}
                        Err(err) => errs.push(format!("detail.groups: {err}")),
                    }
                }
            }
        }
        if e.play.rules.is_empty() && e.play.proxy.is_none() && e.play.url.is_none() {
            errs.push("play 需要 rules / proxy / url 至少一项".to_string());
        }
        for (i, r) in e.play.rules.iter().enumerate() {
            if r.prefixes.is_empty()
                && r.suffixes.is_empty()
                && r.contains.is_none()
                && r.contains_ignore_case.is_none()
                && r.platforms.is_empty()
            {
                errs.push(format!("play.rules[{i}] 没有任何条件"));
            }
        }
        for (i, r) in e.play.rules.iter().enumerate() {
            for (j, p) in r.platforms.iter().enumerate() {
                if p.contains.is_empty() {
                    errs.push(format!("play.rules[{i}].platforms[{j}].contains 不能为空"));
                }
            }
        }
        errs
    }
}

/// 校验请求段至少有一种 URL 来源。
fn check_req(name: &str, path: &str, has_url: bool, errs: &mut Vec<String>) {
    if !has_url && path.trim().is_empty() {
        errs.push(format!("endpoints.{name} 缺少 path 或 url"));
    }
}

/// 校验单个字段规则是否与 `kind` 匹配。
/// POST 端点必须有 body 且字段名非空。
fn check_body(name: &str, method: HttpMethod, body: &[BodyParam], errs: &mut Vec<String>) {
    if method != HttpMethod::Get && body.is_empty() {
        errs.push(format!("endpoints.{name}.body 在 POST 方法下不能为空"));
    }
    for (i, field) in body.iter().enumerate() {
        if field.key.trim().is_empty() {
            errs.push(format!("endpoints.{name}.body[{i}].key 不能为空"));
        }
    }
}

fn validate_field(kind: Kind, name: &str, f: &FieldSpec, errs: &mut Vec<String>) {
    if !f.has_source() {
        errs.push(format!("{name} 缺少取值来源（path 或 sel/self）"));
    }
    match kind {
        Kind::Json => {
            if f.path.is_none() {
                errs.push(format!("{name} 在 kind=json 下必须有 path"));
            }
            if !f.sel.is_empty() || f.self_el.unwrap_or(false) || f.attr.is_some() {
                errs.push(format!("{name} 在 kind=json 下不能使用 sel/self/attr"));
            }
        }
        Kind::Html => {
            if f.path.is_some() {
                errs.push(format!("{name} 在 kind=html 下不能使用 path"));
            }
            if !f.sel.is_empty() && f.self_el.unwrap_or(false) {
                errs.push(format!("{name} 不能同时声明 sel 与 self"));
            }
            if f.attr.is_some() && f.sel.is_empty() && !f.self_el.unwrap_or(false) {
                errs.push(format!("{name} 使用 attr 时必须声明 sel 或 self"));
            }
        }
    }
    if let Some(fb) = &f.fallback {
        validate_field(kind, &format!("{name}.fallback"), fb, errs);
    }
}

/// 解析 JSON 线路组（严格）。
pub fn parse_groups_json(value: &serde_json::Value) -> Result<GroupsJson, String> {
    serde_json::from_value(value.clone()).map_err(|e| format!("JSON 线路组不合法: {e}"))
}

/// 解析 HTML 线路组（严格）。
pub fn parse_groups_html(value: &serde_json::Value) -> Result<GroupsHtml, String> {
    serde_json::from_value(value.clone()).map_err(|e| format!("HTML 线路组不合法: {e}"))
}

fn validate_groups_json(g: &GroupsJson, errs: &mut Vec<String>) {
    if g.single_group.is_some() {
        if g.items.is_some() {
            errs.push("groups.single_group 时不能再声明 items".to_string());
        }
        if g.url.is_none() {
            errs.push("groups 缺少 url 字段".to_string());
        }
        return;
    }
    if g.items.is_none() {
        errs.push("groups 缺少 items".to_string());
    }
    if g.url.is_none() {
        errs.push("groups 缺少 url".to_string());
    }
}

pub(crate) fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_or_many_reads_string_and_array() {
        #[derive(Deserialize)]
        struct S {
            #[serde(deserialize_with = "one_or_many")]
            rows: Vec<String>,
        }
        let a: S = serde_json::from_str(r#"{"rows":"data[]"}"#).unwrap();
        assert_eq!(a.rows, vec!["data[]"]);
        let b: S = serde_json::from_str(r#"{"rows":["data.list[]","list[]"]}"#).unwrap();
        assert_eq!(b.rows.len(), 2);
        assert!(serde_json::from_str::<S>(r#"{"rows":[""]}"#).is_err());
    }

    #[test]
    fn sha256_validation() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"A".repeat(64)));
        assert!(!is_sha256("abc"));
    }
}
