//! 判据 16：审计报告「判定五条」必须全部为 ✅，且第四节真站回归数字
//! 必须与 `_rea/tmp/live-audit-latest.log` 重新解析的结果一致。
//!
//! 背景：`docs/审计报告/兼容性审计.md` 是「100% 判定」的最终人读产物。若某条判据实际失败，
//! 报告里可能出现 ❌ 但流程仍被当成通过（例如只看退出码不看报告内容）。
//! 本测试把「报告结论」与「日志事实」双向锁定。

use regex::Regex;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", p.display()))
}

#[test]
fn audit_verdict_table_all_pass() {
    let audit = read("docs/审计报告/兼容性审计.md");
    let section = audit
        .split("## 一、判定五条")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("审计报告缺少「一、判定五条」");

    let mut rows = 0;
    let mut failures = Vec::new();
    for line in section.lines() {
        let t = line.trim();
        if !t.starts_with("| ") || t.contains("---") || t.contains("判据") {
            continue;
        }
        let cols: Vec<&str> = t.split('|').map(str::trim).collect();
        if cols.len() < 6 {
            continue;
        }
        rows += 1;
        let verdict = cols[cols.len() - 2];
        if !verdict.contains('✅') || verdict.contains('❌') {
            failures.push(format!("第 {} 条判据结论为「{}」", cols[1], verdict));
        }
    }
    assert_eq!(rows, 5, "判定五条应恰有 5 行，实际 {rows} 行");
    assert!(failures.is_empty(), "存在未通过的判据：{}", failures.join("；"));
}

#[test]
fn live_regression_numbers_match_log() {
    let audit = read("docs/审计报告/兼容性审计.md");
    let re = Regex::new(
        r"passed \*\*(\d+)\*\* / failed \*\*(\d+)\*\* / ignored (\d+)（(\d+) 个测试二进制）",
    )
    .unwrap();
    let caps = re
        .captures(&audit)
        .expect("审计报告缺少真站回归数字（passed **N** / failed **N** / ignored N（N 个测试二进制））");
    let a_passed: i64 = caps[1].parse().unwrap();
    let a_failed: i64 = caps[2].parse().unwrap();
    let a_ignored: i64 = caps[3].parse().unwrap();
    let a_bins: i64 = caps[4].parse().unwrap();

    let log_path = root().join("_rea/tmp/live-audit-latest.log");
    assert!(
        log_path.exists(),
        "缺少 {}（先运行 `node _rea/tools/audit-all.mjs` 或 `cargo test -p vbox_core -- --ignored`）",
        log_path.display()
    );
    let log = fs::read_to_string(&log_path).unwrap();
    let line_re =
        Regex::new(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored").unwrap();
    let (mut passed, mut failed, mut ignored, mut bins) = (0i64, 0i64, 0i64, 0i64);
    for c in line_re.captures_iter(&log) {
        passed += c[1].parse::<i64>().unwrap();
        failed += c[2].parse::<i64>().unwrap();
        ignored += c[3].parse::<i64>().unwrap();
        bins += 1;
    }
    assert!(bins > 0, "真站日志中没有 test result 行");
    assert_eq!(
        (a_passed, a_failed, a_ignored, a_bins),
        (passed, failed, ignored, bins),
        "审计报告数字与日志重新解析不一致：报告 ({a_passed}/{a_failed}/{a_ignored}/{a_bins}) vs 日志 ({passed}/{failed}/{ignored}/{bins})"
    );
    assert_eq!(a_failed, 0, "真站回归存在失败项（{a_failed}）");
    assert!(
        audit.contains("结论：**无失败项**"),
        "审计报告未声明「无失败项」"
    );
}
