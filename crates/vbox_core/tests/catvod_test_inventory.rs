//! 判据 52：计划文档「测试目标索引」↔ `crates/vbox_core/tests/` 磁盘 ↔ 全量日志 三方对账。
//!
//! 判据 51 锁住了「计划头行 ↔ 全量日志」，但测试文件的**用途与用例数**此前没有任何
//! 机器约束：新增一个测试文件只要头行数字同步就能过，读者却不知道每个文件在验证什么。
//! 本判据要求计划文档 §170 维护一份逐文件索引（文件名 / 用例数 / ignored 数 / 用途），
//! 并把它与磁盘内容、全量日志做双向核对：
//! - 索引行集合 == `tests/*.rs` 集合（不漏、不虚列）；
//! - 每行用例数/ignored 数 == 文件内 `#[test]` / `#[ignore` 行数；
//! - 索引汇总 == 全量日志的集成测试部分（lib 首块之外）。
//!
//! CI 环境没有 `_rea/`：日志缺失时第二项软跳过，第一项仍然强制。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

struct Row {
    file: String,
    tests: usize,
    ignored: usize,
    purpose_len: usize,
}

fn section(text: &str, heading: &str) -> String {
    // 注意：不能用 text.split("## 170.")，因为子标题 "### 170.1" 同样包含 "## 170." 子串，
    // 会把章节截断在第一个子标题之前（首版即此 bug，导致解析出 0 行）。
    let marker = format!("\n{heading} ");
    let start = text
        .find(&marker)
        .unwrap_or_else(|| panic!("缺少章节 {heading}"))
        + marker.len();
    let rest = &text[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn parse_rows(sec: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in sec.lines() {
        let t = line.trim();
        if !t.starts_with("| `") || !t.contains(".rs`") {
            continue;
        }
        let cells: Vec<&str> = t.split('|').map(str::trim).collect();
        if cells.len() < 6 {
            continue;
        }
        let file = cells[1].trim_matches('`').trim().to_string();
        let tests = cells[2].parse::<usize>().unwrap_or_else(|_| {
            panic!("测试目标索引行 {file} 的用例数不是数字：{:?}", cells[2])
        });
        let ignored = cells[3].parse::<usize>().unwrap_or_else(|_| {
            panic!("测试目标索引行 {file} 的 ignored 数不是数字：{:?}", cells[3])
        });
        rows.push(Row {
            file,
            tests,
            ignored,
            purpose_len: cells[4].chars().count(),
        });
    }
    rows
}

fn count_lines(text: &str, prefix: &str) -> usize {
    text.lines().filter(|l| l.trim_start().starts_with(prefix)).count()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn test_inventory_matches_disk() {
    let plan = fs::read_to_string(root().join("docs/type3兼容/type3-实施计划-原生兼容100%.md"))
        .expect("读取计划文档失败");
    let rows = parse_rows(&section(&plan, "## 170."));
    let dir = root().join("crates/vbox_core/tests");
    let files: BTreeSet<String> = fs::read_dir(&dir)
        .expect("读取 tests 目录失败")
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "rs") && p.is_file())
                .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    assert_eq!(
        rows.len(),
        files.len(),
        "测试目标索引 {} 行 != tests 目录 {} 个 .rs 文件",
        rows.len(),
        files.len()
    );
    let listed: BTreeSet<String> = rows.iter().map(|r| r.file.clone()).collect();
    let missing: Vec<String> = files.difference(&listed).cloned().collect();
    let extra: Vec<String> = listed.difference(&files).cloned().collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "索引与磁盘不一致：缺 {missing:?}，多 {extra:?}"
    );
    for row in &rows {
        assert!(
            row.purpose_len >= 6,
            "{} 的用途描述过短（<6 字符），索引失去可读性",
            row.file
        );
        let text = fs::read_to_string(dir.join(&row.file))
            .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", row.file));
        let actual_tests = count_lines(&text, "#[test]");
        let actual_ignored = count_lines(&text, "#[ignore");
        assert_eq!(
            row.tests, actual_tests,
            "{} 索引用例数 {} != 文件内 #[test] 行数 {actual_tests}",
            row.file, row.tests
        );
        assert_eq!(
            row.ignored, actual_ignored,
            "{} 索引 ignored 数 {} != 文件内 #[ignore 行数 {actual_ignored}",
            row.file, row.ignored
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn test_inventory_reconciles_with_full_log() {
    if std::env::var("FULL_BASELINE_BOOTSTRAP").is_ok() {
        eprintln!("bootstrap: 跳过 test_inventory_reconciles_with_full_log（本轮只刷新日志）");
        return;
    }
    let log_path = root().join("_rea/tmp/audit-full.log");
    let log = match fs::read_to_string(&log_path) {
        Ok(t) => t,
        Err(e) => {
            if std::env::var("CI").is_ok() {
                eprintln!("skip: {} 在 CI 中不可用：{e}", log_path.display());
                return;
            }
            panic!("读取 {} 失败：{e}", log_path.display());
        }
    };
    let plan = fs::read_to_string(root().join("docs/type3兼容/type3-实施计划-原生兼容100%.md"))
        .expect("读取计划文档失败");
    let rows = parse_rows(&section(&plan, "## 170."));
    let sum_tests: usize = rows.iter().map(|r| r.tests).sum();
    let sum_ignored: usize = rows.iter().map(|r| r.ignored).sum();

    let mut blocks: Vec<(usize, usize, usize)> = Vec::new();
    for line in log.lines() {
        let Some(rest) = line.strip_prefix("test result:") else {
            continue;
        };
        let toks: Vec<&str> = rest.split_whitespace().collect();
        let (mut p, mut f, mut i) = (0usize, 0usize, 0usize);
        for (idx, raw) in toks.iter().enumerate() {
            let tok = raw.trim_end_matches(';');
            if matches!(tok, "passed" | "failed" | "ignored") {
                if let Some(n) = idx.checked_sub(1).and_then(|j| toks[j].parse::<usize>().ok()) {
                    match tok {
                        "passed" => p = n,
                        "failed" => f = n,
                        _ => i = n,
                    }
                }
            }
        }
        blocks.push((p, f, i));
    }
    assert!(!blocks.is_empty(), "全量日志没有 test result 行");
    let total: (usize, usize, usize) = blocks.iter().fold((0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
    let (lib_passed, lib_failed, lib_ignored) = blocks[0];
    assert_eq!(lib_failed, 0, "lib 单测存在失败");
    assert_eq!(total.1, 0, "全量日志存在失败项");
    assert_eq!(
        sum_tests - sum_ignored,
        total.0 - lib_passed,
        "索引集成用例通过数 {} != 日志集成通过数 {}",
        sum_tests - sum_ignored,
        total.0 - lib_passed
    );
    assert_eq!(
        sum_ignored,
        total.2 - lib_ignored,
        "索引 ignored 总数 {sum_ignored} != 日志 ignored 总数 {}",
        total.2 - lib_ignored
    );
}
