//! `csp_XBiu`（星标 / 自动规则推断蜘蛛引擎）原生实现 —— 第 152 轮 W2。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/tmp/r150/cfr3/com/github/catvod/spider/XBiu.java`（dex2jar + CFR 0.152，
//!   2757 行；`D(String)` 一处失败）
//! * `_rea/decompiled/88c55af0-fan/deobf/com/github/catvod/spider/XBiu.java`
//!   （jadx：`D(String)` / `guess_rule_vod_id` / `guess_value_*` 可读）
//! * W1 通用层：`crates/vbox_core/src/spider/catvod/xbiu_rule.rs`（446 行 + 16 单测）
//!
//! 本轮（W2）覆盖：规则加载与缓存、`homeContent`（分类推断 + 首页预览）、
//! `categoryContent`（列表块推断）。W3 覆盖详情/播放/搜索，W4 接线与验收。
//!
//! 协议要点（原类逐段还原）：
//! * ext 三态：内联规则 JSON / 规则文档地址（http 且无 `{cateId}`/`{catePg}`）/
//!   站点地址（http 且含占位符 → 规则为空、`homeUrl` 即 ext）。
//! * `homeContent`：`check_rule_cateManual` 从首页 HTML 推断分类（`D`：先 `WH` 取
//!   「分类条」父节点，再用锚点正则 + 前缀/后缀差异抽出 名称/ID）；推断结果写回规则
//!   `cateManual`；`bl=true` 且规则有 `filter` 时一并返回；随后用列表规则抓首页第一页
//!   生成预览列表。
//! * `categoryContent`：`H()` 拼 URL（`{cateId}`/`{catePg}`/筛选占位符替换，残留
//!   `{xxx}` 连同 `/xxx/` 一并删除）→ `tX` 抓页（去 HTML 注释、btwaf 反爬重试）→
//!   `categoryContentImpl`（规则有 `region` 先截区域；`lookback` 数组描述重复块：
//!   `[0]` 块内标记、`[4]` 向上查找层数；逐块用 `vod_id/vod_name/vod_pic/vod_remarks`
//!   切片提取，缺失时走 `guess_value_*` 兜底；`vod_id` 整条 JSON 走 URL-Safe Base64）。
//!
//! 有意偏差（记录于附录 FB）：
//! 1. 分类顺序：原类 Android `JSONObject` 为插入序，这里 `Vec<(String,String)>` 保序，
//!    输出与首页出现顺序一致；
//! 2. HTML 下标按 UTF-8 字节（W1 已记录），XBiu 目标页标签基本 ASCII；
//! 3. `btwaf` 反爬按「取 `btwaf=` 值重试一次 + 收集 Set-Cookie」实现，缺失该规则时不触发；
//! 4. 分类推断中 `SpiderDebug.log` 仅保留为 `tracing::debug!`，不参与逻辑；
//! 5. 上游 8 站当前全部不可达（见附录 FA 探测表），本家族以 mock 夹具验收。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::URL_SAFE;
use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};
use crate::spider::catvod::xbiu_rule::{
    add_http_prefix, ext_kind, find_block_pos, find_sub_string, find_up_nodes, get_lookback_array,
    get_region, load_rule, node_string, sub_string_count, trim_html_string, ExtKind, RuleDoc,
};

/// 站点 api 名。
pub const API: &str = "csp_XBiu";

/// 分类关键词（原类字段 `B`：中/繁中/English 混合）。
const CATEGORY_WORDS: [&str; 10] = ["电影", "剧集", "电视剧", "连续剧", "综艺", "动漫", "電視劇", "電影", "綜藝", "動漫"];

/// 排除词（原类字段 `WH`）。
const EXCLUDE_WORDS: [&str; 16] = [
    "更多", "下载", "首页", "资讯", "留言", "导航", "专题", "短视频", "热榜", "排行", "追剧", "更新", "APP",
    "直播", "label", "Netflix",
];

/// 首页 HTML 推断时的截断长度（原类 32 KiB）。
const HOME_HTML_LIMIT: usize = 32768;

/// 分类推断扫描的锚点上限（原类 30）。
const ANCHOR_LIMIT: usize = 30;

/// 列表每页上限（原类 `limit=90`）。
const PAGE_LIMIT: usize = 90;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// 规则 + 推断结果的进程内缓存（原类静态字段 `a`）。
#[derive(Debug, Clone)]
struct Ctx {
    doc: RuleDoc,
    cate_manual: Vec<(String, String)>,
}

fn cache() -> &'static Mutex<HashMap<String, Ctx>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Ctx>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 是否接管该 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, msg: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, msg).into_core()
}

/// 取规则文档（缓存命中即返回克隆）。
fn ctx_for(cfg: &TvConfig, site: &Site) -> Result<Ctx> {
    let ext = site.ext_string();
    let ext = ext.trim();
    let key = format!("{}|{}", site.key, ext);
    if let Some(ctx) = cache().lock().ok().and_then(|guard| guard.get(&key).cloned()) {
        return Ok(ctx);
    }
    if ext.is_empty() {
        return Err(parse_err(site, "ext 为空：XBiu 需要规则 JSON 或站点地址"));
    }
    let kind = ext_kind(ext);
    let fetched = match kind {
        ExtKind::RuleUrl => Some(fetch_rule_text(cfg, site, ext)?),
        _ => None,
    };
    let doc = load_rule(ext, fetched.as_deref())
        .map_err(|err| parse_err(site, format!("XBiu 规则归一化失败：{err}")))?;
    let mut cate_manual: Vec<(String, String)> = Vec::new();
    if let Some(map) = doc.rule.get("cateManual").and_then(|value| value.as_object()) {
        cate_manual = map
            .iter()
            .map(|(name, id)| {
                let id = id
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| id.to_string());
                (name.clone(), id)
            })
            .collect();
        cate_manual.sort_by_key(|(_, id)| id.parse::<i64>().unwrap_or(i64::MAX));
    }
    let ctx = Ctx { doc, cate_manual };
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, ctx.clone());
    }
    Ok(ctx)
}

/// 抓规则文档文本（`ExtKind::RuleUrl`）。
fn fetch_rule_text(cfg: &TvConfig, site: &Site, ext: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let headers = default_headers();
    http::get_text(&client, ext, &headers, cfg.http.retry)
        .map_err(|err| parse_err(site, format!("规则文档拉取失败：{err}")))
}

/// 默认请求头（原类 `B()`：规则 `header` 优先，缺 User-Agent 时补桌面 UA）。
fn default_headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers
}

/// 规则 `header` 对象 + 默认 UA。
fn headers_for(rule: &Value) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    if let Some(obj) = rule.get("header").and_then(|v| v.as_object()) {
        for (key, value) in obj {
            if let Some(text) = value.as_str() {
                headers.insert(key.clone(), text.to_string());
            }
        }
    }
    if !headers.keys().any(|key| key.eq_ignore_ascii_case("user-agent")) {
        headers.insert("User-Agent".to_string(), UA.to_string());
    }
    headers
}

/// 去掉 HTML 注释（原类 `tX` 的 `replaceAll("<!--.+?-->", "")`；`.` 不跨行）。
fn strip_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        match after.find("-->") {
            Some(end) => rest = &after[end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 抓页面文本（`tX`）：GET → 去注释 → btwaf 反爬重试。
fn get_page(cfg: &TvConfig, site: &Site, ctx: &Ctx, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let headers = headers_for(&ctx.doc.rule);
    let text = http::get_text(&client, url, &headers, cfg.http.retry)
        .map_err(|err| parse_err(site, format!("页面拉取失败 {url}：{err}")))?;
    let text = strip_comments(&text);
    maybe_retry_btwaf(cfg, site, ctx, url, text, &headers)
}

/// `btwaf` 反爬（原类 `h()`）：检测「检测中/跳转中/btwaf」，提取 `btwaf=` 值重试并收集
/// Set-Cookie 写回规则 `header.cookie`；只重试一次。
fn maybe_retry_btwaf(
    cfg: &TvConfig,
    site: &Site,
    ctx: &Ctx,
    url: &str,
    text: String,
    headers: &HashMap<String, String>,
) -> Result<String> {
    let enabled = ctx.doc.rule.get("btwaf").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled
        || !text.contains("检测中")
        || !text.contains("跳转中")
        || !text.contains("btwaf")
    {
        return Ok(text);
    }
    let token = find_sub_string(&text, 0, Some(&json!(["btwaf=", "\""])), "");
    if token.is_empty() {
        return Ok(text);
    }
    let retry_url = format!("{url}?btwaf={token}");
    let client = crate::http::client_for(&cfg.http)?;
    let mut request = client.get(&retry_url);
    for (key, value) in headers {
        request = request.header(key.as_str(), value.as_str());
    }
    let response = match request.send() {
        Ok(response) => response,
        Err(err) => return Err(parse_err(site, format!("btwaf 重试失败：{err}"))),
    };
    let cookies: Vec<String> = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or("").trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    let body = response.text().unwrap_or_default();
    if !cookies.is_empty() {
        if let Ok(mut guard) = cache().lock() {
            if let Some(entry) = guard.get_mut(&format!("{}|{}", site.key, site.ext_string().trim())) {
                if let Some(Value::Object(map)) = entry.doc.rule.get_mut("header") {
                    map.insert("cookie".to_string(), Value::String(cookies.join(";")));
                } else if let Some(rule) = entry.doc.rule.as_object_mut() {
                    let mut header = Map::new();
                    header.insert("cookie".to_string(), Value::String(cookies.join(";")));
                    rule.insert("header".to_string(), Value::Object(header));
                }
            }
        }
    }
    Ok(strip_comments(&body))
}

/// 把整条 JSON 用 Android `Base64.encodeToString(bytes, NO_WRAP|URL_SAFE)` 编码。
fn encode_vod_id(value: &Value) -> String {
    URL_SAFE.encode(value.to_string().as_bytes())
}

/// 原类内部类 `HtmlMatchInfo`：一次锚点匹配的定位与前后缀差异信息。
#[derive(Debug, Clone, Default)]
struct HtmlMatchInfo {
    b: usize,
    c: Vec<usize>,
    d: isize,
    h: String,
    rk: String,
    tx: Option<String>,
    tu: String,
    r: usize,
    tc: usize,
}

impl HtmlMatchInfo {
    fn init(&mut self, caps: &regex::Captures<'_>, group: usize) {
        self.d = -1;
        self.tu = caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
        self.b = caps.get(0).map(|m| m.start()).unwrap_or(0);
        self.h = caps.get(group).map(|m| m.as_str().to_string()).unwrap_or_default();
    }

    /// `TU(other)`：祖先列表同长且首个相同项相等；命中时把共同祖先位置记为 `d`。
    fn link_parents(&mut self, other: &mut Self) -> bool {
        if other.c.len() != self.c.len() {
            return false;
        }
        for i in 0..self.c.len() {
            if self.c[i] != other.c[i] {
                continue;
            }
            if self.d != -1 && self.c[i] as isize != self.d {
                return false;
            }
            self.d = self.c[i] as isize;
            other.d = self.c[i] as isize;
            return true;
        }
        false
    }

    /// `findDiffStr(other, seps)`：逐字符比较，把公共前后缀之外的差异段记为分类 ID。
    fn find_diff_str(&mut self, other: &mut Self, seps: &str) -> bool {
        let a: Vec<char> = self.h.chars().collect();
        let b: Vec<char> = other.h.chars().collect();
        let n = a.len().min(b.len());
        for i in 0..n {
            if a[i] == b[i] && seps.contains(a[i]) {
                self.r = i + 1;
                other.r = i + 1;
            }
            if a[i] != b[i] {
                break;
            }
        }
        self.tc = a.len();
        other.tc = b.len();
        for k in 1..n {
            let ca = a[a.len() - k];
            let cb = b[b.len() - k];
            if ca == cb && seps.contains(ca) {
                self.tc = a.len() - k;
                other.tc = b.len() - k;
            }
            if ca != cb {
                break;
            }
        }
        let slice = |chars: &[char], lo: usize, hi: usize| -> String {
            if lo > hi || hi > chars.len() {
                return String::new();
            }
            chars[lo..hi].iter().collect()
        };
        let unset = match &self.tx {
            None => true,
            Some(text) => text.is_empty() && self.r < self.tc,
        };
        if unset {
            self.tx = Some(slice(&a, self.r, self.tc));
        } else {
            let expect = slice(&a, self.r, self.tc);
            if self.r > self.tc || self.tx.as_deref() != Some(expect.as_str()) {
                return false;
            }
        }
        if other.r < other.tc {
            other.tx = Some(slice(&b, other.r, other.tc));
        }
        true
    }
}

/// 原类 `WH()`：从首页 HTML 找出「分类条」父节点。
fn category_block(html: &str) -> String {
    let anchors = Regex::new(r#"<a.+?href="(.+?)""#).expect("anchor regex");
    let mut list: Vec<HtmlMatchInfo> = Vec::new();
    let mut count = 0usize;
    for caps in anchors.captures_iter(html) {
        count += 1;
        let mut info = HtmlMatchInfo::default();
        info.init(&caps, 1);
        info.rk = trim_html_string(node_string(html, info.b));
        if info.rk.is_empty() {
            if count > ANCHOR_LIMIT && !list.is_empty() {
                break;
            }
            continue;
        }
        if !CATEGORY_WORDS.iter().any(|word| info.rk.contains(word)) {
            if count > ANCHOR_LIMIT && !list.is_empty() {
                break;
            }
            continue;
        }
        info.c = find_up_nodes(html, info.b as isize - 1, 3);
        if !list.is_empty() && !list[0].link_parents(&mut info) {
            if list.len() > 1 {
                return node_string(html, list[0].d.max(0) as usize).to_string();
            }
            list.clear();
        }
        list.push(info);
        if count > ANCHOR_LIMIT && !list.is_empty() {
            break;
        }
    }
    if list.len() > 1 {
        return node_string(html, list[0].d.max(0) as usize).to_string();
    }
    String::new()
}

/// 原类 `D(String)`：从首页 HTML 推断 `cateManual`（名称 → 分类 ID），保持出现顺序。
fn infer_cate_manual(html: &str, seps: &str) -> Vec<(String, String)> {
    let block = category_block(html);
    if block.is_empty() {
        return Vec::new();
    }
    let item_re = Regex::new(r#"<a.+?href="(.+?)".*?["|>](\s*?\S+?\s*?)("|<)"#).expect("item regex");
    let mut list: Vec<HtmlMatchInfo> = Vec::new();
    for caps in item_re.captures_iter(&block) {
        let mut info = HtmlMatchInfo::default();
        info.init(&caps, 1);
        if info.h.chars().count() < 5 {
            continue;
        }
        info.rk = trim_html_string(node_string(&block, info.b));
        if info.rk.is_empty() {
            continue;
        }
        if EXCLUDE_WORDS.iter().any(|word| info.rk.contains(word)) {
            continue;
        }
        if list.is_empty() || list[0].find_diff_str(&mut info, seps) {
            list.push(info);
        }
    }
    let mut cat_indexes: Vec<usize> = Vec::new();
    for (i, info) in list.iter_mut().enumerate() {
        info.tx = None;
        if CATEGORY_WORDS.iter().any(|word| info.rk.contains(word)) {
            cat_indexes.push(i);
        }
    }
    if cat_indexes.len() > 1 {
        let base = cat_indexes[0];
        for &other in cat_indexes.iter().skip(1) {
            if base >= list.len() || other >= list.len() {
                continue;
            }
            let mut moved = list[other].clone();
            list[base].find_diff_str(&mut moved, seps);
            list[other] = moved;
        }
    }
    let base = cat_indexes.first().copied().unwrap_or(0);
    let mut out: Vec<(String, String)> = Vec::new();
    for i in 0..list.len() {
        let missing = list[i].tx.as_deref().map(|text| text.is_empty()).unwrap_or(true);
        if missing {
            if i == base || base >= list.len() {
                continue;
            }
            let mut moved = list[i].clone();
            let ok = list[base].find_diff_str(&mut moved, seps);
            list[i] = moved;
            if !ok {
                continue;
            }
        }
        let name = list[i].rk.clone();
        let id = list[i].tx.clone().unwrap_or_default();
        if name.is_empty() || id.is_empty() {
            continue;
        }
        let valid = !EXCLUDE_WORDS.iter().any(|word| name.contains(word));
        if valid && !out.iter().any(|(existing, _)| existing == &name) {
            out.push((name, id));
        }
    }
    out
}

/// 原类 `guess_rule_vod_id`：从首页 HTML 推测「列表锚点 → id 切片规则」六元组：
/// `[前缀, 后缀, 0, 0, 向上层数, 命中次数]`。
fn guess_rule_vod_id(html: &str, seps: &str) -> Option<Vec<Value>> {
    let anchors = Regex::new(r#"<a.+?href="(.+?)""#).expect("anchor regex");
    let mut map: HashMap<String, Vec<Value>> = HashMap::new();
    let mut list: Vec<HtmlMatchInfo> = Vec::new();
    for caps in anchors.captures_iter(html) {
        let raw = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
        if raw.contains("search") {
            continue;
        }
        let mut info = HtmlMatchInfo::default();
        info.init(&caps, 1);
        info.c = find_up_nodes(html, info.b as isize - 1, 4);
        if !list.is_empty() {
            let last = list[list.len() - 1].clone();
            if info.h == last.h {
                continue;
            }
            let last_index = list.len() - 1;
            if !list[last_index].link_parents(&mut info) {
                if list.len() > 1 {
                    merge_guess(&mut map, &list[0], &list[1], seps, list.len());
                }
                list.clear();
            }
        }
        list.push(info);
        if list.len() > 30 {
            break;
        }
    }
    if list.len() > 5 || (list.len() > 1 && map.is_empty()) {
        if list.len() > 1 {
            let mut first = list[0].clone();
            let mut second = list[1].clone();
            first.find_diff_str(&mut second, seps);
            list[0] = first;
            merge_guess(&mut map, &list[0], &list[1], seps, list.len());
        }
    }
    map.into_values().max_by_key(|arr| arr.get(5).and_then(|v| v.as_i64()).unwrap_or(0))
}

/// 把一次推断结果并入候选表（`guess_rule_vod_id` 内两处相同逻辑）。
fn merge_guess(
    map: &mut HashMap<String, Vec<Value>>,
    first: &HtmlMatchInfo,
    second: &HtmlMatchInfo,
    seps: &str,
    list_len: usize,
) {
    let mut a = first.clone();
    let mut b = second.clone();
    a.find_diff_str(&mut b, seps);
    let tx = a.tx.clone().unwrap_or_default();
    let value = tx.parse::<i64>().unwrap_or(0);
    if value <= 100 || tx.is_empty() {
        return;
    }
    let replaced = a.h.replace(&tx, "{vid}");
    let Some(split) = replaced.find("{vid}") else {
        return;
    };
    let prefix = replaced[..split].to_string();
    let suffix = replaced[split + 5..].to_string();
    let level = a
        .c
        .iter()
        .position(|pos| *pos as isize == a.d)
        .map(|pos| pos as i64 - 1)
        .unwrap_or(0)
        .max(1);
    let total = map.get(&replaced).and_then(|arr| arr.get(5)).and_then(|v| v.as_i64()).unwrap_or(0)
        + list_len as i64;
    let arr = vec![
        Value::String(prefix),
        Value::String(suffix),
        json!(0),
        json!(0),
        json!(level),
        json!(total),
    ];
    map.insert(replaced, arr);
    if total >= 30 {
        map.retain(|_, arr| arr.get(5).and_then(|v| v.as_i64()).unwrap_or(0) >= 30);
    }
}

use std::collections::HashSet;

use crate::spider::catvod::xbiu_rule::{get_lookback_count, trim_html_string_with};

/// 按字符截断（原类 `substring(0, 32768)` 的近似：不切坏 UTF-8）。
fn truncate_to(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    value.chars().take(limit).collect()
}

/// 把当前上下文写回缓存（规则是原类的静态可变字段 `a`）。
fn store_ctx(site: &Site, ctx: &Ctx) {
    let key = format!("{}|{}", site.key, site.ext_string().trim());
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, ctx.clone());
    }
}

/// 抓首页 HTML（原类 `tX(homeUrl, header)`）。
fn fetch_home(cfg: &TvConfig, site: &Site, ctx: &Ctx) -> Result<String> {
    if ctx.doc.home_url.trim().is_empty() {
        return Err(parse_err(site, "规则缺少 homeUrl"));
    }
    get_page(cfg, site, ctx, ctx.doc.home_url.trim())
}

/// 补齐规则里的推断项（原类 `check_rule_cateManual` / `check_rule_vod_id`）：
/// 缺 `cateManual` 时用首页 HTML 跑 `D`；缺 `list.vod_id` 时跑 `guess_rule_vod_id`。
fn ensure_rules(cfg: &TvConfig, site: &Site, ctx: &mut Ctx, page_html: Option<&str>) -> Result<()> {
    let has_cate = !ctx.cate_manual.is_empty();
    let has_vod_id = ctx
        .doc
        .rule
        .get("list")
        .and_then(|list| list.get("vod_id"))
        .map(|value| !value.is_null())
        .unwrap_or(false);
    if has_cate && has_vod_id {
        return Ok(());
    }
    let html = match page_html {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => fetch_home(cfg, site, ctx)?,
    };
    let html = truncate_to(&html, HOME_HTML_LIMIT);
    if !has_cate {
        ctx.cate_manual = infer_cate_manual(&html, &ctx.doc.separators);
    }
    if !has_vod_id {
        if let Some(guess) = guess_rule_vod_id(&html, &ctx.doc.separators) {
            if let Some(list) = ctx
                .doc
                .rule
                .get_mut("list")
                .and_then(|value| value.as_object_mut())
            {
                list.insert("vod_id".to_string(), Value::Array(guess));
            }
        }
    }
    store_ctx(site, ctx);
    Ok(())
}

/// 原类 `H()`：按 `list` 规则拼分类页 URL（本实现不传筛选，`bl=false` 分支）。
fn rule_url(rule: &Value, cate: &str, page: &str) -> String {
    let Some(list) = rule.get("list") else {
        return String::new();
    };
    let mut url = list
        .get(page)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    if url.is_empty() {
        url = list.get("url").and_then(|value| value.as_str()).unwrap_or("").to_string();
    }
    let mut url = url.replace("{cateId}", cate).replace("{catePg}", page);
    // 删除残留占位符 `{xxx}` 及其 `/xxx/` 段。
    while let Some(start) = url.find('{') {
        let Some(rel_end) = url[start..].find('}') else {
            break;
        };
        let end = start + rel_end;
        let name = url[start + 1..end].to_string();
        url.replace_range(start..=end, "");
        let segment = format!("/{name}/");
        url = url.replace(&segment, "");
    }
    url
}

/// 规则字段存在时按 `findSubString` 取值；空串视为缺失。
fn extract_field(node: &str, spec: Option<&Value>) -> Option<String> {
    let text = find_sub_string(node, 0, spec, "");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 原类 `guess_value_vod_name` 的清洗词表。
fn clean_title(raw: &str) -> String {
    let mut out = raw.to_string();
    for word in ["在线", "立即", "观看", "点播", "影片", "信息", "播放", "简介"] {
        out = out.replace(word, "");
    }
    out.trim().to_string()
}

/// 原类 `guess_value_vod_name`：`alt` / `title` 属性，兜底按文本片段出现次数。
fn guess_value_vod_name(node: &str) -> String {
    let mut name = find_sub_string(node, 0, Some(&json!(["alt=\"", "\""])), "");
    if name.is_empty() {
        name = find_sub_string(node, 0, Some(&json!(["\" title=\"", "\""])), "");
    }
    if !name.is_empty() {
        return clean_title(&name);
    }
    let flat = trim_html_string_with(node, "!!!!");
    let mut counts: HashMap<String, usize> = HashMap::new();
    for piece in flat.split("!!!!") {
        let piece = piece.trim();
        if piece.is_empty() || piece.contains("更新") {
            continue;
        }
        *counts.entry(piece.to_string()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(text, _)| clean_title(&text))
        .unwrap_or_default()
}

/// 原类 `guess_value_vod_pic`：`data-original` → `data-src` → `src` → `data-bg`。
fn guess_value_vod_pic(node: &str) -> Option<String> {
    for attr in ["data-original=\"", "data-src=\"", "src=\"", "data-bg=\""] {
        let value = find_sub_string(node, 0, Some(&json!([attr, "\""])), "");
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// 原类 `guess_value_vod_remarks`：按文本片段拼备注（跳过含标题的片段，总长 ≤20）。
fn guess_value_vod_remarks(node: &str, name: &str) -> String {
    let flat = trim_html_string_with(node, "!!!!");
    let mut out = String::new();
    for piece in flat.split("!!!!") {
        let piece = piece.trim();
        if piece.is_empty() || (!name.is_empty() && piece.contains(name)) {
            continue;
        }
        let next = if out.is_empty() {
            piece.to_string()
        } else {
            format!("{out},{piece}")
        };
        if next.chars().count() > 20 {
            break;
        }
        out = next;
    }
    out
}

/// 原类 `categoryContentImpl`：按 `lookback` 标记切重复块并逐块提取条目。
fn parse_category_html(ctx: &Ctx, html: &str, page: &str, region: bool) -> Vec<VideoItem> {
    let rule = &ctx.doc.rule;
    let Some(list) = rule.get("list").filter(|value| value.is_object()) else {
        return Vec::new();
    };
    let html = if region { get_region(html, list) } else { html.to_string() };
    let Some(lookback) = get_lookback_array(list) else {
        return Vec::new();
    };
    let marker = lookback.get(0).and_then(|value| value.as_str()).unwrap_or("").to_string();
    if marker.is_empty() {
        return Vec::new();
    }
    let mut level = get_lookback_count(&lookback).max(1) as usize;
    let id_spec = list.get("vod_id").cloned();
    let name_spec = list.get("vod_name").cloned();
    let pic_spec = list.get("vod_pic").cloned();
    let remark_spec = list.get("vod_remarks").cloned();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<VideoItem> = Vec::new();
    let mut prev: Option<Vec<usize>> = None;
    let mut pos = 0usize;
    let mut adapts = 0;
    let _ = page;
    while pos < html.len() {
        let Some(rel) = html[pos..].find(&marker) else {
            break;
        };
        let idx = pos + rel;
        let up = find_up_nodes(&html, idx as isize - 1, level);
        let start = match &prev {
            None => match up.last() {
                Some(value) => *value,
                None => {
                    pos = idx + marker.len();
                    continue;
                }
            },
            Some(prev_up) => match find_block_pos(prev_up, &up) {
                Some(value) => value,
                None => break,
            },
        };
        let node = node_string(&html, start);
        if sub_string_count(node, &marker) > 3 && level > 1 && adapts < 4 {
            level -= 1;
            prev = None;
            pos = 0;
            adapts += 1;
            continue;
        }
        pos = idx + marker.len();
        let Some(id) = extract_field(node, id_spec.as_ref()) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = extract_field(node, name_spec.as_ref())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| guess_value_vod_name(node));
        if name.is_empty() {
            continue;
        }
        let cover = extract_field(node, pic_spec.as_ref())
            .map(|text| add_http_prefix(rule, &text))
            .or_else(|| guess_value_vod_pic(node).map(|text| add_http_prefix(rule, &text)));
        let remark = extract_field(node, remark_spec.as_ref())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| guess_value_vod_remarks(node, &name));
        let entry = json!({
            "vod_id": id,
            "vod_name": name,
            "vod_pic": cover.clone().unwrap_or_default(),
            "vod_remarks": remark,
        });
        out.push(VideoItem {
            id: encode_vod_id(&entry),
            name,
            cover,
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
        prev = Some(up);
    }
    out
}

/// 首页分类（原类 `homeContent` 的 `class` 部分）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let mut ctx = ctx_for(cfg, site)?;
    ensure_rules(cfg, site, &mut ctx, None)?;
    if ctx.cate_manual.is_empty() {
        return Err(parse_err(site, "无法从首页推断分类（规则缺少 cateManual）"));
    }
    Ok(ctx
        .cate_manual
        .iter()
        .map(|(name, id)| Category {
            type_id: id.clone(),
            type_name: name.clone(),
        })
        .collect())
}

/// 分类列表（原类 `categoryContent`）。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let type_id = type_id.trim();
    if type_id.is_empty() {
        return Err(parse_err(site, "分类 id 为空"));
    }
    let mut ctx = ctx_for(cfg, site)?;
    let page_text = page.max(1).to_string();
    let url = rule_url(&ctx.doc.rule, type_id, &page_text);
    if url.is_empty() {
        return Err(parse_err(site, "规则缺少 list.url"));
    }
    let html = get_page(cfg, site, &ctx, &url)?;
    ensure_rules(cfg, site, &mut ctx, Some(&html))?;
    Ok(parse_category_html(&ctx, &html, &page_text, true))
}

// ============================================================
// W3：详情 / 播放 / 搜索（原类 `detailContent`/`playerContent`/`searchContent`）
// ============================================================

fn b64_decode_text(value: &str) -> Option<String> {
    let mut text = value.trim().to_string();
    if text.is_empty() {
        return None;
    }
    while text.len() % 4 != 0 {
        text.push('=');
    }
    URL_SAFE
        .decode(text.as_bytes())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}

fn decode_vod_json(id: &str) -> Value {
    b64_decode_text(id)
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}))
}

fn detail_page_url(rule: &Value, vid: &str) -> Option<String> {
    let detail = rule.get("detail")?;
    if let Some(u) = detail.get("url").and_then(|v| v.as_str()) {
        return Some(add_http_prefix(rule, &u.replace("{vid}", vid)));
    }
    let arr = rule.get("list")?.get("vod_id")?.as_array()?;
    let pre = arr.first().and_then(|v| v.as_str()).unwrap_or("");
    let post = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
    Some(add_http_prefix(rule, &format!("{pre}{vid}{post}")))
}

fn spec_field(region: &str, spec: &Value, key: &str, from: usize) -> String {
    let raw = find_sub_string(region, from, spec.get(key), "");
    trim_html_string(&raw)
}

fn lookup_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| value.get(*k).and_then(|v| v.as_str()))
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

fn is_video_format(url: &str) -> bool {
    if url.contains("=http") {
        return false;
    }
    [".m3u8", ".mp4", ".mpeg", ".flv", ".mkv"]
        .iter()
        .any(|suffix| url.to_ascii_lowercase().ends_with(suffix))
}

/// 由 `playlist` 规则生成剧集分组（`sM` + `findVodPlayUrl` + `findVodPlayFrom` 语义）。
fn playlist_groups(
    cfg: &TvConfig,
    site: &Site,
    ctx: &Ctx,
    detail_html: &str,
    detail_spec: &Value,
    detail_url: &str,
    vid: &str,
    _id_json: &Value,
) -> Vec<EpisodeGroup> {
    let rule = &ctx.doc.rule;
    let pl = match rule.get("playlist") {
        Some(v) if v.is_object() => v,
        _ => return Vec::new(),
    };
    let mut page = detail_html.to_string();
    if let Some(u) = pl.get("url").and_then(|v| v.as_str()) {
        let detail_rule_url = detail_spec.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if u != detail_rule_url {
            let target = add_http_prefix(rule, &u.replace("{vid}", vid));
            if target != detail_url {
                if let Ok(text) = get_page(cfg, site, ctx, &target) {
                    page = text;
                }
            }
        }
    }
    let region = get_region(&page, pl);
    let spec = match pl.get("vod_play_url") {
        Some(v) if v.is_array() => v,
        _ => return Vec::new(),
    };
    let levels = get_lookback_array(pl)
        .and_then(|lb| lb.get(4).and_then(|v| v.as_u64()))
        .unwrap_or(2) as usize;
    let mut raw_list: Vec<String> = Vec::new();
    match get_lookback_array(pl) {
        Some(lb) => {
            let marker = lb.get(0).and_then(|v| v.as_str()).unwrap_or("");
            if marker.is_empty() {
                raw_list.push(find_sub_string(&region, 0, Some(spec), ""));
            } else {
                let mut pos = 0usize;
                let mut guard = 0;
                while guard < 40 {
                    guard += 1;
                    let Some(idx) = region[pos..].find(marker).map(|i| i + pos) else {
                        break;
                    };
                    let ups = find_up_nodes(&region, idx as isize - 1, levels.max(1));
                    let start = ups.last().copied().unwrap_or(idx);
                    let text = find_sub_string(&region, start, Some(spec), "");
                    if !text.is_empty() {
                        raw_list.push(text);
                    }
                    pos = idx + marker.len();
                }
            }
        }
        None => raw_list.push(find_sub_string(&region, 0, Some(spec), "")),
    }
    let names: Vec<String> = match pl.get("vod_play_from") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                Value::Array(arr) => arr
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("线路")
                    .to_string(),
                _ => "线路".to_string(),
            })
            .collect(),
        Some(Value::String(s)) => s
            .split("$$$")
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in raw_list {
        let joined = raw.replace("\\/", "/");
        if joined.is_empty() || seen.contains(&joined) {
            continue;
        }
        seen.push(joined.clone());
        let mut items: Vec<Episode> = Vec::new();
        for entry in joined.split('#') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let parts: Vec<&str> = entry.split('$').collect();
            let (name, url) = if parts.len() >= 2 {
                (parts[0].trim().to_string(), parts[1].trim().to_string())
            } else {
                (format!("第{}集", items.len() + 1), entry.to_string())
            };
            if url.is_empty() {
                continue;
            }
            items.push(Episode {
                title: name,
                url: add_http_prefix(rule, &url),
            });
        }
        if items.is_empty() {
            continue;
        }
        let group = names
            .get(groups.len())
            .cloned()
            .unwrap_or_else(|| format!("播放列表{}", groups.len() + 1));
        groups.push(EpisodeGroup { group, items });
    }
    groups
}

/// 详情（`detailContent` 语义：base64 id → 规则切片 → 字段回退 → `sM` 线路）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let mut ctx = ctx_for(cfg, site)?;
    let home = fetch_home(cfg, site, &ctx).ok();
    ensure_rules(cfg, site, &mut ctx, home.as_deref())?;
    let rule = ctx.doc.rule.clone();
    let detail_spec = match rule.get("detail") {
        Some(v) if v.is_object() => v.clone(),
        _ => {
            return Err(parse_err(
                site,
                "该规则缺少 detail 节点，无法解析详情（XBiu 依赖规则 JSON 的 detail 段）",
            ))
        }
    };
    let id_json = decode_vod_json(id);
    let vid = lookup_str(&id_json, &["vod_id"])
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string());
    let page_url = detail_page_url(&rule, &vid)
        .ok_or_else(|| parse_err(site, "规则缺少 detail.url/list.vod_id，无法拼详情地址"))?;
    let html = get_page(cfg, site, &ctx, &page_url)?;
    let region = get_region(&html, &detail_spec);
    let mut from = 0usize;
    if let Some(lb) = get_lookback_array(&detail_spec) {
        let marker = lb.get(0).and_then(|v| v.as_str()).unwrap_or("");
        let levels = lb.get(4).and_then(|v| v.as_u64()).unwrap_or(2) as usize;
        if !marker.is_empty() {
            if let Some(idx) = region.find(marker) {
                let ups = find_up_nodes(&region, idx as isize - 1, levels.max(1));
                if let Some(last) = ups.last() {
                    from = *last;
                }
            }
        }
    }
    let name = {
        let raw = spec_field(&region, &detail_spec, "vod_name", from);
        if raw.is_empty() {
            lookup_str(&id_json, &["vod_name"])
                .map(str::to_string)
                .unwrap_or_default()
        } else {
            raw
        }
    };
    let name = if name.is_empty() {
        guess_value_vod_name(&region)
    } else {
        name
    };
    let pic = {
        let raw = spec_field(&region, &detail_spec, "vod_pic", from);
        let raw = if raw.is_empty() {
            lookup_str(&id_json, &["vod_pic"]).unwrap_or("").to_string()
        } else {
            raw
        };
        if raw.is_empty() {
            guess_value_vod_pic(&region)
        } else {
            Some(add_http_prefix(&rule, &raw))
        }
    };
    let type_name = spec_field(&region, &detail_spec, "type_name", from);
    let year = spec_field(&region, &detail_spec, "vod_year", from);
    let area = spec_field(&region, &detail_spec, "vod_area", from);
    let remarks = {
        let raw = spec_field(&region, &detail_spec, "vod_remarks", from);
        if raw.is_empty() {
            guess_value_vod_remarks(&region, &name)
        } else {
            raw
        }
    };
    let director = spec_field(&region, &detail_spec, "vod_director", from);
    let actor = spec_field(&region, &detail_spec, "vod_actor", from);
    let content = {
        let raw = spec_field(&region, &detail_spec, "vod_content", from);
        if raw.is_empty() {
            trim_html_string(&region)
        } else {
            raw
        }
    };
    let mut desc = content;
    if !type_name.is_empty() || !remarks.is_empty() {
        let head = format!("{} {}", type_name, remarks).trim().to_string();
        desc = if desc.is_empty() {
            head
        } else {
            format!("{head}\n{desc}")
        };
    }
    let episodes = playlist_groups(cfg, site, &ctx, &html, &detail_spec, &page_url, &vid, &id_json);
    Ok(VideoDetail {
        id: vid,
        name,
        cover: pic,
        desc: if desc.is_empty() { None } else { Some(desc) },
        year: if year.is_empty() { None } else { Some(year) },
        area: if area.is_empty() { None } else { Some(area) },
        director: if director.is_empty() { None } else { Some(director) },
        actor: if actor.is_empty() { None } else { Some(actor) },
        episodes,
    })
}

/// 播放（`playerContent`/`P8` 语义：按 `play` 规则从播放页切出 `vod_url`）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let fallback = json!({"parse": 1, "playUrl": "", "url": id});
    let mut ctx = ctx_for(cfg, site)?;
    let home = fetch_home(cfg, site, &ctx).ok();
    ensure_rules(cfg, site, &mut ctx, home.as_deref())?;
    let rule = ctx.doc.rule.clone();
    let play = match rule.get("play") {
        Some(v) if v.is_object() => v.clone(),
        _ => return Ok(fallback),
    };
    let html = get_page(cfg, site, &ctx, id)?;
    let region = get_region(&html, &play);
    let mut from = 0usize;
    if let Some(lb) = get_lookback_array(&play) {
        let marker = lb.get(0).and_then(|v| v.as_str()).unwrap_or("");
        let levels = lb.get(4).and_then(|v| v.as_u64()).unwrap_or(2) as usize;
        if !marker.is_empty() {
            if let Some(idx) = region.find(marker) {
                let ups = find_up_nodes(&region, idx as isize - 1, levels.max(1));
                if let Some(last) = ups.last() {
                    from = *last;
                }
            }
        }
    }
    let raw = find_sub_string(&region, from, play.get("vod_url"), "").replace("\\/", "/");
    if raw.is_empty() {
        return Ok(fallback);
    }
    if is_video_format(&raw) {
        return Ok(json!({"parse": 0, "playUrl": "", "url": raw}));
    }
    // 原类遇到非直链时返回空串 → 上层再兜底 parse=1（沿用原播放地址）。
    Ok(fallback)
}

fn search_items_from_json(ctx: &Ctx, text: &str) -> Option<Vec<VideoItem>> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let rows: Vec<Value> = match value {
        Value::Array(items) => items,
        Value::Object(_) => vec![value],
        _ => return None,
    };
    let spec = ctx.doc.rule.get("search").cloned().unwrap_or(Value::Null);
    let mut out = Vec::new();
    for row in rows {
        if !row.is_object() {
            continue;
        }
        let id_key = spec.get("vod_id").and_then(|v| v.as_str()).unwrap_or("id");
        let name_key = spec.get("vod_name").and_then(|v| v.as_str()).unwrap_or("name");
        let pic_key = spec.get("vod_pic").and_then(|v| v.as_str()).unwrap_or("pic");
        let remarks_key = spec.get("vod_remarks").and_then(|v| v.as_str()).unwrap_or("remarks");
        let id = row
            .get(id_key)
            .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let name = row
            .get(name_key)
            .and_then(|v| v.as_str())
            .unwrap_or("未知")
            .to_string();
        let pic = row
            .get(pic_key)
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let remarks = row
            .get(remarks_key)
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .unwrap_or_default();
        out.push(VideoItem {
            id: encode_vod_id(&row),
            name,
            cover: if pic.is_empty() { None } else { Some(pic) },
            remark: if remarks.is_empty() { None } else { Some(remarks) },
        });
    }
    Some(out)
}

fn parse_search_lookback(ctx: &Ctx, html: &str) -> Vec<VideoItem> {
    let rule = &ctx.doc.rule;
    let search_spec = rule.get("search").cloned().unwrap_or(Value::Null);
    let list_spec = rule.get("list").cloned().unwrap_or(Value::Null);
    let field_spec = |key: &str| -> Option<Value> {
        search_spec
            .get(key)
            .cloned()
            .or_else(|| list_spec.get(key).cloned())
    };
    let lookback = get_lookback_array(&search_spec).or_else(|| get_lookback_array(&list_spec));
    let (marker, levels) = match lookback {
        Some(lb) => (
            lb.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            lb.get(4).and_then(|v| v.as_u64()).unwrap_or(2) as usize,
        ),
        None => return Vec::new(),
    };
    if marker.is_empty() {
        return Vec::new();
    }
    let id_spec = field_spec("vod_id");
    let name_spec = field_spec("vod_name");
    let pic_spec = field_spec("vod_pic");
    let remarks_spec = field_spec("vod_remarks");
    let Some(id_spec) = id_spec else {
        return Vec::new();
    };
    let mut out: Vec<VideoItem> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut pos = 0usize;
    let mut guard = 0;
    while guard < 60 {
        guard += 1;
        let Some(idx) = html[pos..].find(&marker).map(|i| i + pos) else {
            break;
        };
        let ups = find_up_nodes(html, idx as isize - 1, levels.max(1));
        let start = ups.last().copied().unwrap_or(idx);
        let block = node_string(html, start);
        let block = if block.is_empty() { &html[start..] } else { block };
        let id = find_sub_string(block, 0, Some(&id_spec), "");
        if !id.is_empty() && !seen.contains(&id) {
            seen.push(id.clone());
            let name = find_sub_string(block, 0, name_spec.as_ref(), "");
            let name = if name.is_empty() {
                guess_value_vod_name(block)
            } else {
                trim_html_string(&name)
            };
            let pic = find_sub_string(block, 0, pic_spec.as_ref(), "");
            let pic = if pic.is_empty() {
                guess_value_vod_pic(block)
            } else {
                Some(add_http_prefix(rule, &pic))
            };
            let remarks = find_sub_string(block, 0, remarks_spec.as_ref(), "");
            let remarks = if remarks.is_empty() {
                guess_value_vod_remarks(block, &name)
            } else {
                trim_html_string(&remarks)
            };
            out.push(VideoItem {
                id: encode_vod_id(&json!({"vod_id": id})),
                name,
                cover: pic,
                remark: if remarks.is_empty() { None } else { Some(remarks) },
            });
        }
        pos = idx + marker.len();
    }
    out
}

/// 无 search 规则时的兜底（`check_rule_search`）：先试 MacCMS suggest 接口。
fn check_rule_search(cfg: &TvConfig, site: &Site, ctx: &mut Ctx, keyword: &str) -> Result<()> {
    if ctx.doc.rule.get("search").is_some() {
        return Ok(());
    }
    let home_url = ctx.doc.home_url.clone();
    let rule_text = ctx.doc.rule.to_string();
    let headers = headers_for(&Value::from(rule_text));
    if !home_url.is_empty() {
        let suggest = add_http_prefix(
            &ctx.doc.rule,
            &format!("index.php/ajax/suggest?mid=1&wd={}", urlencode(keyword)),
        );
        let client = http::client_for(&cfg.http)?;
        if let Ok(text) = http::get_text(&client, &suggest, &headers, cfg.http.retry) {
            if serde_json::from_str::<Value>(text.trim()).is_ok() {
                let search = json!({
                    "vod_id": "id",
                    "vod_name": "name",
                    "vod_pic": "pic",
                    "url": "index.php/ajax/suggest?mid=1&wd={wd}",
                });
                if let Some(obj) = ctx.doc.rule.as_object_mut() {
                    obj.insert("search".to_string(), search);
                }
                store_ctx(site, ctx);
                return Ok(());
            }
        }
    }
    Ok(())
}

/// 搜索（`searchContent` 语义：search 规则优先，缺失时推断；JSON 结果与 lookback 双路径）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let mut ctx = ctx_for(cfg, site)?;
    let home = fetch_home(cfg, site, &ctx).ok();
    ensure_rules(cfg, site, &mut ctx, home.as_deref())?;
    check_rule_search(cfg, site, &mut ctx, keyword)?;
    let rule = ctx.doc.rule.clone();
    let Some(search_spec) = rule.get("search").cloned() else {
        return Ok(Vec::new());
    };
    if !search_spec.is_object() {
        return Ok(Vec::new());
    }
    let url = search_spec
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .replace("{wd}", &urlencode(keyword));
    if url.is_empty() {
        return Ok(Vec::new());
    }
    let full = add_http_prefix(&rule, &url);
    let body = get_page(cfg, site, &ctx, &full)?;
    let region = get_region(&body, &search_spec);
    if let Some(items) = search_items_from_json(&ctx, &region) {
        return Ok(items);
    }
    Ok(parse_search_lookback(&ctx, &region))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_rule() -> Value {
        json!({
            "homeUrl": "http://site.test/",
            "list": {
                "url": "/list/{cateId}---{catePg}---{cateFilter}.html",
                "lb": ["href=\"/v/", 0, 0, 0, 2],
                "vod_id": ["href=\"/v/", "\""],
                "vod_name": ["alt=\"", "\""],
                "vod_pic": ["src=\"", "\""],
                "vod_remarks": ["<b>", "</b>"]
            },
            "cateManual": {"电影": "1", "电视剧": "2"},
            "separators": "/.-_~"
        })
    }

    #[test]
    fn rule_url_replaces_and_cleans_placeholders() {
        let rule = list_rule();
        assert_eq!(
            rule_url(&rule, "2", "3"),
            "/list/2---3---.html",
            "残留 {{cateFilter}} 应被删除（含 /cateFilter/ 段）"
        );
        assert_eq!(rule_url(&json!({}), "1", "1"), "");
    }

    #[test]
    fn truncate_respects_char_limit() {
        assert_eq!(truncate_to("abcdef", 3), "abc");
        assert_eq!(truncate_to("电影电视剧", 2), "电影");
        assert_eq!(truncate_to("abc", 10), "abc");
    }

    #[test]
    fn guess_name_prefers_alt_and_cleans_words() {
        let node = r#"<a href="/v/1"><img alt="在线观看好看的影片" src="/1.jpg"></a>"#;
        assert_eq!(guess_value_vod_name(node), "好看的");
        let node2 = r#"<a href="/v/1" title="第二标题"><span>x</span></a>"#;
        assert_eq!(guess_value_vod_name(node2), "第二标题");
    }

    #[test]
    fn guess_pic_falls_back_through_attributes() {
        assert_eq!(
            guess_value_vod_pic(r#"<img data-original="/a.jpg" data-src="/b.jpg">"#),
            Some("/a.jpg".to_string())
        );
        assert_eq!(
            guess_value_vod_pic(r#"<img data-src="/b.jpg" src="/c.jpg">"#),
            Some("/b.jpg".to_string())
        );
        assert_eq!(
            guess_value_vod_pic(r#"<img src="/c.jpg">"#),
            Some("/c.jpg".to_string())
        );
        assert_eq!(guess_value_vod_pic("<div>无图</div>"), None);
    }

    #[test]
    fn guess_remarks_skips_title_and_caps_length() {
        let node = r#"<b>HD中字</b><span>电影A</span>"#;
        assert_eq!(guess_value_vod_remarks(node, "电影A"), "HD中字");
    }

    #[test]
    fn infer_cate_manual_from_nav() {
        let html = r#"<div class="nav"><a href="/type/1.html">电影</a><a href="/type/2.html">电视剧</a><a href="/type/3.html">综艺</a></div>"#;
        let manual = infer_cate_manual(html, "/.-_~");
        assert_eq!(
            manual,
            vec![
                ("电影".to_string(), "1".to_string()),
                ("电视剧".to_string(), "2".to_string()),
                ("综艺".to_string(), "3".to_string())
            ]
        );
    }

    #[test]
    fn category_block_finds_nav_container() {
        let html = r#"<div class="nav"><a href="/type/1.html">电影</a><a href="/type/2.html">电视剧</a></div>"#;
        let block = category_block(html);
        assert!(block.starts_with(r#"<div class="nav">"#), "block={block}");
        assert!(block.contains("电视剧"));
    }

    #[test]
    fn parse_category_items_extracts_rows() {
        let html = r#"<ul><li><a href="/v/101"><img alt="电影A" src="/p/1.jpg"></a><b>HD</b></li><li><a href="/v/102"><img alt="电影B" src="/p/2.jpg"></a><b>BD</b></li></ul>"#;
        let ctx = Ctx {
            doc: RuleDoc {
                rule: list_rule(),
                rule_text: String::new(),
                home_url: "http://site.test/".to_string(),
                separators: "/.-_~".to_string(),
                keywords: Vec::new(),
            },
            cate_manual: vec![("电影".to_string(), "1".to_string())],
        };
        let items = parse_category_html(&ctx, html, "1", false);
        assert_eq!(items.len(), 2, "items={items:?}");
        assert_eq!(items[0].name, "电影A");
        assert_eq!(items[0].cover.as_deref(), Some("http://site.test/p/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("HD"));
        assert_eq!(items[1].name, "电影B");
        // id 是 base64(JSON) 且可还原原始 vod_id。
        let raw = URL_SAFE.decode(items[0].id.as_bytes()).unwrap();
        let decoded: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(decoded["vod_id"], "101");
        assert_eq!(decoded["vod_name"], "电影A");
    }

    #[test]
    fn guess_rule_vod_id_returns_six_tuple() {
        let html = concat!(
            r#"<ul><li><a href="/vod/1001.html">A</a></li>"#,
            r#"<li><a href="/vod/1002.html">B</a></li>"#,
            r#"<li><a href="/vod/1003.html">C</a></li>"#,
            r#"<li><a href="/vod/1004.html">D</a></li>"#,
            r#"<li><a href="/vod/1005.html">E</a></li>"#,
            r#"<li><a href="/vod/1006.html">F</a></li>"#,
            r#"<li><a href="/vod/1007.html">G</a></li></ul>"#
        );
        let guess = guess_rule_vod_id(html, "/.-_~");
        let arr = guess.expect("应推断出六元组");
        assert_eq!(arr.len(), 6);
        assert!(arr[0].as_str().unwrap_or_default().contains("{vid}") == false
            || arr[0].as_str().unwrap_or_default().contains("/vod/"));
        assert!(arr[4].as_i64().unwrap_or(0) >= 1);
        assert!(arr[5].as_i64().unwrap_or(0) >= 1);
    }
}
