//! type=3 `.js` spider 运行时（catvod / drpy 子集）
//!
//! 脚本来源（`site.api`）：
//! - `http(s)://...`：用 `crate::http` 拉取（套用 `cfg.http` 的 UA / 超时 / 重试）
//! - `file://...`：读取本地脚本
//! - `inline:...`：内联脚本文本（测试 / 调试用）
//!
//! 入口约定（对齐 jun 版 `crawler/js/JsSpider.java`）：
//! 1. 全局函数 `home/homeVod/category/detail/search/play/live/sniffer/isVideo`；或
//! 2. `__jsEvalReturn()` 返回一个包含上述函数的对象（cat 风格入口）；或
//! 3. `__JS_SPIDER__`（对象，或返回对象的函数）；或
//! 4. `export default {...}`（加载前重写为 `globalThis.__JS_SPIDER__ = {...}`）。
//!
//! 注意：每次接口调用都会新建 Runtime 并重新执行脚本 + `init(...)`，因此脚本里的
//! 模块级变量不会跨调用保留；需要跨调用共享的数据请用注入的 `local.get/set`。
//!
//! 返回值归一：JS 字符串优先按 JSON 解析；对象/数组经 `JSON.stringify` 后再解析。
//!
//! 兼容性说明：
//! - `await req(...)` 这类「同步宿主可完成」的 Promise 已支持（驱动 QuickJS job 队列）；
//! - `pdfh`/`pdfa`/`pd`/`pdfla` 由 `html_rule` 提供（Jsoup 规则子集）。
//! - ES module 相对 `import` / CommonJS `require`：按脚本来源 URL 解析并加载（内置 lib 优先），
//!   依赖文本在进程内缓存；入口脚本的 `module.exports` 也会映射到 `__JS_SPIDER__`。
//! 明确不支持（会给出可读错误，不静默）：
//! - 需要真实事件循环的异步 I/O（定时器回调不会在 await 后推进）
//! - 站点级 `jar`（jsapi dex）、QuickJS 字节码（`//bb` / `//DRPY`）
//! - `proxy://` 本地代理桥（`js2Proxy` 返回空串，`getProxy` 原样返回）

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::sandbox::{JsArg, Sandbox, SandboxError};

/// 单次脚本调用超时（首页/详情 30s、播放 15s，统一取 20s）
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(20);
/// 错误信息里截断超长 api（inline 脚本可能很长）
const MAX_LABEL_LEN: usize = 120;

/* ============================================================
 * 对外接口
 * ============================================================ */

/// 首页分类：调用 `home(true)`，解析 `{class:[{type_id,type_name}]}`
pub fn categories(site: &Site, cfg: &TvConfig) -> Result<Vec<Category>> {
    let value = call_json(site, cfg, "home", vec![JsArg::Bool(true)])?;

    let classes = value
        .get("class")
        .or_else(|| value.get("classes"))
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| value.as_array().cloned())
        .ok_or_else(|| {
            CoreError::Config(format!(
                "站点「{}」的 JS home() 未返回 class 数组: {}",
                site.name,
                truncate(&value.to_string(), 200)
            ))
        })?;

    Ok(classes
        .iter()
        .filter_map(|c| {
            let type_id = pick_str(c, &["type_id", "typeId", "id", "tid"])?;
            if type_id.trim().is_empty() {
                return None;
            }
            let type_name = pick_str(c, &["type_name", "typeName", "name"])
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| type_id.clone());
            Some(Category { type_id, type_name })
        })
        .collect())
}

/// 分类列表：调用 `category(tid, pg, true, {})`
pub fn category(site: &Site, cfg: &TvConfig, tid: &str, page: u32) -> Result<Vec<VideoItem>> {
    let value = call_json(
        site,
        cfg,
        "category",
        vec![
            JsArg::Str(tid.to_string()),
            JsArg::Str(page.to_string()),
            JsArg::Bool(true),
            JsArg::Json(json!({})),
        ],
    )?;
    Ok(items_from(&value))
}

/// 详情：调用 `detail(id)`，解析 `{list:[{...vod_play_from/vod_play_url 或 episodes}]}`
pub fn detail(site: &Site, cfg: &TvConfig, id: &str) -> Result<VideoDetail> {
    let value = call_json(site, cfg, "detail", vec![JsArg::Str(id.to_string())])?;

    let obj = first_video(&value).ok_or_else(|| {
        CoreError::Config(format!(
            "站点「{}」的 JS detail() 未返回影片: {}",
            site.name,
            truncate(&value.to_string(), 200)
        ))
    })?;

    let episodes = episodes_of(obj);
    Ok(VideoDetail {
        id: pick_str(obj, &["vod_id", "id"]).unwrap_or_else(|| id.to_string()),
        name: pick_str(obj, &["vod_name", "name", "title"]).unwrap_or_default(),
        cover: pick_str(obj, &["vod_pic", "vod_pic_thumb", "pic", "cover"]),
        desc: pick_str(obj, &["vod_content", "vod_blurb", "content", "desc"]),
        year: pick_str(obj, &["vod_year", "year"]),
        area: pick_str(obj, &["vod_area", "area"]),
        director: pick_str(obj, &["vod_director", "director"]),
        actor: pick_str(obj, &["vod_actor", "actor"]),
        episodes,
    })
}

/// 搜索：调用 `search(keyword, quick)`；脚本没有实现 search 时返回空数组（不算错误）
pub fn search(site: &Site, cfg: &TvConfig, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    let sandbox = sandbox_for(site, cfg)?;
    match sandbox.call(
        "search",
        &[JsArg::Str(keyword.to_string()), JsArg::Bool(quick)],
    ) {
        Ok(value) => Ok(items_from(&value)),
        Err(SandboxError::Missing(_)) => Ok(Vec::new()),
        Err(e) => Err(sandbox_err(site, "search", e)),
    }
}

/// 播放：调用 `play(flag, id, vipFlags)`，返回脚本给的 JSON（由 `source::get_play` 归一）
pub fn player_content(
    site: &Site,
    cfg: &TvConfig,
    flag: &str,
    id: &str,
    vip_flags: &[String],
) -> Result<Json> {
    let vip = Json::Array(vip_flags.iter().map(|s| Json::String(s.clone())).collect());
    call_json(
        site,
        cfg,
        "play",
        vec![
            JsArg::Str(flag.to_string()),
            JsArg::Str(id.to_string()),
            JsArg::Json(vip),
        ],
    )
}

/// 直播：调用 `live(url)`，返回脚本文本（通常是 m3u/txt 或 JSON 文本）
pub fn live(site: &Site, cfg: &TvConfig, url: &str) -> Result<String> {
    let value = call_json(site, cfg, "live", vec![JsArg::Str(url.to_string())])?;
    Ok(match value {
        Json::String(s) => s,
        other => other.to_string(),
    })
}

/// 是否需要手动嗅探（`sniffer()`）；脚本没实现时按 false 处理（对齐 jun）
pub fn sniffer(site: &Site, cfg: &TvConfig) -> Result<bool> {
    let sandbox = sandbox_for(site, cfg)?;
    match sandbox.call("sniffer", &[]) {
        Ok(value) => Ok(value.as_bool().unwrap_or(false)),
        Err(SandboxError::Missing(_)) => Ok(false),
        Err(e) => Err(sandbox_err(site, "sniffer", e)),
    }
}

/// URL 是否视频格式（`isVideo(url)`）；脚本没实现时按 false 处理（对齐 jun）
pub fn is_video(site: &Site, cfg: &TvConfig, url: &str) -> Result<bool> {
    let sandbox = sandbox_for(site, cfg)?;
    match sandbox.call("isVideo", &[JsArg::Str(url.to_string())]) {
        Ok(value) => Ok(value.as_bool().unwrap_or(false)),
        Err(SandboxError::Missing(_)) => Ok(false),
        Err(e) => Err(sandbox_err(site, "isVideo", e)),
    }
}

/* ============================================================
 * 脚本加载 / 沙箱构建
 * ============================================================ */

fn site_headers(site: &Site) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(obj) = site.ext_object() {
        for key in ["referer", "cookie", "user-agent", "User-Agent"] {
            if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
                if !v.trim().is_empty() {
                    out.insert(key.to_string(), v.to_string());
                }
            }
        }
    }
    out
}

fn script_label(site: &Site) -> String {
    let api = site.api.trim();
    let name = if site.name.trim().is_empty() {
        "未命名站点".to_string()
    } else {
        site.name.clone()
    };
    format!("{name}[{}]", truncate(api, MAX_LABEL_LEN))
}

fn sandbox_for(site: &Site, cfg: &TvConfig) -> Result<Sandbox> {
    let mut script = load_script(site, cfg)?;
    if is_drpy_site(site) {
        return Err(CoreError::Config(format!(
            "站点「{}」使用 drpy/drpy2 规则框架，桌面端暂未适配（依赖 cheerio/ES module）；请改用普通 catvod JS 或原生规则引擎",
            site.name
        )));
    }
    if is_quickjs_bytecode(&script) {
        return Err(CoreError::Config(format!(
            "站点「{}」的 JS spider 是 QuickJS 字节码/DRPY 缓存（//bb、//DRPY），桌面端暂不支持：\n\
             字节码与编译它的 QuickJS 引擎版本强绑定，跨引擎加载会被 JS_ReadObject 拒绝；\n\
             请改用同一站点的 .js 源码 spider（MisaTV / Win-box 同样不支持该格式）",
            site.name
        )));
    }
    let label = script_label(site);
    let is_cat = script.contains("__jsEvalReturn") || script.contains("__JS_SPIDER__");
    script = prepare_script(script);

    let headers = site_headers(site);
    let base_url = script_base(site);
    // cat 风格脚本收到 cfg 对象 {stype,skey,ext}；普通脚本收到 ext 本身（对齐 JsSpider.init）
    let init = if is_cat {
        Some(cat_cfg(site))
    } else {
        site.ext.clone()
    };

    // 登记到本地代理桥：收到 /proxy?do=js&siteKey=... 时重建沙箱并调用 proxy(params)
    crate::proxy::register_js_proxy(
        &site.key,
        script.clone(),
        label.clone(),
        cfg.http.clone(),
        headers.clone(),
        init.clone(),
        base_url.clone(),
    );

    let mut sandbox = Sandbox::new(script, label, cfg.http.clone())
        .with_timeout(SCRIPT_TIMEOUT)
        .with_headers(headers)
        .with_site_key(site.key.clone());
    if let Some(base) = base_url {
        sandbox = sandbox.with_base_url(base);
    }
    if let Some(init) = init {
        sandbox = sandbox.with_init(init);
    }
    Ok(sandbox)
}

fn cat_cfg(site: &Site) -> Json {
    let ext = match &site.ext {
        Some(v) => v.clone(),
        None => Json::String(String::new()),
    };
    json!({
        "stype": 3,
        "skey": site.key,
        "ext": ext,
    })
}

/// 脚本来源 URL（ES module / require 的相对路径解析基准）；inline 脚本没有基准
fn script_base(site: &Site) -> Option<String> {
    let api = site.api.trim();
    if api.starts_with("http://") || api.starts_with("https://") || api.starts_with("file://") {
        Some(api.to_string())
    } else {
        None
    }
}

/// 加载脚本文本；http(s) 走 `crate::http`，file:// 读本地，inline: 直接使用
fn load_script(site: &Site, cfg: &TvConfig) -> Result<String> {
    let api = site.api.trim();
    if api.is_empty() {
        return Err(CoreError::Config(format!(
            "站点「{}」的 api 为空，无法加载 JS spider",
            site.name
        )));
    }

    if api.starts_with("http://") || api.starts_with("https://") {
        let client = http::client_for(&cfg.http)?;
        let text = http::get_text(&client, api, &site_headers(site), cfg.http.retry)?;
        if invalid_script(&text) {
            return Err(CoreError::Config(format!(
                "站点「{}」的 JS spider 地址返回的不是脚本（可能是 404/HTML）: {}",
                site.name, api
            )));
        }
        return Ok(text);
    }

    if let Some(rest) = api.strip_prefix("inline:") {
        return Ok(rest.to_string());
    }

    if api.starts_with("file://") {
        let parsed = url::Url::parse(api)
            .map_err(|e| CoreError::Config(format!("非法 file:// 脚本地址 {api}: {e}")))?;
        let path = parsed
            .to_file_path()
            .map_err(|_| CoreError::Config(format!("无法解析本地脚本路径: {api}")))?;
        return std::fs::read_to_string(&path).map_err(|e| {
            CoreError::Config(format!(
                "读取本地 JS spider 失败（{}）: {e}",
                path.display()
            ))
        });
    }

    Err(CoreError::Config(format!(
        "站点「{}」的 JS spider 地址不受支持: {api}（仅支持 http(s) / file:// / inline:）",
        site.name
    )))
}

/// 明显不是脚本的响应（HTML 错误页 / 404 文本）
fn invalid_script(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    lower.starts_with('<')
        || lower.starts_with("{\"code\":404")
        || lower.starts_with("404")
        || lower.starts_with("not found")
}

/// 轻量预处理：去 BOM；把 `export default` 重写成全局赋值（import 由 sandbox 剥离）
fn prepare_script(mut script: String) -> String {
    if let Some(rest) = script.strip_prefix('\u{feff}') {
        script = rest.to_string();
    }
    if script.contains("export default") {
        script = script.replacen("export default", "globalThis.__JS_SPIDER__ =", 1);
    }
    script
}

fn is_quickjs_bytecode(script: &str) -> bool {
    let head = script.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("//bb") || head.starts_with("//DRPY")
}

fn is_drpy_site(site: &Site) -> bool {
    let api = site.api.to_ascii_lowercase();
    if api.contains("drpy") {
        return true;
    }
    site.ext
        .as_ref()
        .map(|v| v.to_string().to_ascii_lowercase().contains("drpy"))
        .unwrap_or(false)
}

fn call_json(site: &Site, cfg: &TvConfig, func: &str, args: Vec<JsArg>) -> Result<Json> {
    let sandbox = sandbox_for(site, cfg)?;
    sandbox
        .call(func, &args)
        .map_err(|e| sandbox_err(site, func, e))
}

/// 把沙箱错误转成带站点/函数上下文的 `CoreError`
fn sandbox_err(site: &Site, func: &str, e: SandboxError) -> CoreError {
    match e {
        SandboxError::Missing(f) => CoreError::Config(format!(
            "站点「{}」的 JS spider 未实现 {}() 函数（type=3 需要 home/category 等入口）",
            site.name, f
        )),
        other => CoreError::Config(format!(
            "站点「{}」调用 JS {func}() 失败: {}",
            site.name, other
        )),
    }
}

/* ============================================================
 * JS 返回值解析
 * ============================================================ */

/// 视图列表：`{list:[...]}` / 根数组 / `{data:{list:[...]}}`
fn items_from(value: &Json) -> Vec<VideoItem> {
    let list = value
        .get("list")
        .and_then(|v| v.as_array())
        .or_else(|| {
            value
                .get("data")
                .and_then(|d| d.get("list"))
                .and_then(|v| v.as_array())
        })
        .or_else(|| value.as_array());
    let Some(list) = list else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|v| {
            let id = pick_str(v, &["vod_id", "id", "video_id"])?;
            if id.trim().is_empty() {
                return None;
            }
            Some(VideoItem {
                id,
                name: pick_str(v, &["vod_name", "name", "title"]).unwrap_or_default(),
                cover: pick_str(v, &["vod_pic", "pic", "cover"]),
                remark: pick_str(v, &["vod_remarks", "remark", "remarks", "note"]),
            })
        })
        .collect()
}

fn first_video(value: &Json) -> Option<&Json> {
    if let Some(list) = value.get("list").and_then(|v| v.as_array()) {
        return list.first();
    }
    if let Some(list) = value
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
    {
        return list.first();
    }
    if value.is_object() && pick_str(value, &["vod_id", "id", "vod_name", "name"]).is_some() {
        return Some(value);
    }
    None
}

/// 详情里的多线路选集：优先 `vod_play_from` + `vod_play_url`，其次自定义 `episodes`
fn episodes_of(obj: &Json) -> Vec<EpisodeGroup> {
    let play_from = pick_str(obj, &["vod_play_from", "play_from"]).unwrap_or_default();
    let play_url = pick_str(obj, &["vod_play_url", "play_url"]).unwrap_or_default();
    if !play_url.trim().is_empty() {
        return parse_play_url(&play_from, &play_url);
    }
    parse_episodes_field(obj)
}

/// 解析 TVBox / 苹果CMS 的选集字段
///
/// - 线路之间用 `$$$` 分隔（对应 `vod_play_from` 的线路名）
/// - 线路内各集用 `#` 分隔，每集 `名称$地址`；没有名称时用「第N集」
fn parse_play_url(play_from: &str, play_url: &str) -> Vec<EpisodeGroup> {
    let bodies: Vec<&str> = play_url.split("$$$").collect();
    let names: Vec<&str> = play_from.split("$$$").collect();

    bodies
        .iter()
        .enumerate()
        .map(|(i, body)| {
            let group = names
                .get(i)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if bodies.len() == 1 {
                        "默认".to_string()
                    } else {
                        format!("线路{}", i + 1)
                    }
                });
            let items: Vec<Episode> = body
                .split('#')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .enumerate()
                .map(|(idx, raw)| match raw.find('$') {
                    Some(pos) => {
                        let title = raw[..pos].trim();
                        let url = raw[pos + 1..].trim();
                        Episode {
                            title: if title.is_empty() {
                                format!("第{}集", idx + 1)
                            } else {
                                title.to_string()
                            },
                            url: url.to_string(),
                        }
                    }
                    None => Episode {
                        title: format!("第{}集", idx + 1),
                        url: raw.to_string(),
                    },
                })
                .collect();
            EpisodeGroup { group, items }
        })
        .filter(|g| !g.items.is_empty())
        .collect()
}

/// 兼容非标准脚本：`episodes: [{group|name, items|urls: [{title|name, url}]}]`
fn parse_episodes_field(obj: &Json) -> Vec<EpisodeGroup> {
    let Some(groups) = obj.get("episodes").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    groups
        .iter()
        .map(|g| {
            let group =
                pick_str(g, &["group", "name", "flag"]).unwrap_or_else(|| "默认".to_string());
            let list = g
                .get("items")
                .or_else(|| g.get("urls"))
                .or_else(|| g.get("episodes"))
                .and_then(|v| v.as_array());
            let items = list
                .map(|list| {
                    list.iter()
                        .enumerate()
                        .filter_map(|(idx, item)| {
                            let url = pick_str(item, &["url", "play_url", "link"])?;
                            let title = pick_str(item, &["title", "name"])
                                .unwrap_or_else(|| format!("第{}集", idx + 1));
                            Some(Episode { title, url })
                        })
                        .collect()
                })
                .unwrap_or_default();
            EpisodeGroup { group, items }
        })
        .filter(|g| !g.items.is_empty())
        .collect()
}

/// 取第一个存在的字符串字段（数字/布尔也转成字符串，兼容 id 为数字的脚本）
fn pick_str(obj: &Json, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            match v {
                Json::String(s) => return Some(s.clone()),
                Json::Number(n) => return Some(n.to_string()),
                Json::Bool(b) => return Some(b.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_play_url_tvbox_style() {
        let groups = parse_play_url(
            "线路A$$$线路B",
            "第1集$http://a/1.m3u8#第2集$http://a/2.m3u8$$$第1集$http://b/1.mp4",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "线路A");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[1].title, "第2集");
        assert_eq!(groups[0].items[1].url, "http://a/2.m3u8");
        assert_eq!(groups[1].group, "线路B");
        assert_eq!(groups[1].items[0].url, "http://b/1.mp4");
    }

    #[test]
    fn parse_play_url_without_names() {
        let groups = parse_play_url("", "http://a/1.m3u8#http://a/2.m3u8");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group, "默认");
        assert_eq!(groups[0].items[0].title, "第1集");
        assert_eq!(groups[0].items[1].title, "第2集");
    }

    #[test]
    fn prepare_script_rewrites_export_default() {
        let code = prepare_script("export default { home: function(){ return {} } }".into());
        assert!(code.starts_with("globalThis.__JS_SPIDER__ ="));
        let with_import = prepare_script("import x from 'y';\nexport default {}".into());
        assert!(!with_import.contains("export default"));
        assert!(with_import.contains("globalThis.__JS_SPIDER__"));
    }

    #[test]
    fn invalid_script_detection() {
        assert!(invalid_script("<html>404</html>"));
        assert!(invalid_script("{\"code\":404}"));
        assert!(invalid_script("   "));
        assert!(!invalid_script("function home(){}"));
    }
}
