//! 随应用集成的运行时：Python / Node.js
//!
//! 发布包可以在打包时选择是否内置（见 `scripts/build.mjs` 与 `scripts/ensure-tools.mjs`）：
//! - 内置时放在 `resources/python/python.exe`、`resources/node/node.exe`；
//! - 启动时这里负责发现它们，并注册给 `vbox_core`（`set_bundled`）。
//!
//! 解析优先级是：设置页指定 → 环境变量（`VBOX_PYTHON` / `VBOX_NODE`）→ 随包内置 →
//! PATH。用户点「自动探测」只会清掉设置页路径，不会把内置运行时丢掉。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

/// 随包目录（相对 Tauri resource_dir）
const PYTHON_SUBDIR: &str = "resources/python";
const NODE_SUBDIR: &str = "resources/node";

const PYTHON_EXE: &str = if cfg!(windows) { "python.exe" } else { "python3" };
const NODE_EXE: &str = if cfg!(windows) { "node.exe" } else { "node" };

/// 去掉 Windows 扩展长度路径前缀（`\\?\` / `\\?\UNC\`），得到用户可读的普通路径。
///
/// Tauri 的 `resource_dir()` 内部对 `current_exe()` 做了 `canonicalize()`，Windows 上
/// 会返回 `\\?\D:\...` 形式的 Verbatim 路径；这个前缀只对 Win32 API 有意义，直接展示在
/// 设置页会变成「解释器：\\?\D:\Program Files\...」这种怪样子。统一在「路径来源」处去掉：
/// - `\\?\D:\dir\file` → `D:\dir\file`
/// - `\\?\UNC\server\share\file` → `\\server\share\file`
///
/// 非 Windows 平台原样返回（同时兼容正斜杠写法，便于跨平台测试）。
pub(crate) fn strip_verbatim_prefix(path: &str) -> String {
    // (前缀, 替换成) —— 更长的 UNC 前缀必须先匹配。
    const PREFIXES: [(&str, &str); 4] = [
        (r"\\?\UNC\", r"\\"),
        (r"\\?\unc\", r"\\"),
        (r"\\?\", ""),
        ("//?/", ""),
    ];
    for (prefix, replacement) in PREFIXES {
        if let Some(head) = path.get(..prefix.len())
            && head.eq_ignore_ascii_case(prefix)
        {
            return format!("{replacement}{}", &path[prefix.len()..]);
        }
    }
    path.to_string()
}

/// 路径 → 展示文本（去掉 Verbatim 前缀）。
pub(crate) fn path_text(path: &Path) -> String {
    strip_verbatim_prefix(&path.to_string_lossy())
}

/// PathBuf 版本的前缀规范化。
fn plain_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if text.contains(r"\\?\") || text.contains("//?/") {
        PathBuf::from(strip_verbatim_prefix(&text))
    } else {
        path
    }
}

/// 启动时探测随包 Python/Node 并注册（找不到就保持「没有内置」）。
pub fn init(app: &AppHandle) {
    let resource_dir = app.path().resource_dir().ok().map(plain_path);
    if let Some(path) = find_bundled(resource_dir.as_deref(), PYTHON_SUBDIR, PYTHON_EXE) {
        let text = path_text(&path);
        vbox_core::python::set_bundled(Some(&text));
    }
    if let Some(path) = find_bundled(resource_dir.as_deref(), NODE_SUBDIR, NODE_EXE) {
        let text = path_text(&path);
        vbox_core::node::set_bundled(Some(&text));
    }
}

/// 内置运行时候选路径（按优先级）：
/// 1. `VBOX_PYTHON_BUNDLED` / `VBOX_NODE_BUNDLED`（调试/自定义）
/// 2. `resource_dir/resources/{python,node}/<exe>`（Tauri 打包布局）
/// 3. `resource_dir/{python,node}/<exe>`（手工归档）
/// 4. exe 同目录及其 `resources/` 子目录
/// 5. 开发期：仓库内 `src-tauri/resources/{python,node}/<exe>`
fn find_bundled(resource_dir: Option<&Path>, subdir: &str, exe: &str) -> Option<PathBuf> {
    let name = subdir.rsplit('/').next().unwrap_or(subdir);
    let env_key = format!(
        "VBOX_{}_BUNDLED",
        name.to_ascii_uppercase()
    );
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(custom) = std::env::var_os(env_key) {
        candidates.push(PathBuf::from(custom));
    }
    if let Some(dir) = resource_dir {
        candidates.push(dir.join(subdir).join(exe));
        candidates.push(dir.join(name).join(exe));
        candidates.push(dir.join(exe));
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(dir) = current.parent()
    {
        candidates.push(dir.join(subdir).join(exe));
        candidates.push(dir.join("resources").join(name).join(exe));
        candidates.push(dir.join(name).join(exe));
        if let Some(root) = dir.parent().and_then(Path::parent) {
            candidates.push(root.join(subdir).join(exe));
        }
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join(subdir).join(exe));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(plain_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vbox-embedded-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    #[test]
    fn detects_python_and_node_in_resource_layout() {
        let dir = temp_dir("layout");
        let resources = dir.join("resources");
        std::fs::create_dir_all(resources.join("python")).expect("建目录失败");
        std::fs::create_dir_all(resources.join("node")).expect("建目录失败");
        let python = resources.join("python").join(PYTHON_EXE);
        let node = resources.join("node").join(NODE_EXE);
        std::fs::write(&python, b"").expect("写 python 失败");
        std::fs::write(&node, b"").expect("写 node 失败");

        assert_eq!(
            find_bundled(Some(&resources), PYTHON_SUBDIR, PYTHON_EXE),
            Some(python)
        );
        assert_eq!(
            find_bundled(Some(&resources), NODE_SUBDIR, NODE_EXE),
            Some(node)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 去掉 `\\?\` / `\\?\UNC\` 前缀（设置页路径展示用）。
    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(
            strip_verbatim_prefix(r"\\?\D:\Program Files\VBox\resources\python\python.exe"),
            r"D:\Program Files\VBox\resources\python\python.exe"
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\python.exe"),
            r"\\server\share\python.exe"
        );
        // 正斜杠写法与非 Windows 普通路径原样保留。
        assert_eq!(
            strip_verbatim_prefix("//?/D:/VBox/python.exe"),
            "D:/VBox/python.exe"
        );
        assert_eq!(strip_verbatim_prefix("/usr/local/bin/python3"), "/usr/local/bin/python3");
        assert_eq!(strip_verbatim_prefix(r"C:\Python312\python.exe"), r"C:\Python312\python.exe");
    }

    /// Tauri `resource_dir()` 在 Windows 上可能带 `\\?\`：探测结果必须是普通路径。
    #[test]
    fn find_bundled_returns_plain_path_for_verbatim_resource_dir() {
        let dir = temp_dir("verbatim");
        let resources = dir.join("resources");
        std::fs::create_dir_all(resources.join("python")).expect("建目录失败");
        let python = resources.join("python").join(PYTHON_EXE);
        std::fs::write(&python, b"").expect("写 python 失败");

        let verbatim = PathBuf::from(format!(r"\\?\{}", resources.to_string_lossy()));
        let found = find_bundled(Some(&verbatim), PYTHON_SUBDIR, PYTHON_EXE)
            .expect("未找到随包 Python");
        assert_eq!(found, python);
        assert!(
            !found.to_string_lossy().contains(r"\\?\"),
            "展示路径不应带 Verbatim 前缀: {:?}",
            found
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_tool_returns_none_for_isolated_dir() {
        let dir = temp_dir("missing");
        // 空 resource_dir：不应命中该目录（仍可能命中开发期路径，
        // 所以这里只验证「目录里没有就至少不会把该目录当成命中」）。
        let found = find_bundled(Some(&dir), "resources/python", PYTHON_EXE);
        if let Some(path) = found {
            assert!(!path.starts_with(&dir), "不应命中空目录: {:?}", path);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 需要先 `npm run fetch:tools`（或单独 fetch:python / fetch:node）把随包产物放好。
    /// 验证：仓库资源目录能被发现，且注册给 vbox_core 后真的能跑起来。
    #[test]
    #[ignore = "需要本地已执行 npm run fetch:tools"]
    fn detects_staged_repo_runtimes() {
        let python = find_bundled(None, PYTHON_SUBDIR, PYTHON_EXE)
            .expect("没找到 src-tauri/resources/python/python.exe（先 npm run fetch:python）");
        let node = find_bundled(None, NODE_SUBDIR, NODE_EXE)
            .expect("没找到 src-tauri/resources/node/node.exe（先 npm run fetch:node）");

        vbox_core::python::set_bundled(Some(&python.to_string_lossy()));
        let py = vbox_core::python::python_status();
        assert!(py.available, "内置 Python 不可用: {:?}", py.error);
        assert_eq!(py.bundled.as_deref(), Some(python.to_string_lossy().as_ref()));
        assert!(
            py.dependencies.iter().all(|dep| dep.available),
            "内置 Python 缺少依赖: {:?}",
            py.dependencies
        );

        vbox_core::node::set_bundled(Some(&node.to_string_lossy()));
        let nd = vbox_core::node::node_status();
        assert!(nd.available, "内置 Node 不可用: {:?}", nd.error);
        assert_eq!(nd.bundled.as_deref(), Some(node.to_string_lossy().as_ref()));

        // 顺带验证 FFmpeg 的随包探测（走同一套 resources 布局）
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let dir = temp_dir("staged-ffmpeg");
        let store = crate::ffmpeg_prefs::FfmpegPrefsStore::new(&dir, Some(resources));
        let ffmpeg = store.status();
        assert_eq!(ffmpeg.source, "bundled", "FFmpeg 未命中随包副本: {:?}", ffmpeg);
        assert!(ffmpeg.available, "随包 FFmpeg 不可用: {:?}", ffmpeg.error);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
