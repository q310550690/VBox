//! `csp_Hdh`（Hdhive 影巢）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Hdhive.java`
//!   （jadx 413 行；`detailContent` 被跳过）
//! * `_rea/tmp/r150/cfr-hdh/com/github/catvod/spider/Hdhive.java`（dex2jar + CFR 0.152，
//!   776 行，含完整 `detailContent`）
//! * 字符串解码：`_rea/tmp/r150/dec-cfr.mjs` → `_rea/tmp/r150/Hdhive.cfr-decode.java`
//!   （105 处 `merge.z.a_0.a(byte[],byte[])` XOR 全部还原、0 处未解）
//! * 站点 `_rea/samples/inventory/sites.json`：1 站 `csp_Hdh`。
//!
//! 协议（原类逐段还原）：
//! * `class Hdhive extends Push`；`init`：ext 按 `$$$` 切分，段 1 交 `super.init`
//!   （Push 的网盘配置），段 2（长度 > 10）为图片代理/推送配置 `j`，段 3 为 `"1"`
//!   且未开 `global_danmu_disabled` 时开弹幕 `k`；`client()` 在 `j` 非空时用播放器 UA。
//! * 鉴权 `b()`：UA Chrome/109、`Referer`、`Accept`；`Authorization: Bearer {token}`，
//!   token 由 `POST /api/v1/login`（硬编码账号）取 `meta.access_token` 后缓存。
//! * 首页 `homeContent`：固定 5 分类（电影/电视剧/动漫/综艺/纪录片）。
//! * 分类 `categoryContent`：`GET /api/v1/public/{movies|tv}?per_page=30&page={page}` +
//!   `&record_type={cateId}&share_num_gt=0`；条目 `title`（回退 `name`）、
//!   id = `{type}_id={id}`、封面 `poster_url`、备注 `video_resolution[0]`。
//! * 搜索 `searchContent`：`GET /api/v1/public/tmdb/search/multi?query={kw}&page=1` →
//!   每项 `id`+`media_type` → `c(id, mediaType)`：
//!   `POST /api/v1/public/{movies|tv}?share_num_gt=0`，body `{"tmdb_id": id}` →
//!   `data` 对象 → 条目标题/id/封面/备注 `updated_at`。
//! * 详情 `detailContent`：`GET /api/v1/customer/resources?{id}&sort_by=is_admin`
//!   `&sort_order=descend&per_page=100` → `data[]`；首项作详情（标题、`movie`/`tv` 封面、
//!   `remark [source[0]]`）；以 `movie_id`（<0 时 `tv_id`）为目标 id 扫描后续条目，
//!   id 相同或标题互含才收；链接按 Quark/Ali/Thunder/YD 正则与 `magnet:` 分流，
//!   磁力线路名 `磁力N`；最多 10 条链接。
//! * 播放 `playerContent`：`DL+0` + `global_magnet_systemopen` → 本机 10079 听书代理；
//!   `磁力/电驴` 或 `ed2k:/magnet:` → 直出 `parse:0`；否则按线路前缀
//!   `夸克/迅雷/YD/`（默认阿里）走网盘引擎；开弹幕时在结果上追加弹幕字段。
//!
//! 有意偏差（桌面端能力边界）：
//! 1. 迅雷（Thunder）/天翼（YD）没有 W9 引擎：详情保留「迅雷云盘 / 天翼云盘」单集线路，
//!    播放返回 `parse:1` 交给客户端（原类用 Push 的对应引擎展开）；
//! 2. `global_magnet_systemopen` 视为关闭：磁力线路保持原始 `magnet:` 地址，`DL+0`
//!    听书代理不启用；
//! 3. 弹幕：原类经 10079 播放器取弹幕并回写 `danmaku` 字段，桌面端无该代理，
//!    `k` 只保留开关语义、不追加弹幕；
//! 4. 图片代理 `W.a.s(poster_url, j)`：桌面端直接用原始 `poster_url`（`j` 仅透传）；
//! 5. 线路名（阿里云盘/夸克网盘 等）为按语义推断的稳定名，原类由 Push 内部拼接；
//! 6. 上游 `www.hdhive.org` 现全部路由返回 SPA「Redirecting…」，故本家族只有 mock 验收。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::{parse_share_link, PanProvider};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};
use crate::spider::catvod::families::{push, yunpan};

/// 站点 api 名。
pub const API: &str = "csp_Hdh";

const BASE: &str = "https://www.hdhive.org";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/109.0.0.0 Safari/537.36";
const REFERER: &str = "https://www.hdhive.org";
const LOGIN_PATH: &str = "/api/v1/login";
const LOGIN_USER: &str = "xrjdsvln@dijingka.com";
const LOGIN_PASS: &str = "zalman123";
const LINE_ALI: &str = "阿里云盘";
const LINE_QUARK: &str = "夸克网盘";
const LINE_THUNDER: &str = "迅雷云盘";
const LINE_YD: &str = "天翼云盘";
const MAGNET_PREFIX: &str = "磁力";
const MAX_LINKS: usize = 10;
const MAX_EPISODES: usize = 300;
const TOKEN_TTL_MS: u64 = 3_600_000;

/// 固定分类（原类 `init` 硬编码 JSON）。
const CATEGORIES: [(&str, &str); 5] = [
    ("movie", "电影"),
    ("tv", "电视剧"),
    ("anime", "动漫"),
    ("zongyi", "综艺"),
    ("documentary", "纪录片"),
];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn token_cache() -> &'static Mutex<HashMap<String, (String, u64)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (String, u64)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 是否接管该 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// ext `$$$` 分段（原类 `init` 语义）：段 0 交 Push、段 1 图片代理、段 2 弹幕开关。
fn ext_segments(site: &Site) -> Vec<String> {
    site.ext_string()
        .split("$$$")
        .map(|s| s.trim().to_string())
        .collect()
}

#[allow(dead_code)] // ext 段 0（Push 配置）桌面端由 `pan` 引擎读取，这里保留语义供单测
fn push_config_segment(site: &Site) -> String {
    ext_segments(site)
        .into_iter()
        .next()
        .unwrap_or_default()
}

#[allow(dead_code)] // 图片代理（原类 `W.a.s`）桌面端透传，见模块偏差 4
fn image_proxy(site: &Site) -> String {
    ext_segments(site)
        .get(1)
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() > 10)
        .unwrap_or_default()
}

#[allow(dead_code)] // 弹幕开关（原类 `k`）桌面端无 10079 代理，见模块偏差 3
fn danmaku_enabled(site: &Site) -> bool {
    ext_segments(site)
        .get(2)
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// 基址：ext 段 4（索引 3）可覆盖（镜像/测试钩子；原类硬编码 `https://www.hdhive.org`）。
fn base_of(site: &Site) -> String {
    ext_segments(site)
        .get(3)
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() > 7 && (s.starts_with("http://") || s.starts_with("https://")))
        .unwrap_or_else(|| BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn login_headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    h.insert("Referer".to_string(), REFERER.to_string());
    h.insert("Accept".to_string(), "application/json, text/plain, */*".to_string());
    h
}

fn auth_headers(token: &str) -> HashMap<String, String> {
    let mut h = login_headers();
    h.insert("Authorization".to_string(), format!("Bearer {token}"));
    h
}

/// 登录并缓存 access token（原类字段 `h`）。
fn login(cfg: &TvConfig, site: &Site) -> Result<String> {
    let key = site.key.clone();
    if let Some((token, at)) = token_cache().lock().ok().and_then(|g| g.get(&key).cloned()) {
        if now_ms().saturating_sub(at) < TOKEN_TTL_MS && !token.is_empty() {
            return Ok(token);
        }
    }
    let client = crate::http::client_for(&cfg.http)?;
    let body = json!({ "username": LOGIN_USER, "password": LOGIN_PASS }).to_string();
    let url = format!("{}{LOGIN_PATH}", base_of(site));
    let text = crate::http::post_json_text(&client, &url, &login_headers(), &body)
        .map_err(|e| host_err(site, format!("登录失败: {e}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("登录响应非 JSON: {e}")))?;
    let token = value
        .get("meta")
        .and_then(|m| m.get("access_token"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(host_err(site, "登录成功但缺少 access_token"));
    }
    if let Ok(mut g) = token_cache().lock() {
        g.insert(key, (token.clone(), now_ms()));
    }
    Ok(token)
}

fn get_json(cfg: &TvConfig, site: &Site, url: &str) -> Result<Value> {
    let client = crate::http::client_for(&cfg.http)?;
    let token = login(cfg, site)?;
    let text = crate::http::get_text(&client, url, &auth_headers(&token), 2)?;
    serde_json::from_str::<Value>(&text)
        .map_err(|e| parse_err(site, format!("响应非 JSON: {e}")))
}

fn post_json(cfg: &TvConfig, site: &Site, url: &str, body: &str) -> Result<Value> {
    let client = crate::http::client_for(&cfg.http)?;
    let token = login(cfg, site)?;
    let text = crate::http::post_json_text(&client, url, &auth_headers(&token), body)?;
    serde_json::from_str::<Value>(&text)
        .map_err(|e| parse_err(site, format!("响应非 JSON: {e}")))
}

fn abs_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("//") {
        return Some(format!("https:{raw}"));
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    Some(format!("{BASE}/{}", raw.trim_start_matches('/')))
}

/// 固定分类（原类 `homeContent` 直接回填 `init` 里的 JSON 数组）。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect())
}

/// 列表地址：`type_id == "movie"` 走 `movies`，其余一律 `tv`（原类二元判断）。
pub fn list_url(base: &str, cat: &str, page: u32) -> String {
    let path = if cat.trim() == "movie" { "movies" } else { "tv" };
    let base = base.trim_end_matches('/');
    format!("{base}/api/v1/public/{path}?per_page=30&page={page}&record_type={cat}&share_num_gt=0")
}

fn item_of(obj: &Value) -> Option<VideoItem> {
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| obj.get("name").and_then(Value::as_str))?;
    if title.trim().is_empty() {
        return None;
    }
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("movie");
    let id = obj.get("id").and_then(Value::as_str).unwrap_or_default();
    let item_id = format!("{kind}_id={id}");
    let cover = obj
        .get("poster_url")
        .and_then(Value::as_str)
        .and_then(abs_url);
    let remark = obj
        .get("video_resolution")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            obj.get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Some(VideoItem {
        id: item_id,
        name: title.trim().to_string(),
        cover,
        remark,
    })
}

/// 分类列表：`data[]` → `VideoItem`（保留原类 id 结构 `{type}_id={id}`）。
pub fn parse_items(value: &Value) -> Vec<VideoItem> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(item_of).collect())
        .unwrap_or_default()
}

/// 分类内容。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let type_id = type_id.trim();
    if type_id.is_empty() {
        return Err(parse_err(site, "分类 id 为空"));
    }
    let url = list_url(&base_of(site), type_id, page.max(1));
    let value = get_json(cfg, site, &url)?;
    Ok(parse_items(&value))
}

/// 搜索聚合：TMDb `media_type` → 站内条目：
/// `POST /api/v1/public/{movies|tv}?share_num_gt=0`，body `{"tmdb_id": id}`。
pub fn tmdb_url(base: &str, media_type: &str) -> String {
    let path = if media_type.trim() == "movie" { "movies" } else { "tv" };
    let base = base.trim_end_matches('/');
    format!("{base}/api/v1/public/{path}?share_num_gt=0")
}

pub fn tmdb_body(tmdb_id: &str) -> String {
    json!({ "tmdb_id": tmdb_id }).to_string()
}

fn tmdb_entry(cfg: &TvConfig, site: &Site, tmdb_id: &str, media_type: &str) -> Option<VideoItem> {
    let value = post_json(cfg, site, &tmdb_url(&base_of(site), media_type), &tmdb_body(tmdb_id)).ok()?;
    let data = value.get("data")?;
    let title = data.get("title").and_then(Value::as_str)?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let kind = data
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(media_type);
    let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
    let cover = data
        .get("poster_url")
        .and_then(Value::as_str)
        .and_then(abs_url);
    let remark = data
        .get("updated_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(VideoItem {
        id: format!("{kind}_id={id}"),
        name: title,
        cover,
        remark,
    })
}

/// 搜索：多路命中（TMDb multi）逐一回查站内条目，失败项跳过（原类 `c()` 返回 null）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, _quick: bool) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "{}/api/v1/public/tmdb/search/multi?query={}&page=1",
        base_of(site),
        urlencode(keyword)
    );
    let value = get_json(cfg, site, &url)?;
    let mut out = Vec::new();
    let Some(items) = value.get("data").and_then(Value::as_array) else {
        return Ok(out);
    };
    for item in items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let media_type = item
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("movie");
        if id.is_empty() {
            continue;
        }
        if let Some(v) = tmdb_entry(cfg, site, id, media_type) {
            out.push(v);
        }
    }
    Ok(out)
}

fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn resource_url(base: &str, id: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/api/v1/customer/resources?{id}&sort_by=is_admin&sort_order=descend&per_page=100")
}

/// 详情页收集到的原始链接（原类四个 Push 引擎列表 + 磁力列表）。
#[derive(Debug, Default, Clone)]
pub struct RawLinks {
    pub ali: Vec<String>,
    pub quark: Vec<String>,
    pub thunder: Vec<String>,
    pub yd: Vec<String>,
    pub magnets: Vec<String>,
}

impl RawLinks {
    fn total(&self) -> usize {
        self.ali.len() + self.quark.len() + self.thunder.len() + self.yd.len() + self.magnets.len()
    }
}

/// 单个链接分类（原类 `Quark.b/Ali.b/Thunder.b/YD.b` 正则 + `magnet:` 前缀）。
fn classify(raw: &str) -> Option<&'static str> {
    if let Some(link) = parse_share_link(raw) {
        match link.provider {
            PanProvider::Ali => return Some("ali"),
            PanProvider::Quark => return Some("quark"),
            _ => {}
        }
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("thunder://") || lower.contains("pan.xunlei.com") {
        return Some("thunder");
    }
    if lower.contains("cloud.189.cn") {
        return Some("yd");
    }
    if lower.starts_with("magnet:") {
        return Some("magnet");
    }
    None
}

fn title_of(value: &Value) -> String {
    value
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| value.get("name").and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn entry_target_id(entry: &Value) -> i64 {
    let movie = entry.get("movie_id").and_then(Value::as_i64).unwrap_or(-1);
    if movie >= 0 {
        return movie;
    }
    entry.get("tv_id").and_then(Value::as_i64).unwrap_or(-1)
}

/// 详情条目扫描：id 相同或标题互含才收；遇到首个不匹配条目即停止（原类 `break`）；
/// 链接按引擎分流，最多 10 条。
pub fn collect_links(entry: &Value, data: &[Value]) -> RawLinks {
    let mut out = RawLinks::default();
    let entry_title = title_of(entry);
    let target = entry_target_id(entry);
    for item in data {
        let movie = item.get("movie_id").and_then(Value::as_i64).unwrap_or(-1);
        let tv = item.get("tv_id").and_then(Value::as_i64).unwrap_or(-1);
        let item_id = if movie >= 0 { movie } else { tv };
        let title = title_of(item);
        let by_id = item_id > 0 && item_id == target;
        let by_title = !title.is_empty()
            && !entry_title.is_empty()
            && (title.contains(&entry_title) || entry_title.contains(&title));
        if !by_id && !by_title {
            break;
        }
        let raw = item.get("url").and_then(Value::as_str).unwrap_or("").trim();
        if raw.is_empty() {
            continue;
        }
        match classify(raw) {
            Some("ali") => out.ali.push(raw.to_string()),
            Some("quark") => out.quark.push(raw.to_string()),
            Some("thunder") => out.thunder.push(raw.to_string()),
            Some("yd") => out.yd.push(raw.to_string()),
            Some("magnet") => out.magnets.push(raw.to_string()),
            _ => continue,
        }
        if out.total() >= MAX_LINKS {
            break;
        }
    }
    out
}

fn expand_engine(
    cfg: &TvConfig,
    site: &Site,
    links: &[String],
    line: &str,
) -> Option<EpisodeGroup> {
    let conf = yunpan::load_pan_config(site);
    let mut items: Vec<Episode> = Vec::new();
    for raw in links {
        let Some(link) = parse_share_link(raw) else { continue };
        let Ok(files) = yunpan::browse_share(cfg, &conf, &link) else {
            continue;
        };
        for file in files {
            if file.is_dir || !yunpan::is_video_name(&file.name) {
                continue;
            }
            items.push(Episode {
                title: file.name.clone(),
                url: format!("{}###{}", link.url, file.fid),
            });
            if items.len() >= MAX_EPISODES {
                break;
            }
        }
        if items.len() >= MAX_EPISODES {
            break;
        }
    }
    if items.is_empty() {
        return None;
    }
    Some(EpisodeGroup {
        group: line.to_string(),
        items,
    })
}

fn single_group(line: &str, url: &str) -> EpisodeGroup {
    EpisodeGroup {
        group: line.to_string(),
        items: vec![Episode {
            title: "播放".to_string(),
            url: url.to_string(),
        }],
    }
}

/// 详情：资源列表 → 首项元信息 + 五类线路（阿里/夸克经 `pan` 引擎展开；
/// 迅雷/天翼无引擎，保留单集线路；磁力每条一线路）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let value = get_json(cfg, site, &resource_url(&base_of(site), id))?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entry = data
        .first()
        .cloned()
        .ok_or_else(|| parse_err(site, "资源列表为空"))?;
    let name = title_of(&entry);
    let cover = entry
        .get("movie")
        .and_then(|m| m.get("poster_url"))
        .and_then(Value::as_str)
        .or_else(|| {
            entry
                .get("tv")
                .and_then(|m| m.get("poster_url"))
                .and_then(Value::as_str)
        })
        .and_then(abs_url);
    let desc = {
        let remark = entry
            .get("remark")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let source = entry
            .get("source")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if source.is_empty() {
            (!remark.is_empty()).then_some(remark)
        } else {
            Some(format!("{remark} [{source}]").trim().to_string())
        }
    };

    let links = collect_links(&entry, &data);
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    if let Some(g) = expand_engine(cfg, site, &links.ali, LINE_ALI) {
        groups.push(g);
    }
    if let Some(g) = expand_engine(cfg, site, &links.quark, LINE_QUARK) {
        groups.push(g);
    }
    for raw in &links.thunder {
        groups.push(single_group(LINE_THUNDER, raw));
    }
    for raw in &links.yd {
        groups.push(single_group(LINE_YD, raw));
    }
    for (idx, raw) in links.magnets.iter().enumerate() {
        groups.push(single_group(&format!("{MAGNET_PREFIX}{}", idx + 1), raw));
    }
    if groups.is_empty() {
        return Err(parse_err(site, "未找到可播放链接"));
    }

    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: groups,
    })
}

/// 播放：磁力/电驴直出 `parse:0`；`###` 走网盘引擎；迅雷/天翼交客户端 `parse:1`。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let flag = flag.trim();
    let id = id.trim();
    if id.starts_with("magnet:") || id.starts_with("ed2k:") {
        return Ok(json!({ "parse": 0, "url": id, "jx": 0, "playUrl": "" }));
    }
    if id.contains("###") {
        return push::player_content(cfg, site, flag, id);
    }
    if flag.starts_with("迅雷") || flag.starts_with("YD") || flag.starts_with("天翼") {
        return Ok(json!({ "parse": 1, "url": id }));
    }
    push::player_content(cfg, site, flag, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: Option<&str>) -> Site {
        let mut s = Site {
            key: "hdh".to_string(),
            name: "影巢".to_string(),
            api: API.to_string(),
            ..Default::default()
        };
        s.ext = ext.map(|e| json!(e));
        s
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Hdh"));
        assert!(handles(" CSP_HDH "));
        assert!(!handles("csp_Hdhive"));
        assert!(!handles("csp_Push"));
    }

    #[test]
    fn categories_are_fixed_five() {
        let cfg = TvConfig::default();
        let s = site(None);
        let cats = categories(&cfg, &s).expect("categories");
        assert_eq!(cats.len(), 5);
        assert_eq!(cats[0].type_id, "movie");
        assert_eq!(cats[1].type_name, "电视剧");
        assert_eq!(cats[4].type_id, "documentary");
    }

    #[test]
    fn list_url_movie_and_tv() {
        let movie = list_url(BASE, "movie", 2);
        assert!(movie.contains("/api/v1/public/movies?"));
        assert!(movie.contains("per_page=30&page=2"));
        assert!(movie.contains("record_type=movie"));
        assert!(movie.ends_with("&share_num_gt=0"));

        let tv = list_url("https://mirror.test/", "anime", 1);
        assert!(tv.contains("/api/v1/public/tv?"));
        assert!(tv.contains("record_type=anime"));
    }

    #[test]
    fn parse_items_maps_fields() {
        let value = json!({
            "data": [
                {
                    "title": "沙丘2",
                    "type": "movie",
                    "id": "693134",
                    "poster_url": "https://img.example/dune.jpg",
                    "video_resolution": ["4K", "1080P"]
                },
                { "name": "无 id 标题", "updated_at": "2026-01-02" }
            ]
        });
        let items = parse_items(&value);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "movie_id=693134");
        assert_eq!(items[0].name, "沙丘2");
        assert_eq!(items[0].cover.as_deref(), Some("https://img.example/dune.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("4K"));
        assert_eq!(items[1].name, "无 id 标题");
        assert_eq!(items[1].remark.as_deref(), Some("2026-01-02"));
    }

    #[test]
    fn tmdb_paths_are_stable() {
        assert!(tmdb_url(BASE, "movie").contains("/public/movies?share_num_gt=0"));
        assert!(tmdb_url(BASE, "tv").contains("/public/tv?share_num_gt=0"));
        assert_eq!(tmdb_body("693134"), r#"{"tmdb_id":"693134"}"#);
    }

    #[test]
    fn collect_links_matches_by_id_and_stops_on_mismatch() {
        let entry = json!({"title": "沙丘", "movie_id": 7, "tv_id": -1});
        let data = vec![
            json!({"title": "沙丘", "movie_id": 7, "url": "https://pan.quark.cn/s/abc"}),
            json!({"title": "沙丘 2", "movie_id": 7, "url": "magnet:?xt=urn:btih:111"}),
            json!({"title": "无关条目", "movie_id": 9, "url": "https://www.aliyundrive.com/s/zzz"}),
            json!({"title": "沙丘", "movie_id": 7, "url": "https://www.aliyundrive.com/s/yyy"}),
        ];
        let links = collect_links(&entry, &data);
        assert_eq!(links.quark.len(), 1);
        assert_eq!(links.magnets.len(), 1);
        // 第三个条目 id 不匹配且标题不含 → 中断，第四个不再收集。
        assert!(links.ali.is_empty());
    }

    #[test]
    fn collect_links_matches_by_title_containment() {
        let entry = json!({"title": "沙丘", "movie_id": -1, "tv_id": -1});
        let data = vec![
            json!({"title": "沙丘 第二季", "url": "https://pan.quark.cn/s/a"}),
            json!({"title": "沙丘 第三季", "url": "https://pan.quark.cn/s/b"}),
        ];
        let links = collect_links(&entry, &data);
        assert_eq!(links.quark.len(), 2);
    }

    #[test]
    fn collect_links_classifies_engines_and_caps() {
        let entry = json!({"title": "测试", "movie_id": -1, "tv_id": -1});
        let mut data = Vec::new();
        for i in 0..12 {
            data.push(json!({"title": "测试", "url": format!("magnet:?xt=urn:btih:{i}")}));
        }
        data.push(json!({"title": "测试", "url": "https://pan.xunlei.com/s/xyz"}));
        let links = collect_links(&entry, &data);
        assert_eq!(links.magnets.len(), 10);
        // 达到上限后不再收集后续链接。
        assert!(links.thunder.is_empty());
    }

    #[test]
    fn ext_segments_split_push_proxy_and_danmaku() {
        let s = site(Some("{\"quark_cookie\":\"\"}$$$https://img.example.com/proxy?url=$$$1$$$https://mirror.test"));
        assert_eq!(base_of(&s), "https://mirror.test");
        assert_eq!(base_of(&site(None)), BASE);
        assert_eq!(push_config_segment(&s), "{\"quark_cookie\":\"\"}");
        assert_eq!(image_proxy(&s), "https://img.example.com/proxy?url=");
        assert!(danmaku_enabled(&s));

        let s2 = site(None);
        assert_eq!(image_proxy(&s2), "");
        assert!(!danmaku_enabled(&s2));
    }

    #[test]
    fn player_magnet_is_direct() {
        let cfg = TvConfig::default();
        let s = site(None);
        let v = player_content(&cfg, &s, "磁力1", "magnet:?xt=urn:btih:abc").expect("player");
        assert_eq!(v["parse"], 0);
        assert_eq!(v["url"], "magnet:?xt=urn:btih:abc");
    }

    #[test]
    fn player_thunder_line_hands_to_client() {
        let cfg = TvConfig::default();
        let s = site(None);
        let v = player_content(&cfg, &s, "迅雷云盘", "thunder://QUFodHRwOi8v").expect("player");
        assert_eq!(v["parse"], 1);
        assert_eq!(v["url"], "thunder://QUFodHRwOi8v");
    }

    #[test]
    fn detail_rejects_blank_id() {
        let cfg = TvConfig::default();
        let s = site(None);
        let err = detail(&cfg, &s, "  ").expect_err("blank id");
        assert!(format!("{err}").contains("catvod.result.parse"), "{err}");
    }

    #[test]
    fn classify_share_links() {
        assert_eq!(classify("https://pan.quark.cn/s/abc"), Some("quark"));
        assert_eq!(classify("https://www.aliyundrive.com/s/abc"), Some("ali"));
        assert_eq!(classify("thunder://QUFodHRw"), Some("thunder"));
        assert_eq!(classify("https://cloud.189.cn/t/abc"), Some("yd"));
        assert_eq!(classify("magnet:?xt=urn:btih:1"), Some("magnet"));
        assert_eq!(classify("https://example.org/watch/1"), None);
    }
}
