//! 在线字幕：搜索 + 下载到本地缓存
//!
//! - 网络与站点解析都在 `vbox_core::subtitle_online`（纯 Rust、可单测）；
//! - 这里只做 Tauri 命令包装：阻塞请求放 `spawn_blocking`（Tauri 2 的同步命令跑在
//!   主线程，直接联网会卡住 UI），下载后落盘到「下载目录/cache/subtitles」；
//! - 站点返回 zip 字幕包时就地解出第一条字幕文件；RAR 目前明确报错（不做半成品解包）。

use std::io::Read;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use vbox_core::subtitle_online::{self, OnlineSubtitle, SubtitleSourceInfo};

/// 可识别的字幕扩展名（也是 zip 包内挑选目标文件的依据）
const SUBTITLE_EXTS: [&str; 11] = [
    "srt", "ass", "ssa", "vtt", "sub", "sup", "smi", "lrc", "ttml", "dfxp", "idx",
];

/// 单个字幕文件最大字节数（防止站点返回异常大文件）
const MAX_SUBTITLE_BYTES: u64 = 32 * 1024 * 1024;

/// 当前可用的在线字幕源。
#[tauri::command]
pub async fn subtitle_sources() -> Vec<SubtitleSourceInfo> {
    subtitle_online::sources()
}

/// 搜索在线字幕（`page` 缺省为 1）。
#[tauri::command]
pub async fn subtitle_search(
    source: String,
    query: String,
    page: Option<u32>,
) -> Result<Vec<OnlineSubtitle>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        subtitle_online::search(&source, &query, page.unwrap_or(1))
    })
    .await
    .map_err(|err| format!("字幕搜索任务失败: {err}"))?
    .map_err(|err| err.to_string())
}

/// 下载一条在线字幕到本地缓存，返回本地文件路径（可直接交给 `player_add_subtitle`）。
#[tauri::command]
pub async fn subtitle_download(
    app: AppHandle,
    source: String,
    id: String,
    name: Option<String>,
) -> Result<String, String> {
    let dir = subtitle_cache_dir(&app);
    tauri::async_runtime::spawn_blocking(move || download_subtitle(&source, &id, name, &dir))
        .await
        .map_err(|err| format!("字幕下载任务失败: {err}"))?
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|err| err.to_string())
}

/* ---------------- 内部实现 ---------------- */

/// 字幕缓存目录：`<下载目录>/cache/subtitles`（与播放器磁盘缓存同根，方便统一清理）。
fn subtitle_cache_dir(app: &AppHandle) -> PathBuf {
    if let Some(store) = app.try_state::<crate::download::DownloadStore>() {
        return store.cache_dir().join("subtitles");
    }
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("subtitles"))
        .unwrap_or_else(|_| std::env::temp_dir().join("vbox-subtitles"))
}

/// 解析直链 → 下载 → （必要时解压）→ 返回最终字幕文件路径。
fn download_subtitle(
    source: &str,
    id: &str,
    name: Option<String>,
    dir: &Path,
) -> vbox_core::error::Result<PathBuf> {
    use vbox_core::error::CoreError;

    let url = subtitle_online::resolve_download(source, id)?;
    let label = sanitize_name(name.as_deref().unwrap_or(id));
    std::fs::create_dir_all(dir)?;

    let url_ext = extension_of(&url);
    let temp = dir.join(format!(".{label}-{id}-{}.part", std::process::id()));
    subtitle_online::download_to(&url, &temp)?;

    // 按文件头判断是压缩包还是单个字幕（站点直链扩展名不一定可信）
    let magic = sniff_subtitle_ext(&temp);
    let result = match magic.as_deref() {
        Some("zip") => extract_zip_subtitle(&temp, dir, &label),
        Some("rar") => extract_rar_subtitle(&temp, dir, &label),
        Some("7z") => Err(CoreError::Config(
            "该字幕是 7z 压缩包，暂不支持自动解包，请手动下载后加载".to_string(),
        )),
        _ => {
            let ext = if SUBTITLE_EXTS.contains(&url_ext.as_str()) {
                url_ext
            } else {
                magic.unwrap_or_else(|| "srt".to_string())
            };
            if !SUBTITLE_EXTS.contains(&ext.as_str()) {
                Err(CoreError::Config(format!("无法识别的字幕文件格式: .{ext}")))
            } else if std::fs::metadata(&temp)?.len() > MAX_SUBTITLE_BYTES {
                Err(CoreError::Config("字幕文件过大，已取消".to_string()))
            } else {
                let dest = unique_path(dir, &label, &ext);
                std::fs::rename(&temp, &dest)?;
                Ok(dest)
            }
        }
    };

    // 临时文件统一清理（解压成功时 rename 已移走，失败时残留 .part）
    let _ = std::fs::remove_file(&temp);
    result
}

/// 从 zip 包里解出第一条字幕文件（忽略目录、忽略超大文件、只取文件名防路径穿越）。
fn extract_zip_subtitle(
    archive_path: &Path,
    dir: &Path,
    label: &str,
) -> vbox_core::error::Result<PathBuf> {
    use vbox_core::error::CoreError;

    let file = std::fs::File::open(archive_path)?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(err) => {
            // 扩展名是 .zip、内容其实是 RAR 的情况在字幕站并不少见
            let hint = match sniff_subtitle_ext(archive_path) {
                Some(kind) if kind == "rar" || kind == "7z" => {
                    format!("该字幕实际是 {kind} 压缩包，暂不支持自动解包")
                }
                _ => format!("字幕压缩包打不开: {err}"),
            };
            return Err(CoreError::Config(hint));
        }
    };
    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let entry_name = entry.name().to_string();
        if entry_name.ends_with('/') || entry.is_dir() {
            continue;
        }
        let ext = extension_of(&entry_name);
        if !SUBTITLE_EXTS.contains(&ext.as_str()) {
            continue;
        }
        if entry.size() > MAX_SUBTITLE_BYTES {
            return Err(CoreError::Config(format!(
                "字幕文件过大（{} 字节）: {entry_name}",
                entry.size()
            )));
        }
        let dest = unique_path(dir, label, &ext);
        let mut out = std::fs::File::create(&dest)?;
        let copied = std::io::copy(&mut entry, &mut out)?;
        if copied > MAX_SUBTITLE_BYTES {
            let _ = std::fs::remove_file(&dest);
            return Err(CoreError::Config(format!("字幕文件过大: {entry_name}")));
        }
        return Ok(dest);
    }
    Err(CoreError::Config(
        "压缩包里没有可识别的字幕文件（.srt/.ass/.ssa/.sup 等）".to_string(),
    ))
}

/// 从 RAR 包里解出第一条字幕文件（内置 UnRAR，不依赖系统安装的 winrar/7z）。
fn extract_rar_subtitle(
    archive_path: &Path,
    dir: &Path,
    label: &str,
) -> vbox_core::error::Result<PathBuf> {
    use vbox_core::error::CoreError;

    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|err| CoreError::Config(format!("RAR 字幕包打不开: {err}")))?;
    loop {
        let header = match archive.read_header() {
            Ok(Some(header)) => header,
            Ok(None) => break,
            Err(err) => return Err(CoreError::Config(format!("RAR 读取失败: {err}"))),
        };
        let (is_file, unpacked_size, entry_name, ext) = {
            let entry = header.entry();
            let entry_name = entry.filename.to_string_lossy().to_string();
            let ext = extension_of(&entry_name);
            (entry.is_file(), entry.unpacked_size, entry_name, ext)
        };
        if is_file && SUBTITLE_EXTS.contains(&ext.as_str()) {
            if unpacked_size > MAX_SUBTITLE_BYTES {
                return Err(CoreError::Config(format!(
                    "字幕文件过大（{unpacked_size} 字节）: {entry_name}"
                )));
            }
            let (data, _rest) = header
                .read()
                .map_err(|err| CoreError::Config(format!("RAR 解压失败: {err}")))?;
            if data.len() as u64 > MAX_SUBTITLE_BYTES {
                return Err(CoreError::Config(format!("字幕文件过大: {entry_name}")));
            }
            let dest = unique_path(dir, label, &ext);
            std::fs::write(&dest, &data)?;
            return Ok(dest);
        }
        archive = match header.skip() {
            Ok(next) => next,
            Err(err) => return Err(CoreError::Config(format!("RAR 跳过条目失败: {err}"))),
        };
    }
    Err(CoreError::Config(
        "RAR 包里没有可识别的字幕文件（.srt/.ass/.ssa/.sup 等）".to_string(),
    ))
}

/// 读前 512 字节判断文件类型：压缩包（zip/rar/7z）或文本字幕（ass/srt/vtt）。
fn sniff_subtitle_ext(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 512];
    let read = file.read(&mut head).ok()?;
    let head = &head[..read];
    if head.starts_with(b"PK\x03\x04") {
        return Some("zip".to_string());
    }
    if head.starts_with(b"Rar!\x1a\x07") {
        return Some("rar".to_string());
    }
    if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Some("7z".to_string());
    }
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    if text.contains("[script info]") || text.contains("[v4+ styles]") {
        return Some("ass".to_string());
    }
    if text.contains("-->") {
        return Some("srt".to_string());
    }
    if text.contains("webvtt") {
        return Some("vtt".to_string());
    }
    None
}

/// URL / 文件名取扩展名（小写，无扩展名返回空串）。
fn extension_of(value: &str) -> String {
    let path = value.split(['?', '#']).next().unwrap_or(value);
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && ext.len() <= 8 => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// 文件名清洗：去掉 Windows 非法字符 / 控制字符，压缩空白并截断。
fn sanitize_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        return "subtitle".to_string();
    }
    // 按字符截断，避免 UTF-8 边界问题
    if cleaned.chars().count() > 80 {
        cleaned = cleaned.chars().take(80).collect();
    }
    cleaned
}

/// 目标文件已存在时追加 `-1` / `-2` 后缀。
fn unique_path(dir: &Path, label: &str, ext: &str) -> PathBuf {
    let ext = if ext.is_empty() { "srt" } else { ext };
    let first = dir.join(format!("{label}.{ext}"));
    if !first.exists() {
        return first;
    }
    for index in 1..1000 {
        let candidate = dir.join(format!("{label}-{index}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{label}-{}.{ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_parsing() {
        assert_eq!(extension_of("https://dl.subhd.me/2026/06/x.ASS"), "ass");
        assert_eq!(extension_of("https://x/y.srt?token=1"), "srt");
        assert_eq!(extension_of("https://x/noext"), "");
    }

    #[test]
    fn name_sanitizing() {
        assert_eq!(sanitize_name("  a/b:c*?  "), "a_b_c__");
        assert_eq!(sanitize_name("   "), "subtitle");
        assert_eq!(sanitize_name("沙丘 Dune 2021"), "沙丘 Dune 2021");
        let long = "字".repeat(200);
        assert_eq!(sanitize_name(&long).chars().count(), 80);
    }

    #[test]
    fn unique_names_do_not_collide() {
        let dir = std::env::temp_dir().join(format!("vbox-sub-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let first = unique_path(&dir, "movie", "ass");
        assert_eq!(first.file_name().unwrap(), "movie.ass");
        std::fs::write(&first, b"x").expect("write");
        let second = unique_path(&dir, "movie", "ass");
        assert_eq!(second.file_name().unwrap(), "movie-1.ass");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_archive_and_text_magic() {
        let dir = std::env::temp_dir().join(format!("vbox-sub-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let rar = dir.join("a.bin");
        std::fs::write(&rar, b"Rar!\x1a\x07\x01\x00rest").expect("write");
        assert_eq!(sniff_subtitle_ext(&rar), Some("rar".to_string()));
        let zip = dir.join("c.bin");
        std::fs::write(&zip, b"PK\x03\x04rest").expect("write");
        assert_eq!(sniff_subtitle_ext(&zip), Some("zip".to_string()));
        let ass = dir.join("b.bin");
        std::fs::write(&ass, "[Script Info]\nScriptType: v4.00+").expect("write");
        assert_eq!(sniff_subtitle_ext(&ass), Some("ass".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 联网端到端（默认忽略）：搜索 → 解析直链 → 下载（必要时解压）→ 落地文件。
    ///
    /// ```text
    /// set VBOX_TEST_SUBHD=1
    /// cargo test -p vbox-desktop --lib subtitle_online -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "需要联网访问 SubHD（设置 VBOX_TEST_SUBHD=1 后手动运行）"]
    fn live_download_online_subtitle() {
        if std::env::var("VBOX_TEST_SUBHD").is_err() {
            eprintln!("skip: 未设置 VBOX_TEST_SUBHD=1");
            return;
        }
        let items = vbox_core::subtitle_online::search("subhd", "沙丘", 1).expect("搜索失败");
        let candidates: Vec<_> = items
            .iter()
            .filter(|item| matches!(item.format.as_str(), "ASS" | "SRT" | "SUP" | "SSA"))
            .take(6)
            .collect();
        assert!(!candidates.is_empty(), "没有合适的字幕结果");

        let dir = std::env::temp_dir().join(format!("vbox-sub-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut downloaded: Option<(PathBuf, u64)> = None;
        for item in &candidates {
            eprintln!("尝试: {} / {} ({})", item.title, item.name, item.format);
            match download_subtitle("subhd", &item.id, Some(item.name.clone()), &dir) {
                Ok(path) => {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    eprintln!("已落地: {} ({size} 字节)", path.display());
                    downloaded = Some((path, size));
                    break;
                }
                Err(err) => eprintln!("失败（换下一条）: {err}"),
            }
        }
        let (path, size) = downloaded.expect("所有候选都下载/解包失败");
        assert!(path.exists(), "字幕文件不存在");
        assert!(size > 100, "字幕文件过小");
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        assert!(!name.ends_with(".part"), "不应残留临时文件: {name}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
