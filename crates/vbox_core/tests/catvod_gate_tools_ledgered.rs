//! 判据 34：所有门禁工具必须同时进入「审计索引」与「审计快照」。
//!
//! `audit-index --check` 只能证明「索引与它收录的文件一致」；如果某个门禁工具
//! 从未被收录，改它不会让索引失效。本判据用**固定清单**堵住这个盲区：
//! 门禁工具集合 → 逐项断言出现在 `docs/审计报告/审计索引.md` 表格路径列与
//! `docs/审计报告/审计快照.json` 的 entries 中。

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 门禁链上缺一不可的工具与产物（新增门禁工具时同步）
const GATE: &[&str] = &[
    "_rea/tools/audit-all.mjs",
    "_rea/tools/audit-index.mjs",
    "_rea/tools/audit-manifest.mjs",
    "_rea/tools/audit-selftest.mjs",
    "_rea/tools/compat-audit.mjs",
    "_rea/tools/live-audit-report.mjs",
    "_rea/tools/lib/live-audit-log.mjs",
    "_rea/tools/live-coverage.mjs",
    "_rea/tools/site-coverage.mjs",
    "_rea/tools/upstream-audit.mjs",
    "_rea/tools/blocked-recheck.mjs",
    "_rea/tools/inventory-drift.mjs",
    "docs/审计报告/订阅漂移复核.md",
    "_rea/samples/inventory/gen-profiles.mjs",
    "_rea/samples/inventory/crawl-sites.mjs",
    ".github/workflows/compat-audit.yml",
    "crates/vbox_core/src/spider/catvod/profiles.json",
    "_rea/samples/inventory/sites.json",
];

#[test]
fn every_gate_tool_is_ledgered() {
    let index = fs::read_to_string(root().join("docs/审计报告/审计索引.md")).expect("读取审计索引");
    let snapshot = fs::read_to_string(root().join("docs/审计报告/审计快照.json")).expect("读取审计快照");
    let mut missing_index = Vec::new();
    let mut missing_snapshot = Vec::new();
    for tool in GATE {
        let quoted = format!("`{tool}`");
        if !index.contains(&quoted) {
            missing_index.push(*tool);
        }
        let json_quoted = format!("\"{tool}\"");
        if !snapshot.contains(&json_quoted) {
            missing_snapshot.push(*tool);
        }
        assert!(
            root().join(tool).exists(),
            "门禁清单登记了不存在的文件：{tool}"
        );
    }
    assert!(
        missing_index.is_empty(),
        "以下门禁工具未进入 docs/审计报告/审计索引.md：{missing_index:?}"
    );
    assert!(
        missing_snapshot.is_empty(),
        "以下门禁工具未进入 docs/审计报告/审计快照.json：{missing_snapshot:?}"
    );
}
