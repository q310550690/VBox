//! 判据 50：审计报告「三、真站回归」章节 ↔ 真站日志 ↔ live-last-full 清单三方对账。
//!
//! 判据 49 证明「源码里的 `#[ignore]` 名字都在日志里跑过且 ok」，但报告正文里的数字
//! （passed / failed / 测试二进制数 / 最近完整真站审计时间）是 `compat-audit.mjs` 单独
//! 汇总出来的，属于另一条计算路径；两条路径若漂移，用户看到的报告就会与真实执行不符。
//! 本判据把三份产物锁在一起：
//! - `docs/审计报告/兼容性审计.md` 的「三、真站回归」章节（人读的结论）；
//! - `_rea/tmp/live-audit-latest.log`（cargo 原始输出的逐二进制 `test result:` 行）；
//! - `_rea/tmp/live-last-full.json`（审计脚本写下的机器可读清单：at/passed/failed/bins）。
//!
//! CI 环境没有 `_rea/`（该目录整体不入库）：检测到 `CI` 且日志缺失时软跳过，
//! 与判据 49 的降级口径一致；本地缺失则显式失败并提示先跑审计。

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_opt(rel: &str) -> Option<String> {
    let path = repo_root().join(rel);
    match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) => {
            if std::env::var("CI").is_ok() {
                eprintln!("skip: {} 在 CI 中不可用（_rea/ 不入库）：{e}", path.display());
                None
            } else {
                panic!(
                    "读取 {} 失败：{e}（先运行 node _rea/tools/audit-all.mjs）",
                    path.display()
                )
            }
        }
    }
}

struct LogTotals {
    passed: u64,
    failed: u64,
    ignored: u64,
    bins: u64,
}

/// 从 cargo 日志里逐行汇总 `test result: ... N passed; M failed; K ignored`。
/// 注意：日志是 stdout+stderr 拼接，二进制头集中在尾部，但汇总行本身仍可逐行累加。
fn parse_log_totals(text: &str) -> LogTotals {
    let mut t = LogTotals {
        passed: 0,
        failed: 0,
        ignored: 0,
        bins: 0,
    };
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("test result:") else {
            continue;
        };
        let mut nums = [0u64; 3];
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        let mut seen = 0usize;
        for (i, tok) in tokens.iter().enumerate() {
            // token 带分号（`1 passed;`），先剥掉再比较。
            let tok = tok.trim_end_matches(';');
            if tok == "passed" || tok == "failed" || tok == "ignored" {
                if let Some(n) = i
                    .checked_sub(1)
                    .and_then(|j| tokens[j].parse::<u64>().ok())
                {
                    nums[seen] = n;
                }
                seen += 1;
                if seen == 3 {
                    break;
                }
            }
        }
        if seen == 3 {
            t.passed += nums[0];
            t.failed += nums[1];
            t.ignored += nums[2];
            t.bins += 1;
        }
    }
    t
}

/// 解析报告「- 结果：passed **43** / failed **0** / ignored 0（123 个测试二进制）」行。
fn parse_report_result_line(report: &str) -> (u64, u64, u64, u64) {
    let line = report
        .lines()
        .find(|l| l.trim_start().starts_with("- 结果：passed"))
        .unwrap_or_else(|| panic!("审计报告缺少「- 结果：passed …」行（三、真站回归章节）"));
    let mut nums: Vec<u64> = Vec::new();
    let mut buf = String::new();
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            buf.push(ch);
        } else if !buf.is_empty() {
            nums.push(buf.parse().unwrap());
            buf.clear();
        }
    }
    if !buf.is_empty() {
        nums.push(buf.parse().unwrap());
    }
    // 预期顺序：passed、failed、ignored、bins（共 4 个数字）。
    assert_eq!(
        nums.len(),
        4,
        "结果行数字个数应为 4（passed/failed/ignored/bins），实际 {nums:?}：{line}"
    );
    (nums[0], nums[1], nums[2], nums[3])
}

fn parse_live_last_full(text: &str) -> (String, u64, u64, u64) {
    // 手工解析，避免为测试引入 serde 之外的依赖；字段名固定。
    let get = |key: &str| -> Option<String> {
        let pat = format!("\"{key}\"");
        let idx = text.find(&pat)? + pat.len();
        let rest = text[idx..].trim_start();
        let rest = rest.strip_prefix(':')?.trim_start();
        let end = rest.find(|c: char| c == ',' || c == '}')?;
        Some(rest[..end].trim().trim_matches('"').to_string())
    };
    let at = get("at").expect("live-last-full.json 缺少 at");
    let passed: u64 = get("passed").expect("缺少 passed").parse().expect("passed 非数字");
    let failed: u64 = get("failed").expect("缺少 failed").parse().expect("failed 非数字");
    let bins: u64 = get("bins").expect("缺少 bins").parse().expect("bins 非数字");
    (at, passed, failed, bins)
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn report_live_section_matches_audit_log() {
    let Some(log) = read_opt("_rea/tmp/live-audit-latest.log") else {
        return;
    };
    let report = fs::read_to_string(repo_root().join("docs/审计报告/兼容性审计.md"))
        .expect("读取 docs/审计报告/兼容性审计.md 失败");
    let totals = parse_log_totals(&log);
    let (passed, failed, ignored, bins) = parse_report_result_line(&report);

    assert_eq!(totals.failed, 0, "真站日志存在 failed 项");
    assert_eq!(passed, totals.passed, "报告 passed 与日志汇总不一致");
    assert_eq!(failed, totals.failed, "报告 failed 与日志汇总不一致");
    assert_eq!(ignored, totals.ignored, "报告 ignored 与日志汇总不一致");
    assert_eq!(bins, totals.bins, "报告测试二进制数与日志 `test result:` 行数不一致");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn live_last_full_manifest_matches_report() {
    let Some(log) = read_opt("_rea/tmp/live-audit-latest.log") else {
        return;
    };
    let Some(manifest) = read_opt("_rea/tmp/live-last-full.json") else {
        return;
    };
    let report = fs::read_to_string(repo_root().join("docs/审计报告/兼容性审计.md"))
        .expect("读取 docs/审计报告/兼容性审计.md 失败");
    let totals = parse_log_totals(&log);
    let (at, passed, failed, bins) = parse_live_last_full(&manifest);

    assert_eq!(passed, totals.passed, "live-last-full.passed 与日志不一致");
    assert_eq!(failed, totals.failed, "live-last-full.failed 与日志不一致");
    assert_eq!(bins, totals.bins, "live-last-full.bins 与日志不一致");
    assert!(
        report.contains(&format!("最近完整真站审计：{at}")),
        "报告「最近完整真站审计」时间应为 {at}（报告与 live-last-full 漂移）"
    );
}
