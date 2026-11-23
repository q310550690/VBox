//! 判据 41：jar spider（`csp_*`）绝不能因为 `ext` 长得像脚本地址而被送进 js/py 车道。
//!
//! 为什么需要这个判据：
//! `crates/vbox_core/src/sources/spider.rs` 的 5 个入口按
//! `drpy → xyq → xbpq → 原生家族 → js → py → 分类错误` 的顺序分派。
//! 其中 `python::is_python_site` 的启发式把「ext 是 http(s)/file 地址 或 含 .py」
//! 视为 py 脚本源。而 jar spider 的 `ext` 恰好经常就是**站点 URL 或配置 JSON 地址**
//! （本次盘点里 612 站点中有 11 个 csp_ 站点属于这种情况，覆盖 10 个 api）。
//! 若不在车道判定里先排除 `csp_*`，这些「已画像未移植 / 明确不支持」的站点
//! 会被送进 Python 运行时，用户看到的是 Python 侧错误，而**不是**带稳定错误码
//! 的分类错误（`catvod.spider.not_ported` / `catvod.host.unsupported` / `catvod.jar.*`），
//! 覆盖率报告与用户体感由此不一致。
//!
//! 本判据直接调用公开入口 `sources::spider::list_categories`（与宿主相同路径），
//! 用「ext 是 URL / ext 是 .py URL / ext 是含 url 字段的对象」三种形态，
//! 断言 csp_ 的 unported/unsupported api 一律返回**分类错误码**而不是运行期错误。

use serde_json::json;
use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::contract::ErrorClass;
use vbox_core::spider::catvod::registry::{lookup, profiles};

fn site(api: &str, ext: serde_json::Value) -> Site {
    Site {
        key: format!("probe-{api}"),
        name: format!("probe-{api}"),
        kind: 3,
        api: api.to_string(),
        jar: "probe.jar".into(),
        ext: Some(ext),
        ..Default::default()
    }
}

/// 期望的错误类：与 `Classification::error` 的映射保持一致。
fn expected_class(api: &str) -> ErrorClass {
    match lookup(api).map(|p| p.strategy()) {
        Some(vbox_core::spider::catvod::contract::Strategy::Unported) => ErrorClass::NotPorted,
        Some(vbox_core::spider::catvod::contract::Strategy::Unsupported) => {
            match lookup(api).and_then(|p| p.reason.as_deref()) {
                Some("protected_guard") => ErrorClass::ProtectedGuard,
                Some("unknown_api") => ErrorClass::UnknownApi,
                Some("class_not_in_downloaded_jars") => ErrorClass::MissingJar,
                _ => ErrorClass::HostApi,
            }
        }
        _ => panic!("{api} 不是 unported/unsupported，判据用错 api"),
    }
}

fn assert_classified(api: &str, ext: serde_json::Value) {
    let cfg = TvConfig::default();
    let site = site(api, ext.clone());
    let err = match spider::list_categories(&cfg, &site) {
        Ok(v) => panic!("{api}: 期望分类错误，却成功返回 {} 个分类（ext={ext}）", v.len()),
        Err(e) => e,
    };
    let text = err.to_string();
    let want = expected_class(api);
    match ErrorClass::detect(&text) {
        Some(got) if got == want => {}
        other => panic!(
            "{api}: 期望 {:?}，实际 {:?}（ext={ext}，err={text}）",
            want.code(),
            other.map(|c| c.code())
        ),
    }
    assert!(
        !text.to_ascii_lowercase().contains("python"),
        "{api}: 分类错误里不应出现 Python 字样（说明进了脚本车道）：{text}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn csp_api_ext_never_enters_script_lane() {
    let profiles = profiles();
    let mut checked = 0;
    for (api, profile) in &profiles.apis {
        if !api.starts_with("csp_") {
            continue;
        }
        let strategy = profile.strategy();
        if strategy != vbox_core::spider::catvod::contract::Strategy::Unported
            && strategy != vbox_core::spider::catvod::contract::Strategy::Unsupported
        {
            continue;
        }
        for ext in [
            json!("https://www.example.com/"),
            json!("https://www.example.com/rule.py"),
            json!({"url": "https://www.example.com/spider.py"}),
        ] {
            assert_classified(api, ext);
            checked += 1;
        }
    }
    assert!(checked >= 150, "覆盖不足：只检查了 {checked} 组");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn inventory_regression_cases_stay_classified() {
    // 来自 `_rea/samples/inventory/sites.json` 的真实配置（612 站点盘点中
    // 11 个 csp_ 站点命中 python 启发式；csp_AppYs 于第 194 轮并入原生家族后，
    // 以仍为 unported 的 csp_Ppxzy 代表 URL ext 场景）。
    let cases: &[(&str, serde_json::Value)] = &[
        ("csp_MLYS", json!("https://www.maolvys.com/")),
        ("csp_Cycdm", json!("http://app.95189371.cn/ciyuancheng.php/v6/")),
        ("csp_Market", json!("https://szyyds.cn/js/app.json")),
        ("csp_SixVGuard", json!("https://www.xb6v.com/")),
        ("csp_Ppxzy", json!("http://www.ppxzy.net/")),
        ("csp_NewCz", json!("https://www.czzyvideo.com/")),
        ("csp_CCTV", json!("http://rihou.vip:88/hccx/央视大全.json")),
        ("csp_Panda", json!("https://d.kstore.space/download/6296/json/短剧.json")),
        ("csp_AliYunPan", json!("http://127.0.0.1:9978/file/TVBox/tok.txt")),
        ("csp_PushAgent", json!("http://127.0.0.1:9978/file/tvbox/tok.txt")),
    ];
    for (api, ext) in cases {
        assert!(
            profiles().apis.contains_key(*api),
            "{api} 不在画像中（回归用例需同步更新）"
        );
        assert_classified(api, ext.clone());
    }
}
