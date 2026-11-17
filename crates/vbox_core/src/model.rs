//! TVBox 核心数据模型（对齐 jun 版 TVBoxOS 的真实配置形态）
//!
//! 本模型兼容 TVBox 生态常见的"脏字段"：
//! - 布尔位置 0/1、true/false、字符串 "1" 混用
//! - `type` 为数字或字符串数字
//! - `ext` 为 JSON 对象 / 数组 / 字符串
//! - `urls` 为纯字符串或 `{name,url}` 对象
//!
//! 与旧版模型相比，本版补齐了 jun 版真实配置里的 `jar / playUrl / playerType /
//! timeout / click / style / flags / hosts / proxy / rules / doh / ads` 等字段。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/* ============================================================
 * 顶层配置
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TvConfig {
    /// 顶层 spider jar 地址（可带 `;md5;hash`，可 `img+` 包装）
    #[serde(default)]
    pub spider: Option<String>,
    /// TVBox `jarCache`：缺省 true；字符串 "false" 也兼容
    #[serde(default = "default_true", deserialize_with = "flexible_bool")]
    pub jar_cache: bool,
    #[serde(default)]
    pub wallpaper: String,
    #[serde(default)]
    pub danmaku: String,

    #[serde(default, deserialize_with = "lenient_sites")]
    pub sites: Vec<Site>,
    #[serde(default, deserialize_with = "lenient_lives")]
    pub lives: Vec<LiveSource>,
    #[serde(default, deserialize_with = "lenient_parsers")]
    pub parses: Vec<Parser>,

    /// VIP 解析白名单 flag（`flags`）
    #[serde(default)]
    pub flags: Vec<String>,
    /// 自定义 DNS 映射：`["host=ip", ...]`
    #[serde(default)]
    pub hosts: Vec<String>,
    /// 上游 HTTP/SOCKS 代理规则
    #[serde(default)]
    pub proxy: Vec<ProxyRule>,
    /// 嗅探 / 广告 / 点击脚本规则（jun 的 `rules` 数组）
    #[serde(default)]
    pub rules: Vec<TvRule>,
    #[serde(default)]
    pub doh: Vec<Doh>,
    /// 追加广告域名
    #[serde(default)]
    pub ads: Vec<String>,

    /// 多仓 / 线路合集地址（仅当顶层没有 `sites` 时作为"线路列表"使用）
    #[serde(default)]
    pub urls: Vec<WarehouseUrl>,

    /// HTTP 客户端规则（不是 TVBox 配置字段，由 VBox 自身维护）
    #[serde(skip)]
    pub http: Rules,
}

impl TvConfig {
    pub fn empty() -> Self {
        Self::default()
    }

    /// 顶层 `sites` 是否为空（用于判断"线路合集"）
    pub fn has_sites(&self) -> bool {
        !self.sites.is_empty()
    }
}

/* ============================================================
 * 容错反序列化：真实 TVBox 配置里经常有缺 key/type/api 的脏站点、
 * 缺 name/url 的解析器；TVBoxOS 是"跳过该条"而不是整份配置失败。
 * ============================================================ */

pub fn lenient_sites<'de, D>(deserializer: D) -> Result<Vec<Site>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<Value>::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(raw.len());
    for (idx, value) in raw.into_iter().enumerate() {
        let Some(obj) = value.as_object() else {
            tracing::warn!("跳过 sites[{idx}]：不是 JSON 对象");
            continue;
        };
        // 与 ApiConfig.parseJson 一致：缺 key/type/api 的站点直接跳过
        if !obj.contains_key("key") || !obj.contains_key("type") || !obj.contains_key("api") {
            tracing::warn!("跳过 sites[{idx}]：缺少 key/type/api 之一");
            continue;
        }
        match serde_json::from_value::<Site>(value) {
            Ok(site) => out.push(site),
            Err(e) => tracing::warn!("跳过 sites[{idx}]：{e}"),
        }
    }
    Ok(out)
}

pub fn lenient_parsers<'de, D>(deserializer: D) -> Result<Vec<Parser>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<Value>::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(raw.len());
    for (idx, value) in raw.into_iter().enumerate() {
        let Some(obj) = value.as_object() else {
            tracing::warn!("跳过 parses[{idx}]：不是 JSON 对象");
            continue;
        };
        if !obj.contains_key("name") || !obj.contains_key("url") {
            tracing::warn!("跳过 parses[{idx}]：缺少 name/url");
            continue;
        }
        match serde_json::from_value::<Parser>(value) {
            Ok(parser) => out.push(parser),
            Err(e) => tracing::warn!("跳过 parses[{idx}]：{e}"),
        }
    }
    Ok(out)
}

pub fn lenient_lives<'de, D>(deserializer: D) -> Result<Vec<LiveSource>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<Value>::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(raw.len());
    for (idx, value) in raw.into_iter().enumerate() {
        if !value.is_object() {
            tracing::warn!("跳过 lives[{idx}]：不是 JSON 对象");
            continue;
        }
        match serde_json::from_value::<LiveSource>(value) {
            Ok(live) => out.push(live),
            Err(e) => tracing::warn!("跳过 lives[{idx}]：{e}"),
        }
    }
    Ok(out)
}

/* ============================================================
 * 站点
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub key: String,
    #[serde(default)]
    pub name: String,
    /// 0=XML、1=苹果CMS JSON、3=Spider、4=HTTP 远程接口、-1=内置推送
    #[serde(rename = "type", deserialize_with = "flexible_i32")]
    pub kind: i32,
    pub api: String,

    #[serde(default = "default_true", deserialize_with = "flexible_bool")]
    pub searchable: bool,
    #[serde(default = "default_true", deserialize_with = "flexible_bool")]
    pub quick_search: bool,
    #[serde(default = "default_true", deserialize_with = "flexible_bool")]
    pub changeable: bool,
    /// jun 里实际是死字段，保留以兼容配置
    #[serde(default = "default_true", deserialize_with = "flexible_bool")]
    pub filterable: bool,

    /// type0/1 的播放页前缀（`playUrl`）
    #[serde(default)]
    pub player_url: String,
    /// 站点扩展参数（对象/数组/字符串均可）
    #[serde(default)]
    pub ext: Option<Value>,
    /// 站点级 jar（http / assets:// / file:// / clan://）
    #[serde(default)]
    pub jar: String,
    /// 1=IJK、2=Exo、10=外部播放器、-1=跟随全局
    #[serde(default = "default_player_type", deserialize_with = "flexible_i32")]
    pub player_type: i32,
    /// 站点级播放信息超时（秒），0=默认 15
    #[serde(default, deserialize_with = "flexible_i32")]
    pub timeout: i32,
    /// 嗅探自动点击脚本 `host;js`
    #[serde(default)]
    pub click: String,
    /// 首页展示风格（TVBox 实际可能是对象，例如 {"type":"rect","ratio":1.68}；
    /// 统一序列化为 JSON 字符串，保持与 SourceBean.safeJsonString 一致）
    #[serde(default, deserialize_with = "flexible_json_string")]
    pub style: String,
    #[serde(default)]
    pub categories: Vec<Category>,
}

impl Site {
    pub fn searchable(&self) -> bool {
        self.searchable
    }

    pub fn play_timeout_secs(&self) -> u64 {
        if self.timeout > 0 {
            self.timeout.clamp(5, 60) as u64
        } else {
            15
        }
    }

    /// ext 的字符串形态（jar/py spider 的 `init(ctx, ext)` 入参）
    pub fn ext_string(&self) -> String {
        match &self.ext {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
        }
    }

    /// ext 的 JSON 对象形态（读取 header/referer/cookie 等）
    pub fn ext_object(&self) -> Option<serde_json::Map<String, Value>> {
        self.ext.as_ref().and_then(|v| v.as_object()).cloned()
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Category {
    pub type_id: String,
    pub type_name: String,
}

/// TVBox 的 categories 两种写法：
/// - 字符串数组：`["电影","电视剧"]` → type_id/type_name 都是该串
/// - 对象数组：`[{"type_id":"1","type_name":"电影"}]`
impl<'de> Deserialize<'de> for Category {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Str(String),
            Obj {
                #[serde(default)]
                type_id: Option<String>,
                #[serde(default)]
                type_name: Option<String>,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Str(s) => Ok(Category {
                type_id: s.clone(),
                type_name: s,
            }),
            Raw::Obj { type_id, type_name } => {
                let id = type_id.unwrap_or_default();
                let name = type_name.unwrap_or_else(|| id.clone());
                Ok(Category {
                    type_id: id,
                    type_name: name,
                })
            }
        }
    }
}

impl Category {
    pub fn new(type_id: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self {
            type_id: type_id.into(),
            type_name: type_name.into(),
        }
    }
}

/* ============================================================
 * 多仓 / 线路
 * ============================================================ */

/// `urls` 里的一项：字符串或 `{name,url}` 对象
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct WarehouseUrl {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
}

impl<'de> Deserialize<'de> for WarehouseUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Str(String),
            Obj {
                url: String,
                #[serde(default)]
                name: Option<String>,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Str(s) => Ok(WarehouseUrl { url: s, name: None }),
            Raw::Obj { url, name } => Ok(WarehouseUrl { url, name }),
        }
    }
}

/* ============================================================
 * 解析器
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Parser {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", deserialize_with = "flexible_i32")]
    pub kind: i32,
    #[serde(default)]
    pub url: String,
    /// `flag` 白名单 + `header`
    #[serde(default)]
    pub ext: Option<Value>,
}

impl Parser {
    pub fn ext_string(&self) -> String {
        match &self.ext {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
        }
    }

    pub fn header_map(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let Some(obj) = self.ext.as_ref().and_then(|v| v.as_object()) else {
            return out;
        };
        for key in ["header", "headers"] {
            if let Some(h) = obj.get(key).and_then(|v| v.as_object()) {
                for (k, v) in h {
                    if let Some(s) = v.as_str() {
                        out.insert(k.clone(), s.to_string());
                    }
                }
            }
        }
        out
    }

    pub fn flags(&self) -> Vec<String> {
        self.ext
            .as_ref()
            .and_then(|v| v.get("flag"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 解析器 URL；`proxy://` 交由上层替换为本地代理地址
    pub fn url_raw(&self) -> &str {
        self.url.trim()
    }
}

/* ============================================================
 * 直播
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveSource {
    #[serde(default)]
    pub name: String,
    /// 配置级直播分组：`lives: [{ "group": "央视", "channels": [...] }]`
    #[serde(default)]
    pub group: Option<String>,
    /// 配置级频道数组；存在时直接作为直播分组解析（而不是远程订阅地址）
    #[serde(default)]
    pub channels: Vec<Value>,
    #[serde(rename = "type", default, deserialize_with = "flexible_i32")]
    pub kind: i32,
    /// type0 的订阅地址
    #[serde(default)]
    pub url: String,
    /// type3 的 spider api（.py/.js/csp_）
    #[serde(default)]
    pub api: String,
    /// spider ext（对象/数组/字符串）
    #[serde(default)]
    pub ext: Option<Value>,
    #[serde(default)]
    pub jar: String,
    #[serde(default)]
    pub epg: Option<String>,
    #[serde(default)]
    pub ua: String,
    /// 组级请求头
    #[serde(default)]
    pub header: Option<Value>,
    #[serde(default, deserialize_with = "flexible_i32")]
    pub player_type: i32,
    #[serde(default, deserialize_with = "flexible_i32")]
    pub timeout: i32,
    #[serde(default)]
    pub logo: String,
}

impl LiveSource {
    /// type0/3 取真实订阅地址（`url` 优先，其次 `api`）
    pub fn address(&self) -> &str {
        if !self.url.trim().is_empty() {
            self.url.trim()
        } else {
            self.api.trim()
        }
    }

    pub fn ext_string(&self) -> String {
        match &self.ext {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
        }
    }
}

/* ============================================================
 * 规则 / 代理 / DoH
 * ============================================================ */

/// jun 的 `rules[]`：嗅探、广告、点击脚本
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TvRule {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub rule: Vec<String>,
    #[serde(default)]
    pub filter: Vec<String>,
    /// 广告正则（hosts+regex）
    #[serde(default)]
    pub regex: Vec<String>,
    /// 点击脚本（hosts+script）
    #[serde(default)]
    pub script: Vec<String>,
}

impl TvRule {
    pub fn host_list(&self) -> Vec<String> {
        if let Some(h) = &self.host {
            if !h.trim().is_empty() {
                return vec![h.clone()];
            }
        }
        self.hosts.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyRule {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Doh {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
}

/* ============================================================
 * HTTP 客户端规则（VBox 自身）
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default = "default_retry")]
    pub retry: u32,
    #[serde(default = "default_ua")]
    pub ua: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            retry: default_retry(),
            ua: default_ua(),
            headers: HashMap::new(),
        }
    }
}

/* ============================================================
 * 点播内容
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub remark: Option<String>,
}

/// 搜索命中：带上来源站点，前端才能正确进入详情
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub site_key: String,
    pub site_name: String,
    #[serde(flatten)]
    pub item: VideoItem,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoDetail {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub director: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub episodes: Vec<EpisodeGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeGroup {
    pub group: String,
    #[serde(default)]
    pub items: Vec<Episode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub title: String,
    pub url: String,
}

/* ============================================================
 * 播放解析结果
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayResult {
    /// 最终直链，或需要 WebView 嗅探的播放页 URL（sniff=true）
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub subtitles: Vec<Subtitle>,
    /// 当前地址是否还需要 WebView 嗅探才能得到真实直链
    #[serde(default)]
    pub sniff: bool,
    /// 命中的解析器名称
    #[serde(default)]
    pub parse_name: Option<String>,
    /// 解析来源（超解析竞速时用于提示）
    #[serde(default)]
    pub jx_from: Option<String>,
    /// 解析过程（前端调试 / 「抓包」面板展示）
    #[serde(default)]
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Subtitle {
    pub lang: String,
    pub url: String,
}

/* ============================================================
 * 默认值 / 容错反序列化
 * ============================================================ */

fn default_true() -> bool {
    true
}

fn default_player_type() -> i32 {
    -1
}

fn default_timeout() -> u64 {
    6000
}

fn default_retry() -> u32 {
    2
}

fn default_ua() -> String {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36".into()
}

/// 兼容 bool / 0-1 / "true" / "1" / "false" 的布尔反序列化
pub fn flexible_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Int(i64),
        Str(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Bool(b) => Ok(b),
        Raw::Int(i) => Ok(i != 0),
        Raw::Str(s) => Ok(matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        )),
    }
}

/// 兼容 "1"/1 的数字反序列化（type 字段常出现字符串）
pub fn flexible_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Str(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Int(i) => Ok(i as i32),
        Raw::Str(s) => Ok(s.trim().parse::<i32>().unwrap_or_default()),
    }
}

/// 兼容"字符串或任意 JSON 值"的字符串字段（TVBox 的 style/click 等可能写成对象）
pub fn flexible_json_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Str(String),
        Json(Value),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Str(s) => Ok(s),
        Raw::Json(v) => Ok(v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_site_with_messy_fields() {
        let raw = r#"{
            "sites": [{
                "key": "a", "name": "A", "type": "3", "api": "csp_X",
                "searchable": "0", "quickSearch": 1,
                "ext": {"site": "x"}, "jar": "http://j", "timeout": "20"
            }],
            "lives": [{"name":"L","api":"http://l","ext":"{}"}],
            "parses": [{"name":"P","type":2,"url":"X","ext":{"header":{"UA":"u"}}}],
            "urls": ["http://a", {"name":"B","url":"http://b"}]
        }"#;
        let cfg: TvConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.sites[0].kind, 3);
        assert!(!cfg.sites[0].searchable);
        assert_eq!(cfg.sites[0].timeout, 20);
        assert_eq!(cfg.urls.len(), 2);
        assert_eq!(cfg.urls[1].name.as_deref(), Some("B"));
        assert_eq!(cfg.parses[0].header_map().get("UA").unwrap(), "u");
        assert!(cfg.jar_cache);
    }

    #[test]
    fn lenient_sites_skip_incomplete_entries() {
        // 真实配置常见：某个 site 缺 type/api；TVBoxOS 跳过该条而不是整份失败
        let raw = r#"{
            "sites": [
                {"key":"bad-no-type","name":"坏","api":"http://bad"},
                {"key":"ok","name":"好","type":1,"api":"http://ok"},
                {"key":"bad-no-api","name":"坏2","type":1}
            ],
            "parses": [
                {"name":"缺 url","type":1},
                {"name":"好解析","type":1,"url":"http://jx"}
            ]
        }"#;
        let cfg: TvConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.sites.len(), 1);
        assert_eq!(cfg.sites[0].key, "ok");
        assert_eq!(cfg.parses.len(), 1);
        assert_eq!(cfg.parses[0].name, "好解析");
    }

    #[test]
    fn parse_real_world_messy_config() {
        // 取自真实订阅里常见的写法：style 是对象、categories 是字符串数组、
        // lives 是内嵌分组（group/channels）而不是 url。
        let raw = r#"{
            "sites": [{
                "key": " 在线┃直播1",
                "name": "📺┃竞技┃直播",
                "type": 3,
                "api": "csp_X",
                "categories": ["电影", "电视剧"],
                "style": {"type": "rect", "ratio": 1.68},
                "header": {"User-Agent": "x"},
                "gridview": "0-0-H"
            }],
            "lives": [{
                "group": "redirect",
                "channels": [{"name":"live","urls":["proxy://do=live&type=txt&ext=aHR0cDovL2EvYi50eHQ="]}]
            }]
        }"#;
        let cfg: TvConfig = serde_json::from_str(raw).unwrap();
        let site = &cfg.sites[0];
        assert_eq!(site.key, " 在线┃直播1");
        assert_eq!(site.categories.len(), 2);
        assert_eq!(site.categories[0].type_id, "电影");
        assert!(site.style.contains("\"type\":\"rect\""));
        assert_eq!(cfg.lives[0].group.as_deref(), Some("redirect"));
        assert_eq!(cfg.lives[0].channels.len(), 1);
    }
}
