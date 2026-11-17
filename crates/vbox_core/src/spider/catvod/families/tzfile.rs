//! `csp_TZFile`（团长资源 / t-rex.tzfile.com）原生实现。
//!
//! 原类：`com.github.catvod.spider.TZFile`（`dffec63f-pg` 版 850 行，`extends Push`；
//! `homeVideoContent` 被 jadx 跳过，核心逻辑完整）。原实现字符串全部是
//! `C0373a.a(byte[], byte[])` 的 XOR 字节数组，已用 `_rea/tmp/deobf3.mjs`
//! 解码为 `_rea/tmp/TZFile-decoded2.java` 后逐行核对。
//!
//! 原协议要点：
//! - `init` 写死 5 个分类：电影 movies / 电视剧 tvshow / 动漫 animation /
//!   纪录片 faction / 综艺 show。
//! - 列表：`{base}/{cate}/page/{page}`，页面里 `ul li`（`post-list-item`）行，
//!   标题 `h2 a`、封面 `img[data-src]`（App 端经 `C0124a.w()` 本地图片代理
//!   包装为 `/i/0/s/{url}`）、备注 `div.post-info .post-list-cat`、id `a[href]`。
//! - 详情：`{base}/{id}.html`，`#primary-home` 里 `h1` 标题、
//!   `article .entry-content img[data-src]` 封面、`.post-meta li.single-date`
//!   日期、`article .entry-content` 简介、`.post-meta li div.post-list-cat a`
//!   演员/分类。
//! - 网盘链接：早期文章在 `article div.entry-content p a` 里直接给出；
//!   现站改由 b2 主题的下载组件按需从 REST 拉取：
//!   `POST /wp-json/b2/v1/getDownloadData`（`{post_id}`）返回按钮清单，
//!   再 `POST /wp-json/b2/v1/getDownloadPageData`（`{post_id,index,i}`）拿
//!   `button.url`（JWT），最后 `GET /redirect?token={url}` 页面里有真正的
//!   网盘分享链接（如 `https://pan.quark.cn/s/...`）。
//! - 播放：按线路文本分派给网盘引擎（含「夸克」→ quark、「迅雷」/`PP極速`
//!   → thunder、`YD` → yd、其余 → ali）。
//!
//! 本实现（桌面端）的有意偏差：
//! 1. 封面不再经 `http://127.0.0.1:10079/i/0/s/` 本地图片代理，直接用原图
//!    地址（桌面端无该代理）。
//! 2. 网盘链接同时支持「正文内联」与「b2 REST 下载组件」两条来源。
//! 3. 迅雷/YD 引擎尚未实现：这两类线路在 `player_content` 里返回明确的
//!    `catvod.host.unsupported`，其余线路走已实现的 Quark/UC/Ali/115 引擎。
//! 4. 详情集数由网盘浏览结果生成（同 moli 家族）；浏览失败时退化为每条
//!    分享链接一集，便于后续在播放时再解析。
//! 5. 分页按模板的 `page/N` 约定递增，不解析总页数。
//! 6. 简介取正文纯文本并截断，不做原实现的字段级清洗。

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::AliClient;
use crate::pan::p115::P115Client;
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider, ShareLink};

use super::super::contract::{CatvodError, ErrorClass};

/// 画像里的 api id。
pub const API: &str = "csp_TZFile";
/// 默认基址。
pub const DEFAULT_BASE: &str = "https://t-rex.tzfile.com";
/// 请求 UA（原类 Chrome/120 系列）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 写死的 5 个分类（与原类 init 一致）。
pub const CATEGORIES: [(&str, &str); 5] = [
    ("movies", "电影"),
    ("tvshow", "电视剧"),
    ("animation", "动漫"),
    ("faction", "纪录片"),
    ("show", "综艺"),
];

const MAX_RESOURCES: usize = 12;
const MAX_EPISODES: usize = 500;
const MAX_DESC: usize = 400;

/// 匹配一个网盘分享链接（各 provider 的域名族）。
fn pan_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"https?://(?:pan\.quark\.cn|drive\.uc\.cn|(?:www\.)?115\.com|pan\.xunlei\.com|caiyun\.139\.com|(?:www\.)?aliyundrive\.com|(?:www\.)?alipan\.com)/[^"'\s<>\\]+"#,
        )
        .expect("pan link regex")
    })
}

/// 画像 api 是否由本模块处理。
pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// 基址：ext 里以 `http` 开头的段（mock/镜像），否则默认域名。
pub fn base_of(site: &Site) -> String {
    for seg in site.ext_string().split("$$$") {
        let seg = seg.trim();
        if seg.starts_with("http://") || seg.starts_with("https://") {
            return seg.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_BASE.to_string()
}

fn headers(base: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), format!("{base}/"));
    h.insert("Accept".to_string(), "text/html,application/xhtml+xml".to_string());
    h.insert("Accept-Language".to_string(), "zh-CN,zh;q=0.9".to_string());
    h
}

fn fetch_text(cfg: &TvConfig, base: &str, url: &str) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    crate::http::get_text(&client, url, &headers(base), 2)
}

fn post_json(cfg: &TvConfig, base: &str, url: &str, body: &Value) -> Result<Value> {
    let client = crate::http::client_for(&cfg.http)?;
    let mut h = headers(base);
    h.insert("Content-Type".to_string(), "application/json".to_string());
    let text = crate::http::post_json_text(&client, url, &h, &body.to_string())?;
    serde_json::from_str(&text).map_err(|e| parse_err_raw(format!("JSON 解析失败: {e}")))
}

fn parse_err_raw(detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, &Site::default(), detail).into_core()
}

fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_tags(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").expect("tag regex"));
    collapse(&re.replace_all(s, " "))
}

/// 分类：原类写死 5 类。
pub fn parse_categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: id.to_string(),
            type_name: name.to_string(),
        })
        .collect()
}

/// 列表页解析。
pub fn parse_list(html: &str, base: &str) -> Vec<VideoItem> {
    static ROW: OnceLock<Regex> = OnceLock::new();
    let row = ROW.get_or_init(|| {
        Regex::new(r#"(?s)<li[^>]*class="[^"]*post-list-item[^"]*"[^>]*>(.*?)</li>"#).expect("row")
    });
    static HREF: OnceLock<Regex> = OnceLock::new();
    let href = HREF.get_or_init(|| Regex::new(r#"<a[^>]+href="(?P<v>[^"]+)""#).expect("href"));
    static TITLE: OnceLock<Regex> = OnceLock::new();
    let title = TITLE.get_or_init(|| {
        Regex::new(r#"(?s)<h2[^>]*>.*?<a[^>]*>(?P<v>.*?)</a>"#).expect("title")
    });
    static IMG_DATA: OnceLock<Regex> = OnceLock::new();
    let img_data = IMG_DATA.get_or_init(|| Regex::new(r#"data-src="(?P<v>[^"]+)""#).expect("img data"));
    static IMG_SRC: OnceLock<Regex> = OnceLock::new();
    let img_src = IMG_SRC.get_or_init(|| Regex::new(r#"<img[^>]+src="(?P<v>[^"]+)""#).expect("img src"));
    static CAT: OnceLock<Regex> = OnceLock::new();
    let cat = CAT.get_or_init(|| {
        Regex::new(r#"(?s)class="[^"]*post-list-cat[^"]*"[^>]*>(?P<v>.*?)</(?:a|span|div)>"#)
            .expect("cat")
    });
    let mut out = Vec::new();
    for cap in row.captures_iter(html) {
        let block = &cap[1];
        let Some(h) = href.captures(block) else { continue };
        let Some(t) = title.captures(block) else { continue };
        let name = strip_tags(&t["v"]);
        if name.is_empty() {
            continue;
        }
        let cover = img_data
            .captures(block)
            .or_else(|| img_src.captures(block))
            .map(|c| join_url(base, c["v"].trim()))
            .filter(|s: &String| !s.ends_with("default-img.jpg"));
        let remark = cat
            .captures(block)
            .map(|c| strip_tags(&c["v"]))
            .filter(|s| !s.is_empty());
        out.push(VideoItem {
            id: join_url(base, h["v"].trim()),
            name,
            cover,
            remark,
        });
    }
    out
}

/// 详情页解析结果。
#[derive(Debug, Default, Clone)]
pub struct DetailData {
    pub name: String,
    pub cover: Option<String>,
    pub desc: Option<String>,
    pub actor: Option<String>,
    pub year: Option<String>,
    /// 正文内联的网盘链接。
    pub links: Vec<String>,
}

/// 解析详情页（HTML 部分）。
pub fn parse_detail(html: &str, base: &str) -> DetailData {
    static H1: OnceLock<Regex> = OnceLock::new();
    let h1 = H1.get_or_init(|| Regex::new(r#"(?s)<h1[^>]*>(?P<v>.*?)</h1>"#).expect("h1"));
    let name = h1.captures(html).map(|c| strip_tags(&c["v"])).unwrap_or_default();
    static IMG: OnceLock<Regex> = OnceLock::new();
    let img = IMG.get_or_init(|| {
        Regex::new(r#"(?s)<div[^>]*class="[^"]*entry-content[^"]*".*?<img[^>]+data-src="(?P<v>[^"]+)""#)
            .expect("cover")
    });
    let cover = img.captures(html).map(|c| join_url(base, c["v"].trim()));
    static DATE: OnceLock<Regex> = OnceLock::new();
    let date = DATE.get_or_init(|| {
        Regex::new(r#"(?s)<li[^>]*class="[^"]*single-date[^"]*"[^>]*>(?P<v>.*?)</li>"#).expect("date")
    });
    let year = date
        .captures(html)
        .map(|c| strip_tags(&c["v"]))
        .and_then(|s| {
            static Y: OnceLock<Regex> = OnceLock::new();
            let y = Y.get_or_init(|| Regex::new(r"(20\d{2})").expect("year"));
            y.captures(&s).map(|c| c[1].to_string())
        });
    static META: OnceLock<Regex> = OnceLock::new();
    let meta = META.get_or_init(|| {
        Regex::new(r#"(?s)<ul[^>]*class="[^"]*post-meta[^"]*"[^>]*>(?P<v>.*?)</ul>"#).expect("meta")
    });
    let actor = meta
        .captures(html)
        .map(|c| c["v"].to_string())
        .and_then(|region| {
            static CAT: OnceLock<Regex> = OnceLock::new();
            let cat = CAT.get_or_init(|| {
                Regex::new(r#"(?s)<div[^>]*class="[^"]*post-list-cat[^"]*"[^>]*>(?P<v>.*?)</div>"#)
                    .expect("cat")
            });
            let names: Vec<String> = cat
                .captures_iter(&region)
                .map(|c| strip_tags(&c["v"]))
                .filter(|s| !s.is_empty())
                .collect();
            if names.is_empty() { None } else { Some(names.join(",")) }
        });
    static DESC: OnceLock<Regex> = OnceLock::new();
    let desc = DESC.get_or_init(|| {
        Regex::new(r#"(?s)<div[^>]*class="[^"]*entry-content[^"]*"[^>]*>(?P<v>.*?)<div[^>]*class="[^"]*download"#)
            .expect("desc")
    });
    let desc = desc
        .captures(html)
        .map(|c| strip_tags(&c["v"]))
        .map(|s| s.chars().take(MAX_DESC).collect::<String>())
        .filter(|s| !s.is_empty());
    let mut links = Vec::new();
    for cap in pan_link_regex().captures_iter(html) {
        let url = cap[0].trim().to_string();
        if !links.contains(&url) {
            links.push(url);
        }
    }
    DetailData {
        name,
        cover,
        desc,
        actor,
        year,
        links,
    }
}

/// 从 `{base}/download?...` 或按钮 link 中取 `post_id/index/i`。
fn parse_button_query(link: &str) -> Option<(String, String, String)> {
    let get = |name: &str| -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"[?&](?P<k>[a-zA-Z_]+)=(?P<v>[^&#]*)"#).expect("query")
        });
        re.captures_iter(link)
            .find(|c| &c["k"] == name)
            .map(|c| c["v"].to_string())
    };
    Some((get("post_id")?, get("index").unwrap_or_else(|| "0".into()), get("i").unwrap_or_else(|| "0".into())))
}

/// 通过 b2 主题 REST 接口取「下载按钮 → 真实网盘分享链接」。
pub fn fetch_rest_links(cfg: &TvConfig, base: &str, post_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let data_url = format!("{base}/wp-json/b2/v1/getDownloadData");
    let body = serde_json::json!({ "post_id": post_id, "index": 0, "i": 0 });
    let list = match post_json(cfg, base, &data_url, &body) {
        Ok(v) => v,
        Err(_) => return out,
    };
    let Some(items) = list.as_array() else { return out };
    for item in items {
        let Some(buttons) = item.get("button").and_then(Value::as_array) else { continue };
        for button in buttons {
            let Some(link) = button.get("link").and_then(Value::as_str) else { continue };
            let Some((post_id, index, i)) = parse_button_query(link) else { continue };
            let page_url = format!("{base}/wp-json/b2/v1/getDownloadPageData");
            let page_body = serde_json::json!({ "post_id": post_id, "index": index, "i": i });
            let page = match post_json(cfg, base, &page_url, &page_body) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(token) = page.get("button").and_then(|b| b.get("url")).and_then(Value::as_str) else {
                continue;
            };
            if token.is_empty() {
                continue;
            }
            let encoded = percent_encoding::utf8_percent_encode(token, percent_encoding::NON_ALPHANUMERIC).to_string();
            let redirect = format!("{base}/redirect?token={encoded}");
            let Ok(html) = fetch_text(cfg, base, &redirect) else { continue };
            for cap in pan_link_regex().captures_iter(&html) {
                let url = cap[0].trim().to_string();
                if !out.contains(&url) {
                    out.push(url);
                }
            }
        }
    }
    out
}

fn provider_name(provider: PanProvider) -> &'static str {
    match provider {
        PanProvider::Quark => "夸克网盘",
        PanProvider::Uc => "UC网盘",
        PanProvider::Ali => "阿里云盘",
        PanProvider::P115 => "115网盘",
    }
}

/// ext 里首个 `{` 开头的段为网盘配置（cookie 等），否则空配置。
pub fn load_pan_config(site: &Site) -> PanConfig {
    for seg in site.ext_string().split("$$$") {
        let seg = seg.trim();
        if seg.starts_with('{') {
            return PanConfig::parse(seg).with_overlay();
        }
    }
    PanConfig::default()
}

fn browse_share(cfg: &TvConfig, conf: &PanConfig, link: &ShareLink) -> Result<Vec<PanEntry>> {
    match link.provider {
        PanProvider::Quark => Ok(QuarkClient::with_rules(&cfg.http, conf.effective_cookie())?.browse(link)?.files),
        PanProvider::Uc => Ok(UcClient::with_rules(&cfg.http, conf.effective_uc_cookie())?.browse(link)?.files),
        PanProvider::Ali => Ok(AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())?.browse(link)?.files),
        PanProvider::P115 => Ok(P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?.browse(link)?.files),
    }
}

fn resolve_play_url(cfg: &TvConfig, conf: &PanConfig, link: &ShareLink, fid: &str) -> Result<String> {
    match link.provider {
        PanProvider::Quark => {
            let c = QuarkClient::with_rules(&cfg.http, conf.effective_cookie())?;
            if let Ok(info) = c.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            c.download_url(fid)
        }
        PanProvider::Uc => {
            let c = UcClient::with_rules(&cfg.http, conf.effective_uc_cookie())?;
            if let Ok(info) = c.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            c.download_url(fid)
        }
        PanProvider::Ali => {
            let c = AliClient::with_rules(&cfg.http, conf.effective_ali_cookie())?;
            let token = c.share_token(&link.share_id, &link.passcode)?;
            c.download_url(&link.share_id, fid, &token.share_token)
        }
        PanProvider::P115 => {
            let c = P115Client::with_rules(&cfg.http, conf.effective_p115_cookie())?;
            c.download_url(&link.share_id, &link.passcode, fid)
        }
    }
}

fn find_url(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if s.starts_with("http") => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(find_url),
        Value::Object(m) => {
            for key in ["url", "play_url", "download_url", "raw_url"] {
                if let Some(Value::String(s)) = m.get(key) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            m.values().find_map(find_url)
        }
        _ => None,
    }
}

fn is_video_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("mp4" | "mkv" | "m3u8" | "avi" | "ts" | "flv" | "wmv" | "mov" | "rmvb" | "webm" | "m4v" | "mpg" | "mpeg" | "iso")
    )
}

/// 分类：写死 5 类。
pub fn categories() -> Vec<Category> {
    parse_categories()
}

/// 列表：`{base}/{cate}/page/{page}`。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let page = page.max(1);
    let url = format!("{base}/{}/page/{page}", cat.trim_matches('/'));
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_list(&html, &base))
}

fn detail_url(base: &str, id: &str) -> String {
    let id = id.trim();
    if id.starts_with("http://") || id.starts_with("https://") {
        id.to_string()
    } else if id.ends_with(".html") {
        join_url(base, id)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), id)
    }
}

fn post_id_of(url: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"/(?P<id>\d+)\.html").expect("post id"));
    re.captures(url).map(|c| c["id"].to_string())
}

/// 详情：页面元数据 + 正文内联链接 + b2 REST 下载组件链接。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site);
    let url = detail_url(&base, id);
    let html = fetch_text(cfg, &base, &url)?;
    let data = parse_detail(&html, &base);
    if data.name.is_empty() {
        return Err(parse_err(site, "详情页缺少标题（页面结构可能已改版）"));
    }
    let mut links = data.links.clone();
    if let Some(pid) = post_id_of(&url) {
        for link in fetch_rest_links(cfg, &base, &pid) {
            if !links.contains(&link) {
                links.push(link);
            }
        }
    }
    let conf = load_pan_config(site);
    let mut episodes: Vec<EpisodeGroup> = Vec::new();
    for raw in links.iter().take(MAX_RESOURCES) {
        let Some(link) = parse_share_link(raw) else { continue };
        let group = provider_name(link.provider).to_string();
        let files = browse_share(cfg, &conf, &link).unwrap_or_default();
        let mut items: Vec<Episode> = files
            .iter()
            .filter(|f| !f.is_dir && is_video_name(&f.name))
            .take(MAX_EPISODES)
            .map(|f| Episode {
                title: f.name.clone(),
                url: format!("{}###{}", link.url, f.fid),
            })
            .collect();
        if items.is_empty() {
            items.push(Episode {
                title: group.clone(),
                url: link.url.clone(),
            });
        }
        episodes.push(EpisodeGroup { group, items });
    }
    Ok(VideoDetail {
        id: url,
        name: data.name,
        cover: data.cover,
        desc: data.desc,
        actor: data.actor,
        year: data.year,
        episodes,
        ..Default::default()
    })
}

/// 搜索：`{base}/?type=post&s={kw}`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site);
    let kw = percent_encoding::utf8_percent_encode(keyword, percent_encoding::NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/?type=post&s={kw}");
    let html = fetch_text(cfg, &base, &url)?;
    Ok(parse_list(&html, &base))
}

/// 播放：`###` 拆分享链接与文件 id，按 provider 调对应引擎。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let (link_url, fid) = match id.split_once("###") {
        Some((u, f)) => (u.trim().to_string(), f.trim().to_string()),
        None => (id.to_string(), String::new()),
    };
    let link = parse_share_link(&link_url)
        .ok_or_else(|| host_err(site, format!("不支持或无直链的播放地址：{link_url}")))?;
    let conf = load_pan_config(site);
    let fid = if fid.is_empty() {
        let files = browse_share(cfg, &conf, &link)?;
        files
            .into_iter()
            .find(|f| !f.is_dir && is_video_name(&f.name))
            .map(|f| f.fid)
            .ok_or_else(|| host_err(site, "分享内未找到视频文件"))?
    } else {
        fid
    };
    let url = resolve_play_url(cfg, &conf, &link, &fid).map_err(|e| {
        let class = ErrorClass::classify_pan_error(&e).unwrap_or(ErrorClass::HostApi);
        CatvodError::new(class, site, format!("网盘直链解析失败：{e}")).into_core()
    })?;
    Ok(serde_json::json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "tzfile".into();
        s.name = "团长资源".into();
        s.api = API.into();
        s.ext = Some(json!(ext));
        s
    }

    const LIST_HTML: &str = r#"
<ul class="b2_gap">
<li class="555 post-list-item item-post-style-1" id="item-119749">
  <div class="item-in"><div class="post-module-thumb">
    <a href="https://trex.tzfile.com/119749.html" class="thumb-link">
      <img class="post-thumb" data-src="https://pic1.imgdb.cn/i/abc.webp" src="https://trex.tzfile.com/default-img.jpg"/>
    </a>
  </div>
  <div class="post-info"><h2 class="entry-title"><a href="https://trex.tzfile.com/119749.html">消失的一方 (2025)</a></h2>
  <div class="post-list-cat"><a href="/movies">电影</a></div></div></div>
</li>
<li class="555 post-list-item">
  <a href="https://trex.tzfile.com/119748.html" class="thumb-link">
    <img class="post-thumb" src="https://trex.tzfile.com/wp-content/themes/b2/default-img.jpg"/>
  </a>
  <h2><a href="https://trex.tzfile.com/119748.html">街头霸王 (2026)</a></h2>
</li>
</ul>"#;

    const DETAIL_HTML: &str = r#"
<div id="primary-home" class="content-area"><h1 class="entry-title">消失的一方 The Invisible Half (2025)</h1>
<ul class="post-meta"><li class="single-date">2025-06-20</li>
<li><div class="post-list-cat"><a href="/actor/1">西山将贵</a></div></li></ul>
<div class="entry-content"><p><img data-src="https://pic1.imgdb.cn/i/cover.webp"/></p>
<p>◎简 介 一个恐怖故事。</p>
<p><a href="https://pan.quark.cn/s/e1934708f5f6">夸克网盘</a></p>
<p><a href="https://www.115.com/s/sw123abc?password=xy9z">115网盘</a></p></div>
<div class="download-box"></div></div>"#;

    #[test]
    fn categories_are_fixed_five() {
        let cats = categories();
        assert_eq!(cats.len(), 5);
        assert_eq!(cats[0].type_id, "movies");
        assert_eq!(cats[1].type_name, "电视剧");
    }

    #[test]
    fn parse_list_extracts_rows() {
        let items = parse_list(LIST_HTML, DEFAULT_BASE);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "https://trex.tzfile.com/119749.html");
        assert_eq!(items[0].name, "消失的一方 (2025)");
        assert_eq!(items[0].cover.as_deref(), Some("https://pic1.imgdb.cn/i/abc.webp"));
        assert_eq!(items[0].remark.as_deref(), Some("电影"));
        // 默认占位图被过滤
        assert!(items[1].cover.is_none());
    }

    #[test]
    fn parse_detail_extracts_fields_and_inline_links() {
        let d = parse_detail(DETAIL_HTML, DEFAULT_BASE);
        assert_eq!(d.name, "消失的一方 The Invisible Half (2025)");
        assert_eq!(d.cover.as_deref(), Some("https://pic1.imgdb.cn/i/cover.webp"));
        assert_eq!(d.year.as_deref(), Some("2025"));
        assert!(d.desc.as_deref().unwrap_or_default().contains("恐怖故事"));
        assert_eq!(d.actor.as_deref(), Some("西山将贵"));
        assert_eq!(d.links.len(), 2);
        assert!(d.links[0].starts_with("https://pan.quark.cn/s/"));
        assert!(d.links[1].contains("115.com/s/"));
    }

    #[test]
    fn parse_button_query_reads_ids() {
        let q = parse_button_query(
            "https://t-rex.tzfile.com/download?post_id=119749&index=0&i=1",
        )
        .unwrap();
        assert_eq!(q, ("119749".to_string(), "0".to_string(), "1".to_string()));
    }

    #[test]
    fn detail_url_normalizes() {
        assert_eq!(
            detail_url(DEFAULT_BASE, "119749"),
            "https://t-rex.tzfile.com/119749"
        );
        assert_eq!(
            detail_url(DEFAULT_BASE, "119749.html"),
            "https://t-rex.tzfile.com/119749.html"
        );
        assert_eq!(
            detail_url(DEFAULT_BASE, "https://x.test/1.html"),
            "https://x.test/1.html"
        );
    }

    #[test]
    fn post_id_of_extracts_numeric() {
        assert_eq!(
            post_id_of("https://t-rex.tzfile.com/119749.html").as_deref(),
            Some("119749")
        );
        assert!(post_id_of("https://t-rex.tzfile.com/tag/x").is_none());
    }

    #[test]
    fn provider_names_are_chinese() {
        assert_eq!(provider_name(PanProvider::Quark), "夸克网盘");
        assert_eq!(provider_name(PanProvider::P115), "115网盘");
    }

    #[test]
    fn is_video_name_filters() {
        assert!(is_video_name("a.MP4"));
        assert!(is_video_name("b.m3u8"));
        assert!(!is_video_name("c.txt"));
    }

    #[test]
    fn find_url_walks_nested_values() {
        let v = json!({"data": {"items": [{"download_url": "https://cdn/x.m3u8"}]}});
        assert_eq!(find_url(&v).as_deref(), Some("https://cdn/x.m3u8"));
    }

    #[test]
    fn player_rejects_empty_and_foreign_id() {
        let cfg = TvConfig::default();
        let err = player_content(&cfg, &site(""), "", "").unwrap_err();
        assert!(err.to_string().contains("播放 id"));
        let err = player_content(&cfg, &site(""), "", "https://example.com/x").unwrap_err();
        assert!(err.to_string().contains("不支持或无直链"), "{err}");
    }

    #[test]
    fn base_defaults_and_ext_override() {
        assert_eq!(base_of(&site("")), DEFAULT_BASE);
        assert_eq!(
            base_of(&site("./lib/x$$$http://127.0.0.1:8080")),
            "http://127.0.0.1:8080"
        );
    }
}
