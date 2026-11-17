//! `csp_SambaShare`：SMB 分享清单型家族。
//!
//! 上游 `SambaShare extends Samba`：ext 按 `$$$` 拆，第 2 段是分享清单（http/内联，
//! 文本行按 `[\s\t]+` 切分为 `[分享路径, 显示名]`），分类 = 清单条目 + 分组名；
//! 列表/详情/播放都经由 Android 侧 `aliproxy` 伴生服务的 `/smb/list`、`/smb/download`。
//!
//! 桌面端等价物：把「分享路径」映射到本机已挂载的 SMB 目录（Windows UNC / 映射盘 /
//! macOS·Linux 挂载点），用 `std::fs` 列目录与定位文件，播放返回 `file://` URL。
//! 站点 ext JSON 钩子：`smb_root`（映射根，第一层子目录对应 share 名）、
//! `list_source`/`list_url`（清单源）；`$$$` 第 2 段作为清单源后备。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::{client_for, get_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::pan::smb::{file_url, SmbBackend, SmbPath};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_SambaShare";
const GROUP_LABEL: &str = "SMB分享";
const PAGE_SIZE: usize = 20;
const MAX_ENTRIES: usize = 500;
const MAX_EPISODES: usize = 500;
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "ts", "m2ts", "webm", "rmvb", "m4v", "mpg", "mpeg",
    "iso", "bdmv",
];

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

#[derive(Debug, Default, Clone)]
struct Hooks {
    list_source: Option<String>,
    list_url: Option<String>,
    smb_root: Option<String>,
}

impl Hooks {
    fn backend(&self) -> SmbBackend {
        match &self.smb_root {
            Some(root) if !root.trim().is_empty() => SmbBackend::new(Some(std::path::PathBuf::from(root.trim()))),
            _ => SmbBackend::default(),
        }
    }
}

fn hooks(site: &Site) -> Hooks {
    let mut out = Hooks::default();
    let text = site.ext_string();
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
            for (key, slot) in [
                ("list_source", &mut out.list_source),
                ("list_url", &mut out.list_url),
                ("smb_root", &mut out.smb_root),
            ] {
                if let Some(v) = map.get(key).and_then(|v| v.as_str()) {
                    if !v.trim().is_empty() {
                        *slot = Some(v.trim().to_string());
                    }
                }
            }
        }
    }
    if out.list_source.is_none() {
        let segment = text.split("$$$").nth(1).unwrap_or("").trim();
        if segment.len() > 7 {
            out.list_source = Some(segment.to_string());
        }
    }
    out
}

fn manifest_text(cfg: &TvConfig, site: &Site, hooks: &Hooks) -> Result<String> {
    if let Some(inline) = &hooks.list_source {
        return Ok(inline.clone());
    }
    if let Some(url) = &hooks.list_url {
        let client = client_for(&cfg.http)?;
        return get_text(&client, url, &Default::default(), 1)
            .map_err(|e| parse_err(site, format!("SMB 清单拉取失败：{e}")));
    }
    Err(parse_err(site, "SMB 分享缺少清单源（ext 需配置 list_source/list_url）"))
}

#[derive(Debug, Clone)]
struct Entry {
    path: String,
    name: String,
}

#[derive(Debug, Default)]
struct Manifest {
    flat: Vec<Entry>,
    groups: BTreeMap<String, Vec<Entry>>,
}

fn normalize_path(input: &str) -> Option<String> {
    SmbPath::parse(input).map(|p| p.share_url())
}

fn push_flat(manifest: &mut Manifest, path: String, name: String) {
    if manifest.flat.len() < MAX_ENTRIES && !manifest.flat.iter().any(|e| e.path == path) {
        manifest.flat.push(Entry { path, name });
    }
}

fn push_group(manifest: &mut Manifest, group: String, path: String, name: String) {
    let bucket = manifest.groups.entry(group).or_default();
    if bucket.len() < MAX_ENTRIES && !bucket.iter().any(|e| e.path == path) {
        bucket.push(Entry { path, name });
    }
}

fn add_entry(manifest: &mut Manifest, raw_path: &str, raw_name: &str) {
    let Some(path) = normalize_path(raw_path) else { return };
    let name = raw_name.trim();
    match name.find('/') {
        Some(idx) => {
            let group = name[..idx].trim().to_string();
            let leaf = name[idx + 1..].replace('/', "|");
            if !group.is_empty() && !leaf.is_empty() {
                push_group(manifest, group, path, leaf);
            } else {
                push_flat(manifest, path, name.to_string());
            }
        }
        None => push_flat(manifest, path, name.to_string()),
    }
}

fn parse_manifest_text(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(path) = parts.next() else { continue };
        let name = parts.next().unwrap_or(path).trim();
        if path.trim().is_empty() {
            continue;
        }
        add_entry(&mut manifest, path.trim(), name);
    }
    manifest
}

fn parse_manifest_json(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return manifest;
    };
    let rows = match &value {
        Value::Array(items) => items.clone(),
        Value::Object(map) => map
            .get("data")
            .or_else(|| map.get("list"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for row in rows {
        let Value::Object(map) = row else { continue };
        let path = ["path", "url", "share", "link", "id"]
            .iter()
            .find_map(|k| map.get(*k).and_then(|v| v.as_str()))
            .unwrap_or("");
        let name = ["name", "title", "type_name"]
            .iter()
            .find_map(|k| map.get(*k).and_then(|v| v.as_str()))
            .unwrap_or(path);
        let group = map.get("group").and_then(|v| v.as_str()).unwrap_or("");
        if path.trim().is_empty() {
            continue;
        }
        if !group.trim().is_empty() {
            add_entry(&mut manifest, path, &format!("{}/{}", group.trim(), name.trim()));
        } else {
            add_entry(&mut manifest, path, name);
        }
    }
    manifest
}

fn parse_manifest(text: &str) -> Manifest {
    if text.trim_start().starts_with('{') || text.trim_start().starts_with('[') {
        let manifest = parse_manifest_json(text);
        if !manifest.flat.is_empty() || !manifest.groups.is_empty() {
            return manifest;
        }
    }
    parse_manifest_text(text)
}

fn load_manifest(cfg: &TvConfig, site: &Site) -> Result<Manifest> {
    let hooks = hooks(site);
    let text = manifest_text(cfg, site, &hooks)?;
    let manifest = parse_manifest(&text);
    if manifest.flat.is_empty() && manifest.groups.is_empty() {
        return Err(parse_err(site, "SMB 清单为空或格式无法识别"));
    }
    Ok(manifest)
}

fn ref_of(path: &str, rel: &str) -> String {
    if rel.is_empty() {
        path.to_string()
    } else {
        format!("{path}###{rel}")
    }
}

fn video_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit_once('.') {
        Some((_, ext)) => VIDEO_EXTS.contains(&ext),
        None => false,
    }
}

fn stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((head, _)) => head.to_string(),
        None => name.to_string(),
    }
}

fn entry_kind_label(entry: &crate::pan::smb::SmbEntry) -> Option<&'static str> {
    if entry.is_dir {
        Some("文件夹")
    } else if video_name(&entry.name) {
        None
    } else {
        None
    }
}

fn path_of(id: &str) -> Option<SmbPath> {
    let head = id.split("###").next().unwrap_or(id);
    SmbPath::parse(head.trim())
}

fn rel_of(id: &str) -> String {
    match id.split_once("###") {
        Some((_, rel)) => rel.trim().to_string(),
        None => String::new(),
    }
}

fn cats_from(manifest: &Manifest) -> Vec<Category> {
    let mut out = Vec::new();
    for entry in &manifest.flat {
        out.push(Category { type_id: entry.path.clone(), type_name: entry.name.clone() });
    }
    for group in manifest.groups.keys() {
        out.push(Category { type_id: group.clone(), type_name: group.clone() });
    }
    out
}

fn browse(backend: &SmbBackend, share_url: &str, snake_rel: &str) -> std::result::Result<Vec<crate::pan::smb::SmbEntry>, String> {
    let mut path = SmbPath::parse(share_url).ok_or_else(|| format!("非法 SMB 路径：{share_url}"))?;
    if !snake_rel.is_empty() {
        path = path.child(snake_rel);
    }
    backend.list(&path)
}

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let manifest = load_manifest(cfg, site)?;
    Ok(cats_from(&manifest))
}

pub fn videos(
    cfg: &TvConfig,
    site: &Site,
    category: &str,
    page: u32,
) -> Result<Vec<VideoItem>> {
    let page = page.max(1);
    let manifest = load_manifest(cfg, site)?;
    let backend = hooks(site).backend();
    if category.is_empty() {
        return Ok(manifest
            .flat
            .iter()
            .map(|e| VideoItem { id: e.path.clone(), name: e.name.clone(), cover: None, remark: None })
            .collect());
    }
    if let Some(entries) = manifest.groups.get(category) {
        let start = (page as usize - 1) * PAGE_SIZE;
        return Ok(entries
            .iter()
            .skip(start)
            .take(PAGE_SIZE)
            .map(|e| VideoItem { id: e.path.clone(), name: e.name.clone(), cover: None, remark: None })
            .collect());
    }
    // 分享目录浏览：id 为分享路径（可带 `###子目录`）。
    let is_known = manifest.flat.iter().any(|e| e.path == category) || category.contains("###");
    if !is_known && SmbPath::parse(category).is_none() {
        return Ok(Vec::new());
    }
    let rel = rel_of(category);
    let head = category.split("###").next().unwrap_or(category).to_string();
    let entries = browse(&backend, &head, &rel).map_err(|e| parse_err(site, e))?;
    Ok(entries
        .into_iter()
        .filter(|e| e.is_dir || video_name(&e.name))
        .map(|e| VideoItem {
            id: ref_of(&head, &join_rel(&rel, &e.name)),
            name: e.name.clone(),
            cover: None,
            remark: entry_kind_label(&e).map(|s| s.to_string()),
        })
        .collect())
}

fn join_rel(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let _ = cfg;
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "SMB 详情 id 为空"));
    }
    let backend = hooks(site).backend();
    let head = id.split("###").next().unwrap_or(id).to_string();
    let rel = rel_of(id);
    let base = SmbPath::parse(&head).ok_or_else(|| parse_err(site, format!("非法 SMB 路径：{id}")))?;
    let target = if rel.is_empty() { base.clone() } else { base.child(&rel) };
    let share_root = SmbPath { dir: String::new(), ..base.clone() };
    let mut episodes = Vec::new();
    let title;
    if backend.is_file(&target) {
        title = stem(target.name());
        episodes.push(Episode { title: title.clone(), url: format!("{}###{}", share_root.share_url(), target.dir.clone()) });
    } else {
        title = if target.dir.is_empty() { base.share.clone() } else { target.name().to_string() };
        let entries = backend.list(&target).map_err(|e| parse_err(site, e))?;
        for entry in entries {
            if entry.is_dir || !video_name(&entry.name) {
                continue;
            }
            if episodes.len() >= MAX_EPISODES {
                break;
            }
            let file_rel = join_rel(&target.dir, &entry.name);
            episodes.push(Episode { title: stem(&entry.name), url: format!("{}###{}", share_root.share_url(), file_rel) });
        }
    }
    if episodes.is_empty() {
        return Err(parse_err(site, format!("SMB 目录下没有可播放的视频：{id}")));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name: title,
        cover: None,
        desc: None,
        year: None,
        area: None,
        director: None,
        actor: None,
        episodes: vec![EpisodeGroup { group: GROUP_LABEL.to_string(), items: episodes }],
    })
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let manifest = load_manifest(cfg, site)?;
    let mut out = Vec::new();
    for entry in manifest.flat.iter().chain(manifest.groups.values().flatten()) {
        if entry.name.to_lowercase().contains(&keyword.to_lowercase())
            || entry.path.to_lowercase().contains(&keyword.to_lowercase())
        {
            out.push(VideoItem { id: entry.path.clone(), name: entry.name.clone(), cover: None, remark: None });
            if out.len() >= MAX_ENTRIES {
                break;
            }
        }
    }
    Ok(out)
}

pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    let _ = cfg;
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "SMB 播放地址为空"));
    }
    let backend = hooks(site).backend();
    let head = id.split("###").next().unwrap_or(id).to_string();
    let rel = rel_of(id);
    let base = SmbPath::parse(&head).ok_or_else(|| parse_err(site, format!("非法 SMB 路径：{id}")))?;
    let target = if rel.is_empty() { base } else { base.child(&rel) };
    let local = backend.local_path(&target);
    if !local.is_file() {
        return Err(parse_err(site, format!("SMB 文件不存在或不可读：{}", local.display())));
    }
    Ok(json!({
        "parse": 0,
        "playUrl": "",
        "url": file_url(&local),
        "header": {},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(ext: &str) -> Site {
        Site { key: "sambashare".into(), name: "SMB分享".into(), api: API.into(), kind: 3, ext: Some(json!(ext)), ..Default::default() }
    }

    #[test]
    fn handles_and_manifest_parse() {
        assert!(handles("csp_SambaShare"));
        assert!(!handles("csp_ThunderShare"));
        let manifest = parse_manifest("host/share 剧集一\nhost/share/sub 分组/子目录\n# comment\n");
        assert_eq!(manifest.flat.len(), 1);
        assert_eq!(manifest.flat[0].path, "host/share");
        assert_eq!(manifest.groups.get("分组").unwrap()[0].name, "子目录");
    }

    #[test]
    fn json_manifest_and_hooks() {
        let m = parse_manifest(r#"[{"path":"host/share","name":"剧集一"},{"path":"host/share/sub","name":"子目录","group":"分组"}]"#);
        assert_eq!(m.flat.len(), 1);
        assert_eq!(m.groups["分组"][0].path, "host/share/sub");
        let h = hooks(&site(r#"{"list_source":"host/share 剧集一","smb_root":"/mnt/smb"}"#));
        assert_eq!(h.list_source.as_deref(), Some("host/share 剧集一"));
        assert_eq!(h.smb_root.as_deref(), Some("/mnt/smb"));
    }

    #[test]
    fn ref_roundtrip_and_paths() {
        assert_eq!(ref_of("h/s", ""), "h/s");
        assert_eq!(ref_of("h/s", "a/b"), "h/s###a/b");
        assert_eq!(rel_of("h/s###a/b"), "a/b");
        let p = SmbPath::parse("user:pw@host/share/sub").unwrap();
        assert_eq!(p.share_url(), "host/share/sub");
    }
}
