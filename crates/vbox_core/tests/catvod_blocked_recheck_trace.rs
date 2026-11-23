//! 判据 74：`site_dead` 阻断复核报告的可追溯性。
//!
//! `docs/审计报告/阻断复核.md` 是「站点是否复活」的唯一人工证据来源；判据 69 只要求
//! 报告里出现 `` `api` `` 行。若报告写错 api、漏掉一个 site_dead 站点，或把
//! 某个 host 归到别的 api，复核结论就可能被误用（放开一个仍死的站点）。
//!
//! 本判据把报告与两侧数据锁在一起：
//! - 报告里的 api 集合 == 画像 `blockedReason=site_dead` 的 api 集合；
//! - 每行的判定必须来自 `dead|parked|unknown`（出现 `alive` 就要人工放开策略）；
//! - 每行引用的 host 必须能在该 api 的站点配置（`sites.json` 的 name/api/ext）
//!   或该 api 的反编译源码中出现（硬编码备用域名也算证据）。

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

const REPORT: &str = "docs/审计报告/阻断复核.md";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
}

fn profiles() -> Value {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/profiles.json"),
    )
    .expect("画像文件必须存在");
    serde_json::from_str(&text).expect("profiles.json 必须是合法 JSON")
}

fn site_dead_apis(p: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (api, entry) in p["apis"].as_object().expect("apis 必须是对象") {
        if entry["blockedReason"].as_str() == Some("site_dead") {
            out.insert(api.clone());
        }
    }
    out
}

struct Row {
    api: String,
    verdict: String,
    hosts: Vec<String>,
}

fn parse_rows(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with("| `csp_") {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(|c| c.trim()).collect();
        if cells.len() != 4 {
            continue;
        }
        let api = cells[0].trim_matches('`').to_string();
        let verdict = cells[2].to_string();
        let mut hosts = Vec::new();
        for part in cells[3].split("<br>") {
            let part = part.trim();
            if let Some(colon) = part.find(':') {
                let host = part[..colon].trim();
                if host.contains('.') && !host.contains(' ') {
                    hosts.push(host.to_string());
                }
            }
        }
        rows.push(Row { api, verdict, hosts });
    }
    rows
}

fn find_source(api: &str) -> String {
    let simple = api.trim_start_matches("csp_");
    let dir = root().join("_rea/decompiled");
    let mut out = String::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let candidate = entry
            .path()
            .join("deobf/com/github/catvod/spider")
            .join(format!("{simple}.java"));
        if candidate.exists() {
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }
    out
}

fn site_strings(api: &str) -> String {
    let path = root().join("_rea/samples/inventory/sites.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    let mut out = String::new();
    for site in value["sites"].as_array().into_iter().flatten() {
        if site["api"].as_str().map(str::trim) == Some(api) {
            for key in ["name", "api", "ext"] {
                if let Some(s) = site[key].as_str() {
                    out.push_str(s);
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn site_dead_set_matches_recheck_report() {
    if !root().join("_rea").exists() {
        eprintln!("_rea 不存在，跳过历史证据判据");
        return;
    }
    let p = profiles();
    let expected = site_dead_apis(&p);
    let report = std::fs::read_to_string(root().join(REPORT)).expect("阻断复核报告必须存在");

    let rows = parse_rows(&report);
    let got: BTreeSet<String> = rows.iter().map(|r| r.api.clone()).collect();
    assert_eq!(got, expected, "报告 api 集合必须与画像 site_dead 集合精确相等");
    assert_eq!(rows.len(), expected.len(), "报告不得出现重复行");

    for row in &rows {
        assert!(
            matches!(row.verdict.as_str(), "dead" | "parked" | "unknown"),
            "{} 的判定必须为 dead/parked/unknown（alive 需人工放开策略）：{}",
            row.api,
            row.verdict
        );
        assert!(!row.hosts.is_empty(), "{} 必须列出至少一个被探查 host", row.api);
    }

    // 汇总行必须与明细一致：疑似复活为 0，且 dead/parked 计数可重算。
    assert!(report.contains("疑似复活 **0**"), "汇总必须明确写出疑似复活 0");
    let dead = rows.iter().filter(|r| r.verdict == "dead").count();
    let parked = rows.iter().filter(|r| r.verdict == "parked").count();
    assert!(
        report.contains(&format!("仍为停放 **{parked}**")),
        "汇总中的「仍为停放」应等于明细 parked 行数 {parked}"
    );
    assert!(
        report.contains(&format!("仍不可达 **{dead}**")),
        "汇总中的「仍不可达」应等于明细 dead 行数 {dead}"
    );

    // 审计报告第 8 节的「N 个 site_dead api」必须与画像计数一致；
    // 该数字曾硬编码为 13（当前 site_dead 仅 3 个），属陈旧快照。
    let audit_path = root().join("docs/审计报告/兼容性审计.md");
    if audit_path.exists() {
        let audit = std::fs::read_to_string(&audit_path).expect("读取审计报告");
        let line = audit
            .lines()
            .find(|l| l.contains("个 `site_dead` api 未发现复活"))
            .expect("审计报告缺少 site_dead 复核行");
        let tail = line.split_once("阻断复核：").map(|(_, t)| t).unwrap_or(line);
        let n: usize = tail
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("无法解析 site_dead 数量: {line}"));
        assert_eq!(n, expected.len(), "审计报告 site_dead 数量与画像不一致: {line}");
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn recheck_hosts_are_traceable_to_config_or_source() {
    if !root().join("_rea").exists() {
        eprintln!("_rea 不存在，跳过历史证据判据");
        return;
    }
    let report = std::fs::read_to_string(root().join(REPORT)).expect("阻断复核报告必须存在");
    let rows = parse_rows(&report);
    assert!(!rows.is_empty());

    let mut checked = 0;
    for row in &rows {
        let config = site_strings(&row.api);
        let source = find_source(&row.api);
        for host in &row.hosts {
            checked += 1;
            assert!(
                config.contains(host) || source.contains(host),
                "{} 的 host {host} 必须能在站点配置或反编译源码中找到",
                row.api
            );
        }
    }
    assert_eq!(checked, 4, "当前报告应恰有 4 个被探查 host");
}
