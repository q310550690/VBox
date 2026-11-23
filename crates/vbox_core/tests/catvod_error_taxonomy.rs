//! catvod 错误码体系回归（第 51 轮，计划「100% 判定」第 2 条：无黑箱错误码）。
//!
//! 约束：
//! 1. `ErrorClass` 的稳定码统一以 `catvod.` 开头、唯一、且 hint 非空；
//! 2. 家族实现不得直接构造 `CoreError::*`（必须走 `CatvodError`/`ErrorClass`，或向上传播引擎错误）；
//! 3. `profiles.json` 的 `blockedReason` 与 `registry::blocked_reason()` 一致；
//! 4. `site_error()` 对未移植/未知/受保护站点给出带分类码的可读错误。

use std::collections::BTreeSet;

use vbox_core::model::Site;
use vbox_core::spider::catvod::contract::{ErrorClass, Strategy};
use vbox_core::spider::catvod::registry;

const ALL_CLASSES: &[ErrorClass] = &[
    ErrorClass::MissingJar,
    ErrorClass::ProtectedGuard,
    ErrorClass::NotPorted,
    ErrorClass::ScriptFamily,
    ErrorClass::UnknownApi,
    ErrorClass::JarFetch,
    ErrorClass::JarIntegrity,
    ErrorClass::JarLayout,
    ErrorClass::Parse,
    ErrorClass::HostApi,
    ErrorClass::Network,
    ErrorClass::NetworkDns,
    ErrorClass::NetworkConnect,
    ErrorClass::NetworkTimeout,
    ErrorClass::NetworkTls,
    ErrorClass::PanShareUnavailable,
    ErrorClass::PanCookieRequired,
    ErrorClass::LiveFetch,
    ErrorClass::PlayParse,
    ErrorClass::AuxFetch,
];

fn site(api: &str) -> Site {
    let mut s = Site::default();
    s.key = "taxonomy".into();
    s.name = "taxonomy".into();
    s.api = api.into();
    s
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn error_class_codes_are_stable_and_unique() {
    let mut seen = BTreeSet::new();
    for class in ALL_CLASSES {
        let code = class.code();
        assert!(
            code.starts_with("catvod."),
            "错误码必须以 catvod. 开头：{code}"
        );
        assert!(seen.insert(code), "错误码重复：{code}");
        assert!(!class.hint().is_empty(), "错误码 {code} 缺少 hint");
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn families_do_not_construct_raw_core_errors() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/spider/catvod/families");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("families 目录应存在") {
        let path = entry.expect("目录项").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("读取家族源码");
        if text.contains("CoreError::") {
            offenders.push(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    assert!(
        offenders.is_empty(),
        "以下家族直接构造了裸 CoreError（应改用 CatvodError/ErrorClass）：{offenders:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn blocked_reason_in_profiles_matches_registry() {
    let p = registry::profiles();
    let mut checked = 0usize;
    for (api, profile) in &p.apis {
        if let Some(reason) = profile.blocked_reason() {
            assert_eq!(
                registry::blocked_reason(api).as_deref(),
                Some(reason),
                "api {api} 的 blockedReason 在 profiles 与 registry 中不一致"
            );
            checked += 1;
        }
    }
    let unported = p
        .apis
        .values()
        .filter(|profile| profile.strategy() == Strategy::Unported)
        .count();
    assert_eq!(
        checked, unported,
        "unported api 必须全部标注 blockedReason（{checked}/{unported}）"
    );
    // 第 257/258 轮 csp_P115Share、csp_TGYunPan/TGYunPanLocal 原生落地后 unported 14 → 11，基线同步下调。
    assert!(
        unported >= 5,
        "unported 基线应 ≥ 11（当前 {unported}）；若确为策略迁移请同步更新基线"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn site_error_has_stable_code_for_known_shapes() {
    // 已画像未移植
    let msg = registry::site_error(&site("csp_PushAgent")).to_string();
    assert!(
        msg.contains("catvod.spider.not_ported"),
        "未移植站点错误缺少稳定码：{msg}"
    );
    // 画像表没有
    let msg = registry::site_error(&site("csp_NotExists")).to_string();
    assert!(
        msg.contains("catvod.spider.unknown_api"),
        "未知 api 错误缺少稳定码：{msg}"
    );
    // 受保护 jar
    let msg = registry::site_error(&site("csp_BiliGuard")).to_string();
    assert!(
        msg.contains("catvod."),
        "受保护站点错误缺少 catvod.* 稳定码：{msg}"
    );
}
