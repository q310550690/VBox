//! 播放缓存（HLS 边下边播的分片缓存）保留策略、统计与清理。
//!
//! 背景：播放缓存目录曾是「进程内临时目录」——应用启动时清空、停止播放时删除。
//! 现在改为**跨重启保留**，并把清理交给用户可配置的策略：
//! - `max_bytes`：缓存总大小上限，0 = 不限制；
//! - `retention_days`：保留天数，0 = 不限制（按会话目录里最新文件时间判断）。
//!
//! 设置文件：`<app_data_dir>/playback-cache.json`；缺失/损坏时回退「都不限制」。
//! 每次启动代理、每次播放会话结束、以及设置变更时都会按策略清理一次。
//!
//! 缓存目录：`<下载目录>/cache/hls/`（与下载共用下载目录，见 `download::DownloadStore`）。
//! 这里只负责策略与清理，目录归属仍是 `hls_proxy`。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// 设置文件名（位于 app_data_dir 下）
const FILE_NAME: &str = "playback-cache.json";
/// 保留天数上限（0 = 不限制；上限防误填导致溢出）
const MAX_RETENTION_DAYS: u32 = 3650;
/// 大小上限上限：100 GB（0 = 不限制）
const MAX_BYTES_LIMIT: u64 = 100 * 1024 * 1024 * 1024;

/// 播放缓存保留策略（持久化到 `<app_data_dir>/playback-cache.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackCachePrefs {
    /// 缓存总大小上限（字节）；0 = 不限制
    #[serde(default)]
    pub max_bytes: u64,
    /// 保留天数；0 = 不限制
    #[serde(default)]
    pub retention_days: u32,
}

impl Default for PlaybackCachePrefs {
    fn default() -> Self {
        Self {
            max_bytes: 0,
            retention_days: 0,
        }
    }
}

impl PlaybackCachePrefs {
    /// 归一化：两个上限都夹到安全范围；0 表示不限制（保留语义）。
    pub fn normalized(self) -> Self {
        Self {
            max_bytes: self.max_bytes.min(MAX_BYTES_LIMIT),
            retention_days: self.retention_days.min(MAX_RETENTION_DAYS),
        }
    }
}

/// 播放缓存当前占用与生效策略（返回给设置页）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackCacheStatus {
    /// 缓存根目录（展示用）
    pub dir: String,
    /// 当前占用字节数
    pub bytes: u64,
    /// 当前文件数
    pub files: u64,
    /// 生效的大小上限（0 = 不限制）
    pub max_bytes: u64,
    /// 生效的保留天数（0 = 不限制）
    pub retention_days: u32,
}

/// 一次清理的结果（用于日志）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub removed_files: u64,
    pub removed_bytes: u64,
}

/// 播放缓存设置存储（Tauri managed state）。
pub struct PlaybackCacheStore {
    path: PathBuf,
    /// 串行化读改写，避免并发保存互相覆盖
    lock: Mutex<()>,
}

impl PlaybackCacheStore {
    /// 绑定到 `<dir>/playback-cache.json`。
    pub fn new(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        Self {
            path: dir.join(FILE_NAME),
            lock: Mutex::new(()),
        }
    }

    /// 设置文件路径（仅测试/排障使用）。
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取设置：文件缺失/损坏时回退默认（都不限制），并归一化。
    pub fn load(&self) -> PlaybackCachePrefs {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match read_prefs(&self.path) {
            Ok(prefs) => prefs.normalized(),
            Err(err) => {
                if self.path.exists() {
                    eprintln!(
                        "[vbox] 播放缓存设置读取失败({}): {err}，使用默认值（不限制）",
                        self.path.display()
                    );
                }
                PlaybackCachePrefs::default()
            }
        }
    }

    /// 归一化后原子写入，返回真正生效的设置。
    pub fn save(&self, prefs: &PlaybackCachePrefs) -> Result<PlaybackCachePrefs, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let normalized = prefs.clone().normalized();
        let text = serde_json::to_string_pretty(&normalized)
            .map_err(|err| format!("序列化播放缓存设置失败: {err}"))?;
        write_atomic(&self.path, &text)?;
        Ok(normalized)
    }
}

/* ---------------- 目录统计 / 清理 ---------------- */

/// 统计缓存目录占用（递归；目录不存在时返回 0）。
pub fn cache_status(root: &Path, prefs: &PlaybackCachePrefs) -> PlaybackCacheStatus {
    let (files, bytes) = dir_stats(root);
    PlaybackCacheStatus {
        dir: root.to_string_lossy().to_string(),
        bytes,
        files,
        max_bytes: prefs.max_bytes,
        retention_days: prefs.retention_days,
    }
}

/// 按策略清理缓存：
/// 1. 保留天数 > 0：整目录判断——会话目录里最新文件时间早于 cutoff 就整目录删除
///    （避免只删一半分片留下无法复用的残缺缓存）；
/// 2. 大小上限 > 0：剩余文件按 mtime 从旧到新删到总量不超过上限；
/// 3. 顺手清掉空目录（异常退出/手动删除留下的空壳）。
pub fn prune_cache(root: &Path, prefs: &PlaybackCachePrefs) -> PruneReport {
    let mut report = PruneReport::default();
    if !root.is_dir() {
        return report;
    }
    let prefs = prefs.clone().normalized();
    let now = SystemTime::now();

    if prefs.retention_days > 0 {
        let cutoff = now
            .checked_sub(Duration::from_secs(
                prefs.retention_days as u64 * 24 * 60 * 60,
            ))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        for dir in immediate_dirs(root) {
            let newest = newest_mtime(&dir);
            let expired = newest.map(|mtime| mtime < cutoff).unwrap_or(true);
            if expired {
                let (files, bytes) = dir_stats(&dir);
                if std::fs::remove_dir_all(&dir).is_ok() {
                    report.removed_files += files;
                    report.removed_bytes += bytes;
                }
            }
        }
    }

    if prefs.max_bytes > 0 {
        let mut entries = collect_files(root);
        let mut total: u64 = entries.iter().map(|(_, size, _)| *size).sum();
        if total > prefs.max_bytes {
            entries.sort_by_key(|(_, _, mtime)| *mtime);
            for (path, size, _) in entries {
                if total <= prefs.max_bytes {
                    break;
                }
                if std::fs::remove_file(&path).is_ok() {
                    total = total.saturating_sub(size);
                    report.removed_files += 1;
                    report.removed_bytes += size;
                }
            }
        }
    }

    remove_empty_dirs(root);
    report
}

/// 清空播放缓存（保留缓存根目录本身），返回清理后的状态。
pub fn clear_cache(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        std::fs::create_dir_all(root).map_err(|err| {
            format!("创建播放缓存目录失败({}): {err}", root.display())
        })?;
        return Ok(());
    }
    let entries = std::fs::read_dir(root).map_err(|err| {
        format!("读取播放缓存目录失败({}): {err}", root.display())
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(err) = result {
            return Err(format!("清理播放缓存失败({}): {err}", path.display()));
        }
    }
    Ok(())
}

/// 读取并反序列化设置文件。
fn read_prefs(path: &Path) -> Result<PlaybackCachePrefs, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("读取失败: {err}"))?;
    serde_json::from_str::<PlaybackCachePrefs>(&text).map_err(|err| format!("解析失败: {err}"))
}

/// 原子写：写临时文件 → rename 覆盖；Windows 上 rename 失败时删目标重试一次。
fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("创建设置目录失败({}): {err}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)
        .map_err(|err| format!("写入临时设置文件失败({}): {err}", tmp.display()))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            if std::fs::remove_file(path).is_ok() && std::fs::rename(&tmp, path).is_ok() {
                return Ok(());
            }
            let _ = std::fs::remove_file(&tmp);
            Err(format!("保存播放缓存设置失败({}): {first}", path.display()))
        }
    }
}

/* ---------------- 目录遍历辅助 ---------------- */

/// 直接子目录（不存在/读取失败返回空）。
fn immediate_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect()
}

/// 递归收集 `(路径, 大小, mtime)`；`mtime` 读取失败按最旧处理（更容易被清理）。
fn collect_files(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    collect_files_inner(root, &mut out);
    out
}

fn collect_files_inner(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            collect_files_inner(&path, out);
        } else if meta.is_file() {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((path, meta.len(), mtime));
        }
    }
}

/// 递归统计 `(文件数, 总字节)`。
fn dir_stats(root: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for (_, size, _) in collect_files(root) {
        files += 1;
        bytes = bytes.saturating_add(size);
    }
    (files, bytes)
}

/// 目录内所有文件里最新的 mtime（目录为空/不可读时返回 None）。
fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    collect_files(dir).into_iter().map(|(_, _, mtime)| mtime).max()
}

/// 删除空目录（递归，保留 `root` 本身）。
fn remove_empty_dirs(root: &Path) {
    fn is_empty_dir(path: &Path) -> bool {
        std::fs::read_dir(path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    }
    fn walk(dir: &Path) {
        for sub in immediate_dirs(dir) {
            walk(&sub);
            if is_empty_dir(&sub) {
                let _ = std::fs::remove_dir(&sub);
            }
        }
    }
    walk(root);
}

/* ---------------- Tauri 命令 ---------------- */

/// 读取播放缓存保留策略。
#[tauri::command]
pub fn playback_cache_prefs(
    store: tauri::State<'_, std::sync::Arc<PlaybackCacheStore>>,
) -> PlaybackCachePrefs {
    store.load()
}

/// 保存策略并立即按新策略清理一次，返回生效值。
#[tauri::command]
pub fn playback_cache_set_prefs(
    prefs: PlaybackCachePrefs,
    hls: tauri::State<'_, std::sync::Arc<crate::hls_proxy::HlsProxy>>,
    store: tauri::State<'_, std::sync::Arc<PlaybackCacheStore>>,
) -> Result<PlaybackCachePrefs, String> {
    let saved = store.save(&prefs)?;
    let report = prune_cache(hls.cache_root(), &saved);
    if report.removed_files > 0 {
        crate::logs::push(
            "info",
            format!(
                "播放缓存已按新策略清理：{} 个文件 / {:.1} MB",
                report.removed_files,
                report.removed_bytes as f64 / (1024.0 * 1024.0)
            ),
        );
    }
    Ok(saved)
}

/// 当前缓存占用 + 生效策略。
#[tauri::command]
pub fn playback_cache_status(
    hls: tauri::State<'_, std::sync::Arc<crate::hls_proxy::HlsProxy>>,
    store: tauri::State<'_, std::sync::Arc<PlaybackCacheStore>>,
) -> PlaybackCacheStatus {
    cache_status(hls.cache_root(), &store.load())
}

/// 清空播放缓存（保留目录），返回清理后的状态。
#[tauri::command]
pub fn playback_cache_clear(
    hls: tauri::State<'_, std::sync::Arc<crate::hls_proxy::HlsProxy>>,
    store: tauri::State<'_, std::sync::Arc<PlaybackCacheStore>>,
) -> Result<PlaybackCacheStatus, String> {
    let root = hls.cache_root();
    clear_cache(root)?;
    let status = cache_status(root, &store.load());
    crate::logs::push(
        "info",
        format!("播放缓存已清空：释放 {} MB", status.bytes as f64 / (1024.0 * 1024.0)),
    );
    Ok(status)
}

/* ---------------- 单元测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "vbox-playback-cache-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            seq
        ))
        .join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, size: usize, mtime: SystemTime) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, vec![0u8; size]).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(mtime).unwrap();
    }

    fn now() -> SystemTime {
        SystemTime::now()
    }

    fn days_ago(days: u64) -> SystemTime {
        now() - Duration::from_secs(days * 24 * 60 * 60)
    }

    /// 默认值：两项都不限制；归一化夹到安全范围。
    #[test]
    fn prefs_default_and_normalize() {
        assert_eq!(PlaybackCachePrefs::default(), PlaybackCachePrefs { max_bytes: 0, retention_days: 0 });
        let normalized = PlaybackCachePrefs {
            max_bytes: u64::MAX,
            retention_days: u32::MAX,
        }
        .normalized();
        assert_eq!(normalized.max_bytes, MAX_BYTES_LIMIT);
        assert_eq!(normalized.retention_days, MAX_RETENTION_DAYS);
    }

    /// 保存后可读回，字段为 camelCase；损坏文件回退默认。
    #[test]
    fn prefs_save_and_reload() {
        let dir = temp_dir("roundtrip");
        let store = PlaybackCacheStore::new(&dir);
        let saved = store
            .save(&PlaybackCachePrefs {
                max_bytes: 2 * 1024 * 1024 * 1024,
                retention_days: 7,
            })
            .unwrap();
        assert_eq!(saved.max_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(store.load(), saved);
        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("\"maxBytes\""));
        assert!(text.contains("\"retentionDays\""));
        assert!(!store.path().with_extension("json.tmp").exists());

        std::fs::write(store.path(), "{ not json").unwrap();
        assert_eq!(store.load(), PlaybackCachePrefs::default());
    }

    /// 两项都不限制：什么都不删。
    #[test]
    fn prune_unlimited_keeps_everything() {
        let root = temp_dir("unlimited").join("hls");
        write_file(&root.join("s1").join("seg1"), 100, days_ago(365));
        write_file(&root.join("s1").join("seg2"), 200, now());
        let report = prune_cache(&root, &PlaybackCachePrefs::default());
        assert_eq!(report.removed_files, 0);
        assert_eq!(dir_stats(&root).0, 2);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// 保留天数：过期的会话目录整体删除，新目录保留。
    #[test]
    fn prune_by_retention_days() {
        let root = temp_dir("age").join("hls");
        write_file(&root.join("old").join("seg1"), 100, days_ago(10));
        write_file(&root.join("new").join("seg1"), 100, now());
        let prefs = PlaybackCachePrefs {
            max_bytes: 0,
            retention_days: 3,
        };
        let report = prune_cache(&root, &prefs);
        assert_eq!(report.removed_files, 1);
        assert!(!root.join("old").exists());
        assert!(root.join("new").exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// 大小上限：按 mtime 从旧到新删到不超过上限。
    #[test]
    fn prune_by_max_bytes_keeps_newest() {
        let root = temp_dir("size").join("hls");
        write_file(&root.join("s1").join("a"), 100, days_ago(3));
        write_file(&root.join("s1").join("b"), 100, days_ago(2));
        write_file(&root.join("s1").join("c"), 100, days_ago(1));
        let prefs = PlaybackCachePrefs {
            max_bytes: 250,
            retention_days: 0,
        };
        let report = prune_cache(&root, &prefs);
        assert_eq!(report.removed_files, 1);
        let (_, bytes) = dir_stats(&root);
        assert!(bytes <= 250, "清理后应不超过上限，实际 {bytes}");
        assert!(root.join("s1").join("c").exists(), "最新的文件应保留");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// 清空缓存：文件和目录都删掉，但缓存根保留；统计归零。
    #[test]
    fn clear_cache_removes_all_but_root() {
        let root = temp_dir("clear").join("hls");
        write_file(&root.join("s1").join("seg"), 128, now());
        std::fs::create_dir_all(root.join("empty")).unwrap();
        clear_cache(&root).unwrap();
        assert!(root.is_dir());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let status = cache_status(&root, &PlaybackCachePrefs::default());
        assert_eq!(status.files, 0);
        assert_eq!(status.bytes, 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
