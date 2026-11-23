//! 判据 13：三处「仅 mock」证据链一致
//!
//! 真站回归矩阵（docs/审计报告/真站回归矩阵.md）标出哪些家族「仅 mock」；
//! 上游复核报告（docs/审计报告/上游复核.md）必须对每个这样的家族给出可核查的上游原因，
//! 且不得出现 alive（活着但没真站用例）或无法定性的 unknown-error。
//!
//! 这三个产物（矩阵 / 上游报告 / 审计第十节）由不同工具生成，本测试锁住它们的集合一致性，
//! 防止某一侧悄悄过期（例如新增 mock-only 家族却忘了跑上游复核）。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repo_docs() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/vbox_core
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("审计报告")
}

fn matrix_mock_only_families() -> BTreeSet<String> {
    let path = repo_docs().join("真站回归矩阵.md");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
    let mut out = BTreeSet::new();
    for line in text.lines() {
        // | `anime1` | csp_Anime1 | 4 | 0 | mock-only | 仅 mock（…） |
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 6 {
            continue;
        }
        let name = cols[1].trim_matches('`');
        let status = cols[5];
        if status == "mock-only" {
            out.insert(name.to_string());
        }
    }
    out
}

fn upstream_report_rows() -> Vec<(String, String)> {
    let path = repo_docs().join("上游复核.md");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
    let mut out = Vec::new();
    for line in text.lines() {
        // | `anime1` | `no-host` | — | 源码与 ext 中均无可用 host |
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 5 {
            continue;
        }
        let name = cols[1].trim_matches('`');
        let status = cols[2].trim_matches('`');
        if name.is_empty() || !status.starts_with(|c: char| c.is_ascii_lowercase()) || !line.starts_with('|') {
            continue;
        }
        if status == "---" || status == "含义" {
            continue;
        }
        out.push((name.to_string(), status.to_string()));
    }
    out
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn mock_only_sets_match_between_matrix_and_upstream_report() {
    let matrix = matrix_mock_only_families();
    let rows = upstream_report_rows();
    let upstream: BTreeSet<String> = rows.iter().map(|(f, _)| f.clone()).collect();
    assert!(!matrix.is_empty(), "矩阵中没有解析到 mock-only 家族（文件格式可能已变）");
    let missing: Vec<_> = matrix.difference(&upstream).cloned().collect();
    let extra: Vec<_> = upstream.difference(&matrix).cloned().collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "矩阵与上游复核的 mock-only 家族集合不一致：\n  缺（矩阵有、报告缺）：{missing:?}\n  多（报告有、矩阵无）：{extra:?}\n请运行 `node _rea/tools/upstream-audit.mjs --write`"
    );
}

fn manual_override_statuses(text: &str) -> std::collections::BTreeMap<String, String> {
    // - `libvio`（`suspended`）：…
    let mut out = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("- `") else { continue };
        let Some((family, tail)) = rest.split_once("`（`") else { continue };
        let Some((status, _)) = tail.split_once("`）") else { continue };
        out.insert(family.to_string(), status.to_string());
    }
    out
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn no_alive_or_unknown_gap_in_upstream_report() {
    let text = fs::read_to_string(repo_docs().join("上游复核.md")).unwrap();
    let rows = upstream_report_rows();
    assert!(!rows.is_empty(), "上游复核报告没有解析到任何家族行");
    let manual = manual_override_statuses(&text);

    // 逐 host 的 alive 只允许出现在「人工取证覆盖为非 alive」的家族里（如 pan：首页活但接口已漂移）。
    let bad_alive: Vec<_> = rows
        .iter()
        .filter(|(f, s)| s == "alive" && manual.get(f).map(|m| m == "alive").unwrap_or(true))
        .collect();
    assert!(
        bad_alive.is_empty(),
        "存在未被人工归因覆盖的 alive（活站但缺真站用例）：{bad_alive:?}"
    );
    let unknown: Vec<_> = rows.iter().filter(|(_, s)| s == "unknown-error").collect();
    assert!(
        unknown.is_empty(),
        "存在无法定性的 unknown-error：{unknown:?}（应归因为 dead / spa-shell / gated / blocked-tls）"
    );
    assert!(
        text.contains("缺口（alive 但无真站用例）：0"),
        "上游复核报告应声明 alive 缺口为 0"
    );
}
