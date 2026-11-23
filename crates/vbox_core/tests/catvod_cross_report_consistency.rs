//! 判据 15：四份证据报告 + 画像/站点清单之间的站点/API 计数交叉一致。
//!
//! 现状：`profiles.json`（画像）、`sites.json`（站点清单）、`docs/审计报告/站点覆盖报告.md`、
//! `docs/审计报告/兼容性审计.md`、`docs/审计报告/真站回归矩阵.md`、`docs/审计报告/上游复核.md` 由不同工具生成。
//! 任一处的计数漂移都应立刻失败——本测试把关键数字逐项对齐，防止「某一侧悄悄过期」。

use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", p.display()))
}

fn profiles() -> Value {
    serde_json::from_str(&read("crates/vbox_core/src/spider/catvod/profiles.json")).expect("profiles.json 不是合法 JSON")
}

fn sites_json() -> Value {
    serde_json::from_str(&read("_rea/samples/inventory/sites.json")).expect("sites.json 不是合法 JSON")
}

fn num(text: &str, pattern: &str, what: &str) -> i64 {
    let re = Regex::new(pattern).unwrap();
    let caps = re
        .captures(text)
        .unwrap_or_else(|| panic!("在报告中找不到 {what}（pattern={pattern}）"));
    caps[1].parse().unwrap_or_else(|e| panic!("{what} 不是数字：{e}"))
}

fn api_strategies(p: &Value) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for (api, entry) in p["apis"].as_object().expect("profiles.apis 不是对象") {
        let s = entry["strategy"].as_str().unwrap_or("").to_string();
        *out.entry(s).or_insert(0) += 1;
        let _ = api;
    }
    out
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn counts_agree_across_profiles_sites_and_reports() {
    let prof = profiles();
    let sites = sites_json();
    let coverage = read("docs/审计报告/站点覆盖报告.md");
    let audit = read("docs/审计报告/兼容性审计.md");
    let matrix = read("docs/审计报告/真站回归矩阵.md");
    let upstream = read("docs/审计报告/上游复核.md");

    // —— 官方计数：画像 totals 为唯一真源 ——
    let p_sites = prof["totals"]["sites"].as_i64().unwrap();
    let p_apis = prof["totals"]["distinctApis"].as_i64().unwrap();
    let arr = sites["sites"].as_array().unwrap();
    let sites_len = arr.len() as i64;
    let mut distinct = std::collections::BTreeSet::new();
    for s in arr {
        distinct.insert(s["api"].as_str().unwrap_or("").trim().to_string());
    }
    assert_eq!(p_sites, sites_len, "画像 totals.sites ≠ sites.json 站点数");
    assert_eq!(p_apis, distinct.len() as i64, "画像 totals.distinctApis ≠ sites.json 去重 api 数");

    // —— 站点覆盖报告 ——
    assert_eq!(num(&coverage, r"\| 站点（去重后） \| (\d+) \|", "站点（去重后）"), p_sites);
    assert_eq!(num(&coverage, r"\| api（去重后） \| (\d+) \|", "api（去重后）"), p_apis);

    // api 策略分布表（位于 pdf 报告「### api 策略分布」与「## 二」之间）
    let api_part = coverage
        .split("### api 策略分布")
        .nth(1)
        .and_then(|s| s.split("## 二").next())
        .expect("覆盖报告缺少 api 策略分布段");
    let api_map = api_strategies(&prof);
    for (strategy, expected) in &api_map {
        let re = Regex::new(&format!(r"\| {strategy} \| (\d+) \|")).unwrap();
        let got: i64 = re
            .captures(api_part)
            .and_then(|c| c[1].parse().ok())
            .unwrap_or_else(|| panic!("覆盖报告缺少 api 策略 {strategy}"));
        assert_eq!(got, *expected, "覆盖报告的 api 策略 {strategy} 与画像不一致");
    }

    // 原生家族聚合行：`共 54 个 api / 37 个家族`
    let native_apis = prof["apis"]
        .as_object()
        .unwrap()
        .values()
        .filter(|a| a["reason"].as_str() == Some("existing_native_implementation"))
        .count() as i64;
    let fam_apis = num(&coverage, r"共 (\d+) 个 api / \d+ 个家族", "原生家族聚合 api 数");
    let fam_count = num(&coverage, r"共 \d+ 个 api / (\d+) 个家族", "原生家族数");
    assert_eq!(fam_apis, native_apis, "覆盖报告的原生 api 数与画像 reason 统计不一致");

    // —— 兼容性审计报告 ——
    assert_eq!(num(&audit, r"站点匹配 (\d+)/\d+", "站点匹配数"), p_sites);
    assert_eq!(num(&audit, r"api (\d+) 个全部有 strategy", "api 总数"), p_apis);
    let a_rulepack = num(&audit, r"其中 rulepack (\d+)、script \d+、unsupported \d+、unported \d+", "审计 rulepack");
    let a_script = num(&audit, r"其中 rulepack \d+、script (\d+)、unsupported \d+、unported \d+", "审计 script");
    let a_unsupported = num(&audit, r"其中 rulepack \d+、script \d+、unsupported (\d+)、unported \d+", "审计 unsupported");
    let a_unported = num(&audit, r"其中 rulepack \d+、script \d+、unsupported \d+、unported (\d+)", "审计 unported");
    assert_eq!(a_rulepack, api_map.get("rulepack").copied().unwrap_or(0));
    assert_eq!(a_script, api_map.get("script").copied().unwrap_or(0));
    assert_eq!(a_unsupported, api_map.get("unsupported").copied().unwrap_or(0));
    assert_eq!(a_unported, api_map.get("unported").copied().unwrap_or(0));

    // 带 blockedReason 的 api 数：审计文案 ↔ 画像实际统计
    let blocked = prof["apis"]
        .as_object()
        .unwrap()
        .values()
        .filter(|a| a.get("blockedReason").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()))
        .count() as i64;
    let a_blocked = num(&audit, r"带 blockedReason：\*\*(\d+)\*\*", "审计 blocked 数");
    assert_eq!(a_blocked, blocked, "审计的 blocked 数与画像不一致");

    // blockedReason 分布之和
    let dist = audit
        .split("blockedReason 分布：")
        .nth(1)
        .and_then(|s| s.lines().next())
        .expect("审计缺少 blockedReason 分布行");
    let pair_re = Regex::new(r"([a-z_]+) (\d+)").unwrap();
    let sum: i64 = pair_re
        .captures_iter(dist)
        .map(|c| c[2].parse::<i64>().unwrap())
        .sum();
    assert_eq!(sum, blocked, "审计的 blockedReason 分布之和 ≠ blocked 总数（{dist}）");

    // —— 真站矩阵 / 上游复核 的家族数与 mock-only 数 ——
    let m_fams = num(&matrix, r"原生家族：\*\*(\d+)\*\*", "矩阵家族数");
    let m_mock = num(&matrix, r"仅 mock（mock-only）：\*\*(\d+)\*\*", "矩阵 mock-only 数");
    let u_mock = num(&upstream, r"mock-only 家族：\*\*(\d+)\*\*", "上游复核 mock-only 数");
    assert_eq!(m_fams, fam_count, "矩阵家族数 ≠ 覆盖报告家族数");
    assert_eq!(m_mock, u_mock, "矩阵 mock-only 数 ≠ 上游复核 mock-only 数");
    // 家族数还须等于 families/mod.rs 的 `pub mod` 数（判据 6 已测，此处交叉确认报告未漂移）
    let mod_src = read("crates/vbox_core/src/spider/catvod/families/mod.rs");
    let mods = mod_src.lines().filter(|l| l.trim_start().starts_with("pub mod ")).count() as i64;
    assert_eq!(m_fams, mods, "矩阵家族数 ≠ families/mod.rs 的 pub mod 数");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn report_headers_are_fresh_and_consistent() {
    // 四份证据报告的头部必须含「生成时间」与「生成命令」，且覆盖既有文件；
    // 具体的新鲜度阈值由 `_rea/tools/compat-audit.mjs` 的第十一节判定，这里只做静态检查。
    for rel in [
        "docs/审计报告/站点覆盖报告.md",
        "docs/审计报告/真站回归矩阵.md",
        "docs/审计报告/上游复核.md",
        "docs/审计报告/阻断复核.md",
    ] {
        let text = read(rel);
        let head: String = text.lines().take(8).collect::<Vec<_>>().join("\n");
        assert!(
            Regex::new(r"生成时间：\d{4}-\d{2}-\d{2}T").unwrap().is_match(&head),
            "{rel} 头部缺少 ISO 生成时间"
        );
        assert!(
            head.contains("生成命令") || head.contains("--write"),
            "{rel} 头部缺少生成命令"
        );
    }
}
