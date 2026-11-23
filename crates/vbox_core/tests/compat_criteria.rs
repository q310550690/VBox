//! 100% 原生兼容的「可执行判据」回归（对应 `docs/type3兼容/type3-实施计划-原生兼容100%.md` 的 100% 判定五条中
//! 能在 crate 内静态验证的部分）。
//!
//! 这里不重新发明数据：画像本身就是契约（`profiles.json` 随 crate 编译），
//! 测试只断言「画像自洽 + 代码接线齐全 + 测试资产齐全」三条硬约束，任何一条被破坏即说明
//! 兼容性出现回退（新增 api 未归类、家族未接线、家族无回归测试等）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use std::sync::OnceLock;

use vbox_core::spider::catvod::families as catvod_families;
use vbox_core::spider::catvod::registry;

fn profiles() -> &'static serde_json::Value {
    static P: OnceLock<serde_json::Value> = OnceLock::new();
    P.get_or_init(|| {
        serde_json::from_str(include_str!("../src/spider/catvod/profiles.json"))
            .expect("profiles.json 不是合法 JSON")
    })
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// C1 归属完备：每个 api 都有 strategy，且非脚本/非规则引擎者必须有归属
/// （native 实现、unported 带 blockedReason、或规则引擎）。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_api_is_classified() {
    let prof = profiles();
    let apis = prof["apis"].as_object().expect("profiles.apis");
    assert!(apis.len() >= 200, "api 数量异常：{}", apis.len());
    for (name, item) in apis {
        let strategy = item["strategy"].as_str().unwrap_or("");
        assert!(
            matches!(strategy, "rulepack" | "unported" | "script" | "unsupported"),
            "{name}: 未知 strategy={strategy}"
        );
        let reason = item["reason"].as_str().unwrap_or("");
        match strategy {
            "rulepack" => assert!(
                reason == "existing_native_implementation"
                    || reason == "existing_native_rule_engine"
                    || item["reason"].is_null(),
                "{name}: rulepack 必须有原生归属，实际 reason={reason}"
            ),
            "unported" => assert!(
                item["blockedReason"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
                "{name}: unported 必须带 blockedReason"
            ),
            _ => {}
        }
    }
}

/// C2 覆盖基线：原生家族 ≥37、native api ≥54、unported 全部带阻断原因。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn coverage_baseline_is_not_regressing() {
    let prof = profiles();
    let apis = prof["apis"].as_object().unwrap();
    let native: Vec<&String> = apis
        .iter()
        .filter(|(_, v)| {
            v["reason"].as_str() == Some("existing_native_implementation")
                || v["reason"].as_str() == Some("existing_native_rule_engine")
        })
        .map(|(k, _)| k)
        .collect();
    assert!(native.len() >= 54, "native api 少于 54：{}", native.len());
    let unported: Vec<&String> = apis
        .iter()
        .filter(|(_, v)| v["strategy"].as_str() == Some("unported"))
        .map(|(k, _)| k)
        .collect();
    assert!(!unported.is_empty());
    for name in &unported {
        assert!(
            apis[*name]["blockedReason"].as_str().is_some(),
            "{name}: unported 缺 blockedReason"
        );
    }
}

/// C3 家族测试资产：`families/` 下每个 `pub mod x;` 都必须有 `tests/catvod_x_family.rs`
/// （或该家族的专属回归文件），保证「家族等价 mock 回归」可复现。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_family_has_regression_test() {
    let families_dir = crate_root().join("src/spider/catvod/families");
    let tests_dir = crate_root().join("tests");
    let mod_rs = std::fs::read_to_string(families_dir.join("mod.rs")).expect("families/mod.rs");
    let mut families = BTreeSet::new();
    for line in mod_rs.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pub mod ") {
            let name = rest.trim_end_matches(';').trim();
            if !name.is_empty() && name != "mod" {
                families.insert(name.to_string());
            }
        }
    }
    assert!(families.len() >= 37, "家族数量异常：{}", families.len());
    let missing: Vec<&String> = families
        .iter()
        .filter(|f| !family_test_file_exists(&tests_dir, f))
        .collect();
    assert!(missing.is_empty(), "缺少家族回归测试：{missing:?}");
}

fn family_test_file_exists(tests_dir: &Path, family: &str) -> bool {
    let exact = tests_dir.join(format!("catvod_{family}_family.rs"));
    if exact.exists() {
        return true;
    }
    // 少数家族用共享测试文件覆盖（如 share 由 catvod_share_family.rs，
    // tzfile/moli 等均按家族命名）；这里只接受显式存在，不做模糊匹配。
    false
}

/// C4 真实站点回归资产：`#[ignore]` 真站用例数量 ≥ 25（当前 30 项），
/// 保证「全链可复现」有真实站点证据而不是只有 mock。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn live_regression_assets_exist() {
    let tests_dir = crate_root().join("tests");
    let mut ignored = 0usize;
    for entry in std::fs::read_dir(&tests_dir).expect("tests dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        ignored += text.matches("#[ignore").count();
    }
    assert!(ignored >= 25, "真站 #[ignore] 用例少于 25：{ignored}");
}

/// 家族分派覆盖：`family_of` 对每个 native 家族名都能给出 Some，
/// 且 `registry::is_native_ready` 与画像一致（防止「画像说已移植但运行时不走原生」）。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn family_of_and_registry_agree() {
    let prof = profiles();
    let apis = prof["apis"].as_object().unwrap();
    let mut checked = 0usize;
    for (name, item) in apis {
        if item["reason"].as_str() != Some("existing_native_implementation") {
            continue;
        }
        checked += 1;
        assert!(
            registry::is_native_ready(name),
            "{name}: 画像标记 native，但 registry::is_native_ready=false"
        );
        assert!(
            catvod_families::family_of(name).is_some(),
            "{name}: 画像标记 native，但 family_of 无归属"
        );
    }
    assert!(checked >= 67, "native api 数量下降：{checked}");
}

/// C6 家族可达性：`families/mod.rs` 里声明的每个家族模块都必须至少被一个 api 命中
/// （防止「新增家族模块却忘接线」或「接线写错导致永远走不到」），
/// 同时 `family_of` 不得返回未声明的家族名。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn every_family_module_is_reachable() {
    let prof = profiles();
    let apis = prof["apis"].as_object().expect("profiles.apis");
    let mod_src = std::fs::read_to_string(
        crate_root().join("src/spider/catvod/families/mod.rs"),
    )
    .expect("读取 families/mod.rs 失败");
    let mut declared = BTreeSet::new();
    for line in mod_src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pub mod ") {
            let name = rest.trim_end_matches(';').trim();
            if !name.is_empty() {
                declared.insert(name.to_string());
            }
        }
    }
    assert!(declared.len() >= 37, "家族模块数量异常：{}", declared.len());

    let mut reachable = BTreeSet::new();
    for name in apis.keys() {
        if let Some(family) = catvod_families::family_of(name) {
            reachable.insert(family.to_string());
        }
    }
    let orphans: Vec<String> = declared.difference(&reachable).cloned().collect();
    assert!(
        orphans.is_empty(),
        "存在未被任何 api 命中的家族模块（忘接线或接线写错）：{orphans:?}"
    );
    let stray: Vec<String> = reachable.difference(&declared).cloned().collect();
    assert!(
        stray.is_empty(),
        "family_of 返回了未声明的家族：{stray:?}"
    );
}

/// 家族裁决一致性（分派无歧义）：对画像里每个 api，
/// - `family_of` 选中的家族必须自己 `handles(api) == true`（否则线上走原生却永远进不去）；
/// - 不允许任何其它家族也匹配同一 api（首个匹配胜出，后置分支为死逻辑，容易在重排时静默改变归属）。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn family_dispatch_is_unambiguous() {
    const FAMILIES: [&str; 77] = ["alist", "anime1", "appget", "appld", "appmao", "appsk", "bttwoo", "apptt", "appysv2", "auete", "bili", "dm84", "ddrk", "douban", "duanju", "firstaid", "huban", "jianpian", "jpys", "iktv", "kugou", "kunyu77", "kanqiu", "libvio", "meijumi", "moli", "mp4mov", "nangua", "p115share", "pushshare",
    "panyq", "pan", "pansearch", "quark", "sixv", "sp360", "tangrenjie", "xb6v", "ying", "yinyuetai", "yqkan", "ysj", "zxzj", "tzfile", "share", "ttian", "xiaozhang", "wogg", "xpanpan", "yingso", "yunpan", "fourkhdr", "push", "xinsj", "seedhub", "yj1211", "liteapple", "xpath", "nmys", "nini", "bdys01", "xiaozhitiao", "czsapp", "gitcafe", "kuaikan", "alishare", "bt0", "gaoqing", "zhaozy", "hdhive", "xbiu", "kkys", "tgyunpan", "pikpakshare", "thundershare", "sambashare", "youtube"];
    let apis = profiles()["apis"].as_object().expect("profiles.apis");
    let mut checked = 0usize;
    let mut unhandled: Vec<&String> = Vec::new();
    for name in apis.keys() {
        let family = catvod_families::family_of(name);
        let matched: Vec<&str> = FAMILIES
            .iter()
            .copied()
            .filter(|f| family_handles(f, name))
            .collect();
        match family {
            Some(f) => {
                checked += 1;
                assert!(
                    matched.contains(&f),
                    "{name}: family_of 选中 {f}，但该家族 handles=false（线上进不了原生）"
                );
                assert!(
                    matched.len() == 1,
                    "{name}: 被多个家族匹配 {matched:?}（分派歧义，接线顺序决定归属）"
                );
            }
            None => {
                assert!(
                    matched.is_empty(),
                    "{name}: 画像未标记 native，但被家族 {matched:?} 匹配（handles 覆盖了未归类 api）"
                );
                unhandled.push(name);
            }
        }
    }
    assert!(checked >= 67, "被原生分派的 api 数量下降：{checked}");
    assert!(!unhandled.is_empty(), "所有 api 都应被分派");
}

fn family_handles(family: &str, api: &str) -> bool {
    match family {
        "alist" => catvod_families::alist::handles(api),
        "p115share" => catvod_families::p115share::handles(api),
        "pushshare" => catvod_families::pushshare::handles(api),
        "panyq" => catvod_families::panyq::handles(api),
        "sambashare" => catvod_families::sambashare::handles(api),
        "thundershare" => catvod_families::thundershare::handles(api),
        "tgyunpan" => catvod_families::tgyunpan::handles(api),
        "pikpakshare" => catvod_families::pikpakshare::handles(api),
        "youtube" => catvod_families::youtube::handles(api),
        "anime1" => catvod_families::anime1::handles(api),
        "appget" => catvod_families::appget::handles(api),
        "appmao" => catvod_families::appmao::handles(api),
        "appld" => catvod_families::appld::handles(api),
        "appsk" => catvod_families::appsk::handles(api),
                "bttwoo" => catvod_families::bttwoo::handles(api),
                "yunpan" => catvod_families::yunpan::handles(api),
        "fourkhdr" => catvod_families::fourkhdr::handles(api),
        "push" => catvod_families::push::handles(api),
        "xinsj" => catvod_families::xinsj::handles(api),
        "seedhub" => catvod_families::seedhub::handles(api),
        "yj1211" => catvod_families::yj1211::handles(api),
        "liteapple" => catvod_families::liteapple::handles(api),
        "xpath" => catvod_families::xpath::handles(api),
        "nmys" => catvod_families::nmys::handles(api),
        "nini" => catvod_families::nini::handles(api),
        "bdys01" => catvod_families::bdys01::handles(api),
        "xiaozhitiao" => catvod_families::xiaozhitiao::handles(api),
        "czsapp" => catvod_families::czsapp::handles(api),
        "gitcafe" => catvod_families::gitcafe::handles(api),
        "kuaikan" => catvod_families::kuaikan::handles(api),
        "alishare" => catvod_families::alishare::handles(api),
        "bt0" => catvod_families::bt0::handles(api),
        "gaoqing" => catvod_families::gaoqing::handles(api),
        "zhaozy" => catvod_families::zhaozy::handles(api),
        "hdhive" => catvod_families::hdhive::handles(api),
        "xbiu" => catvod_families::xbiu::handles(api),
        "kkys" => catvod_families::kkys::handles(api),
        "apptt" => catvod_families::apptt::handles(api),
        "appysv2" => catvod_families::appysv2::handles(api),
        "auete" => catvod_families::auete::handles(api),
        "bili" => catvod_families::bili::handles(api),
        "dm84" => catvod_families::dm84::handles(api),
        "ddrk" => catvod_families::ddrk::handles(api),
        "douban" => catvod_families::douban::handles(api),
        "duanju" => catvod_families::duanju::handles(api),
        "firstaid" => catvod_families::firstaid::handles(api),
        "huban" => catvod_families::huban::handles(api),
        "jianpian" => catvod_families::jianpian::handles(api),
        "jpys" => catvod_families::jpys::handles(api),
        "iktv" => catvod_families::iktv::handles(api),
        "kugou" => catvod_families::kugou::handles(api),
        "kunyu77" => catvod_families::kunyu77::handles(api),
        "kanqiu" => catvod_families::kanqiu::handles(api),
        "libvio" => catvod_families::libvio::handles(api),
        "meijumi" => catvod_families::meijumi::handles(api),
        "moli" => catvod_families::moli::handles(api),
        "mp4mov" => catvod_families::mp4mov::handles(api),
        "nangua" => catvod_families::nangua::handles(api),
        "pan" => catvod_families::pan::handles(api),
        "pansearch" => catvod_families::pansearch::handles(api),
        "quark" => catvod_families::quark::handles(api),
        "sixv" => catvod_families::sixv::handles(api),
        "sp360" => catvod_families::sp360::handles(api),
        "tangrenjie" => catvod_families::tangrenjie::handles(api),
        "xb6v" => catvod_families::xb6v::handles(api),
        "ying" => catvod_families::ying::handles(api),
        "yinyuetai" => catvod_families::yinyuetai::handles(api),
        "yqkan" => catvod_families::yqkan::handles(api),
        "ysj" => catvod_families::ysj::handles(api),
        "zxzj" => catvod_families::zxzj::handles(api),
        "tzfile" => catvod_families::tzfile::handles(api),
        "share" => catvod_families::share::handles(api),
        "ttian" => catvod_families::ttian::handles(api),
        "wogg" => catvod_families::wogg::handles(api),
        "xpanpan" => catvod_families::xpanpan::handles(api),
        "yingso" => catvod_families::yingso::handles(api),
        "xiaozhang" => catvod_families::xiaozhang::handles(api),
        _ => false,
    }
}
