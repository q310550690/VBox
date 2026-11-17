//! 判据 18：真站回归的 flaky 重试必须「可观测」——审计报告里的 flaky 次数与
//! `_rea/tmp/live-flaky.log` 逐次记录一致（近 7 天 / 累计两个口径）。
//!
//! 背景：第 87 轮给 live 步骤加了「失败自动重试一次」，但重试成功只在 stderr 打一行、
//! 写一个 flag 文件，审计报告里看不到。时间一长，就无法回答「这套审计最近有没有靠重试
//! 才通过」。本判据把 flaky 事件变成审计产物的一部分，并让报告数字可被重新计算。

use regex::Regex;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn report_text() -> String {
    let p = root().join("docs/审计报告/兼容性审计.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", p.display()))
}

/// 从报告第四节解析 `- flaky 重试：近 7 天 **N** 次（累计 M 次…）` 的 N/M。
fn reported_flaky_counts(text: &str) -> (u64, u64) {
    let re = Regex::new(r"flaky 重试：近 7 天 \*\*(\d+)\*\* 次（累计 (\d+) 次").expect("flaky 正则");
    let caps = re
        .captures(text)
        .unwrap_or_else(|| panic!("审计报告缺少 flaky 重试统计行（判据 18）"));
    (
        caps[1].parse().expect("近 7 天次数不是数字"),
        caps[2].parse().expect("累计次数不是数字"),
    )
}

/// 读取 flaky 日志：每行一个 ISO8601 时间戳（`audit-all` 在重试成功时追加）。
fn logged_flaky_stamps() -> Vec<String> {
    let p = root().join("_rea/tmp/live-flaky.log");
    if !p.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", p.display()))
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_iso_ms(stamp: &str) -> Option<i128> {
    // 只用固定形状 `YYYY-MM-DDTHH:MM:SS[.fff]Z`，避免引入时间库。
    let b = stamp.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year: i128 = stamp[0..4].parse().ok()?;
    let month: i128 = stamp[5..7].parse().ok()?;
    let day: i128 = stamp[8..10].parse().ok()?;
    let hour: i128 = stamp[11..13].parse().ok()?;
    let min: i128 = stamp[14..16].parse().ok()?;
    let sec: i128 = stamp[17..19].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    // 公历日 → 天数（Howard Hinnant 算法），与时区无关：日志是 UTC。
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 86400 + hour * 3600 + min * 60 + sec) as i128) * 1000)
}

fn now_ms() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间早于 1970")
        .as_millis() as i128
}

fn ref_path() -> &'static Path {
    Path::new("_rea/tmp/live-flaky.log")
}

#[test]
fn report_flaky_counts_match_log() {
    let text = report_text();
    let (reported_7d, reported_total) = reported_flaky_counts(&text);
    let stamps = logged_flaky_stamps();
    let cutoff = now_ms() - 7 * 24 * 3600 * 1000;
    let mut total = 0u64;
    let mut last7 = 0u64;
    for s in &stamps {
        let ms = parse_iso_ms(s).unwrap_or_else(|| panic!("flaky 日志行不是合法 ISO 时间戳：{s}"));
        total += 1;
        if ms >= cutoff {
            last7 += 1;
        }
    }
    assert_eq!(
        reported_total, total,
        "报告里的 flaky 累计次数与日志行数不一致（{}）；日志：{}",
        ref_path().display(),
        stamps.len()
    );
    assert_eq!(reported_7d, last7, "报告里的 flaky 近 7 天次数与日志不一致");
}

#[test]
fn flaky_log_lines_are_iso_utc() {
    for s in logged_flaky_stamps() {
        assert!(s.ends_with('Z'), "flaky 日志必须是 UTC ISO 时间戳（以 Z 结尾）：{s}");
        assert!(parse_iso_ms(&s).is_some(), "flaky 日志时间戳无法解析：{s}");
    }
}
