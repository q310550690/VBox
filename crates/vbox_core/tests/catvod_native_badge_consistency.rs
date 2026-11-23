//! 判据 62：`is_native_ready` 徽章 ↔ 画像策略 ↔ 家族分派的三方等价。
//!
//! 现有判据只覆盖单向接线：`reason=existing_native_implementation` ⇒ 家族分派
//! （profiles_consistency），以及家族分派 ⇒ `is_native_ready`（同上）。但反过来
//! **没有任何判据约束徽章本身**——`is_native_ready` 里多加一个 api 名、或者
//! 画像把某个 rulepack 降级成 unported，都不会变红。本判据把三张表锁成：
//!
//! - 画像 `strategy=rulepack`（93）≡ 徽章列表减去两个不在画像中的别名；
//! - 徽章里除别名外的每个名字，在画像中必须恰好是 rulepack；
//! - rulepack 中三个原生规则引擎（XBPQ / XYQHiker / XYQHikerAL）不归家族，
//!   其余每个 rulepack api 必须有且仅有一个家族分派。

use std::collections::BTreeSet;
use std::path::Path;

use vbox_core::spider::catvod::{families, registry};

/// 只存在于 `is_native_ready`、不在画像里的兼容别名。
/// 它们对应旧转置产物或站点配置里的别名写法，没有独立画像条目。
const ALIASES: &[&str] = &["csp_XBPQ_upgraded", "csp_XYQHikerV2"];

/// 三个走原生规则引擎（非家族模块）的 api。
const ENGINES: &[&str] = &["csp_XBPQ", "csp_XYQHiker", "csp_XYQHikerAL"];

fn badge_names() -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/spider/catvod/registry.rs");
    let text = std::fs::read_to_string(&path).expect("registry.rs 必须存在");
    let start = text.find("pub fn is_native_ready").expect("is_native_ready 必须存在");
    let end = text[start..]
        .find("\n}")
        .map(|i| start + i)
        .expect("is_native_ready 函数体必须闭合");
    text[start..end]
        .match_indices('"')
        .map(|(i, _)| i)
        .collect::<Vec<_>>()
        .chunks(2)
        .filter_map(|pair| {
            if pair.len() == 2 {
                Some(text[start..].get(pair[0] + 1..pair[1]).unwrap_or_default().to_string())
            } else {
                None
            }
        })
        .filter(|s| s.starts_with("csp_"))
        .collect()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn badge_and_rulepack_profiles_are_two_way_consistent() {
    let badges = badge_names();
    assert!(badges.len() >= 90, "徽章数量异常（{}），疑似解析失败", badges.len());
    for alias in ALIASES {
        assert!(badges.contains(*alias), "别名 {alias} 必须保留在徽章列表中");
        assert!(
            !registry::profiles().apis.contains_key(*alias),
            "别名 {alias} 不应出现在画像里（它只用于运行时兼容）"
        );
    }

    let p = registry::profiles();
    let rulepack: BTreeSet<&str> = p
        .apis
        .iter()
        .filter(|(_, a)| a.strategy == "rulepack")
        .map(|(k, _)| k.as_str())
        .collect();

    // 画像里所有 rulepack 必须带徽章，否则原生判定与运行期分派脱节。
    for api in &rulepack {
        assert!(badges.contains(*api), "rulepack api {api} 未登记在 is_native_ready 徽章列表");
    }

    // 徽章里除别名外的每个名字，画像必须存在且策略就是 rulepack。
    for api in &badges {
        if ALIASES.contains(&api.as_str()) {
            continue;
        }
        let profile = p
            .apis
            .get(api)
            .unwrap_or_else(|| panic!("徽章 api {api} 在画像中不存在（陈旧徽章）"));
        assert_eq!(
            profile.strategy, "rulepack",
            "徽章 api {api} 的画像策略应为 rulepack，实际 {}",
            profile.strategy
        );
    }

    let badge_without_alias: BTreeSet<&str> = badges
        .iter()
        .map(String::as_str)
        .filter(|a| !ALIASES.contains(a))
        .collect();
    assert_eq!(
        badge_without_alias, rulepack,
        "徽章（去别名）与画像 rulepack 集合必须完全相等"
    );
    println!("判据 62：徽章 {} 个 = rulepack {} 个 + 别名 {} 个，双向一致", badges.len(), rulepack.len(), ALIASES.len());
}

fn engine_dispatch_names() -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sources/spider.rs");
    let text = std::fs::read_to_string(&path).expect("sources/spider.rs 必须存在");
    let mut names = BTreeSet::new();
    for fname in ["fn api_is_xyq", "fn api_is_xbpq"] {
        let start = text.find(fname).unwrap_or_else(|| panic!("{fname} 必须存在"));
        let end = text[start..].find("\n}").map(|i| start + i).expect("函数体必须闭合");
        let body = &text[start..end];
        for cap in body.split('"').skip(1).step_by(2) {
            if cap.starts_with("csp_") {
                names.insert(cap.to_string());
            }
        }
    }
    names
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn engine_aliases_agree_across_badge_dispatch_and_lane() {
    use vbox_core::model::Site;
    use vbox_core::sources::spider::{self, DispatchLane};

    let badges = badge_names();
    let p = registry::profiles();
    let profile_keys: BTreeSet<&str> = p.apis.keys().map(String::as_str).collect();
    let badge_only: BTreeSet<String> = badges
        .iter()
        .filter(|a| !profile_keys.contains(a.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        badge_only,
        ALIASES.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        "徽章中不在画像里的别名集合必须恰好是已知的两个兼容别名"
    );

    let engines: BTreeSet<String> = p
        .apis
        .iter()
        .filter(|(_, a)| a.reason.as_deref() == Some("existing_native_rule_engine"))
        .map(|(k, _)| k.clone())
        .collect();
    assert_eq!(engines.len(), 3, "画像中的原生规则引擎应恰好 3 个");

    let dispatch = engine_dispatch_names();
    let expected: BTreeSet<String> = engines.union(&badge_only).cloned().collect();
    assert_eq!(
        dispatch, expected,
        "api_is_xyq/api_is_xbpq 的引擎名单必须等于「画像 3 引擎 + 2 个别名」"
    );

    for api in &dispatch {
        let site = Site {
            key: format!("probe-{api}"),
            name: format!("probe-{api}"),
            kind: 3,
            api: api.clone(),
            ..Default::default()
        };
        assert_eq!(
            registry::classify(&site).strategy(),
            vbox_core::spider::catvod::contract::Strategy::Rulepack,
            "引擎 {api} 必须被 classify 判为 rulepack"
        );
        assert_eq!(
            spider::dispatch_lane(&site),
            DispatchLane::Native,
            "引擎 {api} 在报告车道投影中必须是 Native（否则覆盖统计与运行时分派相反）"
        );
    }
    println!("判据 62：引擎 {} = 画像 3 引擎 + 别名 {}，徽章/helper/车道三方一致", dispatch.len(), ALIASES.len());
}
