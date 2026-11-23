//! 判据 11：`docs/审计报告/站点覆盖报告.md` 的分组统计必须与 `profiles.json` × `sites.json` 实际计数**逐项一致**。
//!
//! 报告由 `_rea/tools/site-coverage.mjs` 生成，但生成物与画像之间此前没有测试锁——画像变更后若忘记
//! 重跑，报告会「安静地过期」。本测试把报告口径固化进 cargo test。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(rel)
}

fn read(rel: &str) -> String {
    let path = workspace_path(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()))
}

fn profiles() -> serde_json::Value {
    serde_json::from_str(&read("crates/vbox_core/src/spider/catvod/profiles.json"))
        .expect("profiles.json 不是合法 JSON")
}

fn sites() -> serde_json::Value {
    serde_json::from_str(&read("_rea/samples/inventory/sites.json")).expect("sites.json 不是合法 JSON")
}

fn report() -> String {
    read("docs/审计报告/站点覆盖报告.md")
}

/// 从 `| 站点（去重后） | 612 |` 这类行里取第一个数字。
fn table_number(doc: &str, label: &str) -> u64 {
    let line = doc
        .lines()
        .find(|l| l.contains(label))
        .unwrap_or_else(|| panic!("报告中找不到行 {label:?}（请重跑 node _rea/tools/site-coverage.mjs --write）"));
    line.split('|')
        .filter_map(|cell| cell.trim().parse::<u64>().ok())
        .next()
        .unwrap_or_else(|| panic!("行 {label:?} 中找不到数字: {line}"))
}

fn section<'a>(doc: &'a str, start: &str, end: &str) -> &'a str {
    let from = doc.find(start).unwrap_or_else(|| panic!("报告中找不到章节 {start:?}"));
    let rest = &doc[from + start.len()..];
    let to = rest.find(end).unwrap_or(rest.len());
    &rest[..to]
}

/// 解析 `| rulepack | 128 | 20.9% | ...` 表格为 map。
fn parse_strategy_table(sec: &str) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for line in sec.lines() {
        let cells: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        if cells.len() < 3 {
            continue;
        }
        let key = cells[1];
        if matches!(key, "rulepack" | "script" | "unsupported" | "unported") {
            if let Ok(n) = cells[2].parse::<u64>() {
                out.insert(key.to_string(), n);
            }
        }
    }
    out
}

fn api_strategies(profiles: &serde_json::Value) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for (_, v) in profiles["apis"].as_object().expect("apis 不是对象") {
        let s = v["strategy"].as_str().unwrap_or("unknown").to_string();
        *m.entry(s).or_insert(0) += 1;
    }
    m
}

fn site_strategies(profiles: &serde_json::Value, sites: &serde_json::Value) -> (BTreeMap<String, u64>, u64) {
    let apis = profiles["apis"].as_object().expect("apis 不是对象");
    let mut m = BTreeMap::new();
    let mut unmatched = 0u64;
    for site in sites["sites"].as_array().expect("sites 不是数组") {
        let Some(api) = site["api"].as_str() else { continue };
        match apis.get(api.trim()) {
            Some(v) => {
                let s = v["strategy"].as_str().unwrap_or("unknown").to_string();
                *m.entry(s).or_insert(0) += 1;
            }
            None => unmatched += 1,
        }
    }
    (m, unmatched)
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn overview_counts_match_sources() {
    let doc = report();
    let profiles = profiles();
    let sites = sites();

    let n_sites = sites["sites"].as_array().unwrap().len() as u64;
    let n_apis = profiles["apis"].as_object().unwrap().len() as u64;
    let (_, unmatched) = site_strategies(&profiles, &sites);

    assert_eq!(table_number(&doc, "| 站点（去重后） |"), n_sites, "报告站点数与 sites.json 不一致");
    assert_eq!(table_number(&doc, "| api（去重后） |"), n_apis, "报告 api 数与 profiles 不一致");
    assert_eq!(table_number(&doc, "| 未匹配画像的站点 |"), unmatched, "报告「未匹配画像的站点」与实际不一致");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn strategy_distribution_matches_profiles() {
    let doc = report();
    let profiles = profiles();
    let sites = sites();

    let site_table = parse_strategy_table(section(&doc, "### 站点策略分布", "### api 策略分布"));
    let api_table = parse_strategy_table(section(&doc, "### api 策略分布", "## 二、"));

    let actual_sites = site_strategies(&profiles, &sites).0;
    let actual_apis = api_strategies(&profiles);

    for key in ["rulepack", "script", "unsupported", "unported"] {
        assert_eq!(
            site_table.get(key).copied(),
            actual_sites.get(key).copied(),
            "站点策略 {key} 数量不一致（报告 {:?} vs 实际 {:?}）",
            site_table.get(key),
            actual_sites.get(key)
        );
        assert_eq!(
            api_table.get(key).copied(),
            actual_apis.get(key).copied(),
            "api 策略 {key} 数量不一致（报告 {:?} vs 实际 {:?}）",
            api_table.get(key),
            actual_apis.get(key)
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn blocked_groups_match_profiles() {
    let doc = report();
    let profiles = profiles();

    let mut expected: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (_, v) in profiles["apis"].as_object().unwrap() {
        let Some(reason) = v["blockedReason"].as_str() else { continue };
        let e = expected.entry(reason.to_string()).or_insert((0, 0));
        e.0 += 1;
        e.1 += v["sites"].as_u64().unwrap_or(0);
    }

    let mut found: BTreeSet<String> = BTreeSet::new();
    for line in doc.lines() {
        let Some(rest) = line.strip_prefix("### ") else { continue };
        let Some(open) = rest.find('（') else { continue };
        let reason = &rest[..open];
        if !expected.contains_key(reason) {
            continue;
        }
        found.insert(reason.to_string());
        let inner = &rest[open + '（'.len_utf8()..];
        let (n_api, n_site) = inner
            .split_once(" 个 api / ")
            .and_then(|(a, b)| Some((a.trim().parse::<u64>().ok()?, b.trim_end_matches(" 站）").parse::<u64>().ok()?)))
            .unwrap_or_else(|| panic!("阻断分组标题格式不符: {line}"));
        assert_eq!((n_api, n_site), expected[reason], "阻断分组 {reason} 数量不一致（报告 {n_api}/{n_site}）");
    }

    let missing: Vec<_> = expected.keys().filter(|k| !found.contains(*k)).cloned().collect();
    assert!(missing.is_empty(), "报告缺少阻断分组: {missing:?}");
    // 第 194 轮 csp_AppYs 落地后 player_resolver_aggregator 清零；第 258 轮 TGYunPan×2 落地后
    // decompiler_gap 也清零，当前为 3 种。
    assert!(expected.len() >= 3, "阻断原因种类异常偏少: {}", expected.len());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn native_family_summary_matches_profiles() {
    let doc = report();
    let profiles = profiles();

    let native: BTreeSet<String> = profiles["apis"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, v)| v["reason"].as_str() == Some("existing_native_implementation"))
        .map(|(k, _)| k.clone())
        .collect();
    let mut families: BTreeSet<&'static str> = BTreeSet::new();
    for api in &native {
        let fam = vbox_core::spider::catvod::families::family_of(api)
            .unwrap_or_else(|| panic!("native api {api} 没有 family_of"));
        families.insert(fam);
    }

    let header = doc
        .lines()
        .find(|l| l.contains("## 二、原生实现家族"))
        .expect("报告缺少「原生实现家族」小节");
    // 标题形如：「## 二、原生实现家族（按家族聚合，共 54 个 api / 37 个家族）」
    let number_before = |marker: &str| -> u64 {
        let idx = header.find(marker).unwrap_or_else(|| panic!("标题缺少 {marker:?}: {header}"));
        header[..idx]
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .next_back()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("{marker:?} 前找不到数字: {header}"))
    };
    let n_api = number_before(" 个 api");
    let n_family = number_before(" 个家族");
    assert_eq!(n_api, native.len() as u64, "报告原生 api 数与画像不一致（{header}）");
    assert_eq!(n_family, families.len() as u64, "报告家族数与 family_of 实际家族数不一致（{header}）");
    assert_eq!(n_family, families.len() as u64, "报告家族数与 family_of 实际家族数不一致（{header}）");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn plan_strategy_percentages_match_profiles_and_sites() {
    // 计划 §1.1 的策略分布表长期是手写数字：api 数/站点数/百分比任一改动都可能漏改，
    // 而既有判据只锁「报告计数」，没有锁「计划里的百分比」。此测试重算三方并逐格比对。
    let doc = read("docs/type3兼容/type3-实施计划-原生兼容100%.md");
    let sec = section(&doc, "### 1.1 策略分布", "### 1.2");
    let profiles = profiles();
    let sites = sites();
    let (site_counts, unmatched) = site_strategies(&profiles, &sites);
    let total = sites["sites"].as_array().unwrap().len() as u64;
    assert_eq!(unmatched, 0, "不应有未匹配画像的站点");
    let api_counts = api_strategies(&profiles);
    assert_eq!(total, 612, "站点总数应为 612");
    let mut seen = BTreeSet::new();
    let mut percent_sum = 0.0f64;
    for line in sec.lines() {
        let cells: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        if cells.len() < 5 {
            continue;
        }
        let Some(strategy) = ["rulepack", "script", "unported", "unsupported"]
            .into_iter()
            .find(|s| cells[1].contains(s))
        else {
            continue;
        };
        let api_cell: u64 = cells[2].parse().unwrap_or_else(|_| panic!("{strategy} api 数不可解析: {}", cells[2]));
        let site_cell: u64 = cells[3].parse().unwrap_or_else(|_| panic!("{strategy} 站点数不可解析: {}", cells[3]));
        let pct_cell: f64 = cells[4]
            .trim_end_matches('%')
            .parse()
            .unwrap_or_else(|_| panic!("{strategy} 百分比不可解析: {}", cells[4]));
        assert_eq!(api_cell, *api_counts.get(strategy).unwrap_or(&0), "{strategy} api 数");
        assert_eq!(site_cell, *site_counts.get(strategy).unwrap_or(&0), "{strategy} 站点数");
        let expect = (site_cell as f64 * 1000.0 / total as f64).round() / 10.0;
        assert!((pct_cell - expect).abs() < 0.05, "{strategy} 百分比: 表 {pct_cell} vs 重算 {expect}");
        percent_sum += pct_cell;
        seen.insert(strategy);
    }
    assert_eq!(seen.len(), 4, "计划 §1.1 应覆盖四种策略: {seen:?}");
    assert!((99.8..=100.2).contains(&percent_sum), "四策略百分比之和应约等于 100: {percent_sum}");
    assert_eq!(site_counts.values().sum::<u64>(), total, "四策略站点数之和应等于 612");
}
