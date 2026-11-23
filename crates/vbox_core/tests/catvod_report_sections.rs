//! 判据 28：审计报告的章节编号连续，且「全链可复现」行的计数与磁盘事实一致。
//!
//! 背景：条件章节（如「三、未匹配站点」）缺失时，旧生成器会留下编号断档
//! （一、二、四…）；`#[ignore]` 计数也曾把断言字符串里的字样算进去（55 vs 25）。

use std::fs;
use std::path::PathBuf;

const REPORT: &str = "docs/审计报告/兼容性审计.md";
const TESTS_DIR: &str = "crates/vbox_core/tests";
const LIVE_LOG: &str = "_rea/tmp/live-audit-latest.log";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn cn_to_num(s: &str) -> Option<usize> {
    const TABLE: [&str; 20] = [
        "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二", "十三", "十四",
        "十五", "十六", "十七", "十八", "十九", "二十",
    ];
    TABLE.iter().position(|x| *x == s).map(|i| i + 1)
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn headings_are_contiguous() {
    let text = fs::read_to_string(root().join(REPORT)).expect("读取审计报告");
    let mut nums = Vec::new();
    for line in text.lines() {
        if !line.starts_with("## ") {
            continue;
        }
        let body = &line[3..];
        if let Some(rest) = body.split_once('、') {
            if let Some(n) = cn_to_num(rest.0) {
                nums.push((n, rest.1.to_string()));
            }
        }
    }
    assert!(nums.len() >= 11, "审计报告章节过少：{}", nums.len());
    for (i, (n, title)) in nums.iter().enumerate() {
        assert_eq!(
            *n,
            i + 1,
            "章节编号不连续：第 {} 个章节是「{}、{}」",
            i + 1,
            n,
            title
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn reproducible_row_counts_match_disk() {
    let text = fs::read_to_string(root().join(REPORT)).expect("读取审计报告");
    let mut ignored_on_disk = 0usize;
    for entry in fs::read_dir(root().join(TESTS_DIR)).expect("读取 tests 目录") {
        let path = entry.expect("目录项").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = fs::read_to_string(&path).expect("读取测试文件");
        ignored_on_disk += src
            .lines()
            .filter(|line| line.trim_start().starts_with("#[ignore"))
            .count();
    }
    let marker = "用例 ";
    let pos = text.find(marker).expect("缺少「用例 N 项」");
    let tail = &text[pos + marker.len()..];
    let stated: usize = tail
        .split(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("无法解析用例数：{tail}"));
    assert_eq!(
        stated, ignored_on_disk,
        "「真站 #[ignore] 用例 {stated} 项」与磁盘 {} 项不一致",
        ignored_on_disk
    );

    let bins_part = text
        .split("测试二进制 ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .expect("缺少「测试二进制 N 个」");
    let stated_bins: usize = bins_part.parse().expect("解析测试二进制数");
    let log = root().join(LIVE_LOG);
    assert!(log.exists(), "{LIVE_LOG} 不存在");
    let log_text = fs::read_to_string(&log).expect("读取真站日志");
    let disk_bins = log_text
        .lines()
        .filter(|l| l.starts_with("test result: "))
        .count();
    assert_eq!(
        stated_bins, disk_bins,
        "「测试二进制 {stated_bins} 个」与日志 {disk_bins} 个不一致"
    );
}
