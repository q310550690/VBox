//! 判据 43：612 个真实站点逐站点分派结果 ↔ 画像策略一致性。
//!
//! 为什么需要它：判据 42 校验的是「站点覆盖报告」里的聚合数字，而报告由
//! `_rea/tools/site-coverage.mjs` 用 **JS 镜像谓词** 计算；一旦 Rust 侧的
//! `script_lane_allowed` / `api_is_*` / `registry::classify` 发生变化，
//! 报告仍可能停留在旧口径。本判据用真实的 `_rea/samples/inventory/sites.json`
//! 逐站点调用 Rust 的 `dispatch_lane`，与报告 §五 表交叉验证，并锁住两条
//! 边界：`csp_*` 永不进脚本车道、`type=3` 非 `csp_*` 永不进原生/分类车道。
//!
//! 依赖 `_rea/samples/inventory/sites.json`（与判据 42 / 覆盖报告同源，
//! 不发起任何网络请求）。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use vbox_core::model::Site;
use vbox_core::sources::spider::{dispatch_lane, DispatchLane};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn load_sites() -> Vec<Site> {
    let path = repo_root().join("_rea/samples/inventory/sites.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("无法读取 {}：{e}（判据 43 依赖统一站点清单）", path.display())
    });
    let doc: Value = serde_json::from_str(&raw).expect("sites.json 必须是合法 JSON");
    let sites = doc["sites"].as_array().expect("sites.json 缺少 sites 数组");
    sites
        .iter()
        .map(|s| Site {
            key: s["key"].as_str().unwrap_or_default().to_string(),
            name: s["name"].as_str().unwrap_or_default().to_string(),
            kind: s["type"].as_i64().unwrap_or(3) as i32,
            api: s["api"].as_str().unwrap_or_default().to_string(),
            ext: s["ext"].as_str().map(|v| Value::String(v.to_string())),
            jar: s["jar"].as_str().unwrap_or_default().to_string(),
            ..Site::default()
        })
        .collect()
}

fn load_profiles() -> Value {
    let path = repo_root().join("crates/vbox_core/src/spider/catvod/profiles.json");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取 {}：{e}", path.display()));
    serde_json::from_str(&raw).expect("profiles.json 必须是合法 JSON")
}

fn lane_name(lane: DispatchLane) -> &'static str {
    match lane {
        DispatchLane::Native => "native",
        DispatchLane::Classified => "classified",
        DispatchLane::ScriptDrpy => "drpy",
        DispatchLane::ScriptJs => "js",
        DispatchLane::ScriptPy => "py",
        DispatchLane::Source => "source",
        DispatchLane::Unknown => "unknown",
    }
}

/// 解析 `docs/审计报告/站点覆盖报告.md` 的 `## 五、分派车道分布（附录）` 表。
fn report_lane_counts() -> BTreeMap<String, u64> {
    let path = repo_root().join("docs/审计报告/站点覆盖报告.md");
    let report = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取 {}：{e}", path.display()));
    let start = report
        .find("## 五、分派车道分布")
        .expect("站点覆盖报告缺少「五、分派车道分布」小节（判据 42 已锁定该小节）");
    let mut out = BTreeMap::new();
    for line in report[start..].lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim())
            .collect();
        if cells.len() < 2 {
            continue;
        }
        let name = cells[0];
        if !matches!(
            name,
            "native" | "classified" | "drpy" | "js" | "py" | "source" | "合计"
        ) {
            continue;
        }
        let Ok(count) = cells[1].parse::<u64>() else {
            continue;
        };
        out.insert(name.to_string(), count);
    }
    out
}

#[test]
fn lane_counts_match_coverage_report() {
    let sites = load_sites();
    assert_eq!(sites.len(), 612, "统一站点清单应为 612 条");

    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    for site in &sites {
        *counts.entry(lane_name(dispatch_lane(site))).or_default() += 1;
    }

    let report = report_lane_counts();
    for lane in ["native", "classified", "drpy", "js", "py", "source"] {
        let expected = *report
            .get(lane)
            .unwrap_or_else(|| panic!("报告 §五 缺少车道行 {lane}"));
        let actual = *counts.get(lane).unwrap_or(&0);
        assert_eq!(
            actual, expected,
            "车道 {lane} 的 Rust 逐站点分派计数与站点覆盖报告 §五 不一致（报告生成器与运行时谓词可能漂移）"
        );
    }
    assert_eq!(
        counts.values().sum::<u64>(),
        612,
        "六车道站点数之和必须等于统一站点清单总数"
    );
    assert_eq!(
        report.get("合计").copied(),
        Some(612),
        "报告 §五 合计行必须为 612"
    );
}

#[test]
fn lane_matches_profile_strategy() {
    let sites = load_sites();
    let profiles = load_profiles();
    let mut unknown = Vec::new();

    for site in &sites {
        let api = site.api.trim();
        let strategy = profiles["apis"][api]["strategy"]
            .as_str()
            .unwrap_or_else(|| panic!("画像缺少 api {api}（站点 {}）", site.key));
        let lane = dispatch_lane(site);
        let ok = match lane {
            DispatchLane::Native => strategy == "rulepack",
            DispatchLane::Classified => strategy == "unported" || strategy == "unsupported",
            DispatchLane::ScriptDrpy | DispatchLane::ScriptJs | DispatchLane::ScriptPy => {
                strategy == "script"
            }
            // type≠3 的 CMS/XML 源在画像里按 script 记账，但不进 spider 车道。
            DispatchLane::Source => strategy == "script",
            DispatchLane::Unknown => false,
        };
        if !ok {
            panic!(
                "站点 {}（api={api}）落入车道 {:?}，但画像策略是 {strategy}",
                site.key, lane
            );
        }
        if lane == DispatchLane::Unknown {
            unknown.push(site.key.clone());
        }
    }
    assert!(
        unknown.is_empty(),
        "存在未分派站点（Unknown 车道）：{unknown:?}"
    );
}

#[test]
fn csp_and_non_csp_lanes_are_disjoint() {
    let sites = load_sites();
    let mut csp_sites = 0u64;
    let mut type3_non_csp = 0u64;

    for site in &sites {
        let is_csp = site.api.trim().starts_with("csp_");
        let lane = dispatch_lane(site);
        if is_csp {
            assert_eq!(
                site.kind, 3,
                "csp_ 站点 {} 的 type 不是 3（画像假设被破坏）",
                site.key
            );
            assert!(
                matches!(lane, DispatchLane::Native | DispatchLane::Classified),
                "csp_ 站点 {} 不得进入脚本车道，实际 {:?}",
                site.key,
                lane
            );
            csp_sites += 1;
        } else if site.kind == 3 {
            assert!(
                matches!(
                    lane,
                    DispatchLane::ScriptDrpy | DispatchLane::ScriptJs | DispatchLane::ScriptPy
                ),
                "type=3 非 csp_ 站点 {} 不得进入原生/分类车道，实际 {:?}",
                site.key,
                lane
            );
            type3_non_csp += 1;
        } else {
            assert_eq!(
                lane,
                DispatchLane::Source,
                "type≠3 站点 {} 必须由 cms/xml 适配器处理",
                site.key
            );
        }
    }

    assert_eq!(csp_sites, 311, "csp_ 站点数应为 311（195 原生 + 116 分类）");
    assert_eq!(type3_non_csp, 273, "type=3 非 csp_ 站点数应为 273（脚本车道）");
}
