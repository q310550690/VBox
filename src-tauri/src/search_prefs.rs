//! 搜索设置持久化（JSON）
//!
//! 文件：`<app_data_dir>/search.json`；内容损坏/缺失时回退默认值，不影响搜索。
//! 当前只有一项：`threads`（跨源搜索并发线程数，1..=32，默认 8）。
//!
//! 命令：`search_prefs`（读取）、`search_set_prefs(prefs)`（归一化后保存并返回）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use vbox_core::search::{DEFAULT_SEARCH_THREADS, MAX_SEARCH_THREADS};

/// 设置文件名（位于 app_data_dir 下）
const FILE_NAME: &str = "search.json";

/// 搜索设置（持久化到 `<app_data_dir>/search.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPrefs {
    /// 跨源搜索线程数（并发请求的站点数）
    #[serde(default = "default_threads")]
    pub threads: u32,
}

fn default_threads() -> u32 {
    DEFAULT_SEARCH_THREADS as u32
}

impl Default for SearchPrefs {
    fn default() -> Self {
        Self {
            threads: default_threads(),
        }
    }
}

impl SearchPrefs {
    /// 归一化：线程数夹到 1..=32（上限与 `vbox_core::search::MAX_SEARCH_THREADS` 一致）。
    pub fn normalized(self) -> Self {
        Self {
            threads: self.threads.clamp(1, MAX_SEARCH_THREADS as u32),
        }
    }
}

/// 搜索设置存储（Tauri managed state）。
pub struct SearchPrefsStore {
    path: PathBuf,
    /// 串行化读改写，避免并发保存互相覆盖
    lock: Mutex<()>,
}

impl SearchPrefsStore {
    /// 绑定到 `<dir>/search.json`；目录创建失败时忽略（保存时还会再尝试）。
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

    /// 读取设置：文件缺失或内容损坏时回退默认值（再做一次归一化）。
    pub fn load(&self) -> SearchPrefs {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match read_prefs(&self.path) {
            Ok(prefs) => prefs.normalized(),
            Err(err) => {
                if self.path.exists() {
                    eprintln!(
                        "[vbox] 搜索设置读取失败({}): {err}，使用默认值",
                        self.path.display()
                    );
                }
                SearchPrefs::default()
            }
        }
    }

    /// 归一化后原子写入，返回真正生效的设置。
    pub fn save(&self, prefs: &SearchPrefs) -> Result<SearchPrefs, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let normalized = prefs.clone().normalized();
        let text = serde_json::to_string_pretty(&normalized)
            .map_err(|err| format!("序列化搜索设置失败: {err}"))?;
        write_atomic(&self.path, &text)?;
        Ok(normalized)
    }
}

/// 读取并反序列化设置文件。
fn read_prefs(path: &Path) -> Result<SearchPrefs, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("读取失败: {err}"))?;
    serde_json::from_str::<SearchPrefs>(&text).map_err(|err| format!("解析失败: {err}"))
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
            Err(format!("保存搜索设置失败({}): {first}", path.display()))
        }
    }
}

/* ---------------- Tauri 命令 ---------------- */

/// 读取搜索设置（缺失/损坏时返回默认值）。
#[tauri::command]
pub fn search_prefs(store: tauri::State<'_, SearchPrefsStore>) -> SearchPrefs {
    store.load()
}

/// 保存搜索设置（归一化后落盘并返回生效值）。
#[tauri::command]
pub fn search_set_prefs(
    prefs: SearchPrefs,
    store: tauri::State<'_, SearchPrefsStore>,
) -> Result<SearchPrefs, String> {
    store.save(&prefs)
}

/* ---------------- 单元测试 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!(
                "vbox-search-prefs-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
                seq
            ))
            .join(tag);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 文件不存在时返回默认值。
    #[test]
    fn prefs_default_when_missing() {
        let dir = temp_dir("missing");
        let store = SearchPrefsStore::new(&dir);
        assert_eq!(store.load(), SearchPrefs::default());
        assert!(!store.path().exists());
    }

    /// 保存后可读回，字段为 camelCase，且不残留临时文件。
    #[test]
    fn prefs_save_and_reload() {
        let dir = temp_dir("roundtrip");
        let store = SearchPrefsStore::new(&dir);
        let saved = store.save(&SearchPrefs { threads: 16 }).unwrap();
        assert_eq!(store.load(), saved);
        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("\"threads\""));
        assert!(!store.path().with_extension("json.tmp").exists());
    }

    /// 非法线程数归一化到 1..=32；损坏文件回退默认值。
    #[test]
    fn prefs_normalize_and_fallback() {
        let dir = temp_dir("normalize");
        let store = SearchPrefsStore::new(&dir);
        assert_eq!(store.save(&SearchPrefs { threads: 0 }).unwrap().threads, 1);
        assert_eq!(
            store.save(&SearchPrefs { threads: 999 }).unwrap().threads,
            32
        );

        std::fs::write(store.path(), "{ not json").unwrap();
        assert_eq!(store.load(), SearchPrefs::default());
    }
}
