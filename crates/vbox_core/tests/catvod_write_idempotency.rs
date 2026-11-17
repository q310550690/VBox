//! 判据 32：离线 `--write` 工具必须幂等。
//!
//! 逐个工具连续执行两次，剥离「生成时间 / generatedAt」等时间戳行后比对字节：
//! 不一致说明输出含顺序随机性（集合迭代 / 目录枚举顺序）或隐藏状态，会让审计漂移。
//!
//! 只覆盖离线工具；`upstream-audit` / `blocked-recheck` 依赖网络、`compat-audit`
//! 每次带新时间戳，且由 audit-all 的 `--check`/`--verify` 路径覆盖，不在此列。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn strip_time(content: &str) -> String {
    content
        .lines()
        .filter(|l| {
            !l.contains("生成时间")
                && !l.contains("generatedAt")
                && !l.contains("\"gitHead\"")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn offline_write_tools_are_idempotent() {
    let node = Command::new("node").arg("--version").output();
    if node.is_err() {
        eprintln!("跳过：当前环境没有 node");
        return;
    }
    let tools = [
        "_rea/tools/site-coverage.mjs",
        "_rea/tools/live-coverage.mjs",
        "_rea/tools/audit-index.mjs",
        "_rea/tools/audit-manifest.mjs",
    ];
    for tool in tools {
        let out = Command::new("node")
            .arg(tool)
            .arg("--write")
            .current_dir(root())
            .output()
            .expect("运行工具");
        assert!(
            out.status.success(),
            "{} --write 失败：{}",
            tool,
            String::from_utf8_lossy(&out.stderr)
        );
        let first = match tool {
            "_rea/tools/site-coverage.mjs" => "docs/站点覆盖报告.md",
            "_rea/tools/live-coverage.mjs" => "docs/真站回归矩阵.md",
            "_rea/tools/audit-index.mjs" => "docs/审计索引.md",
            "_rea/tools/audit-manifest.mjs" => "docs/审计快照.json",
            _ => unreachable!(),
        };
        let a = strip_time(&fs::read_to_string(root().join(first)).expect("读取产物"));

        let out2 = Command::new("node")
            .arg(tool)
            .arg("--write")
            .current_dir(root())
            .output()
            .expect("运行工具");
        assert!(out2.status.success(), "{} 第二次 --write 失败", tool);
        let b = strip_time(&fs::read_to_string(root().join(first)).expect("读取产物"));
        assert_eq!(a, b, "{tool} 两次 --write 产物不一致（非幂等）");
    }
}
