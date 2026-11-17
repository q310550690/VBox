//! `csp_ThunderShare`（迅雷分享清单型蜘蛛）原生实现。
//!
//! 来源（REA 反编译证据）：
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/ThunderShare.java`
//!   （`public class ThunderShare extends Thunder`；`init` 原文 429 行块，`categoryContent` jadx 跳过，
//!   Vineflower 恢复件见 `_rea/tmp/w10/vf-deobf/com/github/catvod/spider/ThunderShare.java:69`）。
//! * 协议分析：`_rea/tmp/w10-analysis.md` §W10.1 / §W10.3。
//!
//! 协议（逐段还原）：
//! * `init(Context, String)`：ext 按 `$$$` 拆：[0] 交基类 `Thunder.init`（登录/接口配置），
//!   [1] 清单源（`./` 相对路径时拼到 [0] 的目录；本实现不支持相对路径）。
//! * 清单源两种形态：
//!   1. JSON 数组（或以 `{` 开头的对象）：条目 `{type_id/url, type_name/name}`，
//!      `type_id` 不以 http 开头时补 `https://pan.xunlei.com/s/` 前缀；
//!   2. 纯文本，逐行按 `[\s\t]+` 切分取 `[share, name]`（Android 侧该正则由 7 字节 XOR 解出）；
//!      `name` 含 `/` 时前缀作为分组名。
//! * 内存模型：`h` 带密码清单、`d` 分类表、`e`/`f` 搜索/当前关键词；`homeContent` 同时把
//!   清单条目与组名作为分类返回。
//! * `categoryContent`：组名 → 分页返回组内条目（每页 20）；分享 URL → 浏览分享目录
//!   （Android 版按 `d1.o().Z(...)` 取口令后按 `path` 展开，桌面等价物见 `pan::xunlei`）。
//! * `detailContent`/`playerContent` 继承 `Thunder` 引擎：分享内文件展成剧集、直链下载。
//!   桌面端等价物是 [`crate::pan::xunlei::XunleiDrive`]。
//!
//! 有意偏差（桌面端能力边界 / 上游现状）：
//! 1. 清单源相对路径 `./json/xunlei.txt` 不可解析（`Site` 无配置目录概念）；
//!    改用 ext JSON 钩子 `list_source`（内联内容）/`list_url`（http 地址），或 `$$$` 第 2 段。
//! 2. Android UI 语义（排序筛选项 `orderby`、扫码登录、语音搜索、`JARSEARCH*` 语法）不实现。
//! 3. 迅雷分享目录浏览与直链需要登录态/口令 token；缺失时按 `catvod.pan.login_required`/
//!    `catvod.pan.share_unavailable` 报错。
//! 4. `ext[0]` 的账号/设备注册流程不实现；登录态由 ext JSON `token` 或 `cookie` 提供。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::xunlei::{parse_share, XunleiDrive, UA, XUNLEI_CLIENT_ID, XUNLEI_CLIENT_SECRET};

use super::super::contract::{CatvodError, ErrorClass};

/// 本家族覆盖的 api。
pub const API: &str = "csp_ThunderShare";

/// 迅雷分享前缀（原类 `init` 中的固定串）。
const SHARE_BASE: &str = "https://pan.xunlei.com/s/";

/// 详情线路名。
const GROUP: &str = "迅雷分享";

/// 分类/列表每页条数（原类 `categoryContent` 固定 20）。
const PAGE_SIZE: usize = 20;

/// 清单条数上限（防止脏配置导致内存暴涨）。
const MAX_ENTRIES: usize = 500;

/// 单次展开的最大剧集数。
const MAX_EPISODES: usize = 500;

/// 播放请求头 Referer（原类走引擎）。
const REFERER: &str = "https://pan.xunlei.com/";

/// 是否本家族覆盖的 api。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/// 统一构造 `catvod.result.parse` 错误。
fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

/// ext JSON 钩子（清单源/接口根/登录态）。
#[derive(Debug, Default, Clone)]
struct Hooks {
    list_source: Option<String>,
    list_url: Option<String>,
    api_base: Option<String>,
    auth_base: Option<String>,
    token: Option<String>,
    segment_manifest: Option<String>,
}

/// 取 ext 中的字符串字段。
fn ext_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 解析 ext：JSON 钩子优先；`$$$` 第 2 段作为后备清单源。
fn hooks(site: &Site) -> Hooks {
    let ext = site.ext_string();
    let mut h = Hooks::default();
    let trimmed = ext.trim();
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            h.list_source = ext_str(&v, "list_source");
            h.list_url = ext_str(&v, "list_url");
            h.api_base = ext_str(&v, "xunlei_base");
            h.auth_base = ext_str(&v, "xunlei_auth_base");
            h.token = ext_str(&v, "token");
        }
    }
    let segments: Vec<&str> = ext.split("$$$").map(str::trim).collect();
    if segments.len() > 1 && !segments[1].is_empty() {
        h.segment_manifest = Some(segments[1].to_string());
    }
    h
}

/// 分页切片（页码 1 基）。
fn page_slice<T: Clone>(items: &[T], page: u32) -> Vec<T> {
    let page = page.max(1) as usize;
    let start = (page - 1).saturating_mul(PAGE_SIZE);
    items.iter().skip(start).take(PAGE_SIZE).cloned().collect()
}

/// 读取清单文本：内联内容 > `list_url` > http 段 > 内联段。
fn manifest_text(cfg: &TvConfig, site: &Site) -> Result<String> {
    let h = hooks(site);
    if let Some(text) = h.list_source {
        return Ok(text);
    }
    let source = match (h.list_url, h.segment_manifest) {
        (Some(url), _) => url,
        (None, Some(seg)) => seg,
        (None, None) => String::new(),
    };
    let source = source.trim();
    if source.is_empty() {
        return Err(parse_err(
            site,
            "csp_ThunderShare 缺少清单源：请在 ext 配置 JSON 钩子 {\"list_source\":\"...\"} 或 {\"list_url\":\"http...\"}",
        ));
    }
    if source.starts_with("./") {
        return Err(parse_err(
            site,
            format!("csp_ThunderShare 清单源为相对路径（{source}），桌面端无配置目录，无法解析；请改用 list_source/list_url 钩子"),
        ));
    }
    if source.starts_with("http") {
        let client = client_for(&cfg.http)?;
        return get_text(
            &client,
            source,
            &std::collections::HashMap::new(),
            cfg.http.retry,
        );
    }
    Ok(source.to_string())
}

/// 清单条目：分享链接 + 显示名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    pub name: String,
}

/// 解析后的清单模型。
#[derive(Debug, Default, Clone)]
pub struct Manifest {
    /// 单层清单（保持声明顺序）。
    pub flat: Vec<Entry>,
    /// 分组清单：组名 → 条目。
    pub groups: BTreeMap<String, Vec<Entry>>,
}

fn normalize_share(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with("http") {
        raw.to_string()
    } else {
        format!("{SHARE_BASE}{raw}")
    }
}

fn push_flat(manifest: &mut Manifest, url: &str, name: &str) {
    if manifest.flat.len() >= MAX_ENTRIES {
        return;
    }
    let url = normalize_share(url);
    let name = name.trim();
    let name = if name.is_empty() {
        url.clone()
    } else {
        name.to_string()
    };
    if manifest.flat.iter().any(|e| e.url == url) {
        return;
    }
    manifest.flat.push(Entry { url, name });
}

fn push_group(manifest: &mut Manifest, group: &str, url: &str, name: &str) {
    let group = group.trim();
    if group.is_empty() || manifest.groups.values().map(Vec::len).sum::<usize>() >= MAX_ENTRIES {
        return;
    }
    let entry = Entry {
        url: normalize_share(url),
        name: name.trim().to_string(),
    };
    let bucket = manifest.groups.entry(group.to_string()).or_default();
    if bucket.iter().any(|e| e.url == entry.url) {
        return;
    }
    bucket.push(entry);
}

/// 解析 JSON 形态清单：数组，或带 `data`/`list` 的对象。
fn parse_manifest_json(text: &str) -> Option<Manifest> {
    let value: Value = serde_json::from_str(text).ok()?;
    let rows = match &value {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("data")
            .or_else(|| o.get("list"))
            .and_then(Value::as_array)
            .cloned()?,
        _ => return None,
    };
    let mut manifest = Manifest::default();
    for row in rows {
        let Some(obj) = row.as_object() else { continue };
        let url = ["type_id", "url", "share", "link", "id"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_string();
        if url.is_empty() {
            continue;
        }
        let name = ["type_name", "name", "title"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_string();
        let group = ["group", "type_flag", "folder"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_string();
        if group.is_empty() {
            push_flat(&mut manifest, &url, &name);
        } else {
            push_group(&mut manifest, &group, &url, &name);
        }
    }
    Some(manifest)
}

/// 解析纯文本清单：逐行 `share name`；`name` 含 `/` 时前缀为分组名。
fn parse_manifest_text(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let share = parts.next().unwrap_or("").trim();
        if share.is_empty() {
            continue;
        }
        let name = parts.next().unwrap_or("").trim();
        let name = if name.is_empty() { share } else { name };
        match name.rsplit_once('/') {
            Some((group, leaf)) if !group.trim().is_empty() && !leaf.trim().is_empty() => {
                push_group(&mut manifest, &group.replace('/', "|"), share, leaf);
            }
            _ => push_flat(&mut manifest, share, name),
        }
    }
    manifest
}

/// 解析清单文本（JSON 优先，其次纯文本）。
pub fn parse_manifest(text: &str) -> Manifest {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Some(m) = parse_manifest_json(trimmed) {
            return m;
        }
    }
    parse_manifest_text(trimmed)
}

/// 读取并解析清单。
fn load_manifest(cfg: &TvConfig, site: &Site) -> Result<Manifest> {
    let text = manifest_text(cfg, site)?;
    let manifest = parse_manifest(&text);
    if manifest.flat.is_empty() && manifest.groups.is_empty() {
        return Err(parse_err(site, "csp_ThunderShare 清单为空或无法解析"));
    }
    Ok(manifest)
}

/// 名称查找：按分享 URL 找清单里的显示名。
fn name_of(manifest: &Manifest, url: &str) -> Option<String> {
    if let Some(e) = manifest.flat.iter().find(|e| e.url == url) {
        return Some(e.name.clone());
    }
    for bucket in manifest.groups.values() {
        if let Some(e) = bucket.iter().find(|e| e.url == url) {
            return Some(e.name.clone());
        }
    }
    None
}

/// 构造 PikPak 客户端（接口根可用 ext JSON `pikpak_base`/`pikpak_auth_base` 覆盖，便于 mock）。
fn client(cfg: &TvConfig, site: &Site) -> Result<XunleiDrive> {
    let h = hooks(site);
    let token = h.token.unwrap_or_default();
    let http = client_for(&cfg.http)?;
    match (h.api_base, h.auth_base) {
        (Some(api), Some(auth)) => Ok(XunleiDrive::with_bases(
            http,
            &token,
            &api,
            &auth,
            "thunder",
            XUNLEI_CLIENT_ID,
            XUNLEI_CLIENT_SECRET,
        )),
        (Some(api), None) => Ok(XunleiDrive::with_bases(
            http,
            &token,
            &api,
            &api,
            "thunder",
            XUNLEI_CLIENT_ID,
            XUNLEI_CLIENT_SECRET,
        )),
        _ => XunleiDrive::with_rules(&cfg.http, &token),
    }
}

/// 分享链接 → 条目/剧集 id（`share###fid`，与网盘约定一致）。
fn ref_of(share: &str, fid: &str) -> String {
    format!("{share}###{fid}")
}

/// 拆分 id：`share###fid`；无 `###` 时 fid 为空。
fn split_ref(id: &str) -> (String, String) {
    match id.split_once("###") {
        Some((share, fid)) => (share.trim().to_string(), fid.trim().to_string()),
        None => (id.trim().to_string(), String::new()),
    }
}

/// 分享 URL → (标题, 条目列表)；目录项 id 带 fid。
fn browse_items(
    cfg: &TvConfig,
    site: &Site,
    share: &str,
    fid: &str,
) -> Result<(String, Vec<(String, String, bool)>)> {
    let Some(link) = parse_share(share) else {
        return Err(parse_err(
            site,
            format!("csp_ThunderShare 无法解析 迅雷分享链接：{share}"),
        ));
    };
    let client = client(cfg, site)?;
    let (title, files) = client.share_files(&link.share_id, &link.passcode, fid)?;
    Ok((
        title,
        files
            .into_iter()
            .map(|f| (f.fid, f.name, f.is_dir))
            .collect(),
    ))
}

/// 分类：`f` 每条清单 + `g` 每个组名。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let manifest = load_manifest(cfg, site)?;
    let mut out: Vec<Category> = Vec::new();
    for entry in &manifest.flat {
        out.push(Category {
            type_id: entry.url.clone(),
            type_name: entry.name.clone(),
        });
    }
    for group in manifest.groups.keys() {
        out.push(Category {
            type_id: group.clone(),
            type_name: group.clone(),
        });
    }
    Ok(out)
}

/// 列表：组名 → 组内条目分页；分享 URL → 浏览分享根目录。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let manifest = load_manifest(cfg, site)?;
    let type_id = type_id.trim();
    if type_id.is_empty() {
        return Ok(page_slice(&manifest.flat, page)
            .into_iter()
            .map(|e| VideoItem {
                id: e.url,
                name: e.name,
                cover: None,
                remark: None,
            })
            .collect());
    }
    if let Some(bucket) = manifest.groups.get(type_id) {
        return Ok(page_slice(bucket, page)
            .into_iter()
            .map(|e| VideoItem {
                id: e.url,
                name: e.name,
                cover: None,
                remark: None,
            })
            .collect());
    }
    if type_id.starts_with("http") {
        let (_, items) = browse_items(cfg, site, type_id, "")?;
        return Ok(page_slice(&items, page)
            .into_iter()
            .map(|(fid, name, is_dir)| VideoItem {
                id: ref_of(type_id, &fid),
                name,
                cover: None,
                remark: Some(if is_dir {
                    "文件夹".into()
                } else {
                    String::new()
                })
                .filter(|s| !s.is_empty()),
            })
            .collect());
    }
    Ok(Vec::new())
}

/// 详情：分享目录 → 剧集；单文件 → 单集。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let (share, fid) = split_ref(id);
    if share.is_empty() {
        return Err(parse_err(site, "csp_ThunderShare 详情 id 为空"));
    }
    let manifest = load_manifest(cfg, site).unwrap_or_default();
    let (title, items) = browse_items(cfg, site, &share, &fid)?;
    let name = name_of(&manifest, &share)
        .or_else(|| Some(title.trim().to_string()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| GROUP.to_string());
    let mut episodes: Vec<Episode> = items
        .iter()
        .filter(|(_, _, is_dir)| !*is_dir)
        .take(MAX_EPISODES)
        .map(|(fid, file_name, _)| Episode {
            title: file_name.clone(),
            url: ref_of(&share, fid),
        })
        .collect();
    if episodes.is_empty() {
        // 目录展开后无文件：把目录项作为可继续点击的条目（与 p115share/yunpan 降级一致）。
        episodes = items
            .iter()
            .take(MAX_EPISODES)
            .map(|(fid, file_name, _)| Episode {
                title: file_name.clone(),
                url: ref_of(&share, fid),
            })
            .collect();
    }
    if episodes.is_empty() {
        return Err(parse_err(
            site,
            format!("csp_ThunderShare 分享内没有可浏览内容：{share}"),
        ));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: None,
        desc: Some(format!("迅雷分享：{share}")),
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

/// 搜索：清单本地 contains（page>1 恒空，与原类一致）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let manifest = load_manifest(cfg, site)?;
    let needle = keyword.to_lowercase();
    let mut out: Vec<VideoItem> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut push = |e: &Entry, out: &mut Vec<VideoItem>, seen: &mut Vec<String>| {
        if out.len() >= MAX_ENTRIES || !e.name.to_lowercase().contains(&needle) {
            return;
        }
        if seen.contains(&e.url) {
            return;
        }
        seen.push(e.url.clone());
        out.push(VideoItem {
            id: e.url.clone(),
            name: e.name.clone(),
            cover: None,
            remark: None,
        });
    };
    for entry in &manifest.flat {
        push(entry, &mut out, &mut seen);
    }
    for bucket in manifest.groups.values() {
        for entry in bucket {
            push(entry, &mut out, &mut seen);
        }
    }
    Ok(out)
}

/// 播放：`share###fid` → 下载直链。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let (share, fid) = split_ref(id);
    if share.is_empty() {
        return Err(parse_err(site, "csp_ThunderShare 播放地址为空"));
    }
    if fid.is_empty() {
        return Err(parse_err(
            site,
            "csp_ThunderShare 播放需要先选择分享内文件（id 缺 fid）",
        ));
    }
    let Some(link) = parse_share(&share) else {
        return Err(parse_err(
            site,
            format!("csp_ThunderShare 无法解析 迅雷分享链接：{share}"),
        ));
    };
    let client = client(cfg, site)?;
    let token = client.share_token(&link.share_id, &link.passcode)?;
    let url = client.download_url(
        &link.share_id,
        &link.passcode,
        &token.pass_code_token,
        &fid,
    )?;
    let mut header = serde_json::Map::new();
    header.insert("User-Agent".into(), json!(UA));
    header.insert("Referer".into(), json!(REFERER));
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": url,
        "header": Value::Object(header),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_MANIFEST: &str = "abc123 剧集一\nhttps://pan.xunlei.com/s/def456 剧集二\n";

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_ThunderShare"));
        assert!(handles(" csp_thundershare "));
        assert!(!handles("csp_P115Share"));
    }

    #[test]
    fn parse_manifest_text_reads_flat_and_groups() {
        let m = parse_manifest(TEXT_MANIFEST);
        assert_eq!(m.flat.len(), 2);
        assert_eq!(m.flat[0].url, "https://pan.xunlei.com/s/abc123");
        assert_eq!(m.flat[1].name, "剧集二");
        assert_eq!(m.groups.len(), 0);
    }

    #[test]
    fn parse_manifest_text_group_prefix() {
        let m = parse_manifest("abc123 电影/战狼2\n");
        assert_eq!(m.flat.len(), 0);
        assert_eq!(m.groups.get("电影").map(Vec::len), Some(1));
    }

    #[test]
    fn parse_manifest_json_array_and_object() {
        let m = parse_manifest(
            r#"[{"type_id":"a1","type_name":"剧集一"},{"type_id":"https://pan.xunlei.com/s/a2","type_name":"剧集二"}]"#,
        );
        assert_eq!(m.flat.len(), 2);
        assert_eq!(m.flat[0].url, "https://pan.xunlei.com/s/a1");
        let m2 = parse_manifest(r#"{"list":[{"url":"a1","name":"剧集一","group":"电影"}]}"#);
        assert_eq!(m2.groups.get("电影").map(Vec::len), Some(1));
    }

    #[test]
    fn parse_manifest_rejects_garbage() {
        let m = parse_manifest("   \n# 注释\n");
        assert!(m.flat.is_empty() && m.groups.is_empty());
    }

    #[test]
    fn ref_round_trip() {
        let id = ref_of("https://pan.xunlei.com/s/abc", "f1");
        let (share, fid) = split_ref(&id);
        assert_eq!(share, "https://pan.xunlei.com/s/abc");
        assert_eq!(fid, "f1");
        assert!(split_ref("https://pan.xunlei.com/s/abc").1.is_empty());
    }

    #[test]
    fn hooks_reads_json_and_segments() {
        let site = Site {
            ext: Some(
                json!({"list_source":"a1 剧集一","xunlei_base":"http://127.0.0.1:1/","token":"tk"}),
            ),
            ..Site::default()
        };
        let h = hooks(&site);
        assert_eq!(h.list_source.as_deref(), Some("a1 剧集一"));
        assert_eq!(h.api_base.as_deref(), Some("http://127.0.0.1:1/"));
        assert_eq!(h.token.as_deref(), Some("tk"));
        let site2 = Site {
            ext: Some(json!("配置段$$$a1 剧集一")),
            ..Site::default()
        };
        assert_eq!(hooks(&site2).segment_manifest.as_deref(), Some("a1 剧集一"));
    }
}
