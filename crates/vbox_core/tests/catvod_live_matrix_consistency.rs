//! 真站回归矩阵（docs/审计报告/真站回归矩阵.md）与代码现状的一致性不变量。
//!
//! 矩阵是「37 个原生家族 × 是否有 #[ignore] 真站用例」的静态对账，是兼容性证据链的一部分：
//! 它一旦与 `families/mod.rs` 或测试文件脱节，就会让「哪些家族有真站验证」的结论失真。
//! 这里用 Rust 侧独立复核三件事：
//!   1. 矩阵覆盖的家族集合 == `families/mod.rs` 声明的家族集合；
//!   2. 每行的「真站用例」数 == 该家族测试文件中 `#[ignore]` 的出现次数；
//!   3. live-verified / mock-only 的归类与计数自洽（总和 = 家族数，无 no-native-api 行）。
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/vbox_core 应位于仓库根下")
        .to_path_buf()
}

fn declared_families() -> Vec<String> {
    let src = fs::read_to_string(
        repo_root().join("crates/vbox_core/src/spider/catvod/families/mod.rs"),
    )
    .expect("读 families/mod.rs");
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("pub mod ") {
            if let Some(name) = rest.strip_suffix(';') {
                out.push(name.trim().to_string());
            }
        }
    }
    out
}

fn live_count(family: &str) -> usize {
    let path = repo_root().join(format!("crates/vbox_core/tests/catvod_{family}_family.rs"));
    match fs::read_to_string(path) {
        Ok(src) => src
            .lines()
            .filter(|l| l.trim_start().starts_with("#[ignore"))
            .count(),
        Err(_) => 0,
    }
}

struct Row {
    family: String,
    live: usize,
    status: String,
}

fn matrix_rows() -> Vec<Row> {
    let text = fs::read_to_string(repo_root().join("docs/审计报告/真站回归矩阵.md"))
        .expect("读 docs/审计报告/真站回归矩阵.md（先运行 node _rea/tools/live-coverage.mjs --write）");
    let mut rows = Vec::new();
    for line in text.lines() {
        // 数据行形如：| `alist` | csp_Alist | 1 | 3 | live-verified | … |
        if !line.starts_with("| `") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 6 {
            continue;
        }
        let family = cells[1].trim().trim_matches('`').to_string();
        let live: usize = cells[4].trim().parse().unwrap_or_else(|_| {
            panic!("矩阵行 `{family}` 的真站用例列不是数字：{:?}", cells[4]);
        });
        rows.push(Row {
            family,
            live,
            status: cells[5].trim().to_string(),
        });
    }
    rows
}

#[test]
fn matrix_covers_exactly_declared_families() {
    let rows = matrix_rows();
    let declared = declared_families();
    let doc: Vec<String> = rows.iter().map(|r| r.family.clone()).collect();
    let mut doc_sorted = doc.clone();
    doc_sorted.sort();
    doc_sorted.dedup();
    let mut declared_sorted = declared.clone();
    declared_sorted.sort();
    assert_eq!(
        doc_sorted.len(),
        doc.len(),
        "矩阵存在重复家族行：{doc:?}"
    );
    assert_eq!(
        doc_sorted, declared_sorted,
        "矩阵家族集合与 families/mod.rs 声明不一致（doc={} / declared={}）",
        doc_sorted.len(),
        declared_sorted.len()
    );
}

#[test]
fn matrix_live_counts_match_test_files() {
    let rows = matrix_rows();
    let mut mismatched = Vec::new();
    for r in &rows {
        let actual = live_count(&r.family);
        if r.live != actual {
            mismatched.push(format!("{}: 矩阵={} 实际={}", r.family, r.live, actual));
        }
    }
    assert!(
        mismatched.is_empty(),
        "矩阵的真站用例数过期（重新运行 node _rea/tools/live-coverage.mjs --write）：\n{}",
        mismatched.join("\n")
    );
}

#[test]
fn matrix_status_counts_are_consistent() {
    let rows = matrix_rows();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows {
        *counts.entry(r.status.as_str()).or_default() += 1;
        match r.status.as_str() {
            "live-verified" => assert!(r.live > 0, "`{}` 标记 live-verified 但用例数为 0", r.family),
            "mock-only" => assert_eq!(r.live, 0, "`{}` 标记 mock-only 但含 {} 个 #[ignore]", r.family, r.live),
            "no-native-api" => panic!("`{}` 被标记 no-native-api（家族应至少接管一个 native api）", r.family),
            other => panic!("`{}` 的状态 `{other}` 非预期", r.family),
        }
    }
    let total: usize = counts.values().sum();
    assert_eq!(total, rows.len());
    assert_eq!(counts.get("live-verified").copied().unwrap_or(0) + counts.get("mock-only").copied().unwrap_or(0), total);
    assert!(counts.get("live-verified").copied().unwrap_or(0) >= 10, "有真站用例的家族不应少于 10 个：{counts:?}");
}
