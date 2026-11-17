//! 判据 68：画像的阻断原因必须到达运行时错误消息（而不是只活在台账/测试里）。
//!
//! 背景：第 153–185 轮把 unported 归因固化进画像与归因矩阵，判据 37 锁住了
//! 「画像 ↔ 文档矩阵」；但运行时 `site_error` 过去只输出「难度 ?」与 jar 名，
//! `blocked_reason()` 这个公开函数甚至没有任何调用方 —— 用户看不到「为什么不能播」。
//!
//! 本判据把三层锁在一起：
//! - 画像 unported 的 `blockedReason` / unsupported 的 `reason` 非空；
//! - `registry::site_error` 的消息包含该原因与一句可执行提示；
//! - 提示不是兜底句（说明每个原因都真的被分类），且每个已知原因都能给出专属提示。

use std::collections::BTreeSet;

use vbox_core::spider::catvod::registry::{self, blocked_reason_hint, profiles};
use vbox_core::spider::catvod::contract::ErrorClass;
use vbox_core::model::Site;

fn site(api: &str) -> Site {
    Site {
        key: api.to_string(),
        name: api.to_string(),
        kind: 3,
        api: api.to_string(),
        ..Default::default()
    }
}

/// 每个已知阻断/不支持原因的关键词；新增原因必须同时改 `blocked_reason_hint` 与这里。
fn keyword(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "site_dead" => "失效",
        "engine_dependency" => "引擎",
        "webview_dependency" => "WebView",
        "decompiler_gap" => "反编译",
        "login_required" => "登录",
        "crypto_dependency" => "加密",
        "player_resolver_aggregator" => "解析器",
        "protected_guard" => "保护",
        "unknown_api" => "未出现",
        "class_not_in_downloaded_jars" => "jar",
        "missing_api" => "缺少 api",
        "android_app_market_semantics" => "应用市场",
        "config_center_semantics" => "配置中心",
        "ali_signin_utility" => "登录",
        _ => return None,
    })
}

fn assert_surfaced(api: &str, reason: &str, message: &str) {
    assert!(
        message.contains(reason),
        "{api} 的错误消息未包含阻断原因 {reason}：{message}"
    );
    let kw = keyword(reason).unwrap_or_else(|| panic!("未知阻断原因 {reason}（{api}），请同步判据与提示表"));
    assert!(
        message.contains(kw),
        "{api} 的错误消息缺少原因 {reason} 的提示关键词「{kw}」：{message}"
    );
    assert!(
        !message.contains("尚未分类的阻断原因"),
        "{api} 落到了兜底提示（原因 {reason}）：{message}"
    );
}

#[test]
fn unported_errors_surface_blocked_reason_and_hint() {
    let p = profiles();
    let mut seen = BTreeSet::new();
    let mut count = 0usize;
    for (api, profile) in &p.apis {
        if profile.strategy != "unported" {
            continue;
        }
        let reason = profile
            .blocked_reason
            .as_deref()
            .unwrap_or_else(|| panic!("unported api 缺少 blockedReason：{api}"));
        let msg = registry::site_error(&site(api)).to_string();
        assert!(
            msg.contains(ErrorClass::NotPorted.code()),
            "{api} 的错误码不是 {}：{msg}",
            ErrorClass::NotPorted.code()
        );
        assert_surfaced(api, reason, &msg);
        seen.insert(reason.to_string());
        count += 1;
    }
    assert_eq!(count, 5, "unported api 数应恰为 5，实际 {count}");
    assert_eq!(seen.len(), 3, "unported 阻断原因种类应恰为 3，实际 {:?}", seen);
    // 反向：提示表本身对每个已知原因都必须给出专属文案。
    for reason in seen {
        assert_ne!(
            blocked_reason_hint(&reason),
            blocked_reason_hint("totally_unknown_reason_xyz"),
            "原因 {reason} 没有专属提示"
        );
    }
}

#[test]
fn unsupported_errors_surface_profile_reason_and_hint() {
    let p = profiles();
    let mut checked = 0usize;
    for (api, profile) in &p.apis {
        if profile.strategy != "unsupported" {
            continue;
        }
        // 判定 66 已保证 unsupported 恰好有 reason；这里再断言一次，防画像回退。
        let reason = profile
            .reason
            .as_deref()
            .unwrap_or_else(|| panic!("unsupported api 缺少 reason：{api}"));
        let msg = registry::site_error(&site(api)).to_string();
        assert_surfaced(api, reason, &msg);
        checked += 1;
    }
    assert!(checked > 0, "画像里没有 unsupported api，判据失去意义");
}
