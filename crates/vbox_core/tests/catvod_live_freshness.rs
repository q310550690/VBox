//! 判据 27：审计报告里的「最近完整真站审计」必须是 24 小时内，且与其结果行一致。
//!
//! 动机：连续多轮 `--skip-live` 审计会让「真站回归」证据过期而无任何告警。
//! 本测试只读报告（不联网），时间解析用内建公历算法，不引入时间库。

use std::fs;
use std::path::PathBuf;

const REPORT: &str = "docs/审计报告/兼容性审计.md";
const MAX_AGE_SECS: i64 = 24 * 3600;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 解析 `YYYY-MM-DDTHH:MM:SS(.fff)?Z` 为 Unix 秒。
fn parse_iso_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let se: i64 = s.get(17..19)?.parse().ok()?;
    if bytes[19] != b'Z' && bytes[19] != b'.' && bytes[19] != b'+' && bytes[19] != b'-' {
        // 允许带毫秒/偏移的简化处理：截断到秒后继续。
        let end = s.find(|c: char| !(c.is_ascii_digit() || c == '-'))?;
        if end < 20 {
            return None;
        }
    }
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn recent_full_live_audit_is_fresh() {
    let path = root().join(REPORT);
    assert!(path.exists(), "{REPORT} 不存在");
    let text = fs::read_to_string(&path).expect("读取审计报告");
    let line = text
        .lines()
        .find(|l| l.contains("最近完整真站审计："))
        .expect("审计报告缺少「最近完整真站审计」行");
    assert!(
        !line.contains("（缺失"),
        "最近完整真站审计缺失（需要跑一次不带 --skip-live 的 audit-all）：{line}"
    );
    let stamp_text = line
        .split("最近完整真站审计：")
        .nth(1)
        .and_then(|s| s.split('（').next())
        .map(str::trim)
        .unwrap_or_default();
    let stamp = parse_iso_utc(stamp_text).unwrap_or_else(|| panic!("无法解析时间戳：{stamp_text}"));
    let age = now_secs() - stamp;
    assert!(
        age <= MAX_AGE_SECS,
        "真站证据过期：{} 秒前（上限 {} 秒）",
        age,
        MAX_AGE_SECS
    );
    assert!(age > -300, "时间戳在未来：{stamp_text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn live_evidence_line_matches_status() {
    let text = fs::read_to_string(root().join(REPORT)).expect("读取审计报告");
    let section_start = text
        .find("## 三、真站回归")
        .or_else(|| text.find("## 四、真站回归"))
        .expect("审计报告缺少「真站回归」章节");
    let section = &text[section_start..];
    let next = section[3..]
        .find("\n## ")
        .map(|i| i + 3)
        .unwrap_or(section.len());
    let section = &section[..next];
    assert!(section.contains("最近完整真站审计："), "缺少时效行");
    assert!(section.contains("- 结果：passed **"), "缺少结果行");
    assert!(
        section.contains("结论：**无失败项**") || section.contains("failed **0**"),
        "真站结果不是全绿：{section}"
    );
}
