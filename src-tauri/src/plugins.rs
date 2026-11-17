//! GLSL 用户着色器 / lavfi 滤镜插件（资源型插件 v1）
//!
//! 设计要点：
//! - 插件只是**资源包**：`<app_data_dir>/plugins/<id>/plugin.json` + 若干 `.glsl`
//!   或一条 lavfi 图；不执行任何脚本、不加载动态库。
//! - 扫描时做严格校验（路径穿越、符号链接逃逸、体积、扩展名、滤镜白名单），
//!   单个坏包只标记错误，不影响其它插件与播放。
//! - 启用状态 / 顺序 / 着色器预设持久化在 `<app_data_dir>/plugins.json`（原子写）。
//! - 合成规则：所有已启用插件的着色器按顺序拼接；内置 Anime4K 也是一
//!   个插件（`vbox.anime4k`，启动时安装到 plugins 目录）。其它插件声明
//!   `stackWithBuiltin=false` 时，会顶掉内置 Anime4K、单独生效。
//!   滤镜经白名单校验后写入 mpv 的 `vf` / `af`。
//! - 任何应用失败都回退「空着色器 + 空滤镜」，并把错误写进 `plugins.json` 的
//!   `lastError` 与运行日志，保证插件坏了不影响正常播放。
//! - 安全模式：设置 `VBOX_DISABLE_PLUGINS=1` 启动时跳过全部插件。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::enhance::{self, BUNDLED_PLUGIN_ID};

/// 插件根目录（位于 app_data_dir 下）。
const ROOT_NAME: &str = "plugins";
/// 持久化文件名。
const STATE_NAME: &str = "plugins.json";
const STATE_VERSION: u32 = 1;

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SHADER_BYTES: u64 = 1024 * 1024;
const MAX_PLUGIN_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SHADERS: usize = 32;
const MAX_FILTER_LEN: usize = 1024;

/// lavfi 滤镜白名单（v1 只允许这些；会读文件/执行外部输入的滤镜一律拒绝）。
const FILTER_WHITELIST: &[&str] = &[
    "eq",
    "hue",
    "unsharp",
    "deband",
    "nlmeans",
    "hqdn3d",
    "scale",
    "crop",
    "format",
    "fps",
    "setpts",
    "atempo",
    "loudnorm",
    "acompressor",
    "equalizer",
    "volume",
    "dynaudnorm",
    "alimiter",
    "aresample",
    "pan",
    "bass",
    "treble",
    "afade",
];

/* ---------------- manifest ---------------- */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginManifest {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    min_app_version: String,
    #[serde(default)]
    contributes: Contributes,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contributes {
    #[serde(default)]
    shaders: Vec<ShaderEntry>,
    #[serde(default)]
    video_filters: Vec<FilterEntry>,
    #[serde(default)]
    audio_filters: Vec<FilterEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShaderEntry {
    id: String,
    #[serde(default)]
    name: String,
    files: Vec<String>,
    #[serde(default)]
    gpu_load: String,
    #[serde(default)]
    stack_with_builtin: bool,
    /// 用途分类：upscale / hdr / sharpen / repair / style（空则由前端按默认归类）
    #[serde(default)]
    category: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterEntry {
    id: String,
    #[serde(default)]
    name: String,
    graph: String,
    #[serde(default)]
    gpu_load: String,
    /// 用途分类：upscale / hdr / sharpen / repair / style（空则由前端按默认归类）
    #[serde(default)]
    category: String,
}

/* ---------------- 持久化状态 ---------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginState {
    #[serde(default = "state_version")]
    version: u32,
    #[serde(default)]
    enabled: Vec<String>,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    shader_preset: HashMap<String, String>,
    #[serde(default)]
    last_error: Option<String>,
}

fn state_version() -> u32 {
    STATE_VERSION
}

impl Default for PluginState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            enabled: Vec::new(),
            order: Vec::new(),
            shader_preset: HashMap::new(),
            last_error: None,
        }
    }
}

impl PluginState {
    fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut state: Self = serde_json::from_str(&text).unwrap_or_default();
        if state.version != STATE_VERSION {
            // 版本升级：只保留仍可解释的字段，未知版本按默认处理。
            state = Self::default();
        }
        state
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let data = serde_json::to_string_pretty(self)
            .map_err(|err| format!("序列化插件状态失败: {err}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)
            .map_err(|err| format!("写入插件状态失败({}): {err}", tmp.display()))?;
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(first) => {
                if std::fs::remove_file(path).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                    return Ok(());
                }
                let _ = std::fs::remove_file(&tmp);
                Err(format!("保存插件状态失败: {first}"))
            }
        }
    }
}

/// 插件存储：目录扫描 + 状态持久化。
pub struct PluginStore {
    root: PathBuf,
    state_path: PathBuf,
    state: Mutex<PluginState>,
    disabled: bool,
}

impl PluginStore {
    pub fn new(data_dir: &Path) -> Self {
        let root = data_dir.join(ROOT_NAME);
        let _ = std::fs::create_dir_all(&root);
        let state_path = data_dir.join(STATE_NAME);
        let state = PluginState::load(&state_path);
        let disabled = std::env::var("VBOX_DISABLE_PLUGINS")
            .map(|value| value == "1")
            .unwrap_or(false);
        Self {
            root,
            state_path,
            state: Mutex::new(state),
            disabled,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PluginState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn save_state(&self) -> Result<(), String> {
        let state = self.state();
        state.save(&self.state_path)
    }

    fn record_error(&self, message: impl Into<String>) {
        let message = message.into();
        if let Ok(mut state) = self.state.lock() {
            state.last_error = Some(message.clone());
            let _ = state.save(&self.state_path);
        }
        crate::logs::push("warn", format!("插件：{message}"));
    }

    fn clear_error(&self) {
        if let Ok(mut state) = self.state.lock() {
            if state.last_error.take().is_some() {
                let _ = state.save(&self.state_path);
            }
        }
    }

    /// 扫描插件目录，返回给前端的概览（错误包也会列出）。
    pub fn overview(&self) -> PluginOverview {
        let state = self.state();
        let mut items = self.scan_all(&state);
        let root = self.root.to_string_lossy().into_owned();
        let last_error = state.last_error.clone();
        drop(state);
        items.sort_by_key(|item| item.order);
        PluginOverview {
            root,
            disabled: self.disabled,
            last_error,
            plugins: items,
        }
    }

    fn scan_all(&self, state: &PluginState) -> Vec<PluginInfo> {
        let mut items = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return items;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let fallback_id = entry.file_name().to_string_lossy().into_owned();
            match self.load_plugin(&path, state) {
                Ok(info) => items.push(info),
                Err(err) => items.push(PluginInfo::invalid(fallback_id, err)),
            }
        }
        // 未在 order 中的插件排在后面，按 id 稳定排序。
        items.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        for (index, item) in items.iter_mut().enumerate() {
            item.order = index;
        }
        items
    }

    fn load_plugin(&self, dir: &Path, state: &PluginState) -> Result<PluginInfo, String> {
        let manifest_path = dir.join("plugin.json");
        let meta = std::fs::metadata(&manifest_path)
            .map_err(|err| format!("读取 plugin.json 失败: {err}"))?;
        if !meta.is_file() {
            return Err("plugin.json 不是普通文件".to_string());
        }
        if meta.len() > MAX_MANIFEST_BYTES {
            return Err(format!("plugin.json 超过 {} KB", MAX_MANIFEST_BYTES / 1024));
        }
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|err| format!("读取 plugin.json 失败（需 UTF-8）: {err}"))?;
        let manifest: PluginManifest =
            serde_json::from_str(&text).map_err(|err| format!("plugin.json 解析失败: {err}"))?;

        let dir_name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        validate_id(&manifest.id)?;
        if manifest.id != dir_name {
            return Err(format!(
                "目录名与 id 不一致（目录 {dir_name}，id {}）",
                manifest.id
            ));
        }
        validate_kind(&manifest.kind)?;

        let canonical_dir = dir
            .canonicalize()
            .map_err(|err| format!("插件目录不可访问: {err}"))?;
        let mut total_bytes = meta.len();
        let mut shaders = Vec::new();
        for (index, entry) in manifest.contributes.shaders.iter().enumerate() {
            if entry.id.trim().is_empty() {
                return Err(format!("第 {} 个着色器缺少 id", index + 1));
            }
            if entry.files.is_empty() {
                return Err(format!("着色器 {} 的 files 为空", entry.id));
            }
            let mut files = Vec::new();
            for rel in &entry.files {
                let full = resolve_inside(&canonical_dir, rel)?;
                let file_meta = std::fs::metadata(&full)
                    .map_err(|err| format!("着色器文件不可读({rel}): {err}"))?;
                if !file_meta.is_file() {
                    return Err(format!("着色器不是普通文件: {rel}"));
                }
                if file_meta.len() > MAX_SHADER_BYTES {
                    return Err(format!("着色器 {rel} 超过 1 MB"));
                }
                total_bytes = total_bytes.saturating_add(file_meta.len());
                files.push(rel.replace('\\', "/"));
            }
            shaders.push(ShaderInfo {
                id: entry.id.clone(),
                name: if entry.name.trim().is_empty() {
                    entry.id.clone()
                } else {
                    entry.name.clone()
                },
                files,
                gpu_load: normalize_load(&entry.gpu_load),
                stack_with_builtin: entry.stack_with_builtin,
                category: normalize_category(&entry.category),
            });
        }
        if shaders.len() > 8 {
            return Err("单个插件最多 8 个着色器预设".to_string());
        }

        let mut video_filters = Vec::new();
        let mut audio_filters = Vec::new();
        for entry in &manifest.contributes.video_filters {
            video_filters.push(validate_filter_entry(entry)?);
        }
        for entry in &manifest.contributes.audio_filters {
            audio_filters.push(validate_filter_entry(entry)?);
        }
        if shaders.is_empty() && video_filters.is_empty() && audio_filters.is_empty() {
            return Err("插件未声明任何着色器或滤镜".to_string());
        }
        if total_bytes > MAX_PLUGIN_BYTES {
            return Err(format!(
                "插件体积超过 {} MB",
                MAX_PLUGIN_BYTES / 1024 / 1024
            ));
        }

        let enabled = state.enabled.iter().any(|id| id == &manifest.id);
        let order = state
            .order
            .iter()
            .position(|id| id == &manifest.id)
            .unwrap_or(usize::MAX);
        let preset = state
            .shader_preset
            .get(&manifest.id)
            .cloned()
            .filter(|preset| shaders.iter().any(|shader| &shader.id == preset))
            .or_else(|| {
                // 内置 Anime4K 首次启用默认走「标准」档；其它插件默认第一个预设。
                if manifest.id == BUNDLED_PLUGIN_ID {
                    shaders
                        .iter()
                        .find(|shader| shader.id == "standard")
                        .or_else(|| shaders.first())
                        .map(|shader| shader.id.clone())
                } else {
                    shaders.first().map(|shader| shader.id.clone())
                }
            });

        Ok(PluginInfo {
            id: manifest.id.clone(),
            name: if manifest.name.trim().is_empty() {
                manifest.id.clone()
            } else {
                manifest.name.clone()
            },
            version: manifest.version.clone(),
            kind: manifest.kind.clone(),
            author: manifest.author.clone(),
            description: manifest.description.clone(),
            homepage: manifest.homepage.clone(),
            bundled: enhance::is_bundled(&manifest.id),
            min_app_version: manifest.min_app_version.clone(),
            enabled,
            order,
            status: if !enabled {
                "disabled".to_string()
            } else {
                "enabled".to_string()
            },
            error: None,
            shaders,
            video_filters,
            audio_filters,
            preset,
        })
    }

    /// 返回已启用且校验通过的插件（按 order 排序）。
    fn enabled_plugins(&self, state: &PluginState) -> Result<Vec<PluginInfo>, String> {
        let mut items = self.scan_all(state);
        items.retain(|item| item.enabled && item.error.is_none());
        Ok(items)
    }

    /// 安装/校正内置 Anime4K 插件包（`plugins/vbox.anime4k/`）。
    /// 安装/校正随包插件（Anime4K + ArtCNN + 锐化/去色带/降噪）。
    pub fn install_bundled(&self) -> Result<bool, String> {
        let anime4k = enhance::install_bundled(&self.root)?;
        let extras = enhance::install_extra_bundled(&self.root)?;
        Ok(anime4k || extras)
    }

    /// 是否已经显式配置过该插件（用于旧版 `player.json` 的一次性迁移）。
    fn has_explicit_config(&self, id: &str) -> bool {
        let state = self.state();
        state.enabled.iter().any(|item| item == id) || state.shader_preset.contains_key(id)
    }
}

/// 安装/校正内置插件；错误写日志，不阻塞启动。
pub fn install_bundled_plugin(app: &AppHandle) -> Result<bool, String> {
    let Some(store) = app.try_state::<PluginStore>() else {
        return Ok(false);
    };
    match store.install_bundled() {
        Ok(changed) => {
            if changed {
                crate::logs::push("info", "内置 Anime4K 插件已安装/更新到插件目录");
            }
            Ok(changed)
        }
        Err(err) => {
            crate::logs::push("error", format!("安装内置 Anime4K 插件失败: {err}"));
            Err(err)
        }
    }
}

/// 旧版把画质档位存在 `player.json`（`enhance`）。升级后若内置插件从未配置过，
/// 只迁移一次：off 保持未配置，其余档位转成「启用 + 预设」。
pub fn migrate_bundled_enhance(app: &AppHandle, legacy_mode: &str) {
    let Some(store) = app.try_state::<PluginStore>() else {
        return;
    };
    if store.has_explicit_config(BUNDLED_PLUGIN_ID) {
        return;
    }
    let mode = crate::player_prefs::normalize_enhance(legacy_mode);
    if mode == "off" {
        return;
    }
    {
        let mut state = store.state();
        state.enabled.retain(|id| id != BUNDLED_PLUGIN_ID);
        state.enabled.push(BUNDLED_PLUGIN_ID.to_string());
        state
            .shader_preset
            .insert(BUNDLED_PLUGIN_ID.to_string(), mode.clone());
        if !state.order.iter().any(|id| id == BUNDLED_PLUGIN_ID) {
            state.order.push(BUNDLED_PLUGIN_ID.to_string());
        }
    }
    if let Err(err) = store.save_state() {
        crate::logs::push("warn", format!("迁移画质档位到插件失败: {err}"));
        return;
    }
    crate::logs::push(
        "info",
        format!("已把旧版画质档位 {mode} 迁移为内置 Anime4K 插件预设"),
    );
}

/// 播放器页面的「画质」四档改为驱动内置插件：off=禁用，其余=启用并选预设。
pub fn set_bundled_enhance(app: &AppHandle, mode: &str) -> Result<(), String> {
    let Some(store) = app.try_state::<PluginStore>() else {
        return Ok(());
    };
    let mode = crate::player_prefs::normalize_enhance(mode);
    {
        let mut state = store.state();
        state.enabled.retain(|id| id != BUNDLED_PLUGIN_ID);
        if mode == "off" {
            state.shader_preset.remove(BUNDLED_PLUGIN_ID);
        } else {
            state.enabled.push(BUNDLED_PLUGIN_ID.to_string());
            state
                .shader_preset
                .insert(BUNDLED_PLUGIN_ID.to_string(), mode);
        }
        if !state.order.iter().any(|id| id == BUNDLED_PLUGIN_ID) {
            state.order.push(BUNDLED_PLUGIN_ID.to_string());
        }
    }
    store.save_state()?;
    apply_if_playing(app)
}

/* ---------------- 前端可序列化的结构 ---------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOverview {
    pub root: String,
    pub disabled: bool,
    pub last_error: Option<String>,
    pub plugins: Vec<PluginInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub author: String,
    pub description: String,
    pub homepage: String,
    /// 随包内置插件（启动时会安装/修复；设置页显示「内置」）
    pub bundled: bool,
    pub min_app_version: String,
    pub enabled: bool,
    pub order: usize,
    pub status: String,
    pub error: Option<String>,
    pub shaders: Vec<ShaderInfo>,
    pub video_filters: Vec<FilterInfo>,
    pub audio_filters: Vec<FilterInfo>,
    pub preset: Option<String>,
}

impl PluginInfo {
    fn invalid(id: String, error: String) -> Self {
        let bundled = enhance::is_bundled(&id);
        let name = if id.trim().is_empty() {
            "未知插件".to_string()
        } else {
            id.clone()
        };
        Self {
            id,
            name,
            version: String::new(),
            kind: String::new(),
            author: String::new(),
            description: String::new(),
            homepage: String::new(),
            bundled,
            min_app_version: String::new(),
            enabled: false,
            order: usize::MAX,
            status: "error".to_string(),
            error: Some(error),
            shaders: Vec::new(),
            video_filters: Vec::new(),
            audio_filters: Vec::new(),
            preset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderInfo {
    pub id: String,
    pub name: String,
    pub files: Vec<String>,
    pub gpu_load: String,
    pub stack_with_builtin: bool,
    pub category: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterInfo {
    pub id: String,
    pub name: String,
    pub graph: String,
    pub gpu_load: String,
    pub category: String,
}

/* ---------------- 校验 ---------------- */

fn validate_id(id: &str) -> Result<(), String> {
    let len = id.chars().count();
    if !(2..=64).contains(&len) {
        return Err("id 长度需为 2–64".to_string());
    }
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return Err("id 首字符必须是字母或数字".to_string()),
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
    {
        return Err("id 只允许小写字母、数字与 . _ -".to_string());
    }
    Ok(())
}

fn validate_kind(kind: &str) -> Result<(), String> {
    match kind.trim() {
        "shader" | "filter" | "bundle" | "" => Ok(()),
        _ => Err(format!(
            "type 只能是 shader / filter / bundle（实际 {kind}）"
        )),
    }
}

/// 用途分类归一化；未知/空值保留为空，由前端按 contributions 兜底。
fn normalize_category(category: &str) -> String {
    match category.trim().to_ascii_lowercase().as_str() {
        "upscale" | "super" | "superres" | "超分" => "upscale",
        "hdr" | "color" | "colour" | "色彩" => "hdr",
        "sharpen" | "sharp" | "锐化" => "sharpen",
        "repair" | "denoise" | "deband" | "修复" | "降噪" | "去色带" => "repair",
        "style" | "grade" | "grading" | "调色" | "风格" => "style",
        _ => "",
    }
    .to_string()
}

fn normalize_load(load: &str) -> String {
    match load.trim().to_ascii_lowercase().as_str() {
        "low" | "medium" | "high" => load.trim().to_ascii_lowercase(),
        _ => "low".to_string(),
    }
}

fn validate_filter_entry(entry: &FilterEntry) -> Result<FilterInfo, String> {
    if entry.id.trim().is_empty() {
        return Err("滤镜缺少 id".to_string());
    }
    validate_filter_graph(&entry.graph)?;
    Ok(FilterInfo {
        id: entry.id.clone(),
        name: if entry.name.trim().is_empty() {
            entry.id.clone()
        } else {
            entry.name.clone()
        },
        graph: entry.graph.trim().to_string(),
        gpu_load: normalize_load(&entry.gpu_load),
        category: normalize_category(&entry.category),
    })
}

/// 校验 lavfi 图：`lavfi=[<filter>=<opts>]`，只允许单个白名单滤镜。
fn validate_filter_graph(graph: &str) -> Result<(), String> {
    let text = graph.trim();
    if text.len() > MAX_FILTER_LEN {
        return Err(format!("滤镜图超过 {MAX_FILTER_LEN} 字节"));
    }
    let inner = text
        .strip_prefix("lavfi=[")
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| "滤镜图必须写成 lavfi=[filter=options]".to_string())?;
    if inner.is_empty() {
        return Err("滤镜图不能为空".to_string());
    }
    if inner.contains([';', '\n', '\r', '[', ']']) {
        return Err("滤镜图不能包含 ;、换行或额外标签".to_string());
    }
    if inner.contains(',') {
        return Err("v1 每个滤镜项只允许一个滤镜（不支持逗号串联）".to_string());
    }
    let name = inner
        .split(['=', ':'])
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !FILTER_WHITELIST.contains(&name.as_str()) {
        return Err(format!("滤镜 {name} 不在白名单内"));
    }
    Ok(())
}

/// 把插件内的相对路径解析为真实路径，并确保没有逃逸出插件目录。
fn resolve_inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().replace('\\', "/");
    if rel.is_empty() || rel.len() > 256 {
        return Err("着色器路径为空或过长".to_string());
    }
    let candidate = Path::new(&rel);
    if candidate.is_absolute() {
        return Err(format!("着色器路径不能是绝对路径: {rel}"));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(format!("着色器路径不能包含 . / .. / 根: {rel}")),
        }
    }
    let ext = candidate
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "glsl" {
        return Err(format!(
            "着色器只允许 .glsl 文件（.hook 是用户着色器片段，不能直接进 glsl-shaders 链）: {rel}"
        ));
    }
    let joined = root.join(candidate);
    let canonical = joined
        .canonicalize()
        .map_err(|err| format!("着色器文件不存在或不可读({rel}): {err}"))?;
    if !canonical.starts_with(root) {
        return Err(format!("着色器路径逃逸插件目录: {rel}"));
    }
    Ok(canonical)
}

/* ---------------- 合成 ---------------- */

/// 把多个插件的着色器链按启用顺序拼接；总数量超过上限直接报错，由调用方回退。
fn compose_shader_chain(chains: &[Vec<String>]) -> Result<Vec<String>, String> {
    let mut chain = Vec::new();
    for files in chains {
        chain.extend(files.iter().cloned());
    }
    if chain.len() > MAX_SHADERS {
        return Err(format!(
            "着色器总数超过上限（{} > {MAX_SHADERS}）",
            chain.len()
        ));
    }
    Ok(chain)
}

/// 取插件当前选中的着色器预设；没有预设或预设文件为空时返回 None。
fn selected_shader(plugin: &PluginInfo) -> Option<&ShaderInfo> {
    let preset = plugin.preset.as_deref()?;
    plugin
        .shaders
        .iter()
        .find(|shader| shader.id == preset && !shader.files.is_empty())
}

/// 选中条目的用途分类（超分 / HDR 等）。用于启用时的同组互斥。
fn selected_category(plugin: &PluginInfo) -> Option<&str> {
    if let Some(shader) = selected_shader(plugin) {
        if !shader.category.is_empty() {
            return Some(shader.category.as_str());
        }
    }
    plugin
        .video_filters
        .first()
        .map(|filter| filter.category.as_str())
        .filter(|category| !category.is_empty())
}

/// 生成 mpv 的 `vf` / `af` 列表：`@vbox_<plugin>_<entry>:lavfi=[...]`。
fn compose_filter_chain(entries: &[(String, String)]) -> String {
    entries
        .iter()
        .map(|(name, graph)| {
            let inner = graph
                .trim()
                .strip_prefix("lavfi=[")
                .and_then(|rest| rest.strip_suffix(']'))
                .unwrap_or(graph.trim());
            format!("@vbox_{name}:lavfi=[{inner}]")
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn safe_label(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/* ---------------- 应用 ---------------- */

/// 供 `player_open` / `player_set_enhance` / 插件命令调用的统一入口。
pub fn apply_render(app: &AppHandle) -> Result<(), String> {
    let Some(store) = app.try_state::<PluginStore>() else {
        return Ok(());
    };

    if store.disabled {
        store.clear_error();
        if !player_ready(app) {
            return Ok(());
        }
        return apply_plan(app, &[], "", "");
    }

    let (plugins, had_error) = {
        let state = store.state();
        (store.enabled_plugins(&state)?, state.last_error.is_some())
    };

    // 第三方插件声明 stackWithBuiltin=false 时，顶掉内置 Anime4K（换一套算法而不是叠加）。
    // 多个「替换型」超分同时启用时只取顺序最靠前的一个，避免两条放大链同时跑。
    let exclusive_id = plugins
        .iter()
        .find(|plugin| {
            plugin.id != BUNDLED_PLUGIN_ID
                && selected_shader(plugin)
                    .map(|shader| !shader.stack_with_builtin)
                    .unwrap_or(false)
        })
        .map(|plugin| plugin.id.clone());
    let replace_builtin = exclusive_id.is_some();

    let mut shader_chains = Vec::new();
    let mut video_filters = Vec::new();
    let mut audio_filters = Vec::new();
    for plugin in &plugins {
        if replace_builtin && plugin.id == BUNDLED_PLUGIN_ID {
            continue;
        }
        let is_exclusive = plugin.id != BUNDLED_PLUGIN_ID
            && selected_shader(plugin)
                .map(|shader| !shader.stack_with_builtin)
                .unwrap_or(false);
        if is_exclusive && exclusive_id.as_deref() != Some(plugin.id.as_str()) {
            continue;
        }
        if let Some(shader) = selected_shader(plugin) {
            let dir = store
                .root
                .join(&plugin.id)
                .canonicalize()
                .map_err(|err| format!("插件目录不可访问（{}）: {err}", plugin.id))?;
            let mut files = Vec::new();
            for rel in &shader.files {
                match resolve_inside(&dir, rel) {
                    Ok(path) => files.push(path.to_string_lossy().into_owned()),
                    Err(err) => {
                        store.record_error(err.clone());
                        fallback(app);
                        return Err(err);
                    }
                }
            }
            shader_chains.push(files);
        }
        for filter in &plugin.video_filters {
            video_filters.push((
                safe_label(&format!("{}_{}", plugin.id, filter.id)),
                filter.graph.clone(),
            ));
        }
        for filter in &plugin.audio_filters {
            audio_filters.push((
                safe_label(&format!("{}_{}", plugin.id, filter.id)),
                filter.graph.clone(),
            ));
        }
    }

    let plan = compose_shader_chain(&shader_chains).map(|shaders| {
        (
            shaders,
            compose_filter_chain(&video_filters),
            compose_filter_chain(&audio_filters),
        )
    });
    match plan {
        Ok((shaders, vf, af)) => match apply_plan(app, &shaders, &vf, &af) {
            Ok(()) => {
                if had_error {
                    store.clear_error();
                }
                Ok(())
            }
            Err(err) => {
                store.record_error(err.clone());
                fallback(app);
                Err(err)
            }
        },
        Err(err) => {
            store.record_error(err.clone());
            fallback(app);
            Err(err)
        }
    }
}

/// 插件失败时回到「空着色器 + 空滤镜」，保证画面正常。
fn fallback(app: &AppHandle) {
    if player_ready(app) {
        let _ = apply_plan(app, &[], "", "");
    }
}

fn apply_plan(
    app: &AppHandle,
    shaders: &[String],
    video_filters: &str,
    audio_filters: &str,
) -> Result<(), String> {
    let player = app
        .state::<crate::player::PlayerHub>()
        .player()
        .ok_or_else(|| "播放器尚未创建".to_string())?;

    // 硬件解码帧（d3d11va/videotoolbox 等）不能直接进 lavfi 的 unsharp/deband 等滤镜，
    // 否则 mpv 会报 "failed to configure the filter graph" 并禁用滤镜。
    // 有视频滤镜时临时切软解，关掉后恢复用户在设置里选的 hwdec。
    let wanted_hwdec = if video_filters.is_empty() {
        app.try_state::<crate::player_prefs::PlayerPrefsStore>()
            .map(|store| store.load().hwdec)
            .unwrap_or_else(|| "auto-safe".to_string())
    } else {
        "no".to_string()
    };
    let current_hwdec = player.property_string("hwdec").unwrap_or_default();
    if current_hwdec != wanted_hwdec {
        match player.set_hwdec(&wanted_hwdec) {
            Ok(()) => crate::logs::push(
                "info",
                format!("视频滤镜状态变化：hwdec 切换为 {wanted_hwdec}"),
            ),
            Err(err) => crate::logs::push(
                "warn",
                format!("视频滤镜状态变化：切换 hwdec 为 {wanted_hwdec} 失败：{err}"),
            ),
        }
    }

    player
        .set_glsl_shaders(shaders)
        .map_err(|err| format!("应用着色器失败: {err}"))?;
    player
        .set_video_filters(video_filters)
        .map_err(|err| format!("应用视频滤镜失败: {err}"))?;
    player
        .set_audio_filters(audio_filters)
        .map_err(|err| format!("应用音频滤镜失败: {err}"))?;
    Ok(())
}

/// 播放器未创建时返回 true（设置页可以先改状态，下次起播生效）。
fn player_ready(app: &AppHandle) -> bool {
    app.try_state::<crate::player::PlayerHub>()
        .map(|hub| hub.player().is_some())
        .unwrap_or(false)
}

fn apply_if_playing(app: &AppHandle) -> Result<(), String> {
    if player_ready(app) {
        apply_render(app)
    } else {
        Ok(())
    }
}

/* ---------------- Tauri 命令 ---------------- */

#[tauri::command]
pub fn plugin_overview(store: State<'_, PluginStore>) -> PluginOverview {
    store.overview()
}

#[tauri::command]
pub fn plugin_rescan(app: AppHandle, store: State<'_, PluginStore>) -> PluginOverview {
    // 重新扫描前先校正内置插件（用户可能删掉了 plugins/vbox.anime4k）。
    if let Err(err) = store.install_bundled() {
        store.record_error(err);
    }
    if let Err(err) = apply_if_playing(&app) {
        store.record_error(err);
    }
    store.overview()
}

#[tauri::command]
pub fn plugin_set_enabled(
    app: AppHandle,
    store: State<'_, PluginStore>,
    id: String,
    enabled: bool,
) -> Result<PluginOverview, String> {
    // 超分 / HDR 同一时间只保留一个：启用目标前先停用同用途的其它插件。
    let exclusive: Vec<String> = if enabled {
        let plugins = store.overview().plugins;
        let target = plugins.iter().find(|plugin| plugin.id == id);
        match target.and_then(selected_category) {
            Some("upscale") | Some("hdr") => {
                let category = target
                    .and_then(selected_category)
                    .map(|value| value.to_string());
                plugins
                    .iter()
                    .filter(|plugin| {
                        plugin.enabled
                            && plugin.id != id
                            && selected_category(plugin) == category.as_deref()
                    })
                    .map(|plugin| plugin.id.clone())
                    .collect()
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    {
        let mut state = store.state();
        for other in exclusive {
            state.enabled.retain(|item| item != &other);
        }
        state.enabled.retain(|item| item != &id);
        if enabled {
            state.enabled.push(id.clone());
            // 内置 Anime4K 首次启用默认「标准」档；用户之后可在设置/播放器里改。
            if id == BUNDLED_PLUGIN_ID && !state.shader_preset.contains_key(&id) {
                state
                    .shader_preset
                    .insert(id.clone(), "standard".to_string());
            }
        }
        if !state.order.iter().any(|item| item == &id) {
            state.order.push(id.clone());
        }
    }
    store.save_state()?;
    if let Err(err) = apply_if_playing(&app) {
        store.record_error(err);
    }
    Ok(store.overview())
}

#[tauri::command]
pub fn plugin_set_order(
    app: AppHandle,
    store: State<'_, PluginStore>,
    order: Vec<String>,
) -> Result<PluginOverview, String> {
    {
        let mut state = store.state();
        let mut next = Vec::new();
        for id in order {
            if !next.contains(&id) {
                next.push(id);
            }
        }
        // 未出现在新顺序里的插件保持原有相对顺序，排在最后。
        for id in state.order.clone() {
            if !next.contains(&id) {
                next.push(id);
            }
        }
        state.order = next;
    }
    store.save_state()?;
    if let Err(err) = apply_if_playing(&app) {
        store.record_error(err);
    }
    Ok(store.overview())
}

#[tauri::command]
pub fn plugin_set_preset(
    app: AppHandle,
    store: State<'_, PluginStore>,
    id: String,
    preset: Option<String>,
) -> Result<PluginOverview, String> {
    {
        let mut state = store.state();
        match preset.filter(|value| !value.trim().is_empty()) {
            Some(preset) => {
                state.shader_preset.insert(id, preset);
            }
            None => {
                state.shader_preset.remove(&id);
            }
        }
    }
    store.save_state()?;
    if let Err(err) = apply_if_playing(&app) {
        store.record_error(err);
    }
    Ok(store.overview())
}

#[tauri::command]
pub fn plugin_open_dir(app: AppHandle, store: State<'_, PluginStore>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    std::fs::create_dir_all(&store.root).map_err(|err| format!("创建插件目录失败: {err}"))?;
    app.opener()
        .open_path(store.root.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|err| format!("打开插件目录失败: {err}"))
}

/// 在系统浏览器打开插件主页（只允许 http/https）。
#[tauri::command]
pub fn plugin_open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let url = url.trim().to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("只允许打开 http/https 链接".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| format!("打开插件主页失败: {err}"))
}

/* ---------------- 测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("vbox_plugins_{tag}_{stamp}"));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn id_rules_reject_bad_values() {
        for good in ["ab", "com.example.film-look", "a_b-1"] {
            assert!(validate_id(good).is_ok(), "{good}");
        }
        for bad in ["a", "Abc", "../x", "a b", "", "a/b"] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn filter_graph_requires_whitelist_and_wrapper() {
        assert!(validate_filter_graph("lavfi=[unsharp=5:5:0.8]").is_ok());
        assert!(validate_filter_graph("lavfi=[eq=contrast=1.1]").is_ok());
        assert!(validate_filter_graph("unsharp=5:5:0.8").is_err());
        assert!(validate_filter_graph("lavfi=[movie=/etc/passwd]").is_err());
        assert!(validate_filter_graph("lavfi=[eq=a=1;eq=b=2]").is_err());
        assert!(validate_filter_graph("lavfi=[eq=a=1,unsharp=5:5:1]").is_err());
        assert!(validate_filter_graph("lavfi=[subtitles=x]").is_err());
    }

    #[test]
    fn shader_paths_cannot_escape() {
        let dir = temp_dir("path");
        std::fs::create_dir_all(dir.join("shaders")).expect("dir");
        std::fs::write(dir.join("shaders/ok.glsl"), "//!HOOK MAIN\n").expect("write");
        std::fs::write(dir.join("evil.glsl"), "//!HOOK MAIN\n").expect("write");

        let root = dir.canonicalize().expect("canonical");
        assert!(resolve_inside(&root, "shaders/ok.glsl").is_ok());
        assert!(resolve_inside(&root, "../evil.glsl").is_err());
        assert!(resolve_inside(&root, "/etc/passwd").is_err());
        assert!(resolve_inside(&root, "evil.txt").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shader_chain_concatenates_and_limits() {
        let chain = compose_shader_chain(&[
            vec!["builtin.glsl".to_string()],
            vec!["a.glsl".to_string(), "b.glsl".to_string()],
        ])
        .expect("compose");
        assert_eq!(chain, vec!["builtin.glsl", "a.glsl", "b.glsl"]);

        let too_many = vec![
            (0..=MAX_SHADERS)
                .map(|i| format!("{i}.glsl"))
                .collect::<Vec<_>>(),
        ];
        assert!(compose_shader_chain(&too_many).is_err());
    }

    #[test]
    fn bundled_anime4k_is_listed_and_defaults_to_standard() {
        let dir = temp_dir("bundled");
        let store = PluginStore::new(&dir);
        store.install_bundled().expect("install");
        let overview = store.overview();
        let bundled = overview
            .plugins
            .iter()
            .find(|plugin| plugin.id == BUNDLED_PLUGIN_ID)
            .expect("内置 Anime4K 应出现在插件列表");
        assert_eq!(bundled.kind, "bundle");
        assert_eq!(bundled.shaders.len(), 3);
        assert_eq!(bundled.preset.as_deref(), Some("standard"));
        assert!(!bundled.enabled);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn filter_chain_uses_labels() {
        let chain = compose_filter_chain(&[
            ("p1".to_string(), "lavfi=[unsharp=5:5:0.8]".to_string()),
            ("p2".to_string(), "lavfi=[eq=contrast=1.1]".to_string()),
        ]);
        assert_eq!(
            chain,
            "@vbox_p1:lavfi=[unsharp=5:5:0.8],@vbox_p2:lavfi=[eq=contrast=1.1]"
        );
    }

    #[test]
    fn state_roundtrip_and_corruption_fallback() {
        let dir = temp_dir("state");
        let path = dir.join("plugins.json");
        let state = PluginState {
            enabled: vec!["a".to_string()],
            order: vec!["a".to_string()],
            ..PluginState::default()
        };
        state.save(&path).expect("save");
        let loaded = PluginState::load(&path);
        assert_eq!(loaded.enabled, vec!["a"]);
        std::fs::write(&path, "{ broken").expect("write");
        let fallback = PluginState::load(&path);
        assert!(fallback.enabled.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn overview_lists_invalid_packages_as_errors() {
        let dir = temp_dir("overview");
        let plugin = dir.join("plugins/bad-plugin");
        std::fs::create_dir_all(&plugin).expect("dir");
        std::fs::write(plugin.join("plugin.json"), "{ \"id\": \"bad-plugin\" }").expect("write");
        let store = PluginStore::new(&dir);
        let overview = store.overview();
        assert_eq!(overview.plugins.len(), 1);
        assert_eq!(overview.plugins[0].status, "error");
        assert!(overview.plugins[0].error.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
