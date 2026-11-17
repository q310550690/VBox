//! 判据 32：离线 `--write` 工具必须幂等。
//!
//! 逐个工具连续执行两次，剥离「生成时间 / generatedAt」等时间戳行后比对字节：
//! 不一致说明输出含顺序随机性（集合迭代 / 目录枚举顺序）或隐藏状态，会让审计漂移。
//!
//! 只覆盖离线工具；`upstream-audit` / `blocked-recheck` 依赖网络、`compat-audit`
//! 每次带新时间戳，且由 audit-all 的 `--check`/`--verify` 路径覆盖，不在此列。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// 把 docs 树复制到 `target/tmp/docs-idem` 再写，避免测试进程直接改真实文档。
/// 工具通过 `VBOX_DOCS_ROOT` 指向该副本；这与真实布局走同一套代码路径。
fn docs_copy() -> PathBuf {
    let src = root().join("docs");
    let dst = root().join("target").join("tmp").join("docs-idem");
    let _ = fs::remove_dir_all(&dst);
    copy_tree(&src, &dst);
    dst
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("创建 docs 副本目录");
    for entry in fs::read_dir(src).expect("读取 docs 目录") {
        let entry = entry.expect("读取 docs 条目");
        if entry.file_name() == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("docs 条目类型").is_dir() {
            copy_tree(&from, &to);
        } else {
            fs::copy(&from, &to).expect("复制 docs 文件");
        }
    }
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
    let docs = docs_copy();
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
            .env("VBOX_DOCS_ROOT", &docs)
            .output()
            .expect("运行工具");
        assert!(
            out.status.success(),
            "{} --write 失败：{}",
            tool,
            String::from_utf8_lossy(&out.stderr)
        );
        let first = match tool {
            "_rea/tools/site-coverage.mjs" => "审计报告/站点覆盖报告.md",
            "_rea/tools/live-coverage.mjs" => "审计报告/真站回归矩阵.md",
            "_rea/tools/audit-index.mjs" => "审计报告/审计索引.md",
            "_rea/tools/audit-manifest.mjs" => "审计报告/审计快照.json",
            _ => unreachable!(),
        };
        let a = strip_time(&fs::read_to_string(docs.join(first)).expect("读取产物"));

        let out2 = Command::new("node")
            .arg(tool)
            .arg("--write")
            .current_dir(root())
            .env("VBOX_DOCS_ROOT", &docs)
            .output()
            .expect("运行工具");
        assert!(out2.status.success(), "{} 第二次 --write 失败", tool);
        let b = strip_time(&fs::read_to_string(docs.join(first)).expect("读取产物"));
        assert_eq!(a, b, "{tool} 两次 --write 产物不一致（非幂等）");
    }
}
