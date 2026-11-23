//! `profiles.json` 画像一致性回归（第 50 轮）。
//!
//! 目标：把「画像与家族接线/站点清单/阻断归因」之间的不变量固化成测试，
//! 防止后续改动让 100% 兼容性判定悄悄漂移：
//! 1. totals 与 apis/jars 表自洽；
//! 2. `unported` 100% 带 `blockedReason`（第 49 轮收口后的硬约束）；
//! 3. `blockedReason` 只允许出现已登记枚举；
//! 4. `existing_native_implementation` 的 api 必须真实接线（family_of + is_native_ready）；
//! 5. 所有被打上家族分支的 api 必须 `is_native_ready`；
//! 6. `rulepack` 必须有难度分档；
//! 7. 若本机存在 `_rea/samples/inventory/sites.json`，则站点清单的 api 必须都在画像里。

use std::collections::BTreeSet;

use vbox_core::spider::catvod::{families, registry};

const ALLOWED_BLOCKED: &[&str] = &[
    "decompiler_gap",
    "webview_dependency",
    "engine_dependency",
    "login_required",
    "crypto_dependency",
    "site_dead",
    "anti_bot",
    "xpath_rule_engine",
    "player_resolver_aggregator",
];

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn totals_match_api_and_jar_tables() {
    let p = registry::profiles();
    assert_eq!(
        p.totals.distinct_apis as usize,
        p.apis.len(),
        "totals.distinctApis 应等于 apis 表条目数"
    );
    assert_eq!(
        p.totals.jars as usize,
        p.jars.len(),
        "totals.jars 应等于 jars 表条目数"
    );
    let mut sum = 0usize;
    for (name, count) in &p.totals.strategies {
        sum += *count as usize;
        let actual = p
            .apis
            .values()
            .filter(|a| a.strategy == *name)
            .count();
        assert_eq!(
            actual, *count as usize,
            "totals.strategies[{name}] 与 apis 表统计不一致"
        );
    }
    assert_eq!(sum, p.apis.len(), "策略合计应覆盖全部 api");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn unported_apis_all_carry_a_blocked_reason() {
    let p = registry::profiles();
    for (api, profile) in &p.apis {
        if profile.strategy == "unported" {
            assert!(
                profile.blocked_reason.is_some(),
                "unported api {api} 缺少 blockedReason（第 49 轮起为硬约束）"
            );
        }
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn blocked_reasons_use_known_vocabulary() {
    let allowed: BTreeSet<&str> = ALLOWED_BLOCKED.iter().copied().collect();
    let p = registry::profiles();
    for (api, profile) in &p.apis {
        if let Some(reason) = profile.blocked_reason() {
            assert!(
                allowed.contains(reason),
                "api {api} 的 blockedReason=`{reason}` 不在登记枚举内"
            );
        }
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn native_apis_are_wired_to_a_family() {
    let p = registry::profiles();
    let mut checked = 0usize;
    for (api, profile) in &p.apis {
        if profile.reason.as_deref() == Some("existing_native_implementation") {
            assert!(
                registry::is_native_ready(api),
                "native api {api} 未在 registry::is_native_ready 中登记"
            );
            assert!(
                families::family_of(api).is_some(),
                "native api {api} 未接到任何家族分派（families::family_of 返回 None）"
            );
            checked += 1;
        }
    }
    assert!(checked >= 54, "native api 数量不应低于 54，实际 {checked}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn family_dispatch_targets_are_native_ready() {
    let p = registry::profiles();
    let mut wired = 0usize;
    for api in p.apis.keys() {
        if families::family_of(api).is_some() {
            assert!(
                registry::is_native_ready(api),
                "api {api} 有家族分派但 is_native_ready=false"
            );
            wired += 1;
        }
    }
    assert!(wired >= 54, "已接线的 api 数不应低于 54，实际 {wired}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rulepack_apis_have_difficulty() {
    let p = registry::profiles();
    for (api, profile) in &p.apis {
        if profile.strategy == "rulepack" {
            assert!(
                profile.difficulty().is_some(),
                "rulepack api {api} 缺少 difficulty"
            );
        }
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn sites_json_references_only_known_apis() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../_rea/samples/inventory/sites.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("跳过：未找到 {}（仅本地开发环境有 _rea/）", path.display());
        return;
    };
    let value: serde_json::Value = serde_json::from_str(&text).expect("sites.json 应为合法 JSON");
    let sites = value["sites"].as_array().expect("sites 字段应为数组");
    let p = registry::profiles();
    for site in sites {
        // 注意：sites.json 来自原始多仓配置，个别 api 字符串带尾随空格
        //（如 `https://ikunzyapi.com/api.php/provide/vod `），按 trim 后匹配；
        // 这类站点在生成画像时按 script 计，属已知数据瑕疵，不阻断一致性。
        let api = site["api"].as_str().unwrap_or_default().trim();
        assert!(
            p.apis.contains_key(api),
            "站点清单引用了画像中不存在的 api：`{api}`"
        );
    }
    assert_eq!(
        p.totals.sites as usize,
        sites.len(),
        "profiles.totals.sites 与 sites.json 条目数不一致"
    );
}
