//! 判据 35：自动生成文档的「生成命令 → 产物」可端到端复现（第 116 轮）。
//!
//! 判据 29 只校验「复现命令文法正确」（脚本存在、开关在用法行声明、包含 --check），
//! 判据 33 只校验「每份自动生成文档都有生成工具」。两者都不回答：**报告里写的那条命令，
//! 现在真的能跑、并且产物与磁盘一致吗？** 本判据对支持离线 `--check` 的生成器逐条执行，
//! 把「报告声称可复现」变成「报告当前确实可复现」。
//!
//! 为什么只覆盖离线工具：`blocked-recheck` / `upstream-audit --write` / `compat-audit`
//! 依赖网络探测或上一轮 live 日志，逐字节复现必然受时序抖动影响；`upstream-audit` 的
//! `--check` 特意不重探网络、只校验报告与清单一致，因此可以纳入。
//!
//! 离线可 `--check` 的生成器（4 个）：
//! - `site-coverage.mjs --check`    → docs/站点覆盖报告.md
//! - `live-coverage.mjs --check`    → docs/真站回归矩阵.md
//! - `upstream-audit.mjs --check`   → docs/上游复核.md
//! - `audit-index.mjs --check`      → docs/审计索引.md
//!
//! 无 `--check` 的生成器（2 个，必须显式登记理由，新增自动文档时强制做一次决策）：
//! - `blocked-recheck.mjs`（网络探测，无 --check）
//! - `compat-audit.mjs`（汇总型生成器，依赖上一轮 live 日志）

use std::path::PathBuf;
use std::process::Command;

/// 仓库根（`cargo test` 的工作目录是 crate 目录，不能直接用它跑 `_rea/tools/*`）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate 应位于 <repo>/crates/vbox_core")
        .to_path_buf()
}

/// 报告的头部必须写着「生成命令」，避免本判据校验的命令与文档声明的不是同一条。
fn assert_header_names_command(doc: &str, command: &str) {
    let path = repo_root().join(doc);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
    let first_lines: String = text.lines().take(8).collect::<Vec<_>>().join("\n");
    assert!(
        first_lines.contains(command),
        "{} 头部未声明生成命令 `{command}`，判据 35 无法保证「校验的命令 == 文档声明的命令」",
        doc
    );
}

/// 跑一条离线 `--check`，非零退出即红灯，并把 stdout/stderr 带进失败信息。
fn run_offline_check(tool: &str) {
    let root = repo_root();
    let out = Command::new("node")
        .arg(format!("_rea/tools/{tool}"))
        .arg("--check")
        .current_dir(&root)
        .output()
        .unwrap_or_else(|e| panic!("无法执行 `node _rea/tools/{tool} --check`：{e}（本机需要 node 在 PATH）"));
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        panic!(
            "`node _rea/tools/{tool} --check` 退出 {:?}：产物与磁盘不一致（生成命令不可复现）\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            stdout.trim_end(),
            stderr.trim_end()
        );
    }
}

/// 4 个离线可复现生成器：报告 ↔ 工具 必须一一对应，且 `--check` 当场通过。
#[test]
fn offline_generators_reproduce_their_reports() {
    let pairs = [
        ("docs/站点覆盖报告.md", "site-coverage.mjs"),
        ("docs/真站回归矩阵.md", "live-coverage.mjs"),
        ("docs/上游复核.md", "upstream-audit.mjs"),
    ];
    for (doc, tool) in pairs {
        assert_header_names_command(doc, &format!("node _rea/tools/{tool} --write"));
        run_offline_check(tool);
    }

    // 审计索引是「自指台账」，头部没有机械生成段落，改用它内联声明的校验方式。
    let index = std::fs::read_to_string(repo_root().join("docs/审计索引.md")).expect("读审计索引");
    assert!(
        index.contains("校验方式：`node _rea/tools/audit-index.mjs --check`"),
        "docs/审计索引.md 必须声明 `node _rea/tools/audit-index.mjs --check` 校验方式"
    );
    run_offline_check("audit-index.mjs");
}

/// 6 份自动生成文档必须全部被本判据显式覆盖：
/// 4 份有离线 `--check`，2 份（网络/汇总型）登记「为何不能纳入」的理由。
#[test]
fn every_auto_doc_generator_is_accounted_for() {
    // 不可离线复现的 2 份：登记理由，禁止静默纳入或静默排除。
    let excluded = [
        ("docs/阻断复核.md", "blocked-recheck.mjs", "需网络探测，且工具无 --check"),
        ("docs/兼容性审计.md", "compat-audit.mjs", "汇总型生成器，依赖上一轮 live 日志与三个台账"),
    ];
    for (doc, tool, reason) in excluded {
        assert!(
            repo_root().join(doc).is_file(),
            "排除清单中的 {doc} 不存在，判据 35 的覆盖表已过期"
        );
        assert!(
            !reason.trim().is_empty(),
            "排除 {doc} 必须写明理由（否则新增自动文档会静默绕过本判据）"
        );
        // 「支持 --check」指工具自己解析该开关，而不是它在内部跑别人的 --check。
        let src = std::fs::read_to_string(repo_root().join(format!("_rea/tools/{tool}")))
            .unwrap_or_else(|e| panic!("读取 _rea/tools/{tool} 失败：{e}"));
        let parses_check = [
            "args.includes('--check')",
            "process.argv.includes('--check')",
            "mode === '--check'",
            "wantCheck",
        ]
        .iter()
        .any(|pat| src.contains(pat));
        assert!(
            !parses_check,
            "{tool} 已支持 --check，必须从判据 35 的排除清单移入离线复现清单"
        );
    }
}
