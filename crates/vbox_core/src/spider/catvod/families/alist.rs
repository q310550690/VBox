//! `csp_Alist`（AList / OpenList 多服务器聚合）原生实现。
//!
//! 来源：`_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Alist.java`
//! （355 行）与 `parser/merge/D/a.java`（驱动配置模型）、`parser/merge/D/c.java`
//! （条目模型）。原类 `AList extends Spider`，不含任何 Android 私有能力，
//! 协议全部是 HTTP JSON，可直接在桌面端复刻。
//!
//! 原实现要点：
//! * `ext` 是 JSON：`{"drives":[{name,server,token,path,search,hidden,vodPic,
//!   params:[{path,password}],login:{username,password}}],"vodPic":…}`；
//!   以 `http` 开头时先 GET 远程 JSON。未隐藏的 drive 即首页分类，
//!   分类 id == drive 名；`hidden` 缺省 false、`search` 缺省 true。
//! * 列表：`POST {server}/api/fs/list`（有 token）或 `POST {server}/api/public/path`
//!   （公开），body `{path,password}`，取 `data.content` / `data.files`。
//! * 详情：id = drive 名 + AList 绝对路径；父目录列出视频文件，集名=文件名，
//!   单线路（AList）。
//! * 播放：集 id 是路径，`POST {server}/api/fs/get`（或 `public/path`）取
//!   `data.raw_url` / `data.url` 直链，`parse:0`。
//! * 搜索：对所有 `search` 打开的 drive 并发 `POST /api/fs/search`
//!   （`{parent:"/",keywords,page:1,per_page:100}`）或 `/api/public/search`；
//!   响应含 `Guest user is disabled` 时先 `POST /api/auth/login` 换 token。
//!
//! 有意偏差（桌面端适配，均不影响与原实现的对外契约）：
//! 1. ext 额外兼容「平铺 map」形态（dc 仓「Alist(SP)」站点实际配置是
//!    `{显示名: server}` 19 台服务器）；平铺项按公开 drive 处理（无 token）。
//! 2. 分类分页：AList 一次性返回整目录，`page > 1` 返回空列表（原实现
//!    `page=1,pagecount=1`，页面上不会翻页）。
//! 3. 公开读取优先尝试匿名 `fs/list`（OpenList/新版 AList 允许），失败再回退
//!    到 `public/path`（老版本）；取链同样先 `fs/get` 再 `public/path`。
//! 4. 条目类型用 `is_dir` 与 `type` 双判据（不同版本字段不一致）。
//! 5. 搜索不再实现原代码的 HTML `/search?box=` 兜底（依赖主题模板）；
//!    搜索接口失败的 drive 直接跳过。
//! 6. 播放 id 里原实现还会解析 `~~~名称@@@扩展名@@@路径` 的多文件段（字幕/
//!    多版本候选），桌面端目前只取首段路径，其余段忽略。
//! 7. `vod_pic` 缺省使用原实现的默认图；目录条目备注固定为「文件夹」。

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

/// 画像里的 api id。
pub const API: &str = "csp_Alist";
/// 原实现请求头 UA（`C0373a` 解密后的字面量）。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
/// 原实现缺省封面。
pub const DEFAULT_VOD_PIC: &str = "https://x.imgs.ovh/x/2023/09/05/64f680bb030b4.png";

/// 可播放的视频扩展名（小写，不带点）。
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "m3u8", "avi", "ts", "flv", "wmv", "mov", "rmvb", "webm", "m4v", "mpg",
    "mpeg", "iso", "f4v", "3gp",
];

/// AList 登录信息（原 `D.a.login`）。
#[derive(Debug, Clone, Default)]
pub struct Login {
    pub username: String,
    pub password: String,
}

/// 单台 AList 服务器（原 `D.a` 的桌面等价物）。
#[derive(Debug, Clone, Default)]
pub struct Drive {
    pub name: String,
    pub server: String,
    pub path: String,
    pub token: Option<String>,
    pub search: bool,
    pub hidden: bool,
    pub vod_pic: Option<String>,
    pub login: Option<Login>,
    pub passwords: Vec<(String, String)>,
}

/// ext 解析结果。
#[derive(Debug, Clone, Default)]
pub struct ExtConfig {
    pub drives: Vec<Drive>,
    pub vod_pic: String,
}

/// 目录条目（原 `D.c` 的桌面等价物）。
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub thumb: Option<String>,
    pub parent: Option<String>,
}

impl Entry {
    /// 从 `/api/fs/list`、`/api/public/path`、`/api/fs/search` 的条目 JSON 解析。
    pub fn from_value(v: &Value) -> Option<Self> {
        let name = v.get("name")?.as_str()?.trim().to_string();
        if name.is_empty() {
            return None;
        }
        let kind = v.get("type").and_then(|x| x.as_u64());
        let is_dir = v
            .get("is_dir")
            .and_then(|x| x.as_bool())
            .unwrap_or(kind == Some(1));
        let size = v.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
        let thumb = v
            .get("thumb")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());
        // AList 的 `fs/list`/`fs/search` 用 `path`（父路径），Gson 模型里
        // `path` 与 `parent` 互为别名，两个键都接受。
        let parent = v
            .get("parent")
            .or_else(|| v.get("path"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());
        Some(Self {
            name,
            size,
            is_dir,
            thumb,
            parent,
        })
    }
}

/* ============================================================
 * 对外入口
 * ============================================================ */

pub fn handles(api: &str) -> bool {
    api.eq_ignore_ascii_case(API)
}

/// 首页分类 = 未隐藏的 drive 名（原 `homeContent`）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let conf = load_drives(cfg, site)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for drive in &conf.drives {
        if drive.hidden {
            continue;
        }
        let name = drive.name.trim();
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        out.push(Category {
            type_id: name.to_string(),
            type_name: name.to_string(),
        });
    }
    if out.is_empty() {
        return Err(parse_err(
            site,
            "csp_Alist 的 ext 里没有可用（未 hidden）的服务器",
        ));
    }
    Ok(out)
}

/// 分类列表：id = drive 名 + AList 绝对路径，目录在前、文件在后。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    if page > 1 {
        // AList 目录是一次性返回的，没有服务端分页。
        return Ok(Vec::new());
    }
    let conf = load_drives(cfg, site)?;
    let (idx, rest) = find_drive_index(&conf.drives, cat)
        .ok_or_else(|| parse_err(site, format!("csp_Alist 分类无法匹配到 drive：{cat}")))?;
    let drive_name = conf.drives[idx].name.clone();
    let mut drive = conf.drives[idx].clone();
    let path = if rest.trim().is_empty() {
        "/".to_string()
    } else {
        rest
    };
    let entries = list(cfg, &mut drive, &path, site)?;
    let mut dirs: Vec<&Entry> = entries.iter().filter(|e| e.is_dir).collect();
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    let mut files: Vec<&Entry> = entries.iter().filter(|e| !e.is_dir).collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::with_capacity(dirs.len() + files.len());
    for entry in dirs {
        out.push(VideoItem {
            id: format!("{drive_name}{}", join_path(&path, &entry.name)),
            name: entry.name.clone(),
            cover: drive.vod_pic.clone(),
            remark: Some("文件夹".to_string()),
        });
    }
    for entry in files {
        out.push(VideoItem {
            id: format!("{drive_name}{}", join_path(&path, &entry.name)),
            name: entry.name.clone(),
            cover: entry.thumb.clone().or_else(|| drive.vod_pic.clone()),
            remark: Some(human_size(entry.size)),
        });
    }
    Ok(out)
}

/// 详情：列父目录，视频文件成集（单线路「AList」）。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let conf = load_drives(cfg, site)?;
    let (idx, full) = find_drive_index(&conf.drives, id)
        .ok_or_else(|| parse_err(site, format!("csp_Alist 详情 id 无法匹配到 drive：{id}")))?;
    let drive_name = conf.drives[idx].name.clone();
    let mut drive = conf.drives[idx].clone();
    let parent = parent_of(&full);
    let entries = list(cfg, &mut drive, &parent, site)?;

    let mut files: Vec<Entry> = entries
        .into_iter()
        .filter(|e| !e.is_dir && has_video_ext(&e.name))
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));

    let mut episodes = Vec::with_capacity(files.len());
    for entry in files {
        episodes.push(Episode {
            title: strip_ext(&entry.name),
            url: format!("{drive_name}{}", join_path(&parent, &entry.name)),
        });
    }

    let name = last_segment(&parent)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| drive_name.clone());
    let groups = if episodes.is_empty() {
        Vec::new()
    } else {
        vec![EpisodeGroup {
            group: "AList".to_string(),
            items: episodes,
        }]
    };
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: drive.vod_pic.clone(),
        desc: Some(format!("AList · {}", drive.name)),
        episodes: groups,
        ..Default::default()
    })
}

/// 搜索：逐个 drive 调 `/api/fs/search`（v3）或 `/api/public/search`（公开）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let mut conf = load_drives(cfg, site)?;
    let mut out = Vec::new();
    let mut first_err: Option<crate::error::CoreError> = None;
    for drive in conf.drives.iter_mut() {
        if !drive.search {
            continue;
        }
        match search_one(cfg, drive, keyword) {
            Ok(mut items) => out.append(&mut items),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(err) = first_err {
            return Err(err);
        }
    }
    Ok(out)
}

/// 播放：集 id 是 AList 路径，`fs/get` 取直链。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "csp_Alist 播放地址为空"));
    }
    let conf = load_drives(cfg, site)?;
    let first = id.split("~~~").next().unwrap_or(id);
    let path_part = play_path(first);
    let (idx, full) = find_drive_index(&conf.drives, path_part)
        .ok_or_else(|| parse_err(site, format!("csp_Alist 播放 id 无法匹配到 drive：{path_part}")))?;
    let mut drive = conf.drives[idx].clone();
    let url = resolve(cfg, &mut drive, &full)
        .map_err(|e| parse_err(site, format!("csp_Alist 取直链失败：{e}")))?
        .ok_or_else(|| parse_err(site, "csp_Alist 未从 fs/get 拿到 raw_url"))?;
    Ok(json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": { "User-Agent": UA },
    }))
}

/* ============================================================
 * ext 解析
 * ============================================================ */

/// 读取并解析 ext；`ext` 是 http(s) 时先拉远程 JSON。
pub fn load_drives(cfg: &TvConfig, site: &Site) -> Result<ExtConfig> {
    let raw = site.ext.clone().unwrap_or(Value::Null);
    let value = normalize_ext(cfg, site, raw)?;
    let mut vod_pic = DEFAULT_VOD_PIC.to_string();
    let mut drives: Vec<Drive> = Vec::new();

    match value {
        Value::Object(obj) => {
            if let Some(pic) = obj.get("vodPic").and_then(|v| v.as_str()) {
                if !pic.trim().is_empty() {
                    vod_pic = pic.trim().to_string();
                }
            }
            if let Some(list) = obj.get("drives").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(drive) = parse_drive(item, &vod_pic) {
                        drives.push(drive);
                    }
                }
            } else {
                // 平铺 map：{显示名: serverURL}
                for (name, item) in obj {
                    if name == "vodPic" || name == "drives" {
                        continue;
                    }
                    let Some(server) = item.as_str() else { continue };
                    let server = trim_server(server);
                    if !is_http(&server) {
                        continue;
                    }
                    drives.push(Drive {
                        name: name.clone(),
                        server,
                        search: true,
                        vod_pic: Some(vod_pic.clone()),
                        ..Default::default()
                    });
                }
            }
        }
        Value::Array(list) => {
            for item in list.iter() {
                if let Some(drive) = parse_drive(item, &vod_pic) {
                    drives.push(drive);
                }
            }
        }
        _ => {}
    }

    // 去重（同 server 只留第一台）并补全空名字。
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(drives.len());
    for mut drive in drives {
        if drive.server.trim().is_empty() {
            continue;
        }
        if drive.name.trim().is_empty() {
            drive.name = host_of(&drive.server);
        }
        if !seen.insert(drive.server.clone()) {
            continue;
        }
        if drive.vod_pic.is_none() {
            drive.vod_pic = Some(vod_pic.clone());
        }
        unique.push(drive);
    }

    if unique.is_empty() {
        return Err(parse_err(
            site,
            "csp_Alist 的 ext 未配置任何 AList 服务器（drives 为空）",
        ));
    }
    Ok(ExtConfig {
        drives: unique,
        vod_pic,
    })
}

fn normalize_ext(cfg: &TvConfig, site: &Site, raw: Value) -> Result<Value> {
    let text = match raw {
        Value::Null => String::new(),
        Value::String(s) => s,
        other => return Ok(other),
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(parse_err(site, "csp_Alist 的 ext 为空"));
    }
    if is_http(text) {
        let client = crate::http::client_for(&cfg.http)?;
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_string(), UA.to_string());
        let body = crate::http::get_text(&client, text, &headers, 2)?;
        return serde_json::from_str(&body)
            .map_err(|e| parse_err(site, format!("csp_Alist 远程 ext JSON 解析失败：{e}")));
    }
    serde_json::from_str(text)
        .map_err(|e| parse_err(site, format!("csp_Alist 的 ext 既不是 http 也不是合法 JSON：{e}")))
}

fn parse_drive(value: &Value, default_pic: &str) -> Option<Drive> {
    let obj = value.as_object()?;
    let server = trim_server(obj.get("server")?.as_str()?);
    if !is_http(&server) {
        return None;
    }
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let token = obj
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    let search = obj
        .get("search")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let hidden = obj
        .get("hidden")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let vod_pic = obj
        .get("vodPic")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or_else(|| Some(default_pic.to_string()));
    let login = obj.get("login").and_then(parse_login);
    let passwords = obj
        .get("params")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    let path = item.get("path")?.as_str()?.trim().to_string();
                    let password = item
                        .get("password")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((path, password))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Drive {
        name,
        server,
        path: obj
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        token,
        search,
        hidden,
        vod_pic,
        login,
        passwords,
    })
}

fn parse_login(value: &Value) -> Option<Login> {
    let obj = value.as_object()?;
    let username = obj
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let password = obj
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if username.is_empty() {
        return None;
    }
    Some(Login { username, password })
}

/* ============================================================
 * HTTP 调用
 * ============================================================ */

fn headers_for(drive: &Drive) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    if let Some(token) = drive.token.as_deref().filter(|t| !t.trim().is_empty()) {
        headers.insert("Authorization".to_string(), token.to_string());
    }
    headers
}

fn post(cfg: &TvConfig, drive: &Drive, endpoint: &str, body: &Value) -> Result<String> {
    let client = crate::http::client_for(&cfg.http)?;
    let url = format!("{}{}", drive.server, endpoint);
    crate::http::post_json_text(&client, &url, &headers_for(drive), &body.to_string())
}

/// 带一次「登录后重试」的 POST。
fn call_with_login(
    cfg: &TvConfig,
    drive: &mut Drive,
    endpoint: &str,
    body: &Value,
) -> Result<String> {
    // AList 的「需要登录」有两种表现：HTTP 200 + `Guest user is disabled`，
    // 以及 HTTP 401/403（`http::post_json_text` 会把非 2xx 直接转成错误）。
    // 两种都要尝试登录一次再重试。
    match post(cfg, drive, endpoint, body) {
        Ok(text) => {
            if needs_login(&text) && login(cfg, drive)? {
                return post(cfg, drive, endpoint, body);
            }
            Ok(text)
        }
        Err(err) => {
            if login_required_http(&err.to_string()) && login(cfg, drive)? {
                return post(cfg, drive, endpoint, body);
            }
            Err(err)
        }
    }
}

fn login_required_http(message: &str) -> bool {
    message.contains("HTTP 401") || message.contains("HTTP 403")
}

fn login(cfg: &TvConfig, drive: &mut Drive) -> Result<bool> {
    let Some(login) = drive.login.clone() else {
        return Ok(false);
    };
    let body = json!({ "username": login.username, "password": login.password });
    let client = crate::http::client_for(&cfg.http)?;
    let url = format!("{}/api/auth/login", drive.server);
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    let text = crate::http::post_json_text(&client, &url, &headers, &body.to_string())?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let token = value
        .get("data")
        .and_then(|v| v.get("token"))
        .and_then(|v| v.as_str())
        .or_else(|| value.get("token").and_then(|v| v.as_str()));
    if let Some(token) = token.filter(|t| !t.trim().is_empty()) {
        drive.token = Some(token.to_string());
        return Ok(true);
    }
    Ok(false)
}

fn needs_login(text: &str) -> bool {
    if text.contains("Guest user is disabled") {
        return true;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value) => matches!(
            value.get("code").and_then(|c| c.as_i64()),
            Some(401) | Some(403)
        ),
        Err(_) => false,
    }
}

/// 列目录：先匿名/token `fs/list`，失败再回退 `public/path`。
fn list(cfg: &TvConfig, drive: &mut Drive, path: &str, site: &Site) -> Result<Vec<Entry>> {
    let body = json!({
        "path": path,
        "password": password_for(drive, path),
        "page": 1,
        "per_page": 0,
        "refresh": false,
    });
    let text = call_with_login(cfg, drive, "/api/fs/list", &body)?;
    if let Some(entries) = entries_from(&text, &["content"]) {
        return Ok(entries);
    }
    let public = json!({ "path": path, "password": password_for(drive, path) });
    let text = call_with_login(cfg, drive, "/api/public/path", &public)?;
    entries_from(&text, &["files", "content"]).ok_or_else(|| {
        parse_err(
            site,
            "AList 目录响应既不是合法 JSON，也没有 data/content：上游可能被改版或拦截",
        )
    })
}

/// 取文件直链。
fn resolve(cfg: &TvConfig, drive: &mut Drive, path: &str) -> Result<Option<String>> {
    let body = json!({ "path": path, "password": password_for(drive, path) });
    let text = call_with_login(cfg, drive, "/api/fs/get", &body)?;
    if let Some(url) = url_from(&text) {
        return Ok(Some(url));
    }
    let text = call_with_login(cfg, drive, "/api/public/path", &body)?;
    Ok(url_from(&text))
}

fn search_one(cfg: &TvConfig, drive: &mut Drive, keyword: &str) -> Result<Vec<VideoItem>> {
    let body = json!({
        "parent": "/",
        "keywords": keyword,
        "page": 1,
        "per_page": 100,
    });
    let text = call_with_login(cfg, drive, "/api/fs/search", &body)?;
    let mut entries = entries_from(&text, &["content"]);
    if entries.is_none() {
        let public = json!({ "keyword": keyword, "path": "/" });
        let text = call_with_login(cfg, drive, "/api/public/search", &public)?;
        entries = entries_from(&text, &["content", "files"]);
    }
    let Some(entries) = entries else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries {
        if entry.is_dir || !has_video_ext(&entry.name) {
            continue;
        }
        let parent = entry.parent.clone().unwrap_or_else(|| "/".to_string());
        out.push(VideoItem {
            id: format!("{}{}", drive.name, join_path(&parent, &entry.name)),
            name: entry.name.clone(),
            cover: entry.thumb.clone().or_else(|| drive.vod_pic.clone()),
            remark: Some(drive.name.clone()),
        });
    }
    Ok(out)
}

fn entries_from(text: &str, keys: &[&str]) -> Option<Vec<Entry>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let data = value.get("data")?;
    if let Some(list) = data.as_array() {
        return Some(list.iter().filter_map(Entry::from_value).collect());
    }
    for key in keys {
        if let Some(list) = data.get(*key).and_then(|v| v.as_array()) {
            return Some(list.iter().filter_map(Entry::from_value).collect());
        }
    }
    for key in keys {
        if let Some(obj) = data.get(*key).filter(|v| v.is_object()) {
            return Entry::from_value(obj).map(|entry| vec![entry]);
        }
    }
    None
}

fn url_from(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let data = value.get("data").unwrap_or(&value);
    let raw = data
        .get("raw_url")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("url").and_then(|v| v.as_str()))
        .or_else(|| {
            data.get("files")
                .and_then(|v| v.as_array())
                .and_then(|list| list.first())
                .and_then(|file| file.get("url").and_then(|v| v.as_str()))
        })
        .or_else(|| {
            data.as_array()
                .and_then(|list| list.first())
                .and_then(|file| file.get("url").and_then(|v| v.as_str()))
        })?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("//") {
        return Some(format!("http://{rest}"));
    }
    Some(raw.to_string())
}

/* ============================================================
 * 工具
 * ============================================================ */

fn is_http(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn trim_server(server: &str) -> String {
    server.trim().trim_end_matches('/').to_string()
}

fn host_of(server: &str) -> String {
    server
        .split("//")
        .nth(1)
        .unwrap_or(server)
        .split('/')
        .next()
        .unwrap_or(server)
        .split(':')
        .next()
        .unwrap_or(server)
        .to_string()
}

/// 驱动名 + 绝对路径 → (drive 下标, 剩余路径)；多个前缀命中时取最长。
fn find_drive_index(drives: &[Drive], id: &str) -> Option<(usize, String)> {
    let mut best: Option<(usize, usize)> = None;
    for (index, drive) in drives.iter().enumerate() {
        if split_id(&drive.name, id).is_some() {
            let replace = best
                .map(|(_, len)| drive.name.len() > len)
                .unwrap_or(true);
            if replace {
                best = Some((index, drive.name.len()));
            }
        }
    }
    let (index, _) = best?;
    let rest = split_id(&drives[index].name, id)?.to_string();
    Some((index, rest))
}

fn split_id<'a>(name: &str, id: &'a str) -> Option<&'a str> {
    let rest = id.strip_prefix(name)?;
    if rest.is_empty() {
        return Some("");
    }
    if rest.starts_with('/') {
        return Some(rest);
    }
    None
}

/// 播放 id 里可能带 `名称$路径`；取 `$` 之后的路径形态。
fn play_path(id: &str) -> &str {
    if let Some((_, tail)) = id.rsplit_once('$') {
        if tail.starts_with('/') && !tail.is_empty() {
            return tail;
        }
    }
    id
}

fn password_for(drive: &Drive, path: &str) -> String {
    let mut best: Option<(usize, &str)> = None;
    for (prefix, password) in &drive.passwords {
        let matched = prefix.is_empty() || path.starts_with(prefix.as_str());
        if !matched {
            continue;
        }
        let replace = best
            .map(|(len, _)| prefix.len() > len)
            .unwrap_or(true);
        if replace {
            best = Some((prefix.len(), password.as_str()));
        }
    }
    best.map(|(_, password)| password.to_string())
        .unwrap_or_default()
}

fn join_path(base: &str, name: &str) -> String {
    let base = base.trim_end_matches('/');
    let name = name.trim_start_matches('/');
    if name.is_empty() {
        if base.is_empty() {
            "/".to_string()
        } else {
            format!("{base}/")
        }
    } else if base.is_empty() {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn parent_of(path: &str) -> String {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

fn last_segment(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let segment = trimmed.rsplit('/').next()?;
    if segment.is_empty() {
        None
    } else {
        Some(segment.to_string())
    }
}

fn strip_ext(name: &str) -> String {
    match name.rfind('.') {
        Some(index) if index > 0 => name[..index].to_string(),
        _ => name.to_string(),
    }
}

fn has_video_ext(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit_once('.') {
        Some((_, ext)) => VIDEO_EXTS.contains(&ext),
        None => false,
    }
}

fn human_size(size: u64) -> String {
    if size == 0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/* ============================================================
 * 测试
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site_with(ext: Value) -> Site {
        let mut site = Site::default();
        site.key = "alist-test".to_string();
        site.name = "AList".to_string();
        site.api = API.to_string();
        site.ext = Some(ext);
        site
    }

    fn drive(name: &str, server: &str) -> Drive {
        Drive {
            name: name.to_string(),
            server: server.to_string(),
            search: true,
            vod_pic: Some(DEFAULT_VOD_PIC.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn handles_api_case_insensitive() {
        assert!(handles("csp_Alist"));
        assert!(handles("CSP_ALIST"));
        assert!(!handles("csp_AlistX"));
    }

    #[test]
    fn flat_map_ext_becomes_public_drives() {
        let cfg = TvConfig::default();
        let site = site_with(json!({
            "🌊七米蓝": "https://al.chirmyram.com/",
            "🐉神族九帝": "https://alist.shenzjd.com",
        }));
        let conf = load_drives(&cfg, &site).unwrap();
        assert_eq!(conf.drives.len(), 2);
        assert_eq!(conf.vod_pic, DEFAULT_VOD_PIC);
        let first = conf
            .drives
            .iter()
            .find(|d| d.name.contains("七米蓝"))
            .unwrap();
        assert_eq!(first.server, "https://al.chirmyram.com");
        assert!(first.token.is_none());
        assert!(first.search);
        assert!(!first.hidden);
    }

    #[test]
    fn drives_ext_reads_login_and_flags() {
        let cfg = TvConfig::default();
        let site = site_with(json!({
            "vodPic": "https://img.example/pic.png",
            "drives": [
                {
                    "name": "私有库",
                    "server": "https://a.example/alist/",
                    "token": "",
                    "search": false,
                    "hidden": true,
                    "login": {"username": "u", "password": "p"},
                    "params": [{"path": "/secret", "password": "1234"}]
                }
            ]
        }));
        let conf = load_drives(&cfg, &site).unwrap();
        assert_eq!(conf.vod_pic, "https://img.example/pic.png");
        assert_eq!(conf.drives.len(), 1);
        let drive = &conf.drives[0];
        assert_eq!(drive.server, "https://a.example/alist");
        assert!(!drive.search);
        assert!(drive.hidden);
        assert!(drive.token.is_none());
        assert_eq!(drive.login.as_ref().unwrap().username, "u");
        assert_eq!(password_for(drive, "/secret/x.mp4"), "1234");
        assert_eq!(password_for(drive, "/other/x.mp4"), "");
    }

    #[test]
    fn longest_drive_prefix_wins_and_split() {
        let drives = vec![drive("A", "http://a"), drive("AB", "http://ab")];
        let (index, rest) = find_drive_index(&drives, "AB/电影/x.mp4").unwrap();
        assert_eq!(index, 1);
        assert_eq!(rest, "/电影/x.mp4");
        let (index, rest) = find_drive_index(&drives, "A/剧").unwrap();
        assert_eq!(index, 0);
        assert_eq!(rest, "/剧");
        assert!(find_drive_index(&drives, "ABC").is_none());
    }

    #[test]
    fn entries_parse_list_and_public_shape() {
        let list = r#"{"code":200,"data":{"content":[
            {"name":"电影","size":0,"is_dir":true,"type":1},
            {"name":"a.mp4","size":2048,"is_dir":false,"type":0,"thumb":"https://t.example/a.jpg"}
        ]}}"#;
        let entries = entries_from(list, &["content"]).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "a.mp4");
        assert_eq!(entries[1].size, 2048);
        assert_eq!(entries[1].thumb.as_deref(), Some("https://t.example/a.jpg"));

        let public = r#"{"code":200,"data":{"files":[{"name":"b.mkv","size":10,"type":0}]}}"#;
        let entries = entries_from(public, &["files"]).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_dir);
    }

    #[test]
    fn entries_ignore_unnamed_and_treat_type_one_as_dir() {
        let text = r#"{"data":{"content":[{"name":""},{"name":"d","type":1}]}}"#;
        let entries = entries_from(text, &["content"]).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_dir);
    }

    #[test]
    fn url_from_supports_raw_url_and_protocol_relative() {
        assert_eq!(
            url_from(r#"{"data":{"raw_url":"https://d.example/x.mp4?sign=1"}}"#).unwrap(),
            "https://d.example/x.mp4?sign=1"
        );
        assert_eq!(
            url_from(r#"{"data":{"url":"//c.example/y.m3u8"}}"#).unwrap(),
            "http://c.example/y.m3u8"
        );
        assert!(url_from(r#"{"data":{"files":[]}}"#).is_none());
        assert!(url_from("not json").is_none());
    }

    #[test]
    fn needs_login_detects_guest_disabled_and_forbidden_code() {
        assert!(needs_login(
            r#"{"code":401,"message":"Guest user is disabled"}"#
        ));
        assert!(needs_login(r#"{"code":403}"#));
        assert!(!needs_login(r#"{"code":200,"data":{}}"#));
        assert!(!needs_login("<html>not json</html>"));
        // 非 2xx 被 http 层转成错误后，只能从消息里识别 401/403。
        assert!(login_required_http("HTTP 401 Unauthorized 拉取失败: https://a/api/fs/list"));
        assert!(login_required_http("HTTP 403 Forbidden 拉取失败: https://a/api/fs/list"));
        assert!(!login_required_http("HTTP 500 Internal Server Error 拉取失败"));
    }

    #[test]
    fn path_helpers() {
        assert_eq!(join_path("/", "a"), "/a");
        assert_eq!(join_path("/电影", "a.mp4"), "/电影/a.mp4");
        assert_eq!(join_path("", "a.mp4"), "/a.mp4");
        assert_eq!(parent_of("/电影/a.mp4"), "/电影");
        assert_eq!(parent_of("/a.mp4"), "/");
        assert_eq!(last_segment("/电影/剧集").as_deref(), Some("剧集"));
        assert_eq!(play_path("名称$/电影/a.mp4"), "/电影/a.mp4");
        assert_eq!(play_path("/电影/a.mp4"), "/电影/a.mp4");
        assert_eq!(strip_ext("a.b.mkv"), "a.b");
        assert!(has_video_ext("X.MP4"));
        assert!(!has_video_ext("a.zip"));
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5), "5 B");
    }

    #[test]
    fn categories_skip_hidden_and_dedupe() {
        let cfg = TvConfig::default();
        let site = site_with(json!({
            "drives": [
                {"name": "公开", "server": "https://a.example"},
                {"name": "隐藏", "server": "https://b.example", "hidden": true},
                {"name": "公开", "server": "https://c.example"}
            ]
        }));
        let cats = categories(&cfg, &site).unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, "公开");
    }

    #[test]
    fn empty_ext_is_a_parse_error() {
        let cfg = TvConfig::default();
        let site = site_with(json!({}));
        let err = load_drives(&cfg, &site).unwrap_err();
        assert!(err.to_string().contains("csp_Alist"), "{err}");
    }
}
