//! 播放设置持久化（JSON）
//!
//! 设计要点：
//! - 文件：`<app_data_dir>/player.json`（小体积 JSON，直接读改写）；
//! - 原子写：先写 `player.json.tmp`，再 `rename` 覆盖正式文件，避免进程崩溃留下半截 JSON；
//! - 文件缺失 / 内容损坏时一律回退默认值，不影响启动与播放；
//! - 读取与保存都会做归一化：音量 0..=100，倍速 0.25..=4.0，硬解只接受白名单值
//!   （非法值回退 `auto-safe`），NaN / Infinity 回退默认值；
//! - 音量语义（版本 2）：不是设置页选项，而是「上次关闭播放 / 退出应用时的音量」，
//!   由 `player::remember_volume` 写回，创建播放器时恢复；旧版文件里的默认 80 会迁移成新默认 100。
//!
//! 命令：`player_prefs`（读取）、`player_set_prefs(prefs)`（只接受硬解 / 倍速并保存）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 设置文件名（位于 app_data_dir 下）
const FILE_NAME: &str = "player.json";

/// 当前设置文件版本。
/// - 缺省（0）：旧版，`volume` 是设置页的「默认音量」，默认 80；
/// - 2：音量不再是设置项，`volume` 表示「上次关闭播放 / 退出应用时的音量」。
const PREFS_VERSION: u32 = 2;

/// 默认音量（新安装 / 文件缺失 / 非法值时的初始音量）
const DEFAULT_VOLUME: f64 = 100.0;
/// 旧版设置文件的默认音量（升级时用于识别「这只是旧默认值」）
const LEGACY_DEFAULT_VOLUME: f64 = 80.0;
/// 默认倍速
const DEFAULT_RATE: f64 = 1.0;
/// 默认硬解模式
const DEFAULT_HWDEC: &str = "auto-safe";

/// 允许的硬解模式（与前端设置页选项一致）
const HWDEC_OPTIONS: [&str; 4] = ["auto-safe", "auto", "d3d11va", "no"];

/// 默认画质增强模式（关闭）
const DEFAULT_ENHANCE: &str = "off";

/// 允许的画质增强模式（播放器页面「画质」弹层，与 `enhance::MODES` 对应）：
/// - off：关闭
/// - lite：轻量超分（Anime4K S 档，适合集显 / 低负载）
/// - standard：标准超分（M 档）
/// - strong：强力超分（L 档，需要独显）
pub const ENHANCE_OPTIONS: [&str; 4] = ["off", "lite", "standard", "strong"];

/// 播放设置（持久化到 `<app_data_dir>/player.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackPrefs {
    /// 硬解模式：auto-safe / auto / d3d11va / no
    #[serde(default = "default_hwdec")]
    pub hwdec: String,
    /// 音量 0-100：版本 2 起表示「上次退出时用户调节的音量」，启动播放器时恢复
    #[serde(default = "default_volume")]
    pub volume: f64,
    /// 默认倍速 0.25-4.0
    #[serde(default = "default_rate")]
    pub rate: f64,
    /// 画质增强模式：off / lite / standard / strong（播放器页面的「画质」开关）
    #[serde(default = "default_enhance")]
    pub enhance: String,
    /// 设置文件版本（用于旧版「默认音量 80」的一次性迁移）
    #[serde(default)]
    pub version: u32,
}

fn default_hwdec() -> String {
    DEFAULT_HWDEC.to_string()
}

fn default_volume() -> f64 {
    DEFAULT_VOLUME
}

fn default_rate() -> f64 {
    DEFAULT_RATE
}

fn default_enhance() -> String {
    DEFAULT_ENHANCE.to_string()
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        Self {
            hwdec: default_hwdec(),
            volume: DEFAULT_VOLUME,
            rate: DEFAULT_RATE,
            enhance: default_enhance(),
            version: PREFS_VERSION,
        }
    }
}

impl PlaybackPrefs {
    /// 归一化：非法/越界值回退到合法范围或默认值。
    pub fn normalized(self) -> Self {
        Self {
            hwdec: normalize_hwdec(&self.hwdec),
            volume: normalize_volume(self.volume),
            rate: normalize_rate(self.rate),
            enhance: normalize_enhance(&self.enhance),
            version: self.version,
        }
    }
}

/// 硬解值白名单校验（大小写/空白不敏感；非法值回退 auto-safe）。
fn normalize_hwdec(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    if HWDEC_OPTIONS.contains(&lower.as_str()) {
        lower
    } else {
        DEFAULT_HWDEC.to_string()
    }
}

/// 音量归一：非有限值回退默认，其余夹到 0..=100。
fn normalize_volume(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        DEFAULT_VOLUME
    }
}

/// 倍速归一：非有限值回退默认，其余夹到 0.25..=4.0。
fn normalize_rate(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.25, 4.0)
    } else {
        DEFAULT_RATE
    }
}

/// 画质增强值白名单校验（大小写/空白不敏感；非法值回退 off）。
pub fn normalize_enhance(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    if ENHANCE_OPTIONS.contains(&lower.as_str()) {
        lower
    } else {
        DEFAULT_ENHANCE.to_string()
    }
}

/// 播放设置存储（Tauri managed state）。
pub struct PlayerPrefsStore {
    path: PathBuf,
    /// 串行化读改写，避免并发保存时互相覆盖（命令本身在主线程执行。
    /// 这里加锁是为了将来从工作线程调用时也安全）。
    lock: Mutex<()>,
}

impl PlayerPrefsStore {
    /// 绑定到 `<dir>/player.json`；目录创建失败时忽略（保存时还会再尝试）。
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
    ///
    /// 旧版文件（无 version 字段）会做一次迁移：若音量恰好是旧默认值 80，
    /// 说明用户大概率从未调过音量，重置为新默认 100；其它音量值视为用户主动设置，原样保留。
    pub fn load(&self) -> PlaybackPrefs {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match read_prefs(&self.path) {
            Ok(prefs) => {
                let prefs = prefs.normalized();
                if prefs.version >= PREFS_VERSION {
                    return prefs;
                }
                let migrated = PlaybackPrefs {
                    volume: if (prefs.volume - LEGACY_DEFAULT_VOLUME).abs() < f64::EPSILON {
                        DEFAULT_VOLUME
                    } else {
                        prefs.volume
                    },
                    version: PREFS_VERSION,
                    ..prefs
                };
                if let Ok(text) = serde_json::to_string_pretty(&migrated) {
                    if let Err(err) = write_atomic(&self.path, &text) {
                        eprintln!("[vbox] 迁移播放设置失败({}): {err}", self.path.display());
                    }
                }
                migrated
            }
            Err(err) => {
                if self.path.exists() {
                    eprintln!(
                        "[vbox] 播放设置读取失败({}): {err}，使用默认值",
                        self.path.display()
                    );
                }
                PlaybackPrefs::default()
            }
        }
    }

    /// 归一化后原子写入，返回真正生效的设置。
    pub fn save(&self, prefs: &PlaybackPrefs) -> Result<PlaybackPrefs, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let normalized = PlaybackPrefs {
            version: PREFS_VERSION,
            ..prefs.clone().normalized()
        };
        let text = serde_json::to_string_pretty(&normalized)
            .map_err(|err| format!("序列化播放设置失败: {err}"))?;
        write_atomic(&self.path, &text)?;
        Ok(normalized)
    }
}

/// 读取并反序列化设置文件。
fn read_prefs(path: &Path) -> Result<PlaybackPrefs, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("读取失败: {err}"))?;
    serde_json::from_str::<PlaybackPrefs>(&text).map_err(|err| format!("解析失败: {err}"))
}

/// 原子写：写临时文件 → rename 覆盖；rename 失败时退化为「删除目标后重试」。
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
            // Windows 上目标被其它进程短暂占用时 rename 可能失败：删掉目标再试一次。
            if std::fs::remove_file(path).is_ok() {
                if std::fs::rename(&tmp, path).is_ok() {
                    return Ok(());
                }
            }
            let _ = std::fs::remove_file(&tmp);
            Err(format!("保存播放设置失败({}): {first}", path.display()))
        }
    }
}

/* ---------------- Tauri 命令 ---------------- */

/// 读取播放设置（缺失/损坏时返回默认值）。
#[tauri::command]
pub fn player_prefs(store: tauri::State<'_, PlayerPrefsStore>) -> PlaybackPrefs {
    store.load()
}

/// 保存播放设置（归一化后落盘并返回生效值）。
///
/// 设置页现在只提供硬解 / 倍速；音量由播放器在关闭播放 / 退出应用时写回，
/// 所以这里只覆盖 `hwdec` / `rate`，避免设置页用内存里的旧音量把「上次音量」冲掉。
#[tauri::command]
pub fn player_set_prefs(
    prefs: PlaybackPrefs,
    store: tauri::State<'_, PlayerPrefsStore>,
) -> Result<PlaybackPrefs, String> {
    let mut next = store.load();
    next.hwdec = prefs.hwdec;
    next.rate = prefs.rate;
    store.save(&next)
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
            "vbox-prefs-test-{}-{}-{}",
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
        let store = PlayerPrefsStore::new(&dir);
        let prefs = store.load();
        assert_eq!(prefs.hwdec, "auto-safe");
        assert_eq!(prefs.volume, 100.0);
        assert_eq!(prefs.rate, 1.0);
        assert_eq!(prefs.enhance, "off");
        assert_eq!(prefs.version, PREFS_VERSION);
        assert!(!store.path().exists());
    }

    /// 保存后可读回；保存文件的 JSON 字段为 camelCase 且无临时文件残留。
    #[test]
    fn prefs_save_and_reload() {
        let dir = temp_dir("roundtrip");
        let store = PlayerPrefsStore::new(&dir);
        let saved = store
            .save(&PlaybackPrefs {
                hwdec: "d3d11va".to_string(),
                volume: 55.0,
                rate: 1.5,
                enhance: "standard".to_string(),
                version: 0,
            })
            .unwrap();
        assert_eq!(
            saved,
            PlaybackPrefs {
                hwdec: "d3d11va".to_string(),
                volume: 55.0,
                rate: 1.5,
                enhance: "standard".to_string(),
                version: PREFS_VERSION,
            }
        );

        let loaded = store.load();
        assert_eq!(loaded, saved);

        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("\"hwdec\""));
        assert!(text.contains("\"volume\""));
        assert!(text.contains("\"rate\""));
        assert!(text.contains("\"enhance\""));
        assert!(
            !store.path().with_extension("json.tmp").exists(),
            "原子写不应残留临时文件"
        );
    }

    /// 非法值归一化：越界夹取、未知硬解回退、NaN 回退默认。
    #[test]
    fn prefs_normalize_illegal_values() {
        let dir = temp_dir("normalize");
        let store = PlayerPrefsStore::new(&dir);
        let saved = store
            .save(&PlaybackPrefs {
                hwdec: "unknown-mode".to_string(),
                volume: 250.0,
                rate: 0.01,
                enhance: "ULTRA".to_string(),
                version: 0,
            })
            .unwrap();
        assert_eq!(saved.hwdec, "auto-safe");
        assert_eq!(saved.volume, 100.0);
        assert_eq!(saved.rate, 0.25);
        assert_eq!(saved.enhance, "off");

        let saved = store
            .save(&PlaybackPrefs {
                hwdec: " NO ".to_string(),
                volume: -10.0,
                rate: 10.0,
                enhance: " LITE ".to_string(),
                version: 0,
            })
            .unwrap();
        assert_eq!(saved.hwdec, "no");
        assert_eq!(saved.volume, 0.0);
        assert_eq!(saved.rate, 4.0);
        assert_eq!(saved.enhance, "lite");

        let saved = store
            .save(&PlaybackPrefs {
                hwdec: "auto-safe".to_string(),
                volume: f64::NAN,
                rate: f64::INFINITY,
                enhance: String::new(),
                version: 0,
            })
            .unwrap();
        assert_eq!(saved.volume, 100.0);
        assert_eq!(saved.rate, 1.0);
        assert_eq!(saved.enhance, "off");
    }

    /// 旧版文件迁移：旧默认 80 → 新默认 100；用户主动调过的其它音量保留。
    #[test]
    fn prefs_migrates_legacy_volume() {
        let dir = temp_dir("migrate");
        let store = PlayerPrefsStore::new(&dir);
        std::fs::write(
            store.path(),
            r#"{"hwdec":"auto-safe","volume":80.0,"rate":1.0}"#,
        )
        .unwrap();
        let prefs = store.load();
        assert_eq!(prefs.volume, 100.0);
        assert_eq!(prefs.version, PREFS_VERSION);

        // 迁移结果已写回文件，第二次读取不再变化。
        let again = store.load();
        assert_eq!(again.volume, 100.0);

        // 旧文件里非 80 的音量视为用户主动设置，迁移时保留。
        std::fs::write(
            store.path(),
            r#"{"hwdec":"no","volume":42.0,"rate":1.5}"#,
        )
        .unwrap();
        let kept = store.load();
        assert_eq!(kept.volume, 42.0);
        assert_eq!(kept.hwdec, "no");
        assert_eq!(kept.rate, 1.5);
        assert_eq!(kept.version, PREFS_VERSION);
    }

    /// 文件损坏时回退默认值而不是报错/panic。
    #[test]
    fn prefs_corrupted_file_falls_back_to_default() {
        let dir = temp_dir("corrupted");
        let store = PlayerPrefsStore::new(&dir);
        std::fs::write(store.path(), "{ this is not json").unwrap();
        let prefs = store.load();
        assert_eq!(prefs, PlaybackPrefs::default());

        // 部分字段缺失也应使用对应默认值；已写出的非 80 音量在迁移中保留。
        std::fs::write(store.path(), r#"{"volume": 42}"#).unwrap();
        let prefs = store.load();
        assert_eq!(prefs.hwdec, "auto-safe");
        assert_eq!(prefs.volume, 42.0);
        assert_eq!(prefs.rate, 1.0);
        assert_eq!(prefs.version, PREFS_VERSION);
    }
}
