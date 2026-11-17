//! `csp_Gitcafe`（Gitcafe 阿里分享搜索型）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/fc213119-jojo/deobf/com/github/catvod/spider/Gitcafe.java`
//!   （127 行，`extends Spider`，`searchContent` 可读，但 `detailContent` /
//!   `playerContent` 委托 `PushAgent`——该类是网盘引擎，桌面端不复刻）。
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Gitcafe.java`
//!   （77 行，同源变体：`private Push a`，detail/player 委托 **Push**；但
//!   `searchContent` 被 jadx 跳过 2107 指令）。
//! * 两变体互补：搜索协议取 127 行可读版本（字符串经 `merge.tlV.d` 解码，
//!   `KEY="ATPHRf"` = 十六进制解码后按 KEY 循环异或），详情/播放取 77 行版本
//!   的 `Push` 语义（本项目已在 `families/push.rs` 原生实现，直接复用）。
//!
//! 协议（逐段还原）：
//! * `init`：`super.init` 后 `new Push()` 并 `init`（详情/播放的委托对象）。
//! * `searchContent(kw)`：
//!   1. token 懒加载：POST 表单 `action=get_token`，响应 JSON 的 `data`；
//!   2. POST 表单 `action=search&from=web&token={token}&keyword={kw}`；
//!   3. 响应体若以 `[` 开头、以 `]` 结尾 → 直接是 JSON 数组，否则取对象的
//!      `data` 数组；行字段 `title` / `alikey` / `cat`；
//!   4. 逐行 `vod_id = "https://www.aliyundrive.com/s/" + alikey`，
//!      `vod_name = title`，`vod_pic` 为固定图，`vod_remarks = "√" + cat`；
//!      阿里分享有效性由原类内联校验（`C0332.m1299`：POST
//!      `api.aliyundrive.com/adrive/v3/share_link/get_share_by_anonymous`，
//!      响应含 `share_link is` / `share_link cannot` 视为无效，跳过该行）。
//! * `detailContent`：`this.a.detailContent(list)`（Push：阿里云盘展开 → 「阿里」
//!   线路；官方站 → 「官源」；直链 → 「直连」；其余 → 「网页」）。
//! * `playerContent`：`this.a.playerContent(...)`（Push：`###` 走网盘引擎；
//!   官源/网页 `parse:1`；直连 `parse:0`）。
//!
//! 有意偏差（桌面端能力边界 / 上游现状）：
//! 1. 上游 `gitcafe.net` 在 2026-11 探测已不可用（fetch failed），搜索链路只做
//!    mock 验收；`ext` 为 JSON 时可注入 `search_base` / `ali_base` /
//!    `ali_cookie`（仅测试/镜像钩子，原类 ext 是 Ali proxy token 文本）。
//! 2. token 按 24h 有效期缓存（原类实例生命周期内不刷新）；仅影响长进程。
//! 3. 分享有效性校验逐行发起一次匿名接口请求（忠实原类），网络失败按无效跳过。
//! 4. 详情/播放委托 `families::push`（桌面端原生实现），不引入 PushAgent 引擎。
//! 5. 搜索型蜘蛛没有首页/分类：`categories`/`videos` 返回空列表。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, post_form_text};
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::{parse_share_link, PanConfig, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};
use super::push;

/// 本家族覆盖的 api。
pub const API: &str = "csp_Gitcafe";

/// 原类 `d`：小纸条搜索接口（POST 表单）。
const SEARCH_BASE: &str = "https://gitcafe.net/tool/alipaper/";

/// 原类条目 id 前缀：阿里分享。
const SHARE_PREFIX: &str = "https://www.aliyundrive.com/s/";

/// 原类 `m72()` 的固定 UA（Chrome/94）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36";

/// 原类 `m72()` 的 Origin/Referer。
const ORIGIN: &str = "https://u.gitcafe.net/";

/// 原类搜索结果固定封面（lgstatic 图）。
pub const COVER: &str = "https://www.lgstatic.com/i/image2/M01/15/7E/CgoB5lysLXCADg6ZAABapAHUnQM321.jpg";

/// token 有效期（24h，桌面端长进程防过期；原类实例内不刷新）。
const TOKEN_TTL_MS: u64 = 86_400_000;

/// 是否本家族覆盖的 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Origin".to_string(), ORIGIN.to_string());
    h.insert("Referer".to_string(), ORIGIN.to_string());
    h
}

fn normalize_base(base: &str) -> String {
    let mut s = base.trim().to_string();
    if !s.is_empty() && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// ext JSON 测试钩子（原类 ext 是 Ali proxy token 文本，非 JSON 时全部走默认）。
#[derive(Debug, Clone, Default)]
struct ExtConf {
    search_base: String,
    ali_base: String,
    ali_cookie: String,
}

fn ext_conf(site: &Site) -> ExtConf {
    let raw = site.ext_string();
    let raw = raw.trim();
    if !raw.starts_with('{') {
        return ExtConf::default();
    }
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ExtConf::default();
    };
    let mut conf = ExtConf::default();
    if let Some(s) = value.get("search_base").and_then(|v| v.as_str()) {
        conf.search_base = s.trim().to_string();
    }
    if let Some(s) = value.get("ali_base").and_then(|v| v.as_str()) {
        conf.ali_base = s.trim().to_string();
    }
    if let Some(s) = value.get("ali_cookie").and_then(|v| v.as_str()) {
        conf.ali_cookie = s.trim().to_string();
    } else if let Some(s) = value.get("cookie").and_then(|v| v.as_str()) {
        conf.ali_cookie = s.trim().to_string();
    }
    conf
}

fn search_base(site: &Site) -> String {
    let conf = ext_conf(site);
    if conf.search_base.is_empty() {
        SEARCH_BASE.to_string()
    } else {
        normalize_base(&conf.search_base)
    }
}

fn pan_config(site: &Site) -> PanConfig {
    let mut conf = PanConfig::parse(&site.ext_string()).with_overlay();
    let extra = ext_conf(site);
    if !extra.ali_base.is_empty() {
        conf.ali_base = normalize_base(&extra.ali_base);
    }
    if !extra.ali_cookie.is_empty() {
        conf.ali_cookie = extra.ali_cookie;
    }
    conf
}

fn ali_client(cfg: &TvConfig, conf: &PanConfig) -> Result<AliClient> {
    if conf.ali_base.trim().is_empty() {
        AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())
    } else {
        let client = client_for(&cfg.http)?;
        Ok(AliClient::with_base(
            client,
            conf.effective_ali_cookie(),
            conf.ali_base.trim().trim_end_matches('/'),
        ))
    }
}

/* ============================================================
 * token 缓存（原类实例字段 `f70`）
 * ============================================================ */

fn token_slot() -> &'static Mutex<Option<(String, u64)>> {
    static SLOT: OnceLock<Mutex<Option<(String, u64)>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// token 是否仍在有效期内（`now < expire_at`）。
pub fn token_is_fresh(entry: &Option<(String, u64)>, now: u64) -> bool {
    matches!(entry, Some((token, expire_at)) if !token.is_empty() && now < *expire_at)
}

fn cached_token(now: u64) -> Option<String> {
    let guard = token_slot().lock().ok()?;
    let entry = guard.clone();
    if token_is_fresh(&entry, now) {
        entry.map(|(token, _)| token)
    } else {
        None
    }
}

fn store_token(token: &str, now: u64) {
    if let Ok(mut guard) = token_slot().lock() {
        *guard = Some((token.to_string(), now.saturating_add(TOKEN_TTL_MS)));
    }
}

/// 懒加载 token（原类 `if (f70.isEmpty()) { action=get_token }`）。
fn ensure_token(cfg: &TvConfig, site: &Site, base: &str) -> Result<String> {
    let now = now_ms();
    if let Some(token) = cached_token(now) {
        return Ok(token);
    }
    let client = client_for(&cfg.http)?;
    let mut form = HashMap::new();
    form.insert("action".to_string(), "get_token".to_string());
    let body = post_form_text(&client, base, &headers(), &form)?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| parse_err(site, format!("gitcafe/get_token 响应不是 JSON: {e}")))?;
    let token = value
        .get("data")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(parse_err(
            site,
            "csp_Gitcafe 取 token 失败（上游 gitcafe.net 已停放？）",
        ));
    }
    store_token(&token, now);
    Ok(token)
}

/* ============================================================
 * 搜索（原类 `searchContent`）
 * ============================================================ */

/// 解析 `action=search` 响应：JSON 数组，或 `{"data":[…]}`。
///
/// 返回 `(title, alikey, cat)`；`alikey` / `title` 为空的行被丢弃
/// （原类只判 `alikey` 非空，但空标题无法在 UI 上呈现，记偏差）。
pub fn parse_search_body(body: &str) -> Vec<(String, String, String)> {
    let body = body.trim();
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let rows = if value.is_array() {
        value.as_array().cloned().unwrap_or_default()
    } else {
        value
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for row in rows {
        let title = row
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let alikey = row
            .get("alikey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let cat = row
            .get("cat")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() || alikey.is_empty() {
            continue;
        }
        out.push((title, alikey, cat));
    }
    out
}

/// 条目映射（原类 `vod_id` / `vod_name` / `vod_pic` / `vod_remarks`）。
pub fn item_of(title: &str, alikey: &str, cat: &str) -> VideoItem {
    VideoItem {
        id: format!("{SHARE_PREFIX}{}", alikey.trim()),
        name: title.trim().to_string(),
        cover: Some(COVER.to_string()),
        remark: Some(format!("√{}", cat.trim())),
    }
}

/// 分享有效性校验（原类 `C0332.m1299` 的等价实现）。
fn validate_share(cfg: &TvConfig, conf: &PanConfig, url: &str) -> bool {
    let Some(link) = parse_share_link(url) else {
        return false;
    };
    if link.provider != PanProvider::Ali {
        return false;
    }
    let Ok(client) = ali_client(cfg, conf) else {
        return false;
    };
    // 原类以响应文本含 `share_link is` / `share_link cannot` 判无效；等价地，
    // 匿名接口对失效分享不返回 `share_name`。
    client
        .share_info(&link.share_id)
        .map(|info| !info.share_name.trim().is_empty())
        .unwrap_or(false)
}

/// 搜索入口。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let base = search_base(site);
    let token = ensure_token(cfg, site, &base)?;
    let client = client_for(&cfg.http)?;
    let mut form = HashMap::new();
    form.insert("action".to_string(), "search".to_string());
    form.insert("from".to_string(), "web".to_string());
    form.insert("token".to_string(), token);
    form.insert("keyword".to_string(), keyword.to_string());
    let body = post_form_text(&client, &base, &headers(), &form)?;
    let rows = parse_search_body(&body);
    let conf = pan_config(site);
    let mut out = Vec::new();
    for (title, alikey, cat) in rows {
        let url = format!("{SHARE_PREFIX}{alikey}");
        if validate_share(cfg, &conf, &url) {
            out.push(item_of(&title, &alikey, &cat));
        }
    }
    Ok(out)
}

/* ============================================================
 * 详情 / 播放：委托 `families::push`（原类 77 行变体的 `Push a`）
 * ============================================================ */

/// 详情入口：`id` 为分享/视频地址，语义等同 `csp_Push` 的 detail。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    push::detail(cfg, site, id)
}

/// 播放入口：语义等同 `csp_Push` 的 player。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    push::player_content(cfg, site, flag, id)
}

/* ============================================================
 * 搜索型蜘蛛：没有首页 / 分类
 * ============================================================ */

/// 分类为空（原类未覆写 `homeContent` 的分类部分）。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(Vec::new())
}

/// 分类内容为空（搜索型蜘蛛）。
pub fn videos(
    _cfg: &TvConfig,
    _site: &Site,
    _cat: &str,
    _page: u32,
    _filter: &HashMap<String, String>,
) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// `json!` 便捷宏（保持与其他家族一致的可读性）。
#[allow(dead_code)]
fn _json_probe() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Value) -> Site {
        Site {
            key: "gitcafe".to_string(),
            name: "Gitcafe".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(ext),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Gitcafe"));
        assert!(handles("CSP_GITCAFE"));
        assert!(!handles("csp_Push"));
    }

    #[test]
    fn headers_carry_ua_and_origin() {
        let h = headers();
        assert!(h.get("User-Agent").expect("ua").contains("Chrome/94"));
        assert_eq!(h.get("Origin").map(String::as_str), Some(ORIGIN));
        assert_eq!(h.get("Referer").map(String::as_str), Some(ORIGIN));
    }

    #[test]
    fn normalize_base_appends_slash() {
        assert_eq!(normalize_base("http://a"), "http://a/");
        assert_eq!(normalize_base("http://a/"), "http://a/");
        assert_eq!(normalize_base("  "), "");
    }

    #[test]
    fn ext_conf_parses_hooks_and_ignores_plain_text() {
        let conf = ext_conf(&site(json!({
            "search_base": "http://mock/alipaper/",
            "ali_base": "http://mock/ali",
            "ali_cookie": "ck"
        })));
        assert_eq!(conf.search_base, "http://mock/alipaper/");
        assert_eq!(conf.ali_base, "http://mock/ali");
        assert_eq!(conf.ali_cookie, "ck");
        let plain = ext_conf(&site(json!("./lib/token.json")));
        assert!(plain.search_base.is_empty() && plain.ali_base.is_empty());
    }

    #[test]
    fn search_base_prefers_ext_hook() {
        let base = search_base(&site(json!({"search_base": "http://mock/alipaper"})));
        assert_eq!(base, "http://mock/alipaper/");
        assert_eq!(search_base(&site(json!(""))), SEARCH_BASE);
    }

    #[test]
    fn parse_search_body_accepts_array_and_data_object() {
        let array = r#"[{"title":"A","alikey":"k1","cat":"电影"},{"title":"","alikey":"k2","cat":""}]"#;
        let rows = parse_search_body(array);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "A");
        assert_eq!(rows[0].1, "k1");

        let object = r#"{"success":true,"data":[{"title":"B","alikey":"k3","cat":"剧集"}]}"#;
        let rows = parse_search_body(object);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2, "剧集");
    }

    #[test]
    fn parse_search_body_tolerates_garbage() {
        assert!(parse_search_body("not json").is_empty());
        assert!(parse_search_body(r#"{"data":[]}"#).is_empty());
    }

    #[test]
    fn item_of_builds_share_url_cover_and_remark() {
        let item = item_of("三体", "abc123", "电视剧");
        assert_eq!(item.id, format!("{SHARE_PREFIX}abc123"));
        assert_eq!(item.name, "三体");
        assert_eq!(item.cover.as_deref(), Some(COVER));
        assert_eq!(item.remark.as_deref(), Some("√电视剧"));
    }

    #[test]
    fn token_is_fresh_respects_expiry() {
        assert!(token_is_fresh(&Some(("tok".into(), 100)), 99));
        assert!(!token_is_fresh(&Some(("tok".into(), 100)), 100));
        assert!(!token_is_fresh(&Some((String::new(), 100)), 50));
        assert!(!token_is_fresh(&None, 0));
    }

    #[test]
    fn search_with_empty_keyword_makes_no_request() {
        let items = search(&TvConfig::default(), &site(json!("")), "  ").expect("empty ok");
        assert!(items.is_empty());
    }

    #[test]
    fn categories_and_videos_are_empty() {
        let cfg = TvConfig::default();
        let s = site(json!(""));
        assert!(categories(&cfg, &s).expect("cats").is_empty());
        assert!(videos(&cfg, &s, "x", 1, &HashMap::new()).expect("videos").is_empty());
    }
}
