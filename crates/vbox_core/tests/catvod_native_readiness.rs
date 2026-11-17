//! 判据 39：画像「strategy」与运行时「原生可服务」必须双向互锁（第 156 轮）。
//!
//! 背景：`profiles.json` 是审计报告 / 站点覆盖 / 真站矩阵的单一事实源；
//! `registry::is_native_ready` 与 `families::family_of` 是分派层判断「该 api 是否走原生实现」的两个入口。
//! 若三者漂移，覆盖结论就会失真，而且没有编译错误、没有运行错误，只有沉默的错误路由：
//! - 画像写 `rulepack` 但 `is_native_ready=false`：站点会被当作未移植，静默降级为分类错误；
//! - 画像写非 rulepack 但 `is_native_ready=true`：覆盖被高估，且分派可能走错家族分支；
//! - 画像写 rulepack 但 `family_of` 返回 None：`is_native_ready` 说能做，实际却进不了任何家族。
//!
//! 本判据用库内公开 API 对全部 214 个 api 做全量互锁，并顺带锁住 totals 计数与站点总量，
//! 避免手工改 `profiles.json` 时只改一边。

use std::collections::BTreeMap;

use vbox_core::spider::catvod::{families, registry};

/// 原生规则引擎（旧 lane）：由 `sources/spider.rs` 的 xyq/xbpq 分支直接处理，不进入 `families/`。
/// 依据同文件第 6 行注释与 98–103 行判定函数；此处显式列出，漂移时本判据会报红。
const LEGACY_RULE_ENGINES: [&str; 5] = [
    "csp_XBPQ",
    "csp_XBPQ_upgraded",
    "csp_XYQHiker",
    "csp_XYQHikerAL",
    "csp_XYQHikerV2",
];

fn legacy_engine(api: &str) -> bool {
    LEGACY_RULE_ENGINES.contains(&api.trim())
}

fn strategy_counts() -> (BTreeMap<&'static str, u32>, u32) {
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut sites_sum = 0u32;
    for (api, profile) in &registry::profiles().apis {
        *counts.entry(profile.strategy().as_str()).or_default() += 1;
        sites_sum += profile.sites;
        // 只有 jar 蜘蛛（csp_ 前缀）才有 class/jars；脚本与 CMS API 站点用 URL 直接寻址。
        if api.starts_with("csp_") {
            assert!(
                profile.class.as_deref().is_some_and(|c| !c.is_empty()),
                "{api} 缺 class"
            );
            // 已迁移/待迁移的 api 才需要难度标注（unsupported 的守卫壳/空壳类不做标注）。
            if matches!(profile.strategy().as_str(), "rulepack" | "unported") {
                assert!(
                    profile.difficulty().is_some(),
                    "{api} 的 difficulty 缺失或无法解析: {:?}",
                    profile.difficulty
                );
            }
        }
    }
    (counts, sites_sum)
}

#[test]
fn strategy_counts_and_site_totals_match_totals_block() {
    let p = registry::profiles();
    let (counts, sites_sum) = strategy_counts();

    assert_eq!(
        p.totals.distinct_apis as usize,
        p.apis.len(),
        "totals.distinctApis 与 apis 表长度不一致"
    );
    assert_eq!(
        sites_sum, p.totals.sites,
        "各 api 的 sites 之和必须等于 totals.sites（每个站点恰好归属一个 api）"
    );
    assert_eq!(
        p.jars.len() as u32, p.totals.jars,
        "画像 jars 数与 totals.jars 不一致"
    );
    for key in ["rulepack", "unported", "script", "unsupported"] {
        assert_eq!(
            counts.get(key).copied().unwrap_or(0),
            p.totals.strategies.get(key).copied().unwrap_or(0),
            "strategy={key} 的 apis 计数与 totals.strategies 不一致"
        );
    }
    let sum_counts: u32 = counts.values().sum();
    assert_eq!(
        sum_counts,
        p.totals.distinct_apis,
        "四类 strategy 计数之和必须等于 distinctApis"
    );
}

#[test]
fn native_readiness_is_locked_to_rulepack_strategy() {
    let p = registry::profiles();
    let mut rulepack_apis = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for (api, profile) in &p.apis {
        let strategy = profile.strategy().as_str();
        let ready = registry::is_native_ready(api);
        let family = families::family_of(api);
        let blocked = registry::blocked_reason(api);
        let profile_blocked = profile.blocked_reason();

        match strategy {
            "rulepack" => {
                rulepack_apis += 1;
                if !ready {
                    errors.push(format!(
                        "{api}: 画像=rulepack，但 is_native_ready=false（该站点会被静默降级）"
                    ));
                }
                if family.is_none() && !legacy_engine(api) {
                    errors.push(format!(
                        "{api}: 画像=rulepack，但既无家族也无旧规则引擎分支（is_native_ready 与分派不一致）"
                    ));
                }
                if profile_blocked.is_some() {
                    errors.push(format!(
                        "{api}: 画像=rulepack，却仍带 blockedReason={:?}",
                        profile_blocked
                    ));
                }
            }
            "unported" => {
                if ready {
                    errors.push(format!(
                        "{api}: 画像=unported，但 is_native_ready=true（覆盖被高估）"
                    ));
                }
                if family.is_some() {
                    errors.push(format!(
                        "{api}: 画像=unported，但 family_of={:?}（分派会错误接管）",
                        family
                    ));
                }
                if profile_blocked.is_none() {
                    errors.push(format!("{api}: unported 必须给出 blockedReason"));
                }
                if blocked.is_none() {
                    errors.push(format!("{api}: registry::blocked_reason 应返回原因"));
                }
            }
            other => {
                if ready {
                    errors.push(format!(
                        "{api}: 画像={other}，但 is_native_ready=true（非 rulepack 不应原生服务）"
                    ));
                }
                if family.is_some() {
                    errors.push(format!(
                        "{api}: 画像={other}，但 family_of={:?}（家族不应接管非 rulepack api）",
                        family
                    ));
                }
                if profile_blocked.is_some() {
                    errors.push(format!(
                        "{api}: 画像={other}，不应带 blockedReason（该字段仅 unported 使用）"
                    ));
                }
            }
        }
    }

    assert!(
        errors.is_empty(),
        "画像 ↔ 运行时原生就绪状态漂移：\n{}",
        errors.join("\n")
    );
    assert_eq!(
        rulepack_apis as u32,
        p.totals.strategies.get("rulepack").copied().unwrap_or(0),
        "rulepack api 数量与 totals 不一致"
    );
    assert!(
        rulepack_apis >= 90,
        "原生实现 api 数量异常偏低: {rulepack_apis}"
    );
}
