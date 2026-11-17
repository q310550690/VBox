//! 判据 40：`registry::classify` 的处置结论必须与 `profiles.json` 的策略画像一致。
//!
//! 为什么需要这个判据：
//! `sources/spider.rs` 的 5 个入口在原生家族与 js/py 运行时都未接管时，
//! 会调用 `registry::site_error()`；而 `site_error` 内部就是 `classify().error()`。
//! 也就是说：**用户最终看到的错误码/错误文案，完全由 `classify(&Site)` 决定**。
//! 而覆盖率口径、残余阻断归因、站点覆盖报告全都来自 `profiles.json`。
//! 两者若漂移（画像说 script、运行时判 unsupported 等），就会出现
//! 「报告覆盖率 X% 但用户看到另一套错误」的静默不一致。
//!
//! 本判据用画像里全部 214 个 api 逐一构造最小 `Site`，断言：
//! 1. `classify(site).strategy()` == 画像 `strategy`（脚本/URL/`csp_*` 都覆盖）；
//! 2. 变体内部字段（unported 的 class/jars/difficulty，unsupported 的 reason）与画像一致；
//! 3. `classify(site).error()` 的错误码属于 20 类 ErrorClass，且与策略对应；
//! 4. 空 api 的边界（无 jar→Unknown，有 jar→Unsupported/missing_api）。

use vbox_core::model::Site;
use vbox_core::spider::catvod::contract::{ErrorClass, Strategy};
use vbox_core::spider::catvod::registry::{classify, lookup, profiles, Classification};

/// 构造一个「只用于分类」的最小站点：csp_ 类站点带 jar，脚本类不带。
fn site_for(api: &str) -> Site {
    Site {
        key: "probe".into(),
        name: "probe".into(),
        kind: 3,
        api: api.to_string(),
        jar: if api.starts_with("csp_") {
            "probe.jar".into()
        } else {
            String::new()
        },
        ..Default::default()
    }
}

/// 策略 → 期望的 ErrorClass（`classify().error()` 的映射见 registry.rs）。
fn expected_error_class(api: &str) -> ErrorClass {
    match lookup(api).map(|p| p.strategy()) {
        Some(Strategy::Unported) => ErrorClass::NotPorted,
        Some(Strategy::Unsupported) => match lookup(api).and_then(|p| p.reason.as_deref()) {
            Some("protected_guard") => ErrorClass::ProtectedGuard,
            Some("unknown_api") => ErrorClass::UnknownApi,
            Some("class_not_in_downloaded_jars") => ErrorClass::MissingJar,
            _ => ErrorClass::HostApi,
        },
        // rulepack / script 本不该走 error 路径；若走到，按 registry 的实现是 HostApi。
        Some(Strategy::Rulepack) | Some(Strategy::Script) => ErrorClass::HostApi,
        // 画像里没有的 api：classify→Unknown，error→UnknownApi。
        None => ErrorClass::UnknownApi,
    }
}

#[test]
fn classification_strategy_matches_profile() {
    let profiles = profiles();
    assert!(
        profiles.apis.len() >= 200,
        "画像 api 数量异常：{}",
        profiles.apis.len()
    );
    let mut mismatches = Vec::new();
    for (api, profile) in &profiles.apis {
        let got = classify(&site_for(api)).strategy();
        let want = profile.strategy();
        if got != want {
            mismatches.push(format!(
                "{api}: classify={:?} profile={:?}",
                got.as_str(),
                want.as_str()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "classify 与画像策略漂移 {} 项：\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn classification_variant_details_match_profile() {
    let profiles = profiles();
    for (api, profile) in &profiles.apis {
        match classify(&site_for(api)) {
            Classification::Unported {
                class,
                jars,
                difficulty,
                ..
            } => {
                assert_eq!(
                    Some(class),
                    profile.class.clone(),
                    "{api}: unported class 与画像不一致"
                );
                assert_eq!(jars, profile.jars, "{api}: unported jars 与画像不一致");
                assert_eq!(
                    difficulty.map(|d| d.as_str().to_string()),
                    profile.difficulty.clone(),
                    "{api}: unported difficulty 与画像不一致"
                );
            }
            Classification::Unsupported { reason, .. } => {
                let want = profile
                    .reason
                    .clone()
                    .unwrap_or_else(|| "unsupported_by_profile".to_string());
                assert_eq!(reason, want, "{api}: unsupported reason 与画像不一致");
            }
            Classification::Rulepack { .. } | Classification::Script { .. } => {}
            Classification::Unknown { .. } => {
                panic!("{api}: 画像里存在却分类为 Unknown");
            }
        }
    }
}

#[test]
fn classification_errors_are_taxonomy_coded() {
    let profiles = profiles();
    let mut problems = Vec::new();
    for api in profiles.apis.keys() {
        let site = site_for(api);
        let err = classify(&site).error(&site);
        let want = expected_error_class(api);
        match ErrorClass::detect(&err.to_string()) {
            Some(got) if got == want => {}
            other => problems.push(format!(
                "{api}: 期望 {:?}，实际 {:?}（{}）",
                want.code(),
                other.map(|c| c.code()),
                err
            )),
        }
    }
    assert!(
        problems.is_empty(),
        "分类错误码漂移 {} 项：\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn empty_api_boundaries() {
    // 无 api、无 jar → Unknown（画像表里没有可查项）。
    let site = Site {
        key: "empty".into(),
        name: "empty".into(),
        kind: 3,
        ..Default::default()
    };
    assert!(matches!(classify(&site), Classification::Unknown { .. }));
    assert_eq!(
        ErrorClass::detect(&classify(&site).error(&site).to_string()),
        Some(ErrorClass::UnknownApi)
    );

    // 有 jar 但无 api → 明确 Unsupported/missing_api。
    let site = Site {
        jar: "some.jar".into(),
        ..site.clone()
    };
    match classify(&site) {
        Classification::Unsupported { reason, .. } => assert_eq!(reason, "missing_api"),
        other => panic!("期望 Unsupported/missing_api，实际 {other:?}"),
    }
    assert_eq!(
        ErrorClass::detect(&classify(&site).error(&site).to_string()),
        Some(ErrorClass::HostApi)
    );
}
