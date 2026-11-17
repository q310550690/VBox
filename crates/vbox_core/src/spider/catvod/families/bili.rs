//! `csp_Bili` 家族：B 站（bilibili.com）原生实现。
//!
//! 来源：`com.github.catvod.spider.Bili`（`49fb705c-XbyQ.jar`，289 行，
//! jadx 完整反编译，无跳段；字符串由 `merge.WJz.d` 以 key `ZySGzj`
//! 做 hex→bytes 后逐字节 XOR 解出，已用 `_rea/tools/decode-xor.mjs` 全量解码）。
//!
//! 协议（与原实现一一对应）：
//! - `init(ext)`：ext 为 JSON（若以 `http` 开头则先 GET 再解析），字段
//!   `class`（分类数组 `{type_id,type_name}`）、`filter`（筛选项，桌面端无承载位，
//!   仅保留）、`cookie`（原站登录态；缺失时搜索接口会被风控返回 412）。
//! - 分类：直接来自 ext 的 `class`；`videos(cat,page)` 走搜索接口
//!   `https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={cat}&page={page}`
//!   （原实现以 typeId 作为 keyword，并允许 extend 覆盖 `tid`/追加参数）。
//! - 详情：`https://api.bilibili.com/x/web-interface/view?aid={id}` →
//!   `data.title/pic/tname/desc/duration/pages[]`，播放线路 `B站`，
//!   播放 url 为 `{part}$${aid}+ {cid}`（与 Android 版格式一致，播放时再拆）。
//! - 搜索：同搜索接口，`keyword=` + URL 编码关键词。
//! - 播放：`https://api.bilibili.com/x/player/playurl?avid={aid}&cid={cid}&qn=120&fourk=1`
//!   → `data.durl[0].url`，返回 `{url, header:{User-Agent, Referer}, parse:0, playUrl:""}`。
//!
//! 有意偏差（桌面端语义）：
//! 1. 支持 ext 内的 `apiBase` 覆盖（仅供 mock/镜像，原实现写死 API 域名）；
//! 2. 播放 id 解析更宽容：`{aid}+ {cid}`、`{aid}+{cid}`、纯 aid（自动取
//!    `view` 的首个分 P cid）都可；
//! 3. 搜索风控（HTTP 412/HTML）时返回 `ErrorClass::Parse` 并说明「需要 ext.cookie」，
//!    而不是静默返回空列表。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, VideoDetail, VideoItem};
use crate::model::TvConfig;

use super::super::contract::{CatvodError, ErrorClass};

/// 站点 api 名。
pub const API: &str = "csp_Bili";

/// 默认 API 根（可被 ext.apiBase 覆盖，仅测试/镜像用）。
const DEFAULT_API: &str = "https://api.bilibili.com";
/// 网页根（Referer 用）。
const WEB_BASE: &str = "https://www.bilibili.com";
/// 原实现 UA（WJz 解码所得）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/95.0.4638.54 Safari/537.36";
/// 8 秒超时（与主流程一致）。
const TIMEOUT_MS: u64 = 8_000;

/// ext 解析后的配置。
#[derive(Debug, Default, Clone)]
pub struct Config {
    pub classes: Vec<Category>,
    pub cookie: Option<String>,
    pub api_base: String,
}

impl Config {
    fn api(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }
}

fn site_error(site: &Site, class: ErrorClass, detail: impl Into<String>) -> CoreError {
    CatvodError::new(class, site, detail).into_core()
}

fn normalized_api(api: &str) -> String {
    api.trim().to_ascii_lowercase()
}

/// 是否由本家族处理。
pub fn handles(api: &str) -> bool {
    normalized_api(api) == API.to_ascii_lowercase()
}

fn headers(cfg: &TvConfig, cookie: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("User-Agent".to_string(), UA.to_string());
    map.insert("Referer".to_string(), WEB_BASE.to_string());
    if let Some(c) = cookie {
        if !c.trim().is_empty() {
            map.insert("Cookie".to_string(), c.trim().to_string());
        }
    }
    let _ = cfg;
    map
}

fn http_get(cfg: &TvConfig, url: &str, cookie: Option<&str>) -> Result<String> {
    let mut rules = cfg.http.clone();
    rules.timeout = TIMEOUT_MS;
    let client = crate::http::client_for(&rules)?;
    http::get_text(&client, url, &headers(cfg, cookie), 2)
}

fn parse_json(cfg: &TvConfig, site: &Site, url: &str, body: &str) -> Result<Value> {
    let _ = cfg;
    serde_json::from_str(body).map_err(|e| {
        site_error(
            site,
            ErrorClass::Parse,
            format!("Bilibili API 返回非 JSON（可能是风控页，需要 ext.cookie 或换网络）：{e}（url={url}）"),
        )
    })
}

fn ext_string(site: &Site) -> String {
    site.ext_string()
}

/// 读取 ext：若以 http(s) 开头则下载后解析，否则直接当作 JSON。
pub fn config(cfg: &TvConfig, site: &Site) -> Result<Config> {
    let raw = ext_string(site);
    let text = raw.trim().to_string();
    let mut conf = Config {
        api_base: DEFAULT_API.to_string(),
        ..Default::default()
    };
    if text.is_empty() {
        return Ok(conf);
    }
    let body = if text.starts_with("http://") || text.starts_with("https://") {
        http_get(cfg, &text, None)?
    } else if text.starts_with("clan://") {
        let rest = text.trim_start_matches("clan://");
        let url = if rest.starts_with("http") {
            rest.to_string()
        } else {
            format!("http://{rest}")
        };
        http_get(cfg, &url, None)?
    } else {
        text
    };
    let value: Value = serde_json::from_str(body.trim()).map_err(|e| {
        site_error(
            site,
            ErrorClass::Parse,
            format!("Bili ext 不是合法 JSON：{e}"),
        )
    })?;
    if let Some(classes) = value.get("class").and_then(|v| v.as_array()) {
        for item in classes {
            let id = item
                .get("type_id")
                .map(value_to_id)
                .unwrap_or_default();
            let name = item
                .get("type_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !id.is_empty() {
                conf.classes.push(Category {
                    type_id: id,
                    type_name: name,
                });
            }
        }
    }
    if let Some(cookie) = value.get("cookie").and_then(|v| v.as_str()) {
        if !cookie.trim().is_empty() {
            conf.cookie = Some(cookie.trim().to_string());
        }
    }
    if let Some(base) = value.get("apiBase").and_then(|v| v.as_str()) {
        if base.starts_with("http") {
            conf.api_base = base.trim_end_matches('/').to_string();
        }
    }
    Ok(conf)
}

fn value_to_id(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        s.trim().to_string()
    } else if let Some(n) = v.as_i64() {
        n.to_string()
    } else if let Some(n) = v.as_f64() {
        format!("{}", n as i64)
    } else {
        String::new()
    }
}

/// 分类来自 ext 的 `class`；没有 ext 时不提供分类（与原实现一致）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    Ok(config(cfg, site)?.classes)
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key)
        .map(|x| {
            if let Some(s) = x.as_str() {
                s.trim().to_string()
            } else if let Some(n) = x.as_i64() {
                n.to_string()
            } else {
                String::new()
            }
        })
        .unwrap_or_default()
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn cover_url(pic: &str) -> String {
    let pic = pic.trim();
    if pic.starts_with("//") {
        format!("https:{pic}")
    } else {
        pic.to_string()
    }
}

fn remark_of(duration: &Value) -> String {
    let raw = duration
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| duration.as_i64().map(|n| n.to_string()).unwrap_or_default());
    let head = raw.split(':').next().unwrap_or_default().trim();
    if head.is_empty() {
        String::new()
    } else {
        format!("{head}分钟")
    }
}

fn items_from_search(site: &Site, body: &Value) -> Vec<VideoItem> {
    let _ = site;
    let mut items = Vec::new();
    let Some(result) = body
        .get("data")
        .and_then(|d| d.get("result"))
        .and_then(|r| r.as_array())
    else {
        return items;
    };
    for row in result {
        let id = row
            .get("aid")
            .map(value_to_id)
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let title = strip_tags(&text_of(row, "title"));
        if title.is_empty() {
            continue;
        }
        let pic = cover_url(&text_of(row, "pic"));
        let remark = row.get("duration").map(remark_of).unwrap_or_default();
        items.push(VideoItem {
            id,
            name: title,
            cover: if pic.is_empty() { None } else { Some(pic) },
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    items
}

fn search_url(conf: &Config, keyword: &str, page: u32) -> String {
    let kw = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC);
    format!(
        "{}&page={}",
        conf.api(&format!(
            "/x/web-interface/search/type?search_type=video&keyword={kw}"
        )),
        page.max(1)
    )
}

fn fetch_search(cfg: &TvConfig, site: &Site, keyword: &str, page: u32) -> Result<Vec<VideoItem>> {
    let conf = config(cfg, site)?;
    let url = search_url(&conf, keyword, page);
    let body = http_get(cfg, &url, conf.cookie.as_deref())?;
    let value = parse_json(cfg, site, &url, &body)?;
    check_api_code(site, &value, "搜索")?;
    Ok(items_from_search(site, &value))
}

fn check_api_code(site: &Site, value: &Value, what: &str) -> Result<()> {
    let code = value.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    if code != 0 {
        let msg = value
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("未知错误");
        let hint = if code == -412 || code == 412 {
            "（-412/412 为风控：请在站点 ext 中配置 cookie）"
        } else {
            ""
        };
        return Err(site_error(
            site,
            ErrorClass::HostApi,
            format!("Bilibili {what}失败：code={code} {msg}{hint}"),
        ));
    }
    Ok(())
}

/// 分类列表：以 typeId 作为 keyword 走搜索接口（原实现行为）。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    fetch_search(cfg, site, cat.trim(), page)
}

/// 搜索：keyword 直接查询。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    fetch_search(cfg, site, keyword.trim(), 1)
}

fn detail_from_view(site: &Site, id: &str, body: &Value) -> Result<VideoDetail> {
    let data = body.get("data").ok_or_else(|| {
        site_error(site, ErrorClass::Parse, "Bilibili view 响应缺少 data")
    })?;
    let title = text_of(data, "title");
    if title.is_empty() {
        return Err(site_error(
            site,
            ErrorClass::Parse,
            "Bilibili view 响应缺少 title（视频可能已删除）",
        ));
    }
    let mut episodes = Vec::new();
    if let Some(pages) = data.get("pages").and_then(|p| p.as_array()) {
        for page in pages {
            let cid = page
                .get("cid")
                .map(value_to_id)
                .unwrap_or_default();
            if cid.is_empty() {
                continue;
            }
            let part = strip_tags(&text_of(page, "part"))
                .replace('$', "_")
                .replace('#', "_");
            let part = if part.is_empty() {
                format!("P{}", episodes.len() + 1)
            } else {
                part
            };
            episodes.push(Episode {
                title: part.clone(),
                url: format!("{id}+ {cid}"),
            });
        }
    }
    if episodes.is_empty() {
        return Err(site_error(
            site,
            ErrorClass::Parse,
            "Bilibili view 响应缺少 pages（无分 P 可播放）",
        ));
    }
    let desc = strip_tags(&text_of(data, "desc"));
    Ok(VideoDetail {
        id: id.to_string(),
        name: title,
        cover: {
            let pic = cover_url(&text_of(data, "pic"));
            if pic.is_empty() {
                None
            } else {
                Some(pic)
            }
        },
        desc: if desc.is_empty() { None } else { Some(desc) },
        year: None,
        area: {
            let tname = text_of(data, "tname");
            if tname.is_empty() { None } else { Some(tname) }
        },
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup {
            group: "B站".to_string(),
            items: episodes,
        }],
    })
}

/// 详情：`view?aid=`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let conf = config(cfg, site)?;
    let aid = id.trim().trim_start_matches("av").to_string();
    if aid.is_empty() || !aid.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return Err(site_error(
            site,
            ErrorClass::Parse,
            format!("Bilibili 详情 id 不是数字 aid：{id}"),
        ));
    }
    let url = conf.api(&format!("/x/web-interface/view?aid={aid}"));
    let body = http_get(cfg, &url, conf.cookie.as_deref())?;
    let value = parse_json(cfg, site, &url, &body)?;
    check_api_code(site, &value, "详情")?;
    detail_from_view(site, &aid, &value)
}

fn split_play_id(id: &str) -> (String, Option<String>) {
    if let Some((aid, cid)) = id.split_once('+') {
        (
            aid.trim().trim_start_matches("av").to_string(),
            Some(cid.trim().to_string()).filter(|c| !c.is_empty()),
        )
    } else {
        (id.trim().trim_start_matches("av").to_string(), None)
    }
}

fn first_cid(cfg: &TvConfig, site: &Site, conf: &Config, aid: &str) -> Result<String> {
    let url = conf.api(&format!("/x/web-interface/view?aid={aid}"));
    let body = http_get(cfg, &url, conf.cookie.as_deref())?;
    let value = parse_json(cfg, site, &url, &body)?;
    check_api_code(site, &value, "详情")?;
    value
        .get("data")
        .and_then(|d| d.get("pages"))
        .and_then(|p| p.as_array())
        .and_then(|pages| pages.first())
        .and_then(|p| p.get("cid"))
        .map(value_to_id)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| {
            site_error(
                site,
                ErrorClass::Parse,
                "Bilibili 视频没有分 P，无法取得 cid",
            )
        })
}

/// 播放：`playurl` 取 `durl[0].url`。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let conf = config(cfg, site)?;
    let (aid, cid) = split_play_id(id);
    if aid.is_empty() {
        return Err(site_error(
            site,
            ErrorClass::Parse,
            "Bilibili 播放 id 为空",
        ));
    }
    let cid = match cid {
        Some(c) => c,
        None => first_cid(cfg, site, &conf, &aid)?,
    };
    let url = conf.api(&format!(
        "/x/player/playurl?avid={aid}&cid={cid}&qn=120&fourk=1"
    ));
    let body = http_get(cfg, &url, conf.cookie.as_deref())?;
    let value = parse_json(cfg, site, &url, &body)?;
    check_api_code(site, &value, "播放地址")?;
    let play_url = value
        .get("data")
        .and_then(|d| d.get("durl"))
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|d| d.get("url"))
        .and_then(|u| u.as_str())
        .map(str::to_string)
        .unwrap_or_default();
    if play_url.is_empty() {
        return Err(site_error(
            site,
            ErrorClass::Parse,
            "Bilibili playurl 响应没有 durl[0].url（可能需要登录 cookie）",
        ));
    }
    let mut header = serde_json::Map::new();
    header.insert("User-Agent".to_string(), json!(UA));
    header.insert("Referer".to_string(), json!(WEB_BASE));
    Ok(json!({
        "parse": 0,
        "url": play_url,
        "playUrl": "",
        "header": Value::Object(header),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        let mut s = Site {
            key: "bili".to_string(),
            name: "哔哩哔哩".to_string(),
            api: API.to_string(),
            ..Default::default()
        };
        s.ext = Some(json!(ext));
        s
    }

    #[test]
    fn handles_only_csp_bili() {
        assert!(handles("csp_Bili"));
        assert!(handles(" CSP_bili "));
        assert!(!handles("csp_BiliGuard"));
        assert!(!handles("csp_SixV"));
    }

    #[test]
    fn parses_ext_classes_and_cookie() {
        let cfg = TvConfig::default();
        let conf = config(
            &cfg,
            &site(r#"{"class":[{"type_id":"1","type_name":"动画"},{"type_id":20,"type_name":"番剧"}],"cookie":"a=1"}"#),
        )
        .unwrap();
        assert_eq!(conf.classes.len(), 2);
        assert_eq!(conf.classes[0].type_id, "1");
        assert_eq!(conf.classes[1].type_id, "20");
        assert_eq!(conf.cookie.as_deref(), Some("a=1"));
        assert_eq!(conf.api_base, DEFAULT_API);
    }

    #[test]
    fn empty_ext_yields_no_classes_and_default_api() {
        let cfg = TvConfig::default();
        let conf = config(&cfg, &site("")).unwrap();
        assert!(conf.classes.is_empty());
        assert!(conf.cookie.is_none());
    }

    #[test]
    fn strips_html_tags_and_unescapes() {
        assert_eq!(strip_tags("<em>窗</em> 白噪音 &amp; 雨"), "窗 白噪音 & 雨");
    }

    #[test]
    fn cover_adds_https_for_scheme_relative() {
        assert_eq!(cover_url("//i0.hdslb.com/x.jpg"), "https://i0.hdslb.com/x.jpg");
        assert_eq!(cover_url("http://a/b.jpg"), "http://a/b.jpg");
    }

    #[test]
    fn remark_uses_minutes_before_colon() {
        assert_eq!(remark_of(&json!("12:34")), "12分钟");
        assert_eq!(remark_of(&json!(90)), "90分钟");
        assert_eq!(remark_of(&json!("")), "");
    }

    #[test]
    fn items_map_search_rows() {
        let body = json!({
            "code": 0,
            "data": {"result": [
                {"aid": 123, "title": "<em>窗</em>白噪音", "pic": "//i0.hdslb.com/a.jpg", "duration": "12:34"},
                {"title": "skip-me"}
            ]}
        });
        let items = items_from_search(&site(""), &body);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "123");
        assert_eq!(items[0].name, "窗白噪音");
        assert_eq!(items[0].cover.as_deref(), Some("https://i0.hdslb.com/a.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("12分钟"));
    }

    #[test]
    fn splits_play_id_variants() {
        assert_eq!(split_play_id("123+ 456"), ("123".to_string(), Some("456".to_string())));
        assert_eq!(split_play_id("123+456"), ("123".to_string(), Some("456".to_string())));
        assert_eq!(split_play_id("av123"), ("123".to_string(), None));
    }

    #[test]
    fn detail_maps_pages_to_single_group() {
        let body = json!({
            "code": 0,
            "data": {
                "title": "测试视频", "pic": "//i0.hdslb.com/p.jpg", "tname": "动画",
                "desc": "<p>简介</p>", "duration": 125,
                "pages": [
                    {"cid": 11, "part": "P1 $ # 标题"},
                    {"cid": 12, "part": ""}
                ]
            }
        });
        let d = detail_from_view(&site(""), "42", &body).unwrap();
        assert_eq!(d.name, "测试视频");
        assert_eq!(d.episodes.len(), 1);
        assert_eq!(d.episodes[0].group, "B站");
        assert_eq!(d.episodes[0].items.len(), 2);
        assert_eq!(d.episodes[0].items[0].url, "42+ 11");
        assert_eq!(d.episodes[0].items[0].title, "P1 _ _ 标题");
        assert_eq!(d.episodes[0].items[1].title, "P2");
        assert_eq!(d.cover.as_deref(), Some("https://i0.hdslb.com/p.jpg"));
        assert_eq!(d.desc.as_deref(), Some("简介"));
    }

    #[test]
    fn api_code_error_surfaces_message() {
        let err = check_api_code(&site(""), &json!({"code": -412, "message": "请求被拦截"}), "搜索").unwrap_err();
        let text = format!("{err:?}");
        assert!(text.contains("-412"), "{text}");
        assert!(text.contains("cookie"), "{text}");
    }
}
