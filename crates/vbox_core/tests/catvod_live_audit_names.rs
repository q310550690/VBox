//! 判据 49：`#[ignore]` 用例名 ↔ 最近一次真站审计日志（`_rea/tmp/live-audit-latest.log`）按名对账。
//!
//! 判据 48 保证「矩阵声明的名字 == 源码里的 `#[ignore]` 名字」，但矩阵是**静态期望**；
//! 真站到底跑了什么、是否通过，答案只在审计日志里。本判据把两侧按**用例名多重集**对齐：
//! - 源码里每个 `#[ignore]` 用例名（43 个）都必须在日志里以 ok 出现，出现次数一致；
//! - 日志里不得出现源码中不存在的测试名（残留旧用例 / 拼写漂移）；
//! - 日志不得有 FAILED，且 `test result:` 汇总必须 passed>0、failed=0。
//!
//! 为什么按名多重集而不是 (二进制, 名字)：cargo 把 `Running tests\X.rs` 头写 stderr、
//! 测试结果写 stdout，audit-all 按 stdout+stderr 拼接落盘，二进制归属只能靠尾部的
//! 顺序列表反推；而对账目的只是「同名用例跑没跑、跑了几次」，多重集已足够且更稳健。
//! 库内 live 用例在日志里是 `模块::路径::fn`，取最后一段对齐。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

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

fn collect_expected(dir: &Path, out: &mut BTreeMap<String, usize>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_expected(&path, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(src) = fs::read_to_string(&path) else { continue };
        for name in ignored_names(&src) {
            *out.entry(name).or_insert(0) += 1;
        }
    }
}

fn expected_multiset() -> BTreeMap<String, usize> {
    let root = repo_root();
    let mut out = BTreeMap::new();
    collect_expected(&root.join("crates/vbox_core/tests"), &mut out);
    collect_expected(&root.join("crates/vbox_core/src"), &mut out);
    out
}

struct Observed {
    /// 用例名（库内取最后一段）→ 状态列表
    results: BTreeMap<String, Vec<String>>,
    failed_targets: usize,
    aggregate_failed: usize,
    aggregate_passed: usize,
}

fn parse_log(text: &str) -> Observed {
    let mut results: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut failed_targets = 0usize;
    let mut aggregate_failed = 0usize;
    let mut aggregate_passed = 0usize;
    for line in text.lines() {
        // 注意：必须先判 "test result:"——它同样以 "test " 开头，若放在用例行分支之后
        // 会被 split_once(" ... ") 的 continue 吞掉，导致汇总计数恒为 0。
        if let Some(rest) = line.strip_prefix("test result:") {
            if rest.trim_start().starts_with("FAILED") {
                failed_targets += 1;
            }
            for part in rest.split(';') {
                let tokens: Vec<&str> = part.split_whitespace().collect();
                for (i, tok) in tokens.iter().enumerate() {
                    if *tok != "passed" && *tok != "failed" {
                        continue;
                    }
                    let Some(num) = i
                        .checked_sub(1)
                        .and_then(|j| tokens[j].parse::<usize>().ok())
                    else {
                        continue;
                    };
                    if *tok == "passed" {
                        aggregate_passed += num;
                    } else {
                        aggregate_failed += num;
                    }
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("test ") {
            if let Some((name, status)) = rest.split_once(" ... ") {
                let name = name.trim();
                let short = name.rsplit("::").next().unwrap_or(name).to_string();
                results
                    .entry(short)
                    .or_default()
                    .push(status.trim().to_string());
            }
            continue;
        }
    }
    Observed {
        results,
        failed_targets,
        aggregate_failed,
        aggregate_passed,
    }
}

fn read_log() -> Option<(PathBuf, String)> {
    let path = repo_root().join("_rea/tmp/live-audit-latest.log");
    match fs::read_to_string(&path) {
        Ok(text) => Some((path, text)),
        Err(e) => {
            // 真站证据只在本地 _rea/tmp（该目录整体不入库），CI 的 cargo test 里必然缺失。
            // CI 的职责是静态判据 + 自检注入（audit-all --skip-live --selftest），真站回归交给
            // 本地定期审计；因此 CI 环境下软跳过，本地缺失则显式失败并给出修复命令。
            if std::env::var("CI").is_ok() {
                eprintln!("skip: 真站日志 {} 在 CI 中不可用（_rea/ 不入库），跳过名字级对账", path.display());
                None
            } else {
                panic!("读取 {} 失败：{e}（先运行 node _rea/tools/audit-all.mjs 生成真站日志）", path.display())
            }
        }
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_ignored_test_ran_ok_in_latest_audit() {
    let Some((_path, text)) = read_log() else {
        return;
    };
    let observed = parse_log(&text);
    let expected = expected_multiset();

    assert!(
        expected.len() >= 40,
        "期望的 #[ignore] 用例名过少：{}",
        expected.len()
    );
    assert_eq!(observed.failed_targets, 0, "真站日志存在 FAILED 测试二进制");
    assert_eq!(observed.aggregate_failed, 0, "真站日志汇总 failed != 0");
    assert!(observed.aggregate_passed > 0, "真站日志汇总 passed == 0");

    // 期望名字 ⊆ 观测，且 ok 次数 == 期望次数。
    for (name, want) in &expected {
        let Some(statuses) = observed.results.get(name) else {
            panic!("源码里的 #[ignore] 用例 `{name}` 未在真站日志出现（矩阵与日志漂移）");
        };
        let ok_count = statuses.iter().filter(|s| s.as_str() == "ok").count();
        assert_eq!(
            ok_count, *want,
            "用例 `{name}` 在日志中 ok 次数 {ok_count} != 期望 {want}（状态：{statuses:?}）"
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn audit_log_has_no_unknown_tests() {
    let Some((_path, text)) = read_log() else {
        return;
    };
    let observed = parse_log(&text);
    let expected = expected_multiset();

    // 观测 ⊆ 期望：日志里出现源码中不存在的名字，说明残留旧用例或拼写漂移。
    let unknown: Vec<String> = observed
        .results
        .keys()
        .filter(|name| !expected.contains_key(*name))
        .cloned()
        .collect();
    assert!(
        unknown.is_empty(),
        "真站日志包含源码中不存在的用例：{unknown:?}"
    );

    // 两侧数量一致（多一个少一个都要红）。
    let expected_total: usize = expected.values().sum();
    let observed_total: usize = observed.results.values().map(|v| v.len()).sum();
    assert_eq!(
        expected_total, observed_total,
        "源码 #[ignore] 总数 {expected_total} != 真站日志执行数 {observed_total}"
    );
}
