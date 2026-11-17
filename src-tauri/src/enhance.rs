//! 画质增强：Anime4K 超分着色器（内置插件包）
//!
//! 设计要点：
//! - mpv 没有插件 ABI，画质增强走 **mpv 用户着色器**（`glsl-shaders`）；需要
//!   `vo=gpu` / `vo=gpu-next`，本项目的 libmpv 默认就是 gpu 系列；
//! - 着色器文件以 `include_str!` 内嵌进二进制（约 190 KB），启动时作为**内置插件**
//!   安装到 `<app_data_dir>/plugins/vbox.anime4k/`，与用户插件走同一套扫描/校验/
//!   启用/预设逻辑；
//! - 三档预设（S/M/L 规模），全部来自 Anime4K v4.0.1（MIT，见
//!   `resources/shaders/anime4k/LICENSE-Anime4K.txt`）：
//!   - `lite`：Clamp_Highlights + Upscale_CNN_x2_S（集显/低负载）
//!   - `standard`：Clamp_Highlights + Restore_CNN_S + Upscale_CNN_x2_M
//!   - `strong`：Clamp_Highlights + Restore_CNN_M + Upscale_CNN_x2_L（独显）
//! - 切换是通过覆盖 `glsl-shaders`（path list）整列表完成：先关（空串）再设新列表，
//!   避免不同档位着色器叠加。
//! - 内置插件由应用管理：每次启动按内嵌内容校正 manifest 与着色器文件；
//!   用户的启用状态、预设、顺序仍保存在 `plugins.json`，不会被覆盖。

use std::path::Path;

/// 内置 Anime4K 插件 id（小写、含点，符合插件 id 规则）。
pub const BUNDLED_PLUGIN_ID: &str = "vbox.anime4k";
pub const BUNDLED_PLUGIN_HOMEPAGE: &str = "https://github.com/bloc97/Anime4K";
/// 随包着色器版本（对应 Anime4K v4.0.1）。
pub const BUNDLED_PLUGIN_VERSION: &str = "4.0.1";

/// 内置 ArtCNN 插件 id（通用 2x 放大，MIT）。
pub const BUNDLED_ARTCNN_ID: &str = "vbox.artcnn";
const ARTCNN_HOMEPAGE: &str = "https://github.com/Artoriuz/ArtCNN";
const ARTCNN_SHADER: &str = include_str!("../resources/shaders/artcnn/ArtCNN_C4F16.glsl");

/// 内置 FSRCNNX 插件 id（2x CNN 超分；文件头为 LGPL-3.0，随文件保留完整许可证）。
const FSRCNNX_ID: &str = "vbox.fsrcnnx";
const FSRCNNX_HOMEPAGE: &str = "https://github.com/Chinna95P/mpv-anime-build";
const FSRCNNX_FAST: &str = include_str!("../resources/shaders/fsrcnnx/FSRCNNX_x2_8-0-4-1.glsl");
const FSRCNNX_QUALITY: &str = include_str!("../resources/shaders/fsrcnnx/FSRCNNX_x2_16-0-4-1.glsl");

/// 内置 CAS 插件 id（AMD 对比自适应锐化，MIT）。
const CAS_ID: &str = "vbox.cas";
const CAS_HOMEPAGE: &str = "https://gpuopen.com/fidelityfx-cas/";
const CAS_SHADER: &str = include_str!("../resources/shaders/cas/CAS.glsl");

/// 内置调色 / 风格类着色器（自研，MIT）。
const STYLE_FILM_LOOK: &str = include_str!("../resources/shaders/style/film-look.glsl");
const STYLE_FILM_GRAIN: &str = include_str!("../resources/shaders/style/film-grain.glsl");
const STYLE_CRT: &str = include_str!("../resources/shaders/style/crt-lines.glsl");

/// 单个内嵌着色器（文件名 + 源码）。
struct ShaderFile {
    name: &'static str,
    content: &'static str,
}

/// 内嵌着色器集合：只放实际用到的几个文件，控制二进制体积。
const SHADER_FILES: &[ShaderFile] = &[
    ShaderFile {
        name: "Anime4K_Clamp_Highlights.glsl",
        content: include_str!("../resources/shaders/anime4k/Anime4K_Clamp_Highlights.glsl"),
    },
    ShaderFile {
        name: "Anime4K_Restore_CNN_S.glsl",
        content: include_str!("../resources/shaders/anime4k/Anime4K_Restore_CNN_S.glsl"),
    },
    ShaderFile {
        name: "Anime4K_Restore_CNN_M.glsl",
        content: include_str!("../resources/shaders/anime4k/Anime4K_Restore_CNN_M.glsl"),
    },
    ShaderFile {
        name: "Anime4K_Upscale_CNN_x2_S.glsl",
        content: include_str!("../resources/shaders/anime4k/Anime4K_Upscale_CNN_x2_S.glsl"),
    },
    ShaderFile {
        name: "Anime4K_Upscale_CNN_x2_M.glsl",
        content: include_str!("../resources/shaders/anime4k/Anime4K_Upscale_CNN_x2_M.glsl"),
    },
    ShaderFile {
        name: "Anime4K_Upscale_CNN_x2_L.glsl",
        content: include_str!("../resources/shaders/anime4k/Anime4K_Upscale_CNN_x2_L.glsl"),
    },
];

/// 轻量档：只做钳制高光 + S 档 2x 超分。
const LITE_CHAIN: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Upscale_CNN_x2_S.glsl",
];
/// 标准档：S 档线条重建 + M 档 2x 超分。
const STANDARD_CHAIN: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_S.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
/// 强力档：M 档线条重建 + L 档 2x 超分（GPU 负载最高）。
const STRONG_CHAIN: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_M.glsl",
    "Anime4K_Upscale_CNN_x2_L.glsl",
];

/// 模式 → 着色器文件顺序（`off` / 未知模式返回空切片）。
fn mode_chain(mode: &str) -> &'static [&'static str] {
    match mode {
        "lite" => LITE_CHAIN,
        "standard" => STANDARD_CHAIN,
        "strong" => STRONG_CHAIN,
        _ => &[],
    }
}

/// 预设展示名（写进内置 manifest）。
fn preset_name(mode: &str) -> &'static str {
    match mode {
        "lite" => "轻量",
        "standard" => "标准",
        "strong" => "强力",
        _ => "",
    }
}

/// 预设 GPU 负载（写进内置 manifest）。
fn preset_load(mode: &str) -> &'static str {
    match mode {
        "lite" => "low",
        "standard" => "medium",
        "strong" => "high",
        _ => "low",
    }
}

/// 是否为随包内置插件（设置页显示「内置」标记；启动时会被安装/修复覆盖）。
pub fn is_bundled(id: &str) -> bool {
    matches!(
        id,
        BUNDLED_PLUGIN_ID
            | BUNDLED_ARTCNN_ID
            | FSRCNNX_ID
            | CAS_ID
            | "vbox.film-look"
            | "vbox.film-grain"
            | "vbox.crt"
            | "vbox.sharpen"
            | "vbox.deband"
            | "vbox.denoise"
    )
}

/// 把内置 Anime4K 作为插件安装到 `<app_data_dir>/plugins/vbox.anime4k/`。
///
/// - 目录不存在则创建；manifest 与着色器缺失/内容不一致时重写；
/// - 只校正应用自带文件，不碰 `plugins.json` 里的启用状态与预设；
/// - 返回 `changed` 表示本次是否写入了文件（便于日志）。
pub fn install_bundled(plugins_root: &Path) -> Result<bool, String> {
    let dir = plugins_root.join(BUNDLED_PLUGIN_ID);
    let shader_dir = dir.join("shaders");
    std::fs::create_dir_all(&shader_dir)
        .map_err(|err| format!("创建内置插件目录失败({}): {err}", shader_dir.display()))?;

    let mut changed = false;
    for file in SHADER_FILES {
        let path = shader_dir.join(file.name);
        let needs_write = match std::fs::read_to_string(&path) {
            Ok(existing) => existing != file.content,
            Err(_) => true,
        };
        if needs_write {
            std::fs::write(&path, file.content)
                .map_err(|err| format!("写入内置着色器失败({}): {err}", path.display()))?;
            changed = true;
        }
    }

    let manifest = bundled_manifest();
    let manifest_path = dir.join("plugin.json");
    let needs_manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(existing) => existing != manifest,
        Err(_) => true,
    };
    if needs_manifest {
        std::fs::write(&manifest_path, &manifest)
            .map_err(|err| format!("写入内置插件清单失败({}): {err}", manifest_path.display()))?;
        changed = true;
    }

    Ok(changed)
}

/// 写入一个随包插件：manifest 与文件按内容差异落盘，返回是否发生写入。
fn write_extra_plugin(
    plugins_root: &Path,
    id: &str,
    manifest: &serde_json::Value,
    files: &[(&str, &str)],
) -> Result<bool, String> {
    let dir = plugins_root.join(id);
    let mut changed = false;
    for (rel, content) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("创建内置插件目录失败({}): {err}", parent.display()))?;
        }
        let needs_write = match std::fs::read_to_string(&path) {
            Ok(existing) => existing != *content,
            Err(_) => true,
        };
        if needs_write {
            std::fs::write(&path, content)
                .map_err(|err| format!("写入内置插件文件失败({}): {err}", path.display()))?;
            changed = true;
        }
    }

    let mut text = serde_json::to_string_pretty(manifest)
        .map_err(|err| format!("序列化内置插件清单失败({id}): {err}"))?;
    text.push('\n');
    let manifest_path = dir.join("plugin.json");
    let needs_manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(existing) => existing != text,
        Err(_) => true,
    };
    if needs_manifest {
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("创建内置插件目录失败({}): {err}", dir.display()))?;
        std::fs::write(&manifest_path, &text)
            .map_err(|err| format!("写入内置插件清单失败({}): {err}", manifest_path.display()))?;
        changed = true;
    }
    Ok(changed)
}

/// 安装其它随包插件：ArtCNN 超分 + 锐化/去色带/降噪三个 lavfi 滤镜。
///
/// 这些包和用户插件完全同构；内置只负责落盘与修复，不覆盖启用状态。
pub fn install_extra_bundled(plugins_root: &Path) -> Result<bool, String> {
    let mut changed = false;

    let artcnn = serde_json::json!({
        "id": BUNDLED_ARTCNN_ID,
        "name": "ArtCNN",
        "version": "1.0.0",
        "type": "shader",
        "author": "Joao Chrisostomo / Kacper Michajłow / VBox",
        "homepage": ARTCNN_HOMEPAGE,
        "description": "ArtCNN C4F16 通用 2x 放大（MIT），适合真人/动漫混合内容；启用后会替换内置 Anime4K。",
        "minAppVersion": "0.1.3",
        "contributes": {
            "shaders": [{
                "id": "artcnn",
                "name": "C4F16 2x",
                "files": ["shaders/ArtCNN_C4F16.glsl"],
                "gpuLoad": "medium",
                "stackWithBuiltin": false,
                "category": "upscale"
            }]
        }
    });
    changed |= write_extra_plugin(
        plugins_root,
        BUNDLED_ARTCNN_ID,
        &artcnn,
        &[("shaders/ArtCNN_C4F16.glsl", ARTCNN_SHADER)],
    )?;

    let fsrcnnx = serde_json::json!({
        "id": FSRCNNX_ID,
        "name": "FSRCNNX",
        "version": "1.0.0",
        "type": "shader",
        "author": "igv / VBox",
        "homepage": FSRCNNX_HOMEPAGE,
        "description": "FSRCNNX 2x CNN 超分（LGPL-3.0），偏写实画面；启用后会替换内置 Anime4K。",
        "minAppVersion": "0.1.3",
        "contributes": {
            "shaders": [
                {
                    "id": "fast",
                    "name": "8-0-4-1（快）",
                    "files": ["shaders/FSRCNNX_x2_8-0-4-1.glsl"],
                    "gpuLoad": "medium",
                    "stackWithBuiltin": false,
                    "category": "upscale"
                },
                {
                    "id": "quality",
                    "name": "16-0-4-1（质量）",
                    "files": ["shaders/FSRCNNX_x2_16-0-4-1.glsl"],
                    "gpuLoad": "high",
                    "stackWithBuiltin": false,
                    "category": "upscale"
                }
            ]
        }
    });
    changed |= write_extra_plugin(
        plugins_root,
        FSRCNNX_ID,
        &fsrcnnx,
        &[
            ("shaders/FSRCNNX_x2_8-0-4-1.glsl", FSRCNNX_FAST),
            ("shaders/FSRCNNX_x2_16-0-4-1.glsl", FSRCNNX_QUALITY),
            (
                "LICENSE.txt",
                "FSRCNNX shaders: Copyright (C) 2017-2021 igv, LGPL-3.0-or-later.\n\
                 The full license notice is kept in the header of each .glsl file.\n",
            ),
        ],
    )?;

    let cas = serde_json::json!({
        "id": CAS_ID,
        "name": "CAS 锐化",
        "version": "1.0.0",
        "type": "shader",
        "author": "AMD FidelityFX / VBox",
        "homepage": CAS_HOMEPAGE,
        "description": "对比自适应锐化（MIT），不做分辨率放大，开销低；可与 Anime4K / ArtCNN 叠加。",
        "minAppVersion": "0.1.3",
        "contributes": {
            "shaders": [{
                "id": "cas",
                "name": "CAS",
                "files": ["shaders/CAS.glsl"],
                "gpuLoad": "low",
                "stackWithBuiltin": true,
                "category": "sharpen"
            }]
        }
    });
    changed |= write_extra_plugin(
        plugins_root,
        CAS_ID,
        &cas,
        &[("shaders/CAS.glsl", CAS_SHADER)],
    )?;

    let styles = [
        (
            "vbox.film-look",
            "胶片质感",
            "轻微提高对比与暖色，模拟胶片观感；可与超分、锐化叠加。",
            "film-look.glsl",
            STYLE_FILM_LOOK,
        ),
        (
            "vbox.film-grain",
            "胶片颗粒",
            "叠加细颗粒噪点，适合老片与胶片风格；建议低强度使用。",
            "film-grain.glsl",
            STYLE_FILM_GRAIN,
        ),
        (
            "vbox.crt",
            "CRT 扫描线",
            "轻微扫描线与荧光感，复古 CRT 风格；高分辨率下效果较细。",
            "crt-lines.glsl",
            STYLE_CRT,
        ),
    ];
    for (id, name, description, file_name, content) in styles {
        let manifest = serde_json::json!({
            "id": id,
            "name": name,
            "version": "1.0.0",
            "type": "shader",
            "author": "VBox",
            "description": description,
            "minAppVersion": "0.1.3",
            "contributes": {
                "shaders": [{
                    "id": "style",
                    "name": name,
                    "files": [format!("shaders/{file_name}")],
                    "gpuLoad": "low",
                    "stackWithBuiltin": true,
                    "category": "style"
                }]
            }
        });
        let rel = format!("shaders/{file_name}");
        changed |= write_extra_plugin(plugins_root, id, &manifest, &[(rel.as_str(), content)])?;
    }

    // 每个滤镜插件只声明一个滤镜：当前合成逻辑会应用插件的全部滤镜，
    // 这样用户启用即得到一个明确效果，也不会出现「预设选了但没生效」。
    let filters = [
        (
            "vbox.sharpen",
            "锐化",
            "unsharp",
            "轻量锐化，改善压缩后的发软/发糊画面。",
            "https://ffmpeg.org/ffmpeg-filters.html#unsharp",
            "lavfi=[unsharp=5:5:0.6:3:3:0.0]",
        ),
        (
            "vbox.deband",
            "去色带",
            "deband",
            "抑制暗部渐变中的色带，适合低码率动画/实拍。",
            "https://ffmpeg.org/ffmpeg-filters.html#deband",
            "lavfi=[deband=1thr=0.02:2thr=0.02:3thr=0.02:4thr=0.02]",
        ),
        (
            "vbox.denoise",
            "降噪",
            "denoise",
            "hqdn3d 时域/空域降噪，适合老片源与高 ISO 素材。",
            "https://ffmpeg.org/ffmpeg-filters.html#hqdn3d",
            "lavfi=[hqdn3d=4:3:6:4.5]",
        ),
    ];
    for (id, name, filter_id, description, homepage, graph) in filters {
        let manifest = serde_json::json!({
            "id": id,
            "name": name,
            "version": "1.0.0",
            "type": "filter",
            "author": "FFmpeg / VBox",
            "homepage": homepage,
            "description": description,
            "minAppVersion": "0.1.3",
            "contributes": {
                "videoFilters": [{
                    "id": filter_id,
                    "name": name,
                    "graph": graph,
                    "gpuLoad": "low",
                    "category": if filter_id == "unsharp" { "sharpen" } else { "repair" }
                }]
            }
        });
        changed |= write_extra_plugin(plugins_root, id, &manifest, &[])?;
    }

    Ok(changed)
}

/// 内置插件清单（camelCase，与 `PluginManifest` 反序列化字段一致）。
fn bundled_manifest() -> String {
    let shaders: Vec<serde_json::Value> = ["lite", "standard", "strong"]
        .iter()
        .map(|mode| {
            let files: Vec<String> = mode_chain(mode)
                .iter()
                .map(|name| format!("shaders/{name}"))
                .collect();
            serde_json::json!({
                "id": mode,
                "name": preset_name(mode),
                "files": files,
                "gpuLoad": preset_load(mode),
                "stackWithBuiltin": true,
                "category": "upscale",
            })
        })
        .collect();

    let manifest = serde_json::json!({
        "id": BUNDLED_PLUGIN_ID,
        "name": "Anime4K",
        "version": BUNDLED_PLUGIN_VERSION,
        "type": "bundle",
        "author": "bloc97 / VBox",
        "homepage": BUNDLED_PLUGIN_HOMEPAGE,
        "description": "内置的 Anime4K v4.0.1 动漫超分着色器，提供轻量/标准/强力三档预设。",
        "minAppVersion": "0.1.3",
        "contributes": {
            "shaders": shaders
        }
    });
    let mut text = serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个模式引用的文件都必须在内嵌集合里（防止改名后运行时才报错）。
    #[test]
    fn mode_chains_reference_embedded_files() {
        for mode in ["lite", "standard", "strong"] {
            let chain = mode_chain(mode);
            assert!(!chain.is_empty(), "{mode} 应有着色器链");
            for name in chain {
                assert!(
                    SHADER_FILES.iter().any(|file| file.name == *name),
                    "{mode} 引用了未内嵌的着色器 {name}"
                );
            }
        }
        assert!(mode_chain("off").is_empty());
        assert!(mode_chain("unknown").is_empty());
    }

    /// 内嵌着色器都带 mpv 用户着色器元数据（`//!HOOK`）。
    #[test]
    fn embedded_shaders_are_mpv_user_shaders() {
        for file in SHADER_FILES {
            assert!(
                file.content.contains("//!HOOK"),
                "{} 缺少 mpv 用户着色器元数据",
                file.name
            );
        }
    }

    /// 安装：首轮写入，二轮幂等；着色器被改坏后能恢复。
    #[test]
    fn bundled_plugin_installs_and_repairs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("vbox_bundled_{stamp}"));
        std::fs::create_dir_all(&root).expect("root");

        assert!(install_bundled(&root).expect("install"));
        let dir = root.join(BUNDLED_PLUGIN_ID);
        assert!(dir.join("plugin.json").is_file());
        assert!(dir.join("shaders/Anime4K_Clamp_Highlights.glsl").is_file());
        assert!(!install_bundled(&root).expect("idempotent"));

        std::fs::write(dir.join("shaders/Anime4K_Clamp_Highlights.glsl"), "broken")
            .expect("corrupt");
        assert!(install_bundled(&root).expect("repair"));
        let restored = std::fs::read_to_string(dir.join("shaders/Anime4K_Clamp_Highlights.glsl"))
            .expect("read");
        assert!(restored.contains("//!HOOK"));

        let _ = std::fs::remove_dir_all(root);
    }

    /// 其它随包插件都必须落盘 manifest + homepage，且重复安装是幂等的。
    #[test]
    fn extra_bundled_plugins_are_installed() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("vbox_extra_{stamp}"));
        std::fs::create_dir_all(&root).expect("root");

        assert!(install_extra_bundled(&root).expect("install"));
        for id in [
            BUNDLED_ARTCNN_ID,
            FSRCNNX_ID,
            CAS_ID,
            "vbox.sharpen",
            "vbox.deband",
            "vbox.denoise",
            "vbox.film-look",
            "vbox.film-grain",
            "vbox.crt",
        ] {
            let text = std::fs::read_to_string(root.join(id).join("plugin.json"))
                .unwrap_or_else(|err| panic!("{id} manifest: {err}"));
            assert!(
                text.contains(&format!("\"id\": \"{id}\"")),
                "{id} manifest 内容异常"
            );
        }
        for id in [BUNDLED_ARTCNN_ID, FSRCNNX_ID, CAS_ID] {
            let text = std::fs::read_to_string(root.join(id).join("plugin.json"))
                .unwrap_or_else(|err| panic!("{id} manifest: {err}"));
            assert!(text.contains("\"homepage\""), "{id} 缺少 homepage");
        }
        assert!(
            root.join(BUNDLED_ARTCNN_ID)
                .join("shaders/ArtCNN_C4F16.glsl")
                .is_file()
        );
        assert!(
            root.join(FSRCNNX_ID)
                .join("shaders/FSRCNNX_x2_8-0-4-1.glsl")
                .is_file()
        );
        assert!(root.join(CAS_ID).join("shaders/CAS.glsl").is_file());
        assert!(
            root.join("vbox.film-look")
                .join("shaders/film-look.glsl")
                .is_file()
        );
        assert!(!install_extra_bundled(&root).expect("idempotent"));

        let _ = std::fs::remove_dir_all(root);
    }
}
