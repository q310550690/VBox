//! 判据 48：真站回归矩阵的「用例名清单」必须与测试源码逐个 `#[ignore]` 函数**按名**一致。
//!
//! 判据 14（catvod_live_test_names）只按家族对账**数量**：`A 家族多 1 个、B 家族少 1 个`
//! 这类错位看不出来。本判据要求矩阵说明列列出每个 `#[ignore]` 函数名，并与源码逐个比对：
//! - 家族行：名字集合相等、数量列 == 名字数、状态语义一致（live-verified ⇔ 非空）；
//! - 全局：矩阵声明的家族用例总数 == 名字总数；非家族真站用例与库内 live 用例的
//!   数量 / 文件清单必须与磁盘实扫一致（防「新增真站用例但矩阵没更新」的静默漂移）。
//!
//! 为什么写在说明列而不是新增表格列：矩阵已有 6 列且下游判据按列位解析（判据 14/42/45），
//! 新增第 7 列会破坏既有解析；说明列是自由文本，放名字最安全。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 从测试源码里收集「独占一行 `#[ignore]` 属性 + 紧随其后的 fn 名」。
fn ignored_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = false;
    for line in src.lines() {
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
                pending = false;
            } else if t.starts_with("fn ") {
                pending = false;
            }
        }
    }
    out
}

/// 递归收集目录下所有 .rs 的 `#[ignore]` 用例名（路径, 名字列表）。
fn walk_ignored(dir: &Path, out: &mut Vec<(String, Vec<String>)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_ignored(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(src) = fs::read_to_string(&path) else { continue };
            let names = ignored_names(&src);
            if !names.is_empty() {
                let rel = path
                    .strip_prefix(repo_root())
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, names));
            }
        }
    }
}

struct Row {
    family: String,
    count: usize,
    status: String,
    names: Vec<String>,
}

fn matrix_rows(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if !line.starts_with("| `") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 7 {
            continue;
        }
        let family = cols[1].trim_matches('`').to_string();
        let (Ok(count), true) = (cols[4].parse::<usize>(), true) else {
            continue;
        };
        let status = cols[5].to_string();
        if status != "live-verified" && status != "mock-only" && status != "no-native-api" {
            continue;
        }
        // 说明列里的反引号 token；`#[ignore]` 以 # 开头天然被过滤。
        let mut names = Vec::new();
        let mut rest = cols[6];
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else { break };
            let token = &after[..end];
            if token
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase())
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                names.push(token.to_string());
            }
            rest = &after[end + 1..];
        }
        rows.push(Row { family, count, status, names });
    }
    rows
}

fn read_matrix() -> String {
    let path = repo_root().join("docs/审计报告/真站回归矩阵.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()))
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn matrix_lists_every_ignored_test_by_name() {
    let root = repo_root();
    let text = read_matrix();
    let rows = matrix_rows(&text);
    assert!(rows.len() >= 60, "矩阵家族行过少：{}", rows.len());

    let mut total_names = 0usize;
    let mut counts = BTreeMap::<String, usize>::new();
    for row in &rows {
        counts.entry(row.status.clone()).and_modify(|n| *n += 1).or_insert(1);
        let file = root.join(format!(
            "crates/vbox_core/tests/catvod_{}_family.rs",
            row.family
        ));
        let src = fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", file.display()));
        let disk = ignored_names(&src);
        let mut a = disk.clone();
        let mut b = row.names.clone();
        a.sort();
        b.sort();
        assert_eq!(a, b, "家族 `{}` 的矩阵用例名与源码不一致", row.family);
        assert_eq!(
            row.count,
            row.names.len(),
            "家族 `{}` 的数量列 {} != 名字数 {}",
            row.family,
            row.count,
            row.names.len()
        );
        match row.status.as_str() {
            "live-verified" => assert!(!row.names.is_empty(), "`{}` live-verified 但无名字", row.family),
            _ => assert!(
                row.names.is_empty(),
                "`{}` 状态 {} 却列出了用例名",
                row.family,
                row.status
            ),
        }
        total_names += row.names.len();
    }
    assert!(
        counts.get("live-verified").copied().unwrap_or(0) >= 20,
        "live-verified 家族过少：{counts:?}"
    );

    // 总览声明值必须等于名字总数。
    let declared = text
        .lines()
        .filter_map(|l| l.split("家族测试文件中的 `#[ignore]` 用例数：**").nth(1))
        .filter_map(|l| l.split("**").next())
        .filter_map(|n| n.trim().parse::<usize>().ok())
        .next()
        .unwrap_or(0);
    assert_eq!(declared, total_names, "总览声明 {declared} != 名字总数 {total_names}");

    // 说明：不同家族允许同名测试函数（它们位于不同测试二进制，如
    // catvod_moli_family.rs 与 catvod_tangrenjie_family.rs 都有
    // live_site_categories_and_list）。跨家族同名不构成漂移，判据以
    // 「每个家族行 == 该家族测试文件」为准（上文已逐个比对）。
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn non_family_live_tests_are_documented() {
    let root = repo_root();
    let text = read_matrix();
    let rows = matrix_rows(&text);
    let family_files: BTreeSet<String> = rows
        .iter()
        .map(|r| format!("crates/vbox_core/tests/catvod_{}_family.rs", r.family))
        .collect();

    // 磁盘实扫：tests/ 下非家族文件 + src/ 下全部。
    let mut tests_live = Vec::new();
    walk_ignored(&root.join("crates/vbox_core/tests"), &mut tests_live);
    let mut lib_live = Vec::new();
    walk_ignored(&root.join("crates/vbox_core/src"), &mut lib_live);

    let non_family: Vec<&(String, Vec<String>)> = tests_live
        .iter()
        .filter(|(path, _)| !family_files.contains(path))
        .collect();
    let non_family_count: usize = non_family.iter().map(|(_, names)| names.len()).sum();
    let lib_count: usize = lib_live.iter().map(|(_, names)| names.len()).sum();

    let header = text
        .lines()
        .find(|l| l.starts_with("> 另有 ") && l.contains("库内 live 用例"))
        .unwrap_or_else(|| panic!("矩阵缺少「另有 … 非家族真站用例」说明行"));
    let num_after = |marker: &str| -> usize {
        header
            .split(marker)
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("说明行缺少 {marker} 处的数字：{header}"))
    };
    assert_eq!(num_after("另有 "), non_family_count, "非家族真站用例数漂移：{header}");
    assert_eq!(num_after("与 "), lib_count, "库内 live 用例数漂移：{header}");

    let family_total: usize = rows.iter().map(|r| r.count).sum();
    assert_eq!(
        num_after("ignored 为 "),
        family_total + non_family_count + lib_count,
        "ignored 合计漂移：{header}"
    );

    // 每个非家族 / 库内文件都必须在说明行出现文件名（防新增文件被忽略）。
    for (path, _) in non_family.iter().copied().chain(lib_live.iter()) {
        let base = Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        assert!(
            header.contains(&base),
            "说明行未列出 live 用例文件 `{base}`：{header}"
        );
    }
}
