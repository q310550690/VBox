//! 判据 19：报告新鲜度顺序（报告不得基于更旧的画像/清单生成）。
//!
//! 规则：
//! - 五份自动生成报告都必须在文件头带 `> 生成时间：<ISO>　|　生成命令：…`；
//! - 每份报告的生成时间必须不早于 `_rea/samples/inventory/sites.json` 的 `generatedAt`
//!   （报告的数据源之一）；
//! - `profiles.json` 的 `generatedAt` 为日期粒度（`YYYY-MM-DD`），报告的日期部分必须不早于它。
//!
//! 该判据防「报告基于旧画像/旧清单重新计算」——例如 `profiles.json` 被重新生成后
//! 忘记重跑 `compat-audit.mjs`，审计结论看似通过、实际反映的是旧数据。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn read(p: &str) -> String {
    let path = repo_root().join(p);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

/// Hinnant 公历日公式：返回自 1970-01-01 起的天数。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 解析 `YYYY-MM-DDTHH:MM:SS(.sss)?Z` 为 Unix 秒。
fn parse_iso_epoch(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = std::str::from_utf8(&b[r]).ok()?;
        if !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<i64>().ok()
    };
    let y = num(0..4)?;
    let m = num(5..7)?;
    let d = num(8..10)?;
    let hh = num(11..13)?;
    let mm = num(14..16)?;
    let ss = num(17..19)?;
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !b[19..].iter().any(|&c| c == b'Z') {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// 从 Markdown 头部提取 `> 生成时间：<ISO>　|　生成命令：…`。
fn generated_at(report: &str) -> Option<(String, i64)> {
    for line in report.lines().take(6) {
        let Some(pos) = line.find("生成时间") else {
            continue;
        };
        let rest = &line[pos..];
        let Some(colon) = rest.find(['：', ':']) else {
            continue;
        };
        let colon_len = if rest[colon..].starts_with('：') { '：'.len_utf8() } else { 1 };
        let tail = &rest[colon + colon_len..];
        let token: String = tail
            .trim_start()
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '　' && *c != '｜' && *c != '|')
            .collect();
        if let Some(epoch) = parse_iso_epoch(&token) {
            return Some((token, epoch));
        }
    }
    None
}

const REPORTS: [&str; 5] = [
    "docs/审计报告/兼容性审计.md",
    "docs/审计报告/站点覆盖报告.md",
    "docs/审计报告/真站回归矩阵.md",
    "docs/审计报告/上游复核.md",
    "docs/审计报告/阻断复核.md",
];

#[test]
fn every_report_declares_generated_at() {
    for path in REPORTS {
        let text = read(path);
        let Some((token, epoch)) = generated_at(&text) else {
            panic!("{path} 头部缺少可解析的「> 生成时间：<ISO>　|　生成命令：…」行");
        };
        assert!(epoch > 1_700_000_000, "{path} 生成时间异常: {token}");
    }
}

#[test]
fn reports_are_not_older_than_their_data_sources() {
    let sites = read("_rea/samples/inventory/sites.json");
    let sites_epoch = generated_at(&sites)
        .map(|(_, e)| e)
        .or_else(|| {
            // sites.json 是 JSON：提取 `"generatedAt": "<ISO>"`
            let pos = sites.find("\"generatedAt\"")?;
            let tail = &sites[pos + "\"generatedAt\"".len()..];
            let q1 = tail.find('"')?;
            let tail2 = &tail[q1 + 1..];
            let q2 = tail2.find('"')?;
            parse_iso_epoch(&tail2[..q2])
        })
        .expect("解析 sites.json generatedAt 失败");

    let profiles = read("crates/vbox_core/src/spider/catvod/profiles.json");
    let pos = profiles.find("\"generatedAt\"").expect("profiles.json 缺少 generatedAt");
    let tail = &profiles[pos + "\"generatedAt\"".len()..];
    let q1 = tail.find('"').expect("profiles generatedAt 格式异常");
    let tail2 = &tail[q1 + 1..];
    let q2 = tail2.find('"').expect("profiles generatedAt 格式异常");
    let profiles_date = &tail2[..q2];
    assert_eq!(profiles_date.len(), 10, "profiles generatedAt 应为日期粒度");
    assert!(profiles_date.as_bytes()[4] == b'-', "profiles generatedAt 格式异常: {profiles_date}");

    for path in REPORTS {
        let text = read(path);
        let (token, epoch) = generated_at(&text)
            .unwrap_or_else(|| panic!("{path} 缺少可解析的生成时间"));
        assert!(
            epoch >= sites_epoch,
            "{path} 生成时间 {token} 早于 sites.json 的 generatedAt——报告基于旧清单，需重跑该报告生成器"
        );
        let date = &token[..10];
        assert!(
            date >= profiles_date,
            "{path} 生成日期 {date} 早于 profiles.json 的 {profiles_date}——报告基于旧画像，需重跑该报告生成器"
        );
    }
}
