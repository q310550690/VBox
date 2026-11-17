//! 判据 44：订阅漂移复核报告 ↔ 基线清单 ↔ 画像 三方一致。
//!
//! 为什么需要：用户目标覆盖 `http://xhztv.top/dc`、`https://ztha.top/TVBox/GYCK.json`、
//! `http://xhztv.top/4k.json` 三个入口多仓；`_rea/samples/inventory/sites.json` 是
//! 612 站点/214 api/22 jar 引用的唯一基线，`profiles.json` 又是由它生成。若上游订阅漂移
//! （新增/下架站点、换 api、换 jar），画像仍会显示「全量已覆盖」，形成假满分。
//!
//! `_rea/tools/inventory-drift.mjs --write` 会重抓三个入口并与基线对比，产出
//! `docs/订阅漂移复核.md`。本判据离线校验：
//! 1. 报告「基线」列数字 == 磁盘 `sites.json` 的实测值（报告不能建立在过期数据上）；
//! 2. 报告「本次复核」列与「差异」列自洽（diff == actual - baseline）；
//! 3. 报告结论与差异行一致（零差异 ⇔「未发现站点 / api / jar 引用漂移」）；
//! 4. 基线三角与画像一致（站点 612、api 214、带 jar 26、jar 引用 22），锁定「订阅 → 画像」
//!    这条数据链没有被手工编辑漂移。
//!
//! 降级：CI 无网络时无法重跑 `--write`，本判据只读已入库的报告；报告缺失即红灯，
//! 强制本地跑一次 `node _rea/tools/inventory-drift.mjs --write` 后提交。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate 应位于 <repo>/crates/vbox_core")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("读取 {rel} 失败：{e}"))
}

fn json(rel: &str) -> serde_json::Value {
    serde_json::from_str(&read(rel)).unwrap_or_else(|e| panic!("解析 {rel} 失败：{e}"))
}

/// `sites.json` 的实测指标（与 crawl-sites.mjs 的 totals 口径一致）。
struct Metrics {
    configs: u64,
    configs_parsed: u64,
    sites: u64,
    sites_with_jar: u64,
    distinct_apis: u64,
    jar_refs: u64,
}

fn metrics(sites: &serde_json::Value) -> Metrics {
    let sites_arr = sites["sites"].as_array().expect("sites.sites 数组");
    let configs = sites["configs"].as_array().expect("sites.configs 数组");
    let jars = sites["jars"].as_array().expect("sites.jars 数组");
    let parsed = configs
        .iter()
        .filter(|c| c["kind"].as_str() == Some("config"))
        .count() as u64;
    let mut apis = BTreeSet::new();
    for s in sites_arr {
        // 生成器口径：api 会 trim（个别站点 api 带尾空格）。
        apis.insert(s["api"].as_str().unwrap_or("").trim().to_string());
    }
    Metrics {
        configs: configs.len() as u64,
        configs_parsed: parsed,
        sites: sites_arr.len() as u64,
        sites_with_jar: sites_arr
            .iter()
            .filter(|s| !s["jar"].is_null())
            .count() as u64,
        distinct_apis: apis.len() as u64,
        jar_refs: jars.len() as u64,
    }
}

/// 解析报告 §二 表格的 `| 指标 | 基线 | 本次复核 | 差异 |` 行。
fn summary_rows(report: &str) -> Vec<(String, i64, i64, i64)> {
    let mut rows = Vec::new();
    for line in report.lines() {
        let Some(rest) = line.strip_prefix("| ") else {
            continue;
        };
        if !rest.contains('|') {
            continue;
        }
        let cells: Vec<&str> = rest.split('|').map(|c| c.trim()).collect();
        if cells.len() < 4 {
            continue;
        }
        let (Ok(base), Ok(actual), Ok(diff)) = (
            cells[1].parse::<i64>(),
            cells[2].parse::<i64>(),
            cells[3].parse::<i64>(),
        ) else {
            continue;
        };
        rows.push((cells[0].to_string(), base, actual, diff));
    }
    rows
}

fn row_value<'a>(rows: &'a [(String, i64, i64, i64)], name: &str) -> &'a (String, i64, i64, i64) {
    rows.iter()
        .find(|r| r.0 == name)
        .unwrap_or_else(|| panic!("报告中缺少「{name}」行；实有：{:?}", rows.iter().map(|r| &r.0).collect::<Vec<_>>()))
}

#[test]
fn drift_report_matches_disk_baseline() {
    let report = read("docs/订阅漂移复核.md");
    let sites = json("_rea/samples/inventory/sites.json");
    let m = metrics(&sites);

    // 1. 报告必须声明生成命令与复核时间，便于审计追溯。
    assert!(
        report.contains("生成命令：`node _rea/tools/inventory-drift.mjs --write`"),
        "报告头部未声明生成命令"
    );
    let stamp = report
        .lines()
        .find(|l| l.starts_with("> 复核时间："))
        .expect("报告缺少复核时间行");
    assert!(
        stamp.contains("T") && stamp.contains("Z"),
        "复核时间应为 ISO-8601 UTC：{stamp}"
    );

    // 2. 基线列 == 磁盘实测值；差异列 == 本次 - 基线。
    let rows = summary_rows(&report);
    let expect = [
        ("配置数", m.configs),
        ("站点", m.sites),
        ("站点（带 jar）", m.sites_with_jar),
        ("不同 api", m.distinct_apis),
        ("jar 引用", m.jar_refs),
    ];
    for (name, value) in expect {
        let (_, base, actual, diff) = row_value(&rows, name);
        assert_eq!(*base, value as i64, "「{name}」基线列与磁盘不一致");
        assert_eq!(*diff, *actual - *base, "「{name}」差异列不自洽");
    }
    // 可解析配置允许「配置修复」带来的 +N，但不得减少。
    let (name, base, actual, diff) = row_value(&rows, "可解析配置");
    assert_eq!(*base, m.configs_parsed as i64, "「{name}」基线列与磁盘不一致");
    assert!(*actual >= *base, "可解析配置不应减少");
    assert_eq!(*diff, *actual - *base);

    // 3. 结论与差异行一致：站点/api/jar 全零 ⇔ 「未发现站点 / api / jar 引用漂移」。
    let zero_sites = report.contains("- 站点：新增 0 / 移除 0 / 变更 0");
    let zero_apis = report.contains("- api：新增 0 / 移除 0 / 站点数变化 0");
    let zero_jars = report.contains("- jar 引用：新增 0 / 移除 0 / 引用数变化 0");
    let says_clean = report.contains("未发现站点 / api / jar 引用漂移");
    assert!(
        (zero_sites && zero_apis && zero_jars) == says_clean,
        "结论措辞与差异行不一致：sites={zero_sites} apis={zero_apis} jars={zero_jars} clean={says_clean}"
    );
}

#[test]
fn baseline_triangle_matches_profiles() {
    let sites = json("_rea/samples/inventory/sites.json");
    let profiles = json("crates/vbox_core/src/spider/catvod/profiles.json");
    let m = metrics(&sites);

    // 基线自身与 totals 自洽（防止手工编辑 sites.json 后忘记 totals）。
    let totals = &sites["totals"];
    assert_eq!(totals["sites"].as_u64(), Some(m.sites));
    assert_eq!(totals["configs"].as_u64(), Some(m.configs));
    assert_eq!(totals["configsParsed"].as_u64(), Some(m.configs_parsed));
    assert_eq!(totals["sitesWithJar"].as_u64(), Some(m.sites_with_jar));
    assert_eq!(totals["distinctApis"].as_u64(), Some(m.distinct_apis));
    assert_eq!(totals["jarRefs"].as_u64(), Some(m.jar_refs));

    // 「订阅 → 画像」三角：站点数、不同 api 数必须逐字一致。
    assert_eq!(m.sites, 612, "站点基线应为 612");
    assert_eq!(m.distinct_apis, 214, "不同 api 基线应为 214");
    assert_eq!(m.sites_with_jar, 26);
    assert_eq!(m.jar_refs, 22);
    assert_eq!(totals["roots"].as_u64(), Some(3));
    assert_eq!(
        profiles["totals"]["sites"].as_u64(),
        Some(m.sites),
        "画像 totals.sites 与站点基线不一致"
    );
    assert_eq!(
        profiles["totals"]["distinctApis"].as_u64(),
        Some(m.distinct_apis),
        "画像 totals.distinctApis 与站点基线不一致"
    );
    let apis = profiles["apis"].as_object().expect("profiles.apis 对象");
    assert_eq!(apis.len() as u64, m.distinct_apis);
}
