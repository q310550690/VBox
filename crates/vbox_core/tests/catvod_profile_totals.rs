//! 判据 61：画像 `totals.sites` 的可加性，以及文件头包络的语义自洽。
//!
//! 已有的 `catvod_profiles_consistency` 校验 distinctApis / jars / strategies 三个
//! totals 字段，但 **`totals.sites` 一直没有被任何判据重算**：它既是站点清单的
//! 汇总，又是 100% 覆盖判定的分母。这里补上三条：每个 api 的 sites 求和等于
//! totals.sites、站点清单行数等于 totals.sites、以及画像文件头（schema /
//! generatedAt / source）与 totals 的文本互相印证——生成器若改了计数却忘了
//! 同步 source 描述，本判据会红。

use std::path::Path;

use vbox_core::spider::catvod::registry;

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/vbox_core 应位于仓库两级目录下")
        .to_path_buf()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn totals_sites_equal_sum_and_inventory_rows() {
    let p = registry::profiles();
    let mut sum = 0u32;
    for (api, profile) in &p.apis {
        assert!(
            profile.sites >= 1,
            "api {api} 的 sites 为 0；当前基线所有 api 至少被一个站点引用"
        );
        sum += profile.sites;
    }
    assert_eq!(
        sum, p.totals.sites,
        "Σ apis[].sites 应等于 totals.sites（{sum} vs {}）",
        p.totals.sites
    );

    let sites_path = repo_root().join("_rea/samples/inventory/sites.json");
    let Ok(text) = std::fs::read_to_string(&sites_path) else {
        eprintln!("跳过站点清单行数核对：未找到 {}（仅本地开发环境有 _rea/）", sites_path.display());
        return;
    };
    let inventory: serde_json::Value = serde_json::from_str(&text).expect("sites.json 应为合法 JSON");
    let rows = inventory["sites"].as_array().expect("sites.json 的 sites 应为数组");
    assert_eq!(
        rows.len() as u32,
        p.totals.sites,
        "sites.json 的 sites 行数应等于 totals.sites"
    );
    println!("判据 61：Σ apis[].sites = {}，站点清单 {} 行，均等于 totals.sites", sum, rows.len());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn profile_envelope_is_coherent() {
    let raw_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/spider/catvod/profiles.json");
    let text = std::fs::read_to_string(&raw_path).expect("profiles.json 必须存在");
    let raw: serde_json::Value = serde_json::from_str(&text).expect("profiles.json 应为合法 JSON");

    assert_eq!(raw["schema"].as_u64(), Some(1), "画像 schema 必须为 1");
    let generated_at = raw["generatedAt"].as_str().expect("generatedAt 应为字符串");
    assert!(
        generated_at.len() == 10
            && generated_at.as_bytes()[4] == b'-'
            && generated_at.as_bytes()[7] == b'-',
        "generatedAt 必须是 YYYY-MM-DD，实际 {generated_at:?}"
    );
    let month: u32 = generated_at[5..7].parse().expect("月份应为数字");
    let day: u32 = generated_at[8..10].parse().expect("日期应为数字");
    assert!((1..=12).contains(&month), "generatedAt 月份越界：{generated_at}");
    assert!((1..=31).contains(&day), "generatedAt 日期越界：{generated_at}");
    assert!(
        generated_at[..4].parse::<u32>().map_or(false, |y| (2020..=2100).contains(&y)),
        "generatedAt 年份异常：{generated_at}"
    );

    let source = raw["source"].as_str().expect("source 应为字符串");
    assert!(!source.trim().is_empty(), "source 不能为空");
    assert!(
        source.contains("3 multi-repo roots"),
        "source 应声明三条多仓来源，实际 {source:?}"
    );

    let totals = &raw["totals"];
    let sites = totals["sites"].as_u64().expect("totals.sites 应为整数");
    let jars = totals["jars"].as_u64().expect("totals.jars 应为整数");
    assert!(
        source.contains(&format!("{sites} sites")),
        "source 的站点数与 totals.sites 不一致（source={source:?}, totals={sites}）"
    );
    assert!(
        source.contains(&format!("{jars} usable jars")),
        "source 的 jar 数与 totals.jars 不一致（source={source:?}, totals={jars}）"
    );

    // 编译期加载的画像必须与磁盘原文一致，防止 loader 默认值掩盖字段缺失。
    let loaded = registry::profiles();
    assert_eq!(loaded.schema, 1);
    assert_eq!(loaded.generated_at, generated_at);
    assert_eq!(loaded.source, source);
    assert_eq!(loaded.totals.sites, sites as u32);
    assert_eq!(loaded.totals.jars as u64, jars);
    println!("判据 61：schema=1、generatedAt={generated_at}、source 与 totals({sites} sites / {jars} jars) 互相印证");
}
