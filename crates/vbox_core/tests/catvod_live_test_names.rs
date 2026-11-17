//! 判据 14：真站矩阵的「每家族 N 个真站用例」必须与测试文件里**逐个 `#[ignore]` 函数**对得上。
//!
//! 第 80/81 轮只做了聚合计数对账（23 vs 23），无法发现「A 家族多 1 个、B 家族少 1 个」这类错位。
//! 本测试逐个家族按函数名统计，并校验矩阵状态（live-verified ⇔ 用例数 > 0）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 从测试文件里收集「独占一行的 `#[ignore]` 属性 + 紧随其后的 fn 名」。
fn ignored_test_names(path: &Path) -> Vec<String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut pending = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("#[ignore") {
            pending = true;
            continue;
        }
        if pending {
            if let Some(rest) = t.strip_prefix("fn ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push(name);
                }
            }
            if t.starts_with("fn ") {
                pending = false;
            }
        }
    }
    out
}

/// 解析矩阵：家族 → (真站用例数, 状态)；并返回声明的总数。
fn matrix_rows() -> (BTreeMap<String, (usize, String)>, usize) {
    let path = repo_root().join("docs/真站回归矩阵.md");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
    let mut rows = BTreeMap::new();
    let mut declared = None;
    for line in text.lines() {
        if let Some(rest) = line.split("家族测试文件中的 `#[ignore]` 用例数：**").nth(1) {
            if let Some(num) = rest.split("**").next() {
                declared = num.trim().parse::<usize>().ok();
            }
        }
        // | `alist` | csp_Alist | 1 | 1 | live-verified | … |
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 7 || !line.starts_with('|') {
            continue;
        }
        let name = cols[1].trim_matches('`');
        if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
            continue;
        }
        let (Ok(count), Ok(sites)) = (cols[4].parse::<usize>(), cols[3].parse::<usize>()) else {
            continue;
        };
        let status = cols[5];
        if status != "live-verified" && status != "mock-only" {
            continue;
        }
        let _ = sites;
        rows.insert(name.to_string(), (count, status.to_string()));
    }
    (rows, declared.unwrap_or(0))
}

#[test]
fn per_family_ignored_counts_match_matrix() {
    let (rows, declared) = matrix_rows();
    assert!(!rows.is_empty(), "矩阵中没有解析到家族行");
    assert!(declared > 0, "矩阵中没有声明真站用例总数");

    let tests_dir = repo_root().join("crates/vbox_core/tests");
    let mut total = 0usize;
    let mut problems = Vec::new();
    for (family, (count, status)) in &rows {
        let file = tests_dir.join(format!("catvod_{family}_family.rs"));
        let names = ignored_test_names(&file);
        total += names.len();
        if names.len() != *count {
            problems.push(format!(
                "{family}：矩阵 {count}，源码 {}（{file:?}）",
                names.len()
            ));
        }
        let status_ok = match status.as_str() {
            "live-verified" => !names.is_empty(),
            "mock-only" => names.is_empty(),
            _ => true,
        };
        if !status_ok {
            problems.push(format!("{family}：状态 {status} 与真站用例数 {} 不一致", names.len()));
        }
    }
    assert!(problems.is_empty(), "逐家族对账失败：\n  {}", problems.join("\n  "));
    assert_eq!(total, declared, "真站用例总数与矩阵声明不一致（{total} ≠ {declared}）");
}
