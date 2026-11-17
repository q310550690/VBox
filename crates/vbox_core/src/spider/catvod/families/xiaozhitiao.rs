//! `csp_XiaoZhiTiao`（阿里「小纸条」搜索型）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * 类：`com.github.catvod.spider.XiaoZhiTiao`，站点 1 个（GYCK 仓 `小紙條`，
//!   站点 ext `./lib/token.json` 为 Ali proxy token 文本，桌面端忽略）。
//! * 证据：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/XiaoZhiTiao.java`
//!   （70 行，`jadx` 无 skip：`extends Ali`，仅覆写 `init` + `searchContent`，
//!   `homeContent`/`categoryContent`/`detailContent`/`playerContent` 全部继承
//!   自 `Ali`）；对照变体 `41c87635`（53 行）与 `5d982c72`（75 行）同协议。
//! * 依赖：`Ali.java`（221 行，`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Ali.java`）
//!   的 `detailContent` 用正则
//!   `(www.aliyundrive.com|www.alipan.com)/s/([^\?/]+)(/folder/([^\?/]+))?`
//!   抽出 `share_id` / 提取码 / 子目录，再交给 Ali 引擎枚举目录；`playerContent`
//!   交给 Ali 引擎的本地代理协议。桌面端把这两处映射到 `crate::pan::ali`
//!   （`AliClient::{share_token,browse,play_url,download_url}`）。
//!
//! 协议（逐段还原）：
//! * `init`：`super.init(context, ext)` 之后只记 `new Date()` 作为 token 过期基准。
//! * `searchContent`：POST `https://gitcafe.net/tool/alipaper/`（`merge.S.e.v` =
//!   POST 表单），头 `Host: gitcafe.net` + UA Chrome/122：
//!   1. token 为空或已过期 → 表单 `action=get_token&from=web`，响应
//!      `{"success":true,"data":"<token>"}`，有效期 `now + 86400000`；
//!   2. 表单 `action=search&from=web&keyword={kw}&token={token}`，响应
//!      `{"success":true,"data":[{"alikey","title","creatime"}]}`；
//!   3. 逐条 `vod_id = "https://www.aliyundrive.com/s/" + alikey`、`vod_name = title`、
//!      `vod_remarks = creatime[..10]`（`creatime` 为空则空串）。
//! * `detailContent`：条目 id 即阿里分享链接 → `AliClient` 匿名 `share_token` →
//!   `browse` 根目录（无视频时展开最多 5 个一级目录）→ 剧集 id `"{share_id}|{fid}"`；
//!   线路名「阿里」。
//! * `playerContent`：拆 `"{share_id}|{fid}"` → 复用 `share_token` →
//!   `play_url`（转码）失败回退 `download_url`（原码流）→
//!   `{"parse":0,"playUrl":"","url":"<直链>","header":{UA,Referer}}`。
//!
//! 有意偏差（桌面端能力边界 / 上游现状）：
//! 1. 上游 `gitcafe.net/tool/alipaper/` 在 2026-11 探测已不可用（fetch failed），
//!    搜索链路只做 mock 验收；`ext` 为 JSON 时可注入 `search_base` / `ali_base` /
//!    `ali_cookie`（仅测试/镜像钩子，原类 ext 是 Ali proxy token 文本）。
//! 2. 原类播放依赖 Ali 引擎的本地代理（`parse:0` + 代理地址）；桌面端不做本地代理，
//!    直接返回阿里官方直链（原码流或转码），语义等价但地址不同（记偏差）。
//! 3. 原类详情由引擎返回完整 `vod_play_*` 字符串；桌面端按核心模型映射为
//!    `VideoDetail.episodes`（线路名「阿里」）。
//! 4. 搜索型蜘蛛没有首页/分类：`categories`/`videos` 返回空列表（与原类继承
//!    默认实现的行为一致）。
//! 5. `share_token` 失效/需提取码时返回 `catvod.result.parse` 明确错误，而不是
//!    原类可能的空串（桌面端错误码契约要求）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, post_form_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::{is_video_name, AliClient};
use crate::pan::{parse_share_link, PanConfig, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};

/// 本家族覆盖的 api。
pub const API: &str = "csp_XiaoZhiTiao";

/// 原类 `f`：小纸条搜索接口（POST 表单）。
const SEARCH_BASE: &str = "https://gitcafe.net/tool/alipaper/";

/// 原类 `g`：条目 id 的阿里分享前缀。
const SHARE_PREFIX: &str = "https://www.aliyundrive.com/s/";

/// 原类 `b()` 固定 UA（Chrome/122）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

/// 阿里分享页 Referer（播放请求头）。
const REFERER: &str = "https://www.aliyundrive.com/";

/// token 有效期（原类 `now + 86400000`）。
const TOKEN_TTL_MS: u64 = 86_400_000;

/// 详情线路名（原类由引擎生成，桌面端固定）。
const GROUP: &str = "阿里";

/// 详情展开的一级目录上限（防止分享内目录过多导致请求风暴）。
const MAX_DIRS: usize = 5;

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
    h.insert("Host".to_string(), "gitcafe.net".to_string());
    h.insert("User-Agent".to_string(), UA.to_string());
    h
}

fn normalize_base(base: &str) -> String {
    let mut s = base.trim().to_string();
    if !s.is_empty() && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// ext JSON 测试钩子（原类 ext 是 Ali proxy token 文本，故非 JSON 时全部走默认）。
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
            &normalize_base(&conf.ali_base),
        ))
    }
}

/* ============================================================
 * token 缓存（原类实例字段 `d` + `e`）
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

/// 懒加载 token（原类 `if (isEmpty || expired) { action=get_token }`）。
fn ensure_token(cfg: &TvConfig, site: &Site, base: &str) -> Result<String> {
    let now = now_ms();
    if let Some(token) = cached_token(now) {
        return Ok(token);
    }
    let client = client_for(&cfg.http)?;
    let mut form = HashMap::new();
    form.insert("action".to_string(), "get_token".to_string());
    form.insert("from".to_string(), "web".to_string());
    let body = post_form_text(&client, base, &headers(), &form)?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| parse_err(site, format!("alipaper/get_token 响应不是 JSON: {e}")))?;
    let success = value
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let token = value
        .get("data")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !success || token.is_empty() {
        return Err(parse_err(
            site,
            "csp_XiaoZhiTiao 取 token 失败（上游 gitcafe.net 已停放？）",
        ));
    }
    store_token(&token, now);
    Ok(token)
}

/* ============================================================
 * 搜索（原类 `searchContent`）
 * ============================================================ */

/// 解析 `action=search` 响应（`{"success":bool,"data":[{alikey,title,creatime}]}`）。
pub fn parse_search_json(body: &str, site: &Site) -> Result<Vec<VideoItem>> {
    let value: Value = serde_json::from_str(body)
        .map_err(|e| parse_err(site, format!("alipaper/search 响应不是 JSON: {e}")))?;
    if !value
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for item in value
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let alikey = item.get("alikey").and_then(|v| v.as_str()).unwrap_or("").trim();
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        if alikey.is_empty() || title.is_empty() {
            continue;
        }
        let creatime = item
            .get("creatime")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let remark = if creatime.is_empty() {
            None
        } else {
            Some(creatime.chars().take(10).collect::<String>())
        };
        out.push(VideoItem {
            id: format!("{SHARE_PREFIX}{alikey}"),
            name: title.to_string(),
            cover: None,
            remark,
        });
    }
    Ok(out)
}

/// 搜索入口（`sources/spider.rs` 的 5 入口之一）。
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
    form.insert("keyword".to_string(), keyword.to_string());
    form.insert("token".to_string(), token);
    let body = post_form_text(&client, &base, &headers(), &form)?;
    parse_search_json(&body, site)
}

/* ============================================================
 * 分享 / 剧集 id
 * ============================================================ */

fn share_url(id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http") {
        id.to_string()
    } else {
        format!("{SHARE_PREFIX}{id}")
    }
}

/// 剧集 id：`"{share_id}|{file_id}"`（分享 id 与文件 id 均是安全字符）。
pub fn episode_id(share_id: &str, file_id: &str) -> String {
    format!("{share_id}|{file_id}")
}

/// 拆剧集 id。
pub fn split_episode_id(id: &str) -> Option<(String, String)> {
    let (share_id, file_id) = id.trim().split_once('|')?;
    let share_id = share_id.trim();
    let file_id = file_id.trim();
    if share_id.is_empty() || file_id.is_empty() {
        return None;
    }
    Some((share_id.to_string(), file_id.to_string()))
}

/* ============================================================
 * 详情（原类继承 `Ali.detailContent`）
 * ============================================================ */

fn ali_file_entry(file: crate::pan::ali::AliFile) -> Option<crate::pan::PanEntry> {
    if file.is_dir() || !is_video_name(&file.file_name) {
        return None;
    }
    Some(crate::pan::PanEntry {
        fid: file.file_id,
        name: file.file_name,
        is_dir: false,
        size: file.size,
        thumbnail: file.thumbnail,
    })
}

fn collect_videos(
    client: &AliClient,
    share_id: &str,
    share_token: &str,
    files: &[crate::pan::PanEntry],
) -> Vec<crate::pan::PanEntry> {
    let mut videos: Vec<crate::pan::PanEntry> = files
        .iter()
        .filter(|f| !f.is_dir && is_video_name(&f.name))
        .cloned()
        .collect();
    if !videos.is_empty() {
        return videos;
    }
    for dir in files.iter().filter(|f| f.is_dir).take(MAX_DIRS) {
        if let Ok(page) = client.list_all(share_id, share_token, &dir.fid) {
            videos.extend(page.items.into_iter().filter_map(ali_file_entry));
        }
    }
    videos
}

/// 详情入口：`id` 为阿里分享链接（原类 `detailContent` 的 `list[0]`）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let url = share_url(id);
    let link = parse_share_link(&url)
        .ok_or_else(|| parse_err(site, format!("csp_XiaoZhiTiao 无法识别分享链接: {id}")))?;
    if link.provider != PanProvider::Ali {
        return Err(parse_err(
            site,
            format!("csp_XiaoZhiTiao 仅支持阿里分享，收到: {}", link.provider.name()),
        ));
    }
    let conf = pan_config(site);
    let client = ali_client(cfg, &conf)?;
    let token = client.share_token(&link.share_id, &link.passcode)?;
    if !token.is_valid() {
        return Err(parse_err(
            site,
            format!(
                "阿里分享 {} 无有效 share_token（可能需要提取码或已失效）",
                link.share_id
            ),
        ));
    }
    let browse = client.browse(&link)?;
    let files = collect_videos(&client, &link.share_id, &token.share_token, &browse.files);
    if files.is_empty() {
        return Err(parse_err(site, "csp_XiaoZhiTiao 分享内没有可播放视频"));
    }
    let title = browse.title.trim().to_string();
    let name = if title.is_empty() {
        format!("阿里分享 {}", link.share_id)
    } else {
        title
    };
    let episodes: Vec<Episode> = files
        .iter()
        .map(|file| Episode {
            title: file.name.clone(),
            url: episode_id(&link.share_id, &file.fid),
        })
        .collect();
    let cover = files
        .iter()
        .map(|f| f.thumbnail.trim())
        .find(|t| !t.is_empty())
        .map(|t| t.to_string());
    Ok(VideoDetail {
        id: id.trim().to_string(),
        name: name.clone(),
        cover,
        desc: Some(format!("阿里云盘分享：{name}")),
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: GROUP.to_string(),
            items: episodes,
        }],
    })
}

/* ============================================================
 * 播放（原类继承 `Ali.playerContent` → Ali 引擎）
 * ============================================================ */

/// 播放入口：`id` 为 `"{share_id}|{file_id}"`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let (share_id, file_id) = split_episode_id(id)
        .ok_or_else(|| parse_err(site, format!("csp_XiaoZhiTiao 播放 id 非法: {id}")))?;
    let link = parse_share_link(&share_url(&share_id))
        .ok_or_else(|| parse_err(site, format!("csp_XiaoZhiTiao 无法识别分享 id: {share_id}")))?;
    let conf = pan_config(site);
    let client = ali_client(cfg, &conf)?;
    let token = client.share_token(&link.share_id, &link.passcode)?;
    if !token.is_valid() {
        return Err(parse_err(
            site,
            format!("阿里分享 {} 无有效 share_token", link.share_id),
        ));
    }
    let url = match client.play_url(&link.share_id, &file_id, &token.share_token) {
        Ok(url) if !url.trim().is_empty() => url,
        _ => client.download_url(&link.share_id, &file_id, &token.share_token)?,
    };
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": { "User-Agent": UA, "Referer": REFERER },
    }))
}

/* ============================================================
 * 搜索型蜘蛛：没有首页 / 分类
 * ============================================================ */

/// 分类为空（原类未覆写 `homeContent` 的 category 部分）。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(Vec::new())
}

/// 分类内容为空（搜索型蜘蛛）。
pub fn videos(
    _cfg: &TvConfig,
    _site: &Site,
    _cat: &str,
    _page: u32,
) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Value) -> Site {
        Site {
            key: "xiaozhitiao".to_string(),
            name: "小纸条".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(ext),
            ..Default::default()
        }
    }

    const SEARCH_JSON: &str = r#"{"success":true,"data":[
        {"alikey":"abc123","title":"三体 全集","creatime":"2024-05-01 12:00:00"},
        {"alikey":"","title":"坏条目","creatime":"2024-01-01"},
        {"alikey":"def456","title":"无时间"}
    ]}"#;

    #[test]
    fn handles_accepts_api_case_insensitively() {
        assert!(handles("csp_XiaoZhiTiao"));
        assert!(handles("  CSP_xiaozhitiao "));
        assert!(!handles("csp_Ali"));
        assert!(!handles(""));
    }

    #[test]
    fn episode_id_roundtrip() {
        let id = episode_id("share-1", "file-2");
        assert_eq!(id, "share-1|file-2");
        assert_eq!(
            split_episode_id(&id),
            Some(("share-1".to_string(), "file-2".to_string()))
        );
    }

    #[test]
    fn split_episode_id_rejects_malformed() {
        assert_eq!(split_episode_id("only-share"), None);
        assert_eq!(split_episode_id("|file"), None);
        assert_eq!(split_episode_id("share|"), None);
        assert_eq!(split_episode_id(""), None);
    }

    #[test]
    fn share_url_prefixes_plain_ids_only() {
        assert_eq!(share_url("abc"), format!("{SHARE_PREFIX}abc"));
        assert_eq!(share_url("  abc  "), format!("{SHARE_PREFIX}abc"));
        assert_eq!(
            share_url("https://www.alipan.com/s/xyz"),
            "https://www.alipan.com/s/xyz"
        );
    }

    #[test]
    fn parse_search_json_parses_items_and_skips_invalid() {
        let items = parse_search_json(SEARCH_JSON, &site(Value::Null)).expect("合法 JSON");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, format!("{SHARE_PREFIX}abc123"));
        assert_eq!(items[0].name, "三体 全集");
        assert_eq!(items[0].remark.as_deref(), Some("2024-05-01"));
        assert_eq!(items[1].id, format!("{SHARE_PREFIX}def456"));
        assert_eq!(items[1].remark, None);
    }

    #[test]
    fn parse_search_json_false_success_is_empty() {
        assert!(parse_search_json(r#"{"success":false,"data":[]}"#, &site(Value::Null))
            .expect("合法 JSON")
            .is_empty());
        assert!(parse_search_json(r#"{"success":true}"#, &site(Value::Null))
            .expect("合法 JSON")
            .is_empty());
    }

    #[test]
    fn parse_search_json_rejects_malformed_json() {
        assert!(parse_search_json("not-json", &site(Value::Null)).is_err());
    }

    #[test]
    fn ext_conf_reads_test_hooks() {
        let s = site(json!({
            "search_base": "http://127.0.0.1:9/alipaper",
            "ali_base": "http://127.0.0.1:9/ali",
            "cookie": "ck=1"
        }));
        let conf = ext_conf(&s);
        assert_eq!(conf.search_base, "http://127.0.0.1:9/alipaper");
        assert_eq!(conf.ali_base, "http://127.0.0.1:9/ali");
        assert_eq!(conf.ali_cookie, "ck=1");
        // 真实站点 ext 是 `./lib/token.json` 文本，不是 JSON → 全部默认。
        let real = ext_conf(&site(json!("./lib/token.json")));
        assert!(real.search_base.is_empty() && real.ali_base.is_empty());
    }

    #[test]
    fn search_base_defaults_and_normalizes_trailing_slash() {
        assert_eq!(search_base(&site(Value::Null)), SEARCH_BASE);
        assert_eq!(
            search_base(&site(json!({"search_base": "http://127.0.0.1:9/a"}))),
            "http://127.0.0.1:9/a/"
        );
    }

    #[test]
    fn pan_config_merges_ext_hooks() {
        let conf = pan_config(&site(json!({
            "ali_base": "http://127.0.0.1:9/ali/",
            "ali_cookie": "ck=2"
        })));
        assert_eq!(conf.ali_base, "http://127.0.0.1:9/ali/");
        assert_eq!(conf.effective_ali_cookie(), "ck=2");
        // 非 JSON ext（真实站点 token 文本）→ 空配置。
        let real = pan_config(&site(json!("./lib/token.json")));
        assert!(real.ali_base.is_empty() && real.effective_ali_cookie().is_empty());
    }

    #[test]
    fn token_is_fresh_respects_expiry() {
        let entry = Some(("tok".to_string(), 100));
        assert!(token_is_fresh(&entry, 50));
        assert!(!token_is_fresh(&entry, 100));
        assert!(!token_is_fresh(&Some((String::new(), 100)), 50));
        assert!(!token_is_fresh(&None, 50));
    }

    #[test]
    fn detail_rejects_unparsable_share() {
        let err = detail(&TvConfig::default(), &site(json!("")), "").expect_err("空 id 应报错");
        assert!(format!("{err}").contains("catvod."), "错误应为 Catvod 错误码: {err}");
    }

    #[test]
    fn player_content_rejects_malformed_id() {
        let err = player_content(
            &TvConfig::default(),
            &site(json!("")),
            "flag",
            "no-pipe-id",
        )
        .expect_err("非法 id 应报错");
        assert!(format!("{err}").contains("catvod."));
    }
}
