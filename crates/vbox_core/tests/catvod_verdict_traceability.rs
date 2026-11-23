//! 判据 26：审计报告「判定五条」的每条结论都必须能追溯到具体回归测试。
//!
//! 「100% 判定五条」是审计报告的结论骨架。此前第 2/3/5 条只在文字里提到测试，
//! 第 1/4 条完全没有测试引用——读者无法从报告跳到「是哪个测试在保证它」。
//! 本判据要求：五条中每一行的「现状」列至少出现一个反引号包裹的测试名，
//! 且 `crates/vbox_core/tests/<测试名>.rs` 真实存在。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn report() -> String {
    let path = repo_root().join("docs/审计报告/兼容性审计.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

/// 从「## 一、判定五条」章节中取出五行的「现状」单元格。
fn verdict_rows(text: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_section = line.contains("判定五条");
            continue;
        }
        if !in_section || !line.starts_with("| ") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // cells[0] 为空（行首 |），cells[1]=编号、cells[2]=判据、cells[3]=现状、cells[4]=结论
        if cells.len() >= 6 && cells[1].chars().all(|c| c.is_ascii_digit()) && !cells[1].is_empty() {
            rows.push(cells[3].to_string());
        }
    }
    rows
}

/// 现状列里反引号包裹、且形如 `<test>.rs` 对应测试文件的标识符。
fn test_refs(cell: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = cell.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(end) = cell[i + 1..].find('`') {
                let token = &cell[i + 1..i + 1 + end];
                if token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    && (token.starts_with("catvod_")
                        || token == "compat_criteria"
                        || token == "sources_error_boundary")
                {
                    out.insert(token.to_string());
                }
                i = i + 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn verdict_rows_each_reference_existing_tests() {
    let rows = verdict_rows(&report());
    assert_eq!(rows.len(), 5, "「判定五条」应有且仅有 5 行，实际 {}", rows.len());

    let test_dir = repo_root().join("crates/vbox_core/tests");
    let mut missing_refs = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let refs = test_refs(row);
        assert!(
            !refs.is_empty(),
            "判定第 {} 条的「现状」没有引用任何具体测试（应至少有 `catvod_*`/`compat_criteria`/`sources_error_boundary` 之一）",
            idx + 1
        );
        for name in refs {
            let path = test_dir.join(format!("{name}.rs"));
            if !path.exists() {
                missing_refs.push(format!("第 {} 条 → {name}", idx + 1));
            }
        }
    }
    assert!(
        missing_refs.is_empty(),
        "判定条目引用了不存在的测试文件：{}",
        missing_refs.join("、")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn verdict_conclusions_are_marked() {
    let text = report();
    let mut in_section = false;
    let mut count = 0;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_section = line.contains("判定五条");
            continue;
        }
        if !in_section || !line.starts_with("| ") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() >= 6 && cells[1].chars().all(|c| c.is_ascii_digit()) && !cells[1].is_empty() {
            count += 1;
            assert!(
                cells[4].contains('✅') || cells[4].contains('❌') || cells[4].contains('⚠'),
                "判定第 {} 条的结论未标注状态：{}",
                cells[1],
                cells[4]
            );
        }
    }
    assert_eq!(count, 5, "「判定五条」应恰有 5 行带状态结论");
}
