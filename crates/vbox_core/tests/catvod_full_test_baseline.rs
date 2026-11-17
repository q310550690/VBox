//! 判据 51：全量测试日志 ↔ 计划文档「测试基线」行 ↔ 测试目标文件集合三方对账。
//!
//! 判据 50 锁住了「真站证据」的三方一致性；但计划文档头部那行测试基线
//! （`133 个测试目标全绿（lib 899 passed + 5 ignored，全仓 1670 passed / 43 ignored）`）
//! 此前是**纯手写数字**，加了一个测试文件却忘了更新、或日志是上一轮的旧日志，
//! 都不会有任何判据报警。本判据把三者锁在一起：
//! - `_rea/tmp/audit-full.log`（审计脚本落盘的最近一次全量原始输出）；
//! - 计划文档头部「| 测试 | … 个测试目标全绿（lib …） …」行（人读基线）；
//! - `crates/vbox_core/tests/*.rs`（每个文件 = 一个测试目标，必须都出现在日志里）。
//!
//! CI 环境没有 `_rea/`：检测到 `CI` 且日志缺失时软跳过，与判据 49/50 同口径。

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_full_log() -> Option<String> {
    let path = repo_root().join("_rea/tmp/audit-full.log");
    match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) => {
            if std::env::var("CI").is_ok() {
                eprintln!(
                    "skip: {} 在 CI 中不可用（_rea/ 不入库）：{e}",
                    path.display()
                );
                None
            } else {
                panic!(
                    "读取 {} 失败：{e}（先运行 node _rea/tools/audit-all.mjs）",
                    path.display()
                );
            }
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Totals {
    passed: u64,
    failed: u64,
    ignored: u64,
}

impl Totals {
    fn add(&mut self, o: Totals) {
        self.passed += o.passed;
        self.failed += o.failed;
        self.ignored += o.ignored;
    }
}

/// 解析日志：返回（按执行顺序的每个二进制汇总、Running 头数量、是否有 Doc-tests）。
fn parse_log(text: &str) -> (Vec<Totals>, usize, bool) {
    let mut blocks: Vec<Totals> = Vec::new();
    let mut running = 0usize;
    let mut has_doc_tests = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Running ") {
            running += 1;
        }
        if trimmed.starts_with("Doc-tests ") {
            has_doc_tests = true;
        }
        let Some(rest) = line.strip_prefix("test result:") else {
            continue;
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        let mut t = Totals::default();
        let mut seen = 0usize;
        for (i, raw) in tokens.iter().enumerate() {
            let tok = raw.trim_end_matches(';');
            if tok == "passed" || tok == "failed" || tok == "ignored" {
                if let Some(n) = i
                    .checked_sub(1)
                    .and_then(|j| tokens[j].parse::<u64>().ok())
                {
                    match tok {
                        "passed" => t.passed = n,
                        "failed" => t.failed = n,
                        _ => t.ignored = n,
                    }
                }
                seen += 1;
                if seen == 3 {
                    break;
                }
            }
        }
        blocks.push(t);
    }
    (blocks, running, has_doc_tests)
}

/// 从计划文档头部「| 测试 | …」行提取 5 个数字：
/// 测试目标数、lib passed、lib ignored、全仓 passed、全仓 ignored。
fn plan_baseline_numbers(plan: &str) -> (u64, u64, u64, u64, u64) {
    let line = plan
        .lines()
        .find(|l| l.starts_with("| 测试 |") && l.contains("个测试目标全绿"))
        .unwrap_or_else(|| panic!("计划文档头部缺少「| 测试 | …个测试目标全绿…」基线行"));
    let cell = line
        .split('|')
        .find(|c| c.contains("个测试目标全绿"))
        .unwrap_or_else(|| panic!("测试基线行的格式化单元格缺失"));
    let mut nums: Vec<u64> = Vec::new();
    let mut buf = String::new();
    for ch in cell.chars() {
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
    assert_eq!(
        nums.len(),
        5,
        "测试基线行数字个数应为 5（目标数/lib passed/lib ignored/全仓 passed/全仓 ignored），实际 {nums:?}"
    );
    (nums[0], nums[1], nums[2], nums[3], nums[4])
}

#[test]
fn full_log_matches_plan_test_baseline() {
    // 自引用引导：本判据自身也计入 targets/passed，而日志只能在上一次全量之后生成。
    // bootstrap 轮整体跳过本测试，使日志得到干净的「134 targets / 1672 passed / 0 failed」，
    // 更新计划头行后必须不带该变量复跑，届时本测试才真正开始校对。
    if std::env::var("FULL_BASELINE_BOOTSTRAP").is_ok() {
        eprintln!("bootstrap: 跳过 full_log_matches_plan_test_baseline（本轮只刷新日志）");
        return;
    }
    let Some(log) = read_full_log() else { return };
    let plan = fs::read_to_string(repo_root().join("docs/type3兼容/type3-实施计划-原生兼容100%.md"))
        .expect("读取计划文档失败");
    let (blocks, running, has_doc_tests) = parse_log(&log);
    assert!(!blocks.is_empty(), "全量日志没有任何 test result 行");
    assert!(has_doc_tests, "全量日志缺少 Doc-tests 块");

    let mut total = Totals::default();
    for b in &blocks {
        total.add(*b);
    }
    let lib = blocks[0];
    let targets = (running + 1) as u64; // Running 头（lib + 各测试文件）+ Doc-tests
    let (plan_targets, plan_lib_passed, plan_lib_ignored, plan_passed, plan_ignored) =
        plan_baseline_numbers(&plan);

    assert_eq!(total.failed, 0, "全量日志存在失败项");
    assert_eq!(lib.failed, 0, "lib 单测存在失败项");
    assert_eq!(plan_targets, targets, "计划文档目标数 {plan_targets} != 日志 {targets}");
    assert_eq!(plan_lib_passed, lib.passed, "计划文档 lib passed 与日志首块不一致");
    assert_eq!(plan_lib_ignored, lib.ignored, "计划文档 lib ignored 与日志首块不一致");
    assert_eq!(plan_passed, total.passed, "计划文档全仓 passed 与日志汇总不一致");
    assert_eq!(plan_ignored, total.ignored, "计划文档全仓 ignored 与日志汇总不一致");
}

#[test]
fn full_log_covers_all_test_targets() {
    if std::env::var("FULL_BASELINE_BOOTSTRAP").is_ok() {
        eprintln!("bootstrap: 跳过 full_log_covers_all_test_targets（本轮只刷新日志）");
        return;
    }
    let Some(log) = read_full_log() else { return };
    let tests_dir = repo_root().join("crates/vbox_core/tests");
    let mut stems: Vec<String> = Vec::new();
    for entry in fs::read_dir(&tests_dir).expect("读取 tests 目录失败").flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") && path.is_file() {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                stems.push(stem.to_string());
            }
        }
    }
    stems.sort();
    let (_blocks, running, _doc) = parse_log(&log);
    // Running 头 = lib（src/lib.rs） + 每个顶端测试文件各一个。
    assert_eq!(
        running,
        stems.len() + 1,
        "日志 Running 头 {running} 个 != 测试文件 {} 个 + lib",
        stems.len()
    );
    for stem in &stems {
        assert!(
            log.contains(&format!("Running tests\\{stem}.rs"))
                || log.contains(&format!("Running tests/{stem}.rs")),
            "测试文件 {stem}.rs 没有出现在全量日志的 Running 头里（日志是旧的或该目标未编译）"
        );
    }
}
