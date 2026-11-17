//! 阻断证据可追溯性回归（第 69 轮）。
//!
//! 目标：`profiles.json` 里每条 `blockedReason` 都必须能在画像生成器
//! `_rea/samples/inventory/gen-profiles.mjs` 的 `BLOCKED_REASONS` 中找到**带注释证据**的登记行，
//! 且生成器登记与画像完全一致。这样「为什么做不了」不再只存在于讨论/记忆里，而是可从仓库源码复核。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use vbox_core::spider::catvod::contract::Strategy;
use vbox_core::spider::catvod::registry;

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

fn generator_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../_rea/samples/inventory/gen-profiles.mjs")
}

/// 读取画像生成器源码；`_rea/` 不存在时返回 `None`（发布环境降级）。
fn generator_source() -> Option<String> {
    let path = generator_path();
    if !path.exists() {
        eprintln!("跳过：未找到画像生成器 {}", path.display());
        return None;
    }
    Some(std::fs::read_to_string(&path).expect("读取 gen-profiles.mjs"))
}

/// 解析生成器中 `["csp_X", "reason"]` 形式的登记行 → (api, reason, 是否带注释)。
fn parse_blocked_entries(src: &str) -> BTreeMap<String, (String, bool)> {
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.contains("const BLOCKED_REASONS") {
            inside = true;
            continue;
        }
        if inside && trimmed.starts_with("]);") {
            break;
        }
        if !inside || !trimmed.starts_with("[\"csp_") {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('"').collect();
        if parts.len() < 4 {
            continue;
        }
        let api = parts[1].to_string();
        let reason = parts[3].to_string();
        let commented = trimmed
            .split_once("//")
            .map(|(_, c)| c.trim().chars().count() >= 4)
            .unwrap_or(false);
        out.insert(api, (reason, commented));
    }
    out
}

#[test]
fn every_blocked_api_has_commented_evidence() {
    let Some(src) = generator_source() else {
        return;
    };
    let profiles = registry::profiles();
    let mut problems = Vec::new();
    for (api, profile) in &profiles.apis {
        let Some(reason) = profile.blocked_reason() else {
            continue;
        };
        let needle = format!("[\"{api}\", \"{reason}\"]");
        let Some(line) = src.lines().find(|line| line.contains(&needle)) else {
            problems.push(format!("{api}: 生成器缺少登记行 {needle}"));
            continue;
        };
        let Some((_, comment)) = line.split_once("//") else {
            problems.push(format!("{api}: 登记行缺少证据注释"));
            continue;
        };
        if comment.trim().chars().count() < 4 {
            problems.push(format!("{api}: 证据注释过短（{comment:?}）"));
        }
    }
    assert!(
        problems.is_empty(),
        "以下 api 的阻断归因缺少可复核证据：\n{}",
        problems.join("\n")
    );
}

#[test]
fn generator_blocked_entries_match_profiles() {
    let Some(src) = generator_source() else {
        return;
    };
    let entries = parse_blocked_entries(&src);
    let profiles = registry::profiles();
    let mut profile_map = BTreeMap::new();
    for (api, profile) in &profiles.apis {
        if let Some(reason) = profile.blocked_reason() {
            profile_map.insert(api.clone(), reason.to_string());
        }
    }
    let gen_map: BTreeMap<String, String> = entries
        .into_iter()
        .map(|(api, (reason, _))| (api, reason))
        .collect();
    assert_eq!(
        gen_map, profile_map,
        "生成器 BLOCKED_REASONS 与 profiles.json 的 blockedReason 集合必须完全一致"
    );
}

#[test]
fn blocked_reasons_are_known_and_cover_all_unported() {
    let profiles = registry::profiles();
    let unported = profiles
        .apis
        .values()
        .filter(|profile| profile.strategy() == Strategy::Unported)
        .count();
    let mut blocked = 0usize;
    let mut reasons = BTreeSet::new();
    for profile in profiles.apis.values() {
        if let Some(reason) = profile.blocked_reason() {
            blocked += 1;
            assert!(
                ALLOWED_BLOCKED.contains(&reason),
                "未登记的 blockedReason：{reason}"
            );
            reasons.insert(reason.to_string());
        }
    }
    assert_eq!(
        blocked, unported,
        "unported api 必须全部带 blockedReason（{blocked}/{unported}）"
    );
    // 第 257 轮 csp_P115Share（W12.1）、第 258 轮 csp_TGYunPan/TGYunPanLocal（W12.2）原生落地后 unported 14 → 11，
    // 基线随迁移同步下调；若确为策略迁移请继续同步更新。
    assert!(
        unported >= 5,
        "unported 基线应 ≥ 7（当前 {unported}）；若确为策略迁移请同步更新基线"
    );
    // 第 194 轮 csp_AppYs 并入 appysv2 后 player_resolver_aggregator 清零；第 258 轮 TGYunPan×2 落地后
    // decompiler_gap 也清零，当前为 3 种（engine_dependency / site_dead / webview_dependency）。
    assert!(
        reasons.len() >= 3,
        "阻断原因应覆盖多种类型（当前 {} 种）",
        reasons.len()
    );
}
