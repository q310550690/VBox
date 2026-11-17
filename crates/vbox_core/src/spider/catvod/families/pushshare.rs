//! `csp_PushShare`（推送资源）原生实现。
//!
//! 反编译证据（只读参考）：
//! * `_rea/tmp/w10/vf-deobf/com/github/catvod/spider/PushShare.java`（Vineflower）
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/PushShare.java`（jadx）
//! * 同目录 `Push.java`（基类：`playerContent` 返回 `parse:0` + 直链 + 请求头）
//!
//! 原版 PushShare 自身几乎不含播放逻辑：`init` 把一份「分享链接 + 显示名」清单当作分类，
//! `homeContent` 原样返回清单，`categoryContent` / `detailContent` / `playerContent` 都按分享链接的
//! 网盘类型委派给 Ali / Quark / UC / P115 / Thunder / PikPak 子引擎。本实现用仓库已有的 `pan`
//! 客户端完成同样的委派，统一 `id = {分享链接}###{目录或文件 fid}`。
//!
//! 与反编译版的有意偏差：
//! 1. ext 同时支持 JSON 对象（`list_source` / `list_url`）与 `$$$` 第二段两种清单源写法；原版只认第二段。
//! 2. 清单文本的显示名保留空格（`splitn(2, whitespace)`），原版按 `[\s\t]+` 切分后只取第二个 token。
//! 3. JSON 清单额外兼容 `{url,name}` / `{type_id,type_name}` 对象与 `{list:[...]}` 包装。
//! 4. 分类保持扁平（每个清单条目一个分类），不实现原版磁力分组与 orderby 筛选项。
//! 5. `search` 只在清单显示名上做本地包含匹配，不做原版针对阿里「资源」的远程并发搜索。
//! 6. `videos` 把目录项作为可下钻条目、视频文件作为可直接播放条目（原版分类页直接委派子引擎首页）。
//! 7. `detail` 只展开一层目录，视频文件作为单线路剧集；给到文件 fid 时用浅层 BFS 定位并降级为单集。
//!
//! 该家族此前 unported，语义以本实现与测试为准，不要求与反编译版逐字节等价。

use std::collections::{BTreeSet, HashMap, VecDeque};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::ali::{AliClient, API_BASE as ALI_API_BASE, REFERER as ALI_REFERER, UA as ALI_UA};
use crate::pan::p115::{is_video_name, P115Client, UA as P115_UA, WEB_BASE as P115_WEB_BASE};
use crate::pan::pikpak::PikPakClient;
use crate::pan::quark::{QuarkClient, DRIVE_BASE as QUARK_DRIVE_BASE, UA as QUARK_UA};
use crate::pan::uc::{UcClient, DRIVE_BASE as UC_DRIVE_BASE, UA as UC_UA};
use crate::pan::xunlei::{
    parse_share as parse_xunlei_share, ShareRef, XunleiDrive, API_BASE as XUNLEI_API_BASE,
    AUTH_BASE as XUNLEI_AUTH_BASE, PIKPAK_API_BASE, PIKPAK_AUTH_BASE, PIKPAK_SHARE_WEB,
    SHARE_WEB as XUNLEI_SHARE_WEB, UA as XUNLEI_UA, XUNLEI_CLIENT_ID, XUNLEI_CLIENT_SECRET,
};
use crate::pan::{parse_share_link, PanConfig, PanEntry, PanProvider, ShareLink};

use super::super::contract::{CatvodError, ErrorClass};

/// 家族 API 名（与 CatVod 侧 `csp_PushShare` 保持一致）。
pub const API: &str = "csp_PushShare";

/// 剧集组名（推送资源清单本身不分组）。
const GROUP: &str = "推送资源";
/// 分类分页大小（沿用仓库其它网盘家族约定）。
const PAGE_SIZE: usize = 20;
/// 清单条目上限，避免异常 ext 撑爆内存。
const MAX_ENTRIES: usize = 500;
/// 单次详情最多返回的剧集数。
const MAX_EPISODES: usize = 500;
/// 定位文件 fid 时最多浏览的目录数。
const MAX_DIRS: usize = 200;
/// 定位文件 fid 时的最大 BFS 深度。
const MAX_DEPTH: u32 = 3;

const P115_REFERER: &str = "https://115.com/";
const QUARK_REFERER: &str = "https://pan.quark.cn/";
const UC_REFERER: &str = "https://drive.uc.cn/";

/// 是否由本家族处理该 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail.into()).into_core()
}

/// ext 里的可选项（清单源与各网盘 mock/自建网关基址）。
#[derive(Debug, Default, Clone)]
struct Hooks {
    list_source: Option<String>,
    list_url: Option<String>,
    segment_manifest: Option<String>,
    p115_base: Option<String>,
    quark_base: Option<String>,
    uc_base: Option<String>,
    ali_base: Option<String>,
    xunlei_base: Option<String>,
    xunlei_auth_base: Option<String>,
    pikpak_base: Option<String>,
    pikpak_auth_base: Option<String>,
    token: Option<String>,
}

fn hooks(site: &Site) -> Hooks {
    let ext = site.ext_string();
    let mut hooks = Hooks::default();
    if ext.trim().starts_with('{') {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&ext) {
            hooks.list_source = value_text(map.get("list_source"));
            hooks.list_url = text_field(&map, &["list_url"]);
            hooks.p115_base = text_field(&map, &["p115_base", "115_base"]);
            hooks.quark_base = text_field(&map, &["quark_base", "drive_base"]);
            hooks.uc_base = text_field(&map, &["uc_base"]);
            hooks.ali_base = text_field(&map, &["ali_base"]);
            hooks.xunlei_base = text_field(&map, &["xunlei_base", "thunder_base"]);
            hooks.xunlei_auth_base = text_field(&map, &["xunlei_auth_base"]);
            hooks.pikpak_base = text_field(&map, &["pikpak_base"]);
            hooks.pikpak_auth_base = text_field(&map, &["pikpak_auth_base"]);
            hooks.token = text_field(&map, &["token"]);
        }
    }
    if let Some(second) = ext.split("$$$").map(str::trim).nth(1) {
        if !second.is_empty() {
            hooks.segment_manifest = Some(second.to_string());
        }
    }
    hooks
}

fn text_field(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        map.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn value_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// 读取清单源文本：内联 `list_source` 优先，其次 `list_url` / `$$$` 第二段（http 抓取或内联文本）。
fn manifest_text(cfg: &TvConfig, site: &Site) -> Result<String> {
    let hooks = hooks(site);
    if let Some(text) = hooks.list_source.as_deref() {
        return Ok(text.to_string());
    }
    let source = hooks
        .list_url
        .as_deref()
        .or(hooks.segment_manifest.as_deref())
        .unwrap_or("")
        .trim();
    if source.is_empty() {
        return Err(parse_err(
            site,
            "csp_PushShare 缺少清单源：请在 ext 配置 list_source / list_url 或第二段（$$$）",
        ));
    }
    if source.starts_with("./") || source.starts_with(".\\") {
        return Err(parse_err(
            site,
            format!("csp_PushShare 清单源为相对路径（桌面端无配置目录）：{source}"),
        ));
    }
    // 只有「单 token 且以 http 开头」才当成远程清单：内联文本可能整行就是一个分享链接 + 显示名。
    let single_token = !source.chars().any(char::is_whitespace);
    if single_token && (source.starts_with("http://") || source.starts_with("https://")) {
        let client = client_for(&cfg.http)?;
        return get_text(&client, source, &HashMap::new(), cfg.http.retry);
    }
    Ok(source.to_string())
}

/// 清单条目：分享链接 + 显示名。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    url: String,
    name: String,
}

/// 反编译版对裸分享 id 按长度补域名（Quark 12 / 阿里 10 / PikPak 26）。
fn normalize_share(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.contains("://") {
        return raw.to_string();
    }
    let head = raw
        .split('?')
        .next()
        .unwrap_or(raw)
        .split('#')
        .next()
        .unwrap_or(raw)
        .trim();
    match head.len() {
        12 => format!("https://pan.quark.cn/s/{head}"),
        10 => format!("https://www.alipan.com/s/{head}"),
        26 => format!("https://mypikpak.com/s/{head}"),
        _ => raw.to_string(),
    }
}

fn parse_manifest(text: &str) -> Vec<Entry> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            let entries = json_entries(&value);
            if !entries.is_empty() {
                return entries;
            }
        }
    }
    parse_manifest_lines(text)
}

fn parse_manifest_lines(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let link = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        if link.is_empty() || name.is_empty() {
            continue;
        }
        let url = normalize_share(link);
        if url.is_empty() {
            continue;
        }
        entries.push(Entry {
            url,
            name: name.to_string(),
        });
    }
    entries
}

fn json_entries(value: &Value) -> Vec<Entry> {
    match value {
        Value::Array(items) => entries_from_array(items),
        Value::Object(map) => {
            for key in ["list", "data", "items", "sites", "sources", "entries"] {
                if let Some(child) = map.get(key) {
                    let entries = json_entries(child);
                    if !entries.is_empty() {
                        return entries;
                    }
                }
            }
            entry_from_object(map).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

fn entries_from_array(items: &[Value]) -> Vec<Entry> {
    let mut entries = Vec::new();
    for item in items {
        match item {
            Value::Object(map) => entries.extend(entry_from_object(map)),
            Value::String(text) => entries.extend(parse_manifest_lines(text)),
            Value::Array(_) => entries.extend(json_entries(item)),
            _ => {}
        }
    }
    entries
}

fn entry_from_object(map: &serde_json::Map<String, Value>) -> Option<Entry> {
    let url = ["url", "type_id", "link", "share", "id"]
        .iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(normalize_share)
        .filter(|url| !url.is_empty())?;
    let name = ["name", "type_name", "title", "label", "display", "remark"]
        .iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| url.clone());
    Some(Entry { url, name })
}

fn load_manifest(cfg: &TvConfig, site: &Site) -> Result<Vec<Entry>> {
    let text = manifest_text(cfg, site)?;
    let mut entries = parse_manifest(&text);
    entries.truncate(MAX_ENTRIES);
    if entries.is_empty() {
        return Err(parse_err(
            site,
            "csp_PushShare 清单为空或没有有效条目（每行需要「分享链接 显示名」）",
        ));
    }
    Ok(entries)
}

fn pan_conf(site: &Site) -> PanConfig {
    PanConfig::parse(&site.ext_string())
}

fn base_with_slash(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        String::new()
    } else if raw.ends_with('/') {
        raw.to_string()
    } else {
        format!("{raw}/")
    }
}

fn pick_base<'a>(hook: Option<&'a str>, configured: &'a str, default: &'a str) -> String {
    let raw = hook
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let configured = configured.trim();
            (!configured.is_empty()).then_some(configured)
        })
        .unwrap_or(default);
    base_with_slash(raw)
}

/// 分享链接解析结果：pan 通用 / 迅雷 / PikPak。
#[derive(Debug, Clone)]
enum Target {
    Pan(ShareLink),
    Xunlei(ShareRef),
    PikPak(ShareRef),
}

fn target_of(raw: &str) -> Option<Target> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(link) = parse_share_link(raw) {
        return Some(Target::Pan(link));
    }
    let share = parse_xunlei_share(raw)?;
    if share.url.starts_with(PIKPAK_SHARE_WEB) {
        Some(Target::PikPak(share))
    } else {
        Some(Target::Xunlei(share))
    }
}

fn target_or_err(site: &Site, share: &str) -> Result<Target> {
    target_of(share)
        .ok_or_else(|| parse_err(site, format!("csp_PushShare 不支持的分享类型：{share}")))
}

fn ref_of(share: &str, fid: &str) -> String {
    format!("{share}###{fid}")
}

fn split_ref(id: &str) -> (String, String) {
    match id.split_once("###") {
        Some((share, fid)) => (share.trim().to_string(), fid.trim().to_string()),
        None => (id.trim().to_string(), String::new()),
    }
}

/// 浏览分享内目录（fid 为空表示分享根目录）。
fn browse(cfg: &TvConfig, site: &Site, share: &str, fid: &str) -> Result<Vec<PanEntry>> {
    match target_or_err(site, share)? {
        Target::Pan(mut link) => {
            if !fid.is_empty() {
                link.sub_fid = fid.to_string();
            }
            pan_browse(cfg, site, &link)
        }
        Target::Xunlei(share_ref) => xunlei_browse(cfg, site, &share_ref, fid, false),
        Target::PikPak(share_ref) => xunlei_browse(cfg, site, &share_ref, fid, true),
    }
}

fn pan_browse(cfg: &TvConfig, site: &Site, link: &ShareLink) -> Result<Vec<PanEntry>> {
    let hooks = hooks(site);
    let conf = pan_conf(site);
    let client = client_for(&cfg.http)?;
    Ok(match &link.provider {
        PanProvider::P115 => {
            let base = pick_base(hooks.p115_base.as_deref(), "", P115_WEB_BASE);
            P115Client::with_base(client, conf.effective_p115_cookie(), &base)
                .browse(link)?
                .files
        }
        PanProvider::Quark => {
            let base = pick_base(
                hooks.quark_base.as_deref(),
                &conf.drive_base,
                QUARK_DRIVE_BASE,
            );
            QuarkClient::with_base(client, conf.effective_cookie(), &base)
                .browse(link)?
                .files
        }
        PanProvider::Uc => {
            let base = pick_base(hooks.uc_base.as_deref(), &conf.uc_base, UC_DRIVE_BASE);
            UcClient::with_base(client, conf.effective_uc_cookie(), &base)
                .browse(link)?
                .files
        }
        PanProvider::Ali => {
            let base = pick_base(hooks.ali_base.as_deref(), &conf.ali_base, ALI_API_BASE);
            AliClient::with_base(client, conf.effective_ali_cookie(), &base)
                .browse(link)?
                .files
        }
    })
}

fn xunlei_browse(
    cfg: &TvConfig,
    site: &Site,
    share: &ShareRef,
    fid: &str,
    pikpak: bool,
) -> Result<Vec<PanEntry>> {
    let hooks = hooks(site);
    let client = client_for(&cfg.http)?;
    let token = hooks.token.as_deref().unwrap_or("");
    if pikpak {
        let api_base = pick_base(hooks.pikpak_base.as_deref(), "", PIKPAK_API_BASE);
        let auth_base = pick_base(hooks.pikpak_auth_base.as_deref(), "", PIKPAK_AUTH_BASE);
        let drive = PikPakClient::with_base(client, token, &api_base, &auth_base);
        let (_title, files) = drive.share_files(&share.share_id, &share.passcode, fid)?;
        Ok(files)
    } else {
        let api_base = pick_base(hooks.xunlei_base.as_deref(), "", XUNLEI_API_BASE);
        let auth_base = pick_base(hooks.xunlei_auth_base.as_deref(), "", XUNLEI_AUTH_BASE);
        let drive = XunleiDrive::with_bases(
            client,
            token,
            &api_base,
            &auth_base,
            "迅雷",
            XUNLEI_CLIENT_ID,
            XUNLEI_CLIENT_SECRET,
        );
        let (_title, files) = drive.share_files(&share.share_id, &share.passcode, fid)?;
        Ok(files)
    }
}

fn page_slice(entries: &[Entry], page: u32) -> &[Entry] {
    let page = page.max(1) as usize;
    let start = (page - 1) * PAGE_SIZE;
    if start >= entries.len() {
        &[]
    } else {
        &entries[start..(start + PAGE_SIZE).min(entries.len())]
    }
}

/// 分类 id 支持三种写法：清单链接本身、1 基序号、以及（容错）直接传显示名以外的值。
fn resolve_cat(entries: &[Entry], cat: &str) -> String {
    if target_of(cat).is_some() {
        return cat.to_string();
    }
    if let Ok(index) = cat.parse::<usize>() {
        if (1..=entries.len()).contains(&index) {
            return entries[index - 1].url.clone();
        }
    }
    cat.to_string()
}

/// 分类：每个清单条目一个分类（扁平，不做「显示名/子类」前缀分组）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let entries = load_manifest(cfg, site)?;
    Ok(entries
        .into_iter()
        .map(|entry| Category {
            type_id: entry.url,
            type_name: entry.name,
        })
        .collect())
}

/// 分类列表：空分类返回整份清单（分页）；否则浏览对应分享的根目录。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let cat = cat.trim();
    let entries = load_manifest(cfg, site)?;
    if cat.is_empty() {
        return Ok(page_slice(&entries, page)
            .iter()
            .map(|entry| VideoItem {
                id: entry.url.clone(),
                name: entry.name.clone(),
                cover: None,
                remark: Some("分享清单".to_string()),
            })
            .collect());
    }
    let share = resolve_cat(&entries, cat);
    let files = browse(cfg, site, &share, "")?;
    Ok(files
        .into_iter()
        .take(MAX_ENTRIES)
        .map(|file| {
            let cover = if file.thumbnail.trim().is_empty() {
                None
            } else {
                Some(file.thumbnail.clone())
            };
            let remark = file.is_dir.then(|| "文件夹".to_string());
            VideoItem {
                id: ref_of(&share, &file.fid),
                name: file.name,
                cover,
                remark,
            }
        })
        .collect())
}

/// 详情：分享根目录下的视频文件成为单线路剧集；文件 fid 通过浅层 BFS 定位后作为单集。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    let (share, fid) = split_ref(id);
    if share.is_empty() {
        return Err(parse_err(site, "csp_PushShare 详情 id 为空"));
    }
    let entries = load_manifest(cfg, site).unwrap_or_default();
    let name = entries
        .iter()
        .find(|entry| entry.url == share)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| GROUP.to_string());
    let files = browse(cfg, site, &share, &fid)?;
    let mut episodes = video_episodes(&share, &files);
    if episodes.is_empty() && !fid.is_empty() {
        if let Some(file) = find_entry(cfg, site, &share, &fid)? {
            if !file.is_dir {
                episodes.push(Episode {
                    title: file.name,
                    url: ref_of(&share, &fid),
                });
            }
        }
    }
    if episodes.is_empty() {
        episodes = files
            .iter()
            .filter(|file| file.is_dir)
            .take(MAX_EPISODES)
            .map(|file| Episode {
                title: file.name.clone(),
                url: ref_of(&share, &file.fid),
            })
            .collect();
    }
    if episodes.is_empty() {
        return Err(parse_err(
            site,
            format!("csp_PushShare 分享目录为空或没有可播放文件：{share}###{fid}"),
        ));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: None,
        desc: Some(format!("推送资源：{share}")),
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

fn video_episodes(share: &str, files: &[PanEntry]) -> Vec<Episode> {
    files
        .iter()
        .filter(|file| !file.is_dir && is_video_name(&file.name))
        .take(MAX_EPISODES)
        .map(|file| Episode {
            title: file.name.clone(),
            url: ref_of(share, &file.fid),
        })
        .collect()
}

/// 浅层 BFS：在分享目录树里定位某个 fid（给到文件 fid 时详情降级为单集）。
fn find_entry(
    cfg: &TvConfig,
    site: &Site,
    share: &str,
    target_fid: &str,
) -> Result<Option<PanEntry>> {
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    queue.push_back((String::new(), 0));
    let mut listed = 0usize;
    while let Some((fid, depth)) = queue.pop_front() {
        if !visited.insert(fid.clone()) {
            continue;
        }
        if listed >= MAX_DIRS {
            break;
        }
        listed += 1;
        for file in browse(cfg, site, share, &fid)? {
            if file.fid == target_fid {
                return Ok(Some(file));
            }
            if file.is_dir && depth < MAX_DEPTH {
                queue.push_back((file.fid.clone(), depth + 1));
            }
        }
    }
    Ok(None)
}

/// 搜索：只在清单显示名上做本地包含匹配（大小写不敏感）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let entries = load_manifest(cfg, site)?;
    let needle = keyword.to_lowercase();
    Ok(entries
        .into_iter()
        .filter(|entry| entry.name.to_lowercase().contains(&needle))
        .take(MAX_ENTRIES)
        .map(|entry| VideoItem {
            id: entry.url,
            name: entry.name,
            cover: None,
            remark: None,
        })
        .collect())
}

/// 播放：按分享链接的网盘类型取直链，返回 CatVod 约定的 `parse:0` 结构。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let (share, fid) = split_ref(id);
    if share.is_empty() {
        return Err(parse_err(site, "csp_PushShare 播放地址为空"));
    }
    if fid.is_empty() {
        return Err(parse_err(
            site,
            "csp_PushShare 播放需要先选择分享内文件（id 缺 fid）",
        ));
    }
    let target = target_or_err(site, &share)?;
    let url = resolve_direct_url(cfg, site, &target, &fid)?;
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": header_for(&target),
    }))
}

fn resolve_direct_url(cfg: &TvConfig, site: &Site, target: &Target, fid: &str) -> Result<String> {
    let hooks = hooks(site);
    let conf = pan_conf(site);
    let client = client_for(&cfg.http)?;
    match target {
        Target::Pan(link) => match &link.provider {
            PanProvider::P115 => {
                let base = pick_base(hooks.p115_base.as_deref(), "", P115_WEB_BASE);
                let client = P115Client::with_base(client, conf.effective_p115_cookie(), &base);
                client.download_url(&link.share_id, &link.passcode, fid)
            }
            PanProvider::Quark => {
                let base = pick_base(
                    hooks.quark_base.as_deref(),
                    &conf.drive_base,
                    QUARK_DRIVE_BASE,
                );
                let client = QuarkClient::with_base(client, conf.effective_cookie(), &base);
                client.download_url(fid)
            }
            PanProvider::Uc => {
                let base = pick_base(hooks.uc_base.as_deref(), &conf.uc_base, UC_DRIVE_BASE);
                let client = UcClient::with_base(client, conf.effective_uc_cookie(), &base);
                client.download_url(fid)
            }
            PanProvider::Ali => {
                let base = pick_base(hooks.ali_base.as_deref(), &conf.ali_base, ALI_API_BASE);
                let client = AliClient::with_base(client, conf.effective_ali_cookie(), &base);
                let token = client.share_token(&link.share_id, &link.passcode)?;
                client.download_url(&link.share_id, fid, &token.share_token)
            }
        },
        Target::Xunlei(share) => {
            let api_base = pick_base(hooks.xunlei_base.as_deref(), "", XUNLEI_API_BASE);
            let auth_base = pick_base(hooks.xunlei_auth_base.as_deref(), "", XUNLEI_AUTH_BASE);
            let drive = XunleiDrive::with_bases(
                client,
                hooks.token.as_deref().unwrap_or(""),
                &api_base,
                &auth_base,
                "迅雷",
                XUNLEI_CLIENT_ID,
                XUNLEI_CLIENT_SECRET,
            );
            let token = drive.share_token(&share.share_id, &share.passcode)?;
            drive.download_url(
                &share.share_id,
                &share.passcode,
                &token.pass_code_token,
                fid,
            )
        }
        Target::PikPak(share) => {
            let api_base = pick_base(hooks.pikpak_base.as_deref(), "", PIKPAK_API_BASE);
            let auth_base = pick_base(hooks.pikpak_auth_base.as_deref(), "", PIKPAK_AUTH_BASE);
            let drive = PikPakClient::with_base(
                client,
                hooks.token.as_deref().unwrap_or(""),
                &api_base,
                &auth_base,
            );
            let token = drive.share_token(&share.share_id, &share.passcode)?;
            let pass_code_token = token
                .get("pass_code_token")
                .and_then(Value::as_str)
                .unwrap_or("");
            drive.download_url(&share.share_id, &share.passcode, pass_code_token, fid)
        }
    }
}

fn header_for(target: &Target) -> Value {
    let (ua, referer) = match target {
        Target::Pan(link) => match &link.provider {
            PanProvider::P115 => (P115_UA, P115_REFERER),
            PanProvider::Quark => (QUARK_UA, QUARK_REFERER),
            PanProvider::Uc => (UC_UA, UC_REFERER),
            PanProvider::Ali => (ALI_UA, ALI_REFERER),
        },
        Target::Xunlei(_) => (XUNLEI_UA, XUNLEI_SHARE_WEB),
        Target::PikPak(_) => (XUNLEI_UA, PIKPAK_SHARE_WEB),
    };
    json!({ "User-Agent": ua, "Referer": referer })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_with_ext(ext: Value) -> Site {
        Site {
            key: "pushshare-test".to_string(),
            name: "推送资源测试".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(ext),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_PushShare"));
        assert!(handles(" CSP_PUSHSHARE "));
        assert!(handles("csp_pushshare"));
        assert!(!handles("csp_P115Share"));
        assert!(!handles("csp_PushShare2"));
    }

    #[test]
    fn parses_text_manifest_with_comments_and_multiword_names() {
        let text = "# 注释行\nhttps://115.com/s/abc123 剧集 一\n\nabcdef1234 阿里剧集\n";
        let entries = parse_manifest(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].url, "https://115.com/s/abc123");
        assert_eq!(entries[0].name, "剧集 一");
        assert_eq!(entries[1].url, "https://www.alipan.com/s/abcdef1234");
        assert_eq!(entries[1].name, "阿里剧集");
    }

    #[test]
    fn normalizes_bare_share_ids_by_length() {
        assert_eq!(
            normalize_share("abcdef123456"),
            "https://pan.quark.cn/s/abcdef123456"
        );
        assert_eq!(
            normalize_share("abcdef1234"),
            "https://www.alipan.com/s/abcdef1234"
        );
        assert_eq!(
            normalize_share("abcdefghijklmnopqrstuvwxyz"),
            "https://mypikpak.com/s/abcdefghijklmnopqrstuvwxyz"
        );
        assert_eq!(
            normalize_share("https://example.com/v/1"),
            "https://example.com/v/1"
        );
        assert_eq!(normalize_share("example.com/v/1"), "example.com/v/1");
    }

    #[test]
    fn parses_json_manifest_array_and_object_wrapper() {
        let array = r#"[{"url":"https://115.com/s/abc123","name":"剧集一"},{"type_id":"https://pan.quark.cn/s/abc","type_name":"电影"}]"#;
        let entries = parse_manifest(array);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "剧集一");
        assert_eq!(entries[1].url, "https://pan.quark.cn/s/abc");
        assert_eq!(entries[1].name, "电影");

        let wrapped = r#"{"list":["https://115.com/s/abc123 剧集一"]}"#;
        let entries = parse_manifest(wrapped);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "剧集一");

        let single = r#"{"url":"https://www.alipan.com/s/abcdef1234","name":"阿里电影"}"#;
        let entries = parse_manifest(single);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "阿里电影");
    }

    #[test]
    fn rejects_garbage_manifest() {
        assert!(parse_manifest("").is_empty());
        assert!(parse_manifest("::::").is_empty());
        assert!(parse_manifest("# 只有注释").is_empty());
    }

    #[test]
    fn ref_round_trips() {
        let share = "https://115.com/s/abc123";
        let reference = ref_of(share, "f1");
        assert_eq!(reference, "https://115.com/s/abc123###f1");
        assert_eq!(split_ref(&reference), (share.to_string(), "f1".to_string()));
        assert_eq!(
            split_ref(" https://115.com/s/abc123 "),
            (share.to_string(), String::new())
        );
    }

    #[test]
    fn hooks_reads_json_object_and_second_segment() {
        let site = site_with_ext(json!({
            "list_source": "https://115.com/s/abc123 剧集",
            "p115_base": "http://127.0.0.1:1/",
        }));
        let h = hooks(&site);
        assert_eq!(
            h.list_source.as_deref(),
            Some("https://115.com/s/abc123 剧集")
        );
        assert_eq!(h.p115_base.as_deref(), Some("http://127.0.0.1:1/"));
        assert!(h.segment_manifest.is_none());

        let site = site_with_ext(json!("own-ext$$$https://115.com/s/abc123 剧集"));
        let h = hooks(&site);
        assert_eq!(
            h.segment_manifest.as_deref(),
            Some("https://115.com/s/abc123 剧集")
        );
    }

    #[test]
    fn target_of_covers_all_six_providers() {
        assert!(matches!(
            target_of("https://115.com/s/abc123"),
            Some(Target::Pan(link)) if link.provider == PanProvider::P115
        ));
        assert!(matches!(
            target_of("https://pan.quark.cn/s/abcdef123456"),
            Some(Target::Pan(link)) if link.provider == PanProvider::Quark
        ));
        assert!(matches!(
            target_of("https://www.alipan.com/s/abcdef1234"),
            Some(Target::Pan(link)) if link.provider == PanProvider::Ali
        ));
        assert!(matches!(
            target_of("https://drive.uc.cn/s/abcdef123456"),
            Some(Target::Pan(link)) if link.provider == PanProvider::Uc
        ));
        assert!(matches!(
            target_of("https://pan.xunlei.com/s/VNabc?pwd=1"),
            Some(Target::Xunlei(_))
        ));
        assert!(matches!(
            target_of("https://mypikpak.com/s/VNabc"),
            Some(Target::PikPak(_))
        ));
        assert!(target_of("https://example.com/v/1").is_none());
        assert!(target_of("").is_none());
    }

    #[test]
    fn missing_manifest_source_is_parse_error() {
        let cfg = TvConfig::default();
        let site = site_with_ext(json!({"p115_base": "http://127.0.0.1:1/"}));
        let err = categories(&cfg, &site).expect_err("缺少清单源应报错");
        let text = format!("{err}");
        assert!(text.contains("清单源"), "{text}");
        assert!(text.contains("catvod.result.parse"), "{text}");
    }

    #[test]
    fn player_requires_fid() {
        let cfg = TvConfig::default();
        let site = site_with_ext(json!({"list_source": "https://115.com/s/abc123 剧集"}));
        let err =
            player_content(&cfg, &site, "", "https://115.com/s/abc123").expect_err("缺 fid 应报错");
        let text = format!("{err}");
        assert!(text.contains("fid"), "{text}");
        assert!(text.contains("catvod.result.parse"), "{text}");
    }
}
