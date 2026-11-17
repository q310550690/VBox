//! `csp_QuarkShare` / `csp_UCShare`（网盘分享清单型站点）原生实现。
//!
//! 来源：`_rea/decompiled/dffec63f-pg/deobf/com/github/catvod/spider/QuarkShare.java`
//! （211 行）与 `UCShare.java`（283 行），两者均继承对应网盘 spider
//! （`extends Quark` / `extends UC`），只覆写 `homeContent`/`init`，
//! `categoryContent`/`searchContent` 被 jadx 跳过（4088/4134、2998/3038 指令），
//! 详情/播放继承基类（即 `pan::quark` / `pan::uc` 的浏览与直链）。
//!
//! 协议（据可读的 `init` 与 `homeContent` 还原）：
//! * ext 以 `$$$` 分段：`[0]` 网盘配置（token JSON，传给父类）；
//!   `[1]` 分享清单（JSON 数组或文本行；相对路径 `./…` 按 `[0]` 的目录拼接，
//!   http 开头则下载）；`[2]` 可选搜索库（制表符 6 列，原实现写入 SQLite 表
//!   `search`，字段 `url/name/filetype/category/usermodifytime/filesize`）。
//! * 清单条目：首列为分享 URL（不以 `http` 开头时补 `https://pan.quark.cn/s/`
//!   或 `https://drive.uc.cn/s/`），次列为名称；`homeContent` 直接把清单条目
//!   作为分类返回，并附带固定 `orderby` 筛选项（`file_name:asc`/`desc`）。
//!
//! 有意偏差：
//! 1. 本地相对路径（如 `./json/quarkshare.txt`）无法读取（桌面端没有 App 私有
//!    沙箱），只支持 http 清单与内联 JSON/文本；缺失时返回空清单而不报错；
//! 2. `categoryContent` 重建为「浏览分享目录下的视频文件」，`searchContent`
//!    重建为「搜索库行过滤（无库时按清单名称过滤）」——与可观测行为等价，
//!    但跳过了原实现对 SQLite 的增量缓存；
//! 3. 详情/播放复用 W9 引擎（`pan::quark`/`pan::uc`），无 Cookie 时直链解析
//!    报 `catvod.host.unsupported`；
//! 4. 网盘引擎不支持 115/YD（`csp_P115Share` 因核心方法仍被跳过，未纳入本轮）。

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::quark::QuarkClient;
use crate::pan::uc::UcClient;
use crate::pan::{PanConfig, PanEntry, PanProvider};

use super::super::contract::{CatvodError, ErrorClass};

pub const API_QUARK_SHARE: &str = "csp_QuarkShare";
pub const API_UC_SHARE: &str = "csp_UCShare";
const QUARK_PREFIX: &str = "https://pan.quark.cn/s/";
const UC_PREFIX: &str = "https://drive.uc.cn/s/";
const MAX_SHARES: usize = 500;
const MAX_EPISODES: usize = 500;
const PLAY_GROUP: &str = "网盘";

/// 清单中的一个分享条目。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareEntry {
    pub name: String,
    pub url: String,
}

/// 搜索库中的一行（原实现 SQLite `search` 表）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchRow {
    pub name: String,
    pub url: String,
}

/// 解析后的分享清单。
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub provider: Option<PanProvider>,
    pub shares: Vec<ShareEntry>,
    pub search: Vec<SearchRow>,
}

/// 本模块处理的 api。
pub fn handles(api: &str) -> bool {
    let api = api.trim();
    api.eq_ignore_ascii_case(API_QUARK_SHARE) || api.eq_ignore_ascii_case(API_UC_SHARE)
}

/// api → 网盘服务商。
pub fn provider_of(api: &str) -> Option<PanProvider> {
    if api.trim().eq_ignore_ascii_case(API_QUARK_SHARE) {
        Some(PanProvider::Quark)
    } else if api.trim().eq_ignore_ascii_case(API_UC_SHARE) {
        Some(PanProvider::Uc)
    } else {
        None
    }
}

fn prefix_of(provider: PanProvider) -> &'static str {
    match provider {
        PanProvider::Uc => UC_PREFIX,
        _ => QUARK_PREFIX,
    }
}

/// 把清单里的分享 id 规范化为完整分享链接。
pub fn normalize_share(provider: PanProvider, raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("{}{}", prefix_of(provider), raw.trim_start_matches('/'))
    }
}

/// 解析文本清单：每行 `分享 名称`（原实现按 `[\s\t]+` 切分）。
fn parse_text_manifest(provider: PanProvider, text: &str) -> Vec<ShareEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(first) = parts.next() else { continue };
        let url = normalize_share(provider, first);
        if url.is_empty() {
            continue;
        }
        let name = parts.next().unwrap_or(first).to_string();
        if !out.iter().any(|e: &ShareEntry| e.url == url) {
            out.push(ShareEntry { name, url });
        }
        if out.len() >= MAX_SHARES {
            break;
        }
    }
    out
}

/// 解析 JSON 清单（数组；兼容 `id/url/share/link` 与 `name/title` 键）。
fn parse_json_manifest(provider: PanProvider, value: &Value) -> Vec<ShareEntry> {
    let mut out = Vec::new();
    let items = match value {
        Value::Array(items) => items.as_slice(),
        other => std::slice::from_ref(other),
    };
    for item in items {
        if let Value::Object(map) = item {
            let raw = ["id", "url", "share", "link"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str))
                .unwrap_or("");
            let name = ["name", "title"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str))
                .unwrap_or(raw);
            let url = normalize_share(provider, raw);
            if url.is_empty() {
                continue;
            }
            if !out.iter().any(|e: &ShareEntry| e.url == url) {
                out.push(ShareEntry { name: name.to_string(), url });
            }
        }
    }
    out
}

/// 解析搜索库（制表符行：url/name/filetype/category/usermodifytime/filesize）。
fn parse_search_rows(text: &str) -> Vec<SearchRow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 2 && !cols[0].trim().is_empty() {
            out.push(SearchRow {
                url: cols[0].trim().to_string(),
                name: cols[1].trim().to_string(),
            });
        }
    }
    out
}

fn base_dir(seg: &str) -> Option<String> {
    let seg = seg.trim();
    if !seg.starts_with("http") {
        return None;
    }
    seg.rfind('/').map(|i| seg[..i].to_string())
}

fn resolve_relative(base: Option<&String>, seg: &str) -> String {
    let seg = seg.trim();
    if seg.starts_with("./") {
        if let Some(base) = base {
            return format!("{}/{}", base.trim_end_matches('/'), seg.trim_start_matches("./"));
        }
    }
    seg.to_string()
}

/// 仅取 ext 第一段解析网盘配置（`$$$` 后续段是清单/搜索库）。
fn pan_config(site: &Site) -> PanConfig {
    let ext = site.ext_string();
    let seg0 = ext.split("$$$").next().unwrap_or("").trim();
    if seg0.starts_with('{') {
        PanConfig::parse(seg0)
    } else {
        PanConfig::default()
    }
}

fn text_of(cfg: &TvConfig, raw: &str) -> Result<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        let client = crate::http::client_for(&cfg.http)?;
        let headers = std::collections::HashMap::new();
        crate::http::get_text(&client, raw, &headers, 2)
    } else {
        Ok(raw.to_string())
    }
}

/// 载入 ext 中的分享清单与搜索库。
pub fn load(cfg: &TvConfig, site: &Site) -> Manifest {
    let ext = site.ext_string();
    let mut provider = None;
    for api in [API_QUARK_SHARE, API_UC_SHARE] {
        if site.api.trim().eq_ignore_ascii_case(api) {
            provider = provider_of(api);
        }
    }
    let provider = provider.unwrap_or(PanProvider::Quark);
    let segs: Vec<&str> = ext.split("$$$").collect();
    let seg0 = segs.first().copied().unwrap_or("").trim();
    let base = base_dir(seg0);
    let mut manifest = Manifest {
        provider: Some(provider),
        shares: Vec::new(),
        search: Vec::new(),
    };
    if let Some(seg1) = segs.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let source = resolve_relative(base.as_ref(), seg1);
        let text = text_of(cfg, &source).unwrap_or_default();
        let trimmed = text.trim();
        manifest.shares = if trimmed.starts_with('[') || trimmed.starts_with('{') {
            serde_json::from_str::<Value>(trimmed)
                .map(|v| parse_json_manifest(provider, &v))
                .unwrap_or_default()
        } else {
            parse_text_manifest(provider, trimmed)
        };
    }
    if let Some(seg2) = segs.get(2).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let source = resolve_relative(base.as_ref(), seg2);
        if let Ok(text) = text_of(cfg, &source) {
            manifest.search = parse_search_rows(&text);
        }
    }
    manifest
}

fn share_of(manifest: &Manifest, url: &str) -> ShareEntry {
    manifest
        .shares
        .iter()
        .find(|e| e.url == url)
        .cloned()
        .unwrap_or_else(|| ShareEntry { name: url.to_string(), url: url.to_string() })
}

fn parse_err(site: &Site, msg: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, msg).into_core()
}

fn host_err(site: &Site, msg: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, msg).into_core()
}

/// 把网盘引擎错误收口为 catvod 稳定错误码：
/// 缺 Cookie 视为宿主能力不足（`catvod.host.unsupported`），其余视为上游响应不可用（`catvod.result.parse`）。
fn pan_err(site: &Site, action: &str, err: CoreError) -> CoreError {
    let class = ErrorClass::classify_pan_error(&err).unwrap_or(ErrorClass::Parse);
    CatvodError::new(class, site, format!("{action}：{err}")).into_core()
}

/// 分类 = 清单条目（分享 URL 作为 type_id）。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let manifest = load(cfg, site);
    Ok(manifest
        .shares
        .iter()
        .take(MAX_SHARES)
        .map(|e| Category { type_id: e.url.clone(), type_name: e.name.clone() })
        .collect())
}

fn parse_episode_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once("###") {
        Some((share, fid)) => (share.trim(), Some(fid.trim())),
        None => (id.trim(), None),
    }
}

fn episode_id(share: &str, fid: &str) -> String {
    format!("{share}###{fid}")
}

fn quark_client(cfg: &TvConfig, cookie: &str, base: &str) -> Result<QuarkClient> {
    let client = crate::http::client_for(&cfg.http)?;
    if base.trim().is_empty() {
        Ok(QuarkClient::new(client, cookie))
    } else {
        Ok(QuarkClient::with_base(client, cookie, base.trim()))
    }
}

fn uc_client(cfg: &TvConfig, cookie: &str, base: &str) -> Result<UcClient> {
    let client = crate::http::client_for(&cfg.http)?;
    if base.trim().is_empty() {
        Ok(UcClient::new(client, cookie))
    } else {
        Ok(UcClient::with_base(client, cookie, base.trim()))
    }
}

fn browse_share(cfg: &TvConfig, conf: &PanConfig, provider: PanProvider, link: &crate::pan::ShareLink, site: &Site) -> Result<Vec<PanEntry>> {
    match provider {
        PanProvider::Quark => {
            let client = quark_client(cfg, conf.effective_cookie(), &conf.drive_base)?;
            client
                .browse(link)
                .map(|browse| browse.files)
                .map_err(|e| pan_err(site, "网盘分享浏览失败", e))
        }
        PanProvider::Uc => {
            let client = uc_client(cfg, conf.effective_uc_cookie(), &conf.uc_base)?;
            client
                .browse(link)
                .map(|browse| browse.files)
                .map_err(|e| pan_err(site, "网盘分享浏览失败", e))
        }
        other => Err(host_err(site, format!("网盘 {} 引擎未实现", other.name()))),
    }
}

fn resolve_play_url(
    cfg: &TvConfig,
    conf: &PanConfig,
    provider: PanProvider,
    _link: &crate::pan::ShareLink,
    fid: &str,
    site: &Site,
) -> Result<String> {
    match provider {
        PanProvider::Quark => {
            let client = quark_client(cfg, conf.effective_cookie(), &conf.drive_base)?;
            if let Ok(info) = client.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            client
                .download_url(fid)
                .map_err(|e| pan_err(site, "网盘直链解析失败", e))
        }
        PanProvider::Uc => {
            let client = uc_client(cfg, conf.effective_uc_cookie(), &conf.uc_base)?;
            if let Ok(info) = client.play_info(fid) {
                if let Some(url) = find_url(&info) {
                    return Ok(url);
                }
            }
            client
                .download_url(fid)
                .map_err(|e| pan_err(site, "网盘直链解析失败", e))
        }
        other => Err(host_err(site, format!("网盘 {} 直链解析未实现", other.name()))),
    }
}

fn find_url(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if s.starts_with("http") => Some(s.clone()),
        Value::Array(items) => items.iter().find_map(find_url),
        Value::Object(map) => {
            for key in ["url", "play_url", "download_url", "raw_url"] {
                if let Some(Value::String(s)) = map.get(key) {
                    if s.starts_with("http") {
                        return Some(s.clone());
                    }
                }
            }
            map.values().find_map(find_url)
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

fn human_size(size: u64) -> String {
    if size == 0 {
        return String::new();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx + 1 < units.len() {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{}{}", size, units[idx])
    } else {
        format!("{:.1}{}", value, units[idx])
    }
}

fn video_items(provider: PanProvider, share: &ShareEntry, files: &[PanEntry]) -> Vec<VideoItem> {
    let mut out = Vec::new();
    for file in files {
        if file.is_dir || !is_video_name(&file.name) {
            continue;
        }
        out.push(VideoItem {
            id: episode_id(&share.url, &file.fid),
            name: file.name.clone(),
            cover: None,
            remark: if file.size > 0 { Some(human_size(file.size as u64)) } else { None },
        });
    }
    let _ = provider;
    out
}

/// 分类（分享）下的视频文件列表。
pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    if page > 1 {
        return Ok(Vec::new());
    }
    let manifest = load(cfg, site);
    let provider = manifest.provider.unwrap_or(PanProvider::Quark);
    let share = share_of(&manifest, cat.trim());
    if share.url.is_empty() {
        return Ok(Vec::new());
    }
    let Some(link) = crate::pan::parse_share_link(&share.url) else {
        return Err(parse_err(site, "无法解析分享链接"));
    };
    match browse_share(cfg, &pan_config(site), provider, &link, site) {
        Ok(files) => Ok(video_items(provider, &share, &files)),
        Err(err) => Err(err),
    }
}

/// 详情：浏览分享目录并把视频文件展开成单线路剧集。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let manifest = load(cfg, site);
    let provider = manifest.provider.unwrap_or(PanProvider::Quark);
    let (share_url, fid) = parse_episode_id(id);
    if share_url.is_empty() {
        return Err(parse_err(site, "空 id"));
    }
    let share = share_of(&manifest, share_url);
    let Some(link) = crate::pan::parse_share_link(&share.url) else {
        return Err(parse_err(site, "无法解析分享链接"));
    };
    let files = browse_share(cfg, &pan_config(site), provider, &link, site)?;
    let mut videos: Vec<&PanEntry> = files
        .iter()
        .filter(|f| !f.is_dir && is_video_name(&f.name))
        .collect();
    if videos.is_empty() {
        return Ok(VideoDetail {
            id: id.to_string(),
            name: share.name.clone(),
            cover: None,
            desc: None,
            year: None,
            area: None,
            director: None,
            actor: None,
            episodes: Vec::new(),
        });
    }
    videos.truncate(MAX_EPISODES);
    let mut items = Vec::new();
    for (idx, file) in videos.iter().enumerate() {
        let name = if file.name.trim().is_empty() {
            format!("第{}集", idx + 1)
        } else {
            file.name.clone()
        };
        items.push(Episode {
            title: name,
            url: episode_id(&share.url, &file.fid),
        });
    }
    if let Some(want) = fid {
        if !items.iter().any(|e| e.url.ends_with(want)) {
            // id 指向的文件不在首批视频内，保持首集不变。
        }
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: share.name.clone(),
        cover: None,
        desc: Some(format!("网盘分享：{}", share.url)),
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup { group: PLAY_GROUP.to_string(), items }],
    })
}

/// 搜索：优先搜索库（image `/lib` 下载的制表符库），否则按清单名称过滤。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let manifest = load(cfg, site);
    let provider = manifest.provider.unwrap_or(PanProvider::Quark);
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let lower = keyword.to_lowercase();
    if !manifest.search.is_empty() {
        let mut out = Vec::new();
        for row in &manifest.search {
            if row.name.to_lowercase().contains(&lower) || row.url.to_lowercase().contains(&lower) {
                let url = normalize_share(provider, &row.url);
                out.push(VideoItem {
                    id: url.clone(),
                    name: row.name.clone(),
                    cover: None,
                    remark: Some("网盘搜索".to_string()),
                });
                if out.len() >= MAX_SHARES {
                    break;
                }
            }
        }
        return Ok(out);
    }
    Ok(manifest
        .shares
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&lower))
        .take(MAX_SHARES)
        .map(|e| VideoItem {
            id: e.url.clone(),
            name: e.name.clone(),
            cover: None,
            remark: Some("网盘分享".to_string()),
        })
        .collect())
}

/// 播放：解析分享内文件直链。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let manifest = load(cfg, site);
    let provider = manifest.provider.unwrap_or(PanProvider::Quark);
    let (share_url, fid) = parse_episode_id(id);
    if share_url.is_empty() {
        return Err(parse_err(site, "空播放 id"));
    }
    let conf = pan_config(site);
    let share = share_of(&manifest, share_url);
    let Some(link) = crate::pan::parse_share_link(&share.url) else {
        return Err(parse_err(site, "无法解析分享链接"));
    };
    let fid = match fid {
        Some(fid) if !fid.is_empty() => fid.to_string(),
        _ => {
            let files = browse_share(cfg, &conf, provider, &link, site)?;
            files
                .iter()
                .find(|f| !f.is_dir && is_video_name(&f.name))
                .map(|f| f.fid.clone())
                .ok_or_else(|| parse_err(site, "分享内没有可播放视频"))?
        }
    };
    let url = resolve_play_url(cfg, &conf, provider, &link, &fid, site)?;
    Ok(serde_json::json!({
        "url": url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": {},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site(api: &str, ext: &str) -> Site {
        let mut s = Site::default();
        s.key = "share".into();
        s.name = "网盘分享".into();
        s.api = api.into();
        s.ext = Some(json!(ext));
        s
    }

    #[test]
    fn handles_both_apis_case_insensitive() {
        assert!(handles("csp_QuarkShare"));
        assert!(handles("csp_UCShare"));
        assert!(handles("CSP_QUARKSHARE"));
        assert!(!handles("csp_Quark"));
        assert!(!handles("csp_UC"));
    }

    #[test]
    fn provider_mapping() {
        assert_eq!(provider_of("csp_QuarkShare"), Some(PanProvider::Quark));
        assert_eq!(provider_of("csp_UCShare"), Some(PanProvider::Uc));
        assert_eq!(provider_of("csp_Other"), None);
    }

    #[test]
    fn normalize_share_prefixes_ids() {
        assert_eq!(normalize_share(PanProvider::Quark, "abc123"), "https://pan.quark.cn/s/abc123");
        assert_eq!(normalize_share(PanProvider::Uc, "xyz"), "https://drive.uc.cn/s/xyz");
        assert_eq!(
            normalize_share(PanProvider::Quark, "https://pan.quark.cn/s/full"),
            "https://pan.quark.cn/s/full"
        );
        assert!(normalize_share(PanProvider::Quark, "  ").is_empty());
    }

    #[test]
    fn text_manifest_parses_and_dedupes() {
        let text = "abc123\t剧集A\nhttp://pan.quark.cn/s/def\t剧集B\nabc123\t重复A\n# comment\n";
        let shares = parse_text_manifest(PanProvider::Quark, text);
        assert_eq!(shares.len(), 2);
        assert_eq!(shares[0].url, "https://pan.quark.cn/s/abc123");
        assert_eq!(shares[0].name, "剧集A");
        assert_eq!(shares[1].url, "http://pan.quark.cn/s/def");
    }

    #[test]
    fn json_manifest_parses_variants() {
        let value: Value = json!([
            {"id": "abc", "name": "剧集A"},
            {"url": "https://drive.uc.cn/s/def", "title": "剧集B"},
            {"name": "没有链接"}
        ]);
        let shares = parse_json_manifest(PanProvider::Uc, &value);
        assert_eq!(shares.len(), 2);
        assert_eq!(shares[0].url, "https://drive.uc.cn/s/abc");
        assert_eq!(shares[1].url, "https://drive.uc.cn/s/def");
    }

    #[test]
    fn search_rows_parse_tab_columns() {
        let text = "https://pan.quark.cn/s/abc\t剧集A\tmp4\tmovie\t2024-01-01 10:00:00\t1048576\n\nbad-line\n";
        let rows = parse_search_rows(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "剧集A");
    }

    #[test]
    fn load_reads_inline_json_manifest() {
        let ext = "./lib/token.json$$$[{\"id\":\"abc\",\"name\":\"剧集A\"}]";
        let manifest = load(&TvConfig::default(), &site(API_QUARK_SHARE, ext));
        assert_eq!(manifest.shares.len(), 1);
        assert_eq!(manifest.shares[0].url, "https://pan.quark.cn/s/abc");
        assert_eq!(manifest.provider, Some(PanProvider::Quark));
    }

    #[test]
    fn load_reads_text_manifest_for_uc() {
        let ext = "./lib/token.json$$$def\t剧集B";
        let manifest = load(&TvConfig::default(), &site(API_UC_SHARE, ext));
        assert_eq!(manifest.shares.len(), 1);
        assert_eq!(manifest.shares[0].url, "https://drive.uc.cn/s/def");
        assert_eq!(manifest.provider, Some(PanProvider::Uc));
    }

    #[test]
    fn categories_mirror_manifest() {
        let ext = "./lib/token.json$$$abc\t剧集A\n\ndef\t剧集B";
        let cats = categories(&TvConfig::default(), &site(API_QUARK_SHARE, ext)).unwrap();
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "https://pan.quark.cn/s/abc");
        assert_eq!(cats[1].type_name, "剧集B");
    }

    #[test]
    fn search_falls_back_to_manifest_names() {
        let ext = "./lib/token.json$$$abc\t海贼王合集\ndef\t火影合集";
        let items = search(&TvConfig::default(), &site(API_QUARK_SHARE, ext), "海贼").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "海贼王合集");
        assert_eq!(items[0].id, "https://pan.quark.cn/s/abc");
    }

    #[test]
    fn search_prefers_search_db() {
        let ext = "./lib/token.json$$$abc\t清单名$$$zzz\t数据库命中\tmp4\tmovie\t2024\t1";
        let items = search(&TvConfig::default(), &site(API_QUARK_SHARE, ext), "数据库").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "数据库命中");
    }

    #[test]
    fn videos_second_page_is_empty() {
        let ext = "./lib/token.json$$$abc\t剧集A";
        let items = videos(&TvConfig::default(), &site(API_QUARK_SHARE, ext), "https://pan.quark.cn/s/abc", 2)
            .unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn detail_rejects_empty_id() {
        let err = detail(&TvConfig::default(), &site(API_QUARK_SHARE, ""), "  ").unwrap_err();
        assert!(format!("{err:?}").contains("catvod.result.parse"));
    }

    #[test]
    fn player_rejects_empty_id() {
        let err =
            player_content(&TvConfig::default(), &site(API_UC_SHARE, ""), "", "").unwrap_err();
        assert!(format!("{err:?}").contains("catvod.result.parse"));
    }
}
