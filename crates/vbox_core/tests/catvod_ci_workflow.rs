//! 判据 20：CI 工作流必须真的执行审计门禁与自检。
//!
//! 第 95 轮新增 `.github/workflows/compat-audit.yml`，但「工作流里写了什么」此前没有任何测试保护：
//! 删掉 `--selftest`、把 `audit-all` 换成别的命令、去掉产物上传，都不会被现有判据发现。
//! 本测试从仓库根读取工作流文本，断言关键命令与结构存在。

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
    let path = repo_root().join(".github/workflows/compat-audit.yml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

#[test]
fn workflow_runs_audit_gate_and_selftest() {
    let w = workflow();
    assert!(
        w.contains("node _rea/tools/audit-all.mjs --skip-live --selftest"),
        "compat-audit.yml 必须执行 `audit-all.mjs --skip-live --selftest`（10 类门禁注入自检）"
    );
}

#[test]
fn workflow_runs_core_tests_and_checks_tool_syntax() {
    let w = workflow();
    assert!(w.contains("cargo test -p vbox_core"), "compat-audit.yml 必须运行 `cargo test -p vbox_core`");
    assert!(
        w.contains("node --check") && w.contains("_rea/tools/*.mjs") && w.contains("_rea/tools/lib/*.mjs"),
        "compat-audit.yml 必须对全部审计工具与共享库做 `node --check`"
    );
}

#[test]
fn workflow_triggers_and_uploads_artifacts() {
    let w = workflow();
    for key in ["workflow_dispatch:", "push:", "pull_request:"] {
        assert!(w.contains(key), "compat-audit.yml 缺少触发条件 {key}");
    }
    assert!(
        w.contains("actions/upload-artifact@v4"),
        "compat-audit.yml 必须上传审计产物（upload-artifact）"
    );
    assert!(
        w.contains("if: always()"),
        "compat-audit.yml 的产物上传必须 `if: always()`，失败时也能取现场"
    );
    for artifact in [
        "docs/审计报告/兼容性审计.md",
        "docs/审计报告/站点覆盖报告.md",
        "docs/审计报告/真站回归矩阵.md",
        "docs/审计报告/上游复核.md",
        "docs/审计报告/阻断复核.md",
        "docs/审计报告/审计索引.md",
        "docs/审计报告/审计快照.json",
    ] {
        assert!(w.contains(artifact), "compat-audit.yml 产物未包含 {artifact}");
    }
}

#[test]
fn workflow_pins_node_and_rust() {
    let w = workflow();
    assert!(w.contains("actions/setup-node@v4"), "compat-audit.yml 必须 setup-node");
    assert!(
        w.contains("node-version: ${{ env.NODE_VERSION }}") || w.contains("node-version:"),
        "compat-audit.yml 必须固定 Node 版本"
    );
    assert!(
        w.contains("dtolnay/rust-toolchain@stable"),
        "compat-audit.yml 必须安装 stable Rust 工具链"
    );
    assert!(w.contains("ubuntu-latest"), "compat-audit.yml 应运行于 ubuntu-latest");
}

#[test]
fn workflow_is_tracked_by_audit_ledgers() {
    // CI 工作流本身就是门禁配置：被误删/静默改弱时必须能被台账发现。
    let root = repo_root();
    let index = fs::read_to_string(root.join("docs/审计报告/审计索引.md")).expect("读取审计索引失败");
    assert!(
        index.contains(".github/workflows/compat-audit.yml"),
        "docs/审计报告/审计索引.md 必须收录 .github/workflows/compat-audit.yml（否则删除/改弱工作流不会被 index --check 发现）"
    );
    let snapshot = fs::read_to_string(root.join("docs/审计报告/审计快照.json")).expect("读取审计快照失败");
    assert!(
        snapshot.contains(".github/workflows/compat-audit.yml"),
        "docs/审计报告/审计快照.json 必须收录 .github/workflows/compat-audit.yml（归档应包含全部门禁配置）"
    );
}
