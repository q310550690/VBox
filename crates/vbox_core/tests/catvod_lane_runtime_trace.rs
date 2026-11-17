//! 判据 64：612 个真实站点的**运行时车道函数**必须落进站点覆盖报告的车道账本。
//!
//! 已有判据 42 校验「报告车道数 ↔ 画像策略站点数」，但报告是由 `_rea/tools/site-coverage.mjs`
//! **独立实现**的一套判定逻辑生成的；Rust 侧 `sources::spider::dispatch_lane` 是另一套实现。
//! 两边只靠人工同步，任何一边改了优先级（drpy/js/py 启发式、script_lane_allowed、type 判定）
//! 都不会有编译错误，只会让「报告说 native 195，运行时却走了别的车道」。
//!
//! 本判据直接从 `_rea/samples/inventory/sites.json` 重建 612 个 `Site`，调用真实
//! `dispatch_lane`，把结果与报告表逐车道比对；并要求：
//! - 没有 `unknown` 车道（脚本策略的站点必须能落到 drpy/js/py 之一）；
//! - 每个站点的车道与其画像策略一致（rulepack→native、unported/unsupported→classified、
//!   script→drpy/js/py、type≠3→source）。
//!
//! `_rea/` 缺失时 CI 软跳过。

use std::collections::BTreeMap;
use std::path::PathBuf;

use vbox_core::model::Site;
use vbox_core::sources::spider::{self, DispatchLane};
use vbox_core::spider::catvod::contract::Strategy;
use vbox_core::spider::catvod::registry;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
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

/// 从站点清单重建 `Site`（ext 在清单里以字符串保存，足以复现车道判定）。
fn real_sites() -> Option<Vec<Site>> {
    let path = root().join("_rea/samples/inventory/sites.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).expect("sites.json 不是合法 JSON");
    let rows = value["sites"].as_array().expect("sites.json 缺少 sites 数组");
    Some(
        rows.iter()
            .map(|row| Site {
                key: row["key"].as_str().unwrap_or_default().to_string(),
                name: row["name"].as_str().unwrap_or_default().to_string(),
                kind: row["type"].as_i64().unwrap_or(0) as i32,
                api: row["api"].as_str().unwrap_or_default().to_string(),
                jar: row["jar"].as_str().unwrap_or_default().to_string(),
                ext: row["ext"].as_str().map(|s| serde_json::Value::String(s.to_string())),
                ..Default::default()
            })
            .collect(),
    )
}

/// 报告「分派车道分布」表：车道 → 站点数。
fn report_lane_table() -> BTreeMap<String, u64> {
    let path = root().join("docs/站点覆盖报告.md");
    let text = std::fs::read_to_string(&path).expect("读取站点覆盖报告失败");
    let start = text
        .find("## 五、分派车道分布")
        .expect("报告缺少「分派车道分布」小节");
    let section = &text[start..];
    let mut table = BTreeMap::new();
    for line in section.lines() {
        let Some(rest) = line.strip_prefix("| ") else { continue };
        let cells: Vec<&str> = rest.split('|').map(str::trim).collect();
        if cells.len() < 2 || cells[0] == "车道" || cells[0] == "---" {
            continue;
        }
        if let Ok(n) = cells[1].parse::<u64>() {
            table.insert(cells[0].to_string(), n);
        }
    }
    table
}

#[test]
fn runtime_lane_tally_matches_report_table() {
    let Some(sites) = real_sites() else {
        eprintln!("跳过判据 64：sites.json 不存在（CI 不提交逆向产物）");
        return;
    };
    assert_eq!(sites.len(), 612, "站点清单行数应为 612");

    let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
    for site in &sites {
        *tally.entry(lane_name(spider::dispatch_lane(site))).or_default() += 1;
    }
    let table = report_lane_table();
    let lanes = ["native", "classified", "drpy", "js", "py", "source"];
    let mut errors = Vec::new();
    for lane in lanes {
        let runtime = tally.get(lane).copied().unwrap_or(0);
        let reported = table.get(lane).copied().unwrap_or(u64::MAX);
        if runtime != reported {
            errors.push(format!("车道 {lane}: 运行时 {runtime} 站 vs 报告 {reported} 站"));
        }
    }
    assert_eq!(tally.get("unknown").copied().unwrap_or(0), 0, "存在 unknown 车道站点");
    assert_eq!(
        tally.values().sum::<u64>(),
        612,
        "六车道之和应为 612"
    );
    assert_eq!(table.get("合计").copied(), Some(612), "报告合计行应为 612");
    assert!(
        errors.is_empty(),
        "Rust 运行时车道与报告账本不一致（两套实现漂移）：\n{}",
        errors.join("\n")
    );
    println!("判据 64：612 站运行时车道 = 报告表 {table:?}");
}

#[test]
fn every_real_site_lands_in_its_strategy_lane() {
    let Some(sites) = real_sites() else {
        eprintln!("跳过判据 64：sites.json 不存在");
        return;
    };
    let p = registry::profiles();
    let mut errors = Vec::new();
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for site in &sites {
        let lane = spider::dispatch_lane(site);
        let expected = if site.kind != 3 {
            Some(DispatchLane::Source)
        } else {
            match p.apis.get(site.api.trim()).map(|a| a.strategy()) {
                Some(Strategy::Rulepack) => Some(DispatchLane::Native),
                Some(Strategy::Unported) | Some(Strategy::Unsupported) => {
                    Some(DispatchLane::Classified)
                }
                Some(Strategy::Script) => match lane {
                    DispatchLane::ScriptDrpy | DispatchLane::ScriptJs | DispatchLane::ScriptPy => {
                        Some(lane)
                    }
                    _ => None,
                },
                None => None,
            }
        };
        match expected {
            Some(want) if want == lane => {
                *counts.entry(lane_name(lane)).or_default() += 1;
            }
            Some(want) => errors.push(format!(
                "站点 {}（api={}，type={}）：运行时 lane={}，按画像策略应为 {}",
                site.key,
                site.api,
                site.kind,
                lane_name(lane),
                lane_name(want)
            )),
            None => errors.push(format!(
                "站点 {}（api={}，type={}）：画像缺失或脚本车道落到 {:?}",
                site.key,
                site.api,
                site.kind,
                lane
            )),
        }
    }
    assert!(errors.is_empty(), "站点车道与画像策略不一致：\n{}", errors.join("\n"));
    assert_eq!(counts.values().sum::<u64>(), 612, "覆盖站点数应为 612");
    println!("判据 64：612 站车道逐站符合画像策略 {counts:?}");
}
