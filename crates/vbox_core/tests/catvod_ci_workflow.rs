//! 判据 20：发版工作流必须完整保留（GitHub Actions 只做发版）。
//!
//! 2026-11-17 起仓库 CI 只保留 `.github/workflows/build.yml`（发版）：tag 触发 /
//! 手动触发 → Windows + macOS 构建 → 上传产物 → 创建 GitHub Release。
//! 审计流水线（173 个判据）依赖私有文档 `docs/` 与 `_rea/` 样本，公开仓库与外部
//! 协作者无法执行，已移回本地 `node _rea/tools/audit-all.mjs`。
//!
//! 本判据保护发版工作流本身：触发条件、双平台构建、产物上传与 Release 发布、
//! 工具链固定、台账收录。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn workflow() -> String {
    let path = repo_root().join(".github/workflows/build.yml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn release_workflow_triggers_on_tags_and_manual_dispatch() {
    let w = workflow();
    for key in ["workflow_dispatch:", "push:", "tags:", "- \"v*\""] {
        assert!(
            w.contains(key),
            "build.yml 缺少触发条件 {key}（发版必须支持 tag 触发）"
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn release_workflow_builds_both_platforms_and_publishes_release() {
    let w = workflow();
    for key in [
        "runs-on: windows-latest",
        "runs-on: macos-latest",
        "npm run tauri:build",
        "softprops/action-gh-release@v2",
        "scripts/latest-json.mjs",
        "contents: write",
    ] {
        assert!(w.contains(key), "build.yml 缺少发版关键步骤 {key}");
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn release_workflow_uploads_artifacts_and_pins_toolchain() {
    let w = workflow();
    for key in [
        "actions/upload-artifact@v4",
        "actions/download-artifact@v4",
        "dtolnay/rust-toolchain@stable",
        "pnpm/action-setup@v4",
        "actions/setup-node@v4",
        "node-version:",
    ] {
        assert!(w.contains(key), "build.yml 缺少产物上传/工具链固定 {key}");
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn release_workflow_is_tracked_by_audit_ledgers() {
    let root = repo_root();
    let index = fs::read_to_string(root.join("docs/审计报告/审计索引.md")).expect("读取审计索引失败");
    assert!(
        index.contains(".github/workflows/build.yml"),
        "docs/审计报告/审计索引.md 必须收录 .github/workflows/build.yml（否则删弱发版工作流不会被台账发现）"
    );
    let snapshot = fs::read_to_string(root.join("docs/审计报告/审计快照.json")).expect("读取审计快照失败");
    assert!(
        snapshot.contains(".github/workflows/build.yml"),
        "docs/审计报告/审计快照.json 必须收录 .github/workflows/build.yml（归档应包含发版配置）"
    );
}
