//! 判据 17：审计索引与审计快照在 Rust 测试侧也可校验（不只在 audit-all 里执行）。
//!
//! `_rea/tools/audit-index.mjs --check` 校验「门禁台账」的 sha256 与磁盘一致；
//! `_rea/tools/audit-manifest.mjs --verify` 校验「归档快照」的 sha256/缺失/新增。
//! 把它们接入 cargo test 后，直接跑测试（不跑 audit-all）也能发现产物漂移。

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn run_node(args: &[&str]) -> std::process::Output {
    Command::new("node")
        .args(args)
        .current_dir(root())
        .output()
        .unwrap_or_else(|e| panic!("无法启动 node（判据 17 需要 Node；请确认 PATH）：{e}"))
}

fn assert_ok(label: &str, out: std::process::Output) {
    if !out.status.success() {
        panic!(
            "{label} 失败（exit={:?}）\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn audit_index_and_manifest_verify_from_rust() {
    assert_ok(
        "audit-index --check",
        run_node(&["_rea/tools/audit-index.mjs", "--check"]),
    );
    assert_ok(
        "audit-manifest --verify",
        run_node(&["_rea/tools/audit-manifest.mjs", "--verify"]),
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn audit_snapshot_shape_is_sane() {
    let path = root().join("docs/审计报告/审计快照.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
    let v: Value = serde_json::from_str(&text).expect("审计快照不是合法 JSON");
    let files = v["entries"]
        .as_array()
        .expect("审计快照缺少 entries 数组（结构应为 schema/generatedAt/gitHead/totals/entries/missing）");
    assert!(files.len() >= 15, "审计快照文件数过少（{}），可能被误裁剪", files.len());
    // 每一条都必须有 path/bytes/lines/sha256，且 sha256 为 64 位 hex
    for f in files {
        let p = f["path"].as_str().unwrap_or_else(|| panic!("快照条目缺少 path：{f}"));
        let sha = f["sha256"].as_str().unwrap_or_else(|| panic!("快照条目 {p} 缺少 sha256"));
        assert_eq!(sha.len(), 64, "快照条目 {p} 的 sha256 长度不是 64");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "快照条目 {p} 的 sha256 含非 hex 字符");
        assert!(f["bytes"].as_u64().is_some(), "快照条目 {p} 缺少 bytes");
        assert!(f["lines"].as_u64().is_some(), "快照条目 {p} 缺少 lines");
    }
    let totals = &v["totals"];
    assert_eq!(
        totals["files"].as_u64().unwrap_or(0) as usize,
        files.len(),
        "快照 totals.files 与 entries 实际条目数不一致"
    );
}
