//! catvod 契约层集成测试（不访问网络、不执行 jar）。
//!
//! 覆盖计划 §3.1 / §7.3 的三条验收线：
//! 1. 画像表（profiles.json）内嵌且自洽；
//! 2. 未覆盖站点返回**分类错误**（`catvod.spider.not_ported` 等），
//!    而不是旧的「需 Android Host」；
//! 3. jar 字段解析、加载边界、结果归一化在公共 API 上可用。

use std::time::Duration;

use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::{self, contract::JarSpec, loader::JarStore, registry, Difficulty};

fn site(key: &str, api: &str, jar: &str) -> Site {
    Site {
        key: key.into(),
        name: format!("站点{key}"),
        kind: 3,
        api: api.into(),
        jar: jar.into(),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn profile_table_is_embedded_and_plausible() {
    let totals = catvod::profile_totals();
    assert!(totals.sites >= 200, "样本站点数 {}", totals.sites);
    assert!(totals.distinct_apis >= 100);
    assert_eq!(totals.jars as usize, registry::profiles().jars.len());
    assert!(registry::profiles().generated_at.starts_with("2026-"));
    // 守卫标记与 DEX 画像一致（3 个样本：szyyds x.jpg、ztha.jar、dxawi guard jar）
    let guarded: Vec<_> = registry::profiles()
        .jars
        .iter()
        .filter(|j| j.guarded)
        .map(|j| j.name.as_str())
        .collect();
    assert_eq!(guarded.len(), 3, "守卫 jar 应为 3 个: {guarded:?}");
    assert!(totals.jars >= 20, "jar 样本数 {}", totals.jars);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn native_ready_only_for_rulepack_families() {
    assert!(registry::is_native_ready("csp_XBPQ"));
    assert!(registry::is_native_ready("csp_XYQHikerV2"));
    // jar 家族移植后也应标记为 native-ready；尚未移植的保持 false。
    assert!(registry::is_native_ready("csp_Bili"));
    assert!(registry::is_native_ready("csp_Kkys"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn unported_site_returns_classified_error_instead_of_android_host() {
    let cfg = TvConfig::default();
    let s = site("pushagent", "csp_PushAgent", "");
    let err = vbox_core::sources::spider::list_categories(&cfg, &s).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("catvod.spider.not_ported"), "缺少分类码: {msg}");
    assert!(
        msg.contains("com.github.catvod.spider.PushAgent"),
        "缺少类名证据: {msg}"
    );
    assert!(!msg.contains("需 Android Host"), "不应再返回旧提示: {msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn unknown_api_error_mentions_unknown_api_code() {
    let cfg = TvConfig::default();
    let s = site("mystery", "csp_NotInAnyJar", "https://x/y.jar");
    let err = vbox_core::sources::spider::search(&cfg, &s, "kw", true).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("catvod.spider.unknown_api"), "{msg}");
    assert!(msg.contains("csp_NotInAnyJar"), "{msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn guarded_family_is_reported_as_protected() {
    let cfg = TvConfig::default();
    let s = site("guarded", "csp_BiliGuard", "");
    let err = vbox_core::sources::spider::get_detail(&cfg, &s, "1").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("catvod.jar.protected"), "{msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn script_family_is_never_routed_to_jar_layer() {
    // .js 站点走脚本路径；这里只验证分类器不把它当成 jar 家族
    match registry::classify(&site("js", "https://a/b.js", "")) {
        registry::Classification::Script { .. } => {}
        other => panic!("js 站点分类错误: {other:?}"),
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn jar_spec_parses_md5_and_disguise() {
    let spec = JarSpec::parse("img+http://a/b.jpg;md5;DDAF13D7923EA1E333EA06ED930778A2")
        .unwrap()
        .unwrap();
    assert!(spec.disguised);
    assert_eq!(spec.url, "http://a/b.jpg");
    assert_eq!(
        spec.md5.as_deref(),
        Some("ddaf13d7923ea1e333ea06ed930778a2")
    );
    assert_eq!(spec.cache_key(), "ddaf13d7923ea1e333ea06ed930778a2.jar");
    assert!(JarSpec::parse("").unwrap().is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn jar_store_classifies_assets_and_remote_layout() {
    let dir = std::env::temp_dir().join(format!("vbox-catvod-it-{}", std::process::id()));
    let store = JarStore::new(&dir).with_ttl(Duration::from_secs(60));
    let client = reqwest::blocking::Client::new();
    let s = site("x", "csp_X", "assets://spider.jar");
    let err = store.load("assets://spider.jar", &s, &client).unwrap_err();
    assert!(err.to_string().contains("catvod.jar.layout"), "{err}");
    // 空 jar 字段
    let err = store.load("", &s, &client).unwrap_err();
    assert!(err.to_string().contains("catvod.jar.layout"), "{err}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn normalize_roundtrip_from_catvod_json() {
    use vbox_core::spider::catvod::normalize;
    let home: serde_json::Value = serde_json::from_str(
        r#"{"class":[{"type_id":"1","type_name":"电影"}],
            "list":[{"vod_id":"11","vod_name":"示例","vod_pic":"p.jpg","vod_remarks":"HD"}]}"#,
    )
    .unwrap();
    let cats = normalize::categories(&home);
    let items = normalize::videos(&home);
    assert_eq!(cats.len(), 1);
    assert_eq!(items[0].id, "11");

    let detail: serde_json::Value = serde_json::from_str(
        r#"{"list":[{"vod_id":"11","vod_name":"示例","vod_content":"简介",
             "vod_play_from":"线路一$$$线路二",
             "vod_play_url":"第1集$https://a/1.m3u8#第2集$https://a/2.m3u8$$$正片$https://b/1.m3u8"}]}"#,
    )
    .unwrap();
    let d = normalize::detail(&detail).unwrap();
    assert_eq!(d.episodes.len(), 2);
    assert_eq!(d.episodes[1].group, "线路二");
    assert_eq!(d.episodes[1].items[0].url, "https://b/1.m3u8");

    let play: serde_json::Value =
        serde_json::from_str(r#"{"url":"https://v/1.m3u8","parse":1,"jx":0,"header":"Referer=https://a/"}"#)
            .unwrap();
    let p = normalize::play(&play).unwrap();
    assert!(p.sniff);
    assert_eq!(p.headers.get("Referer").map(String::as_str), Some("https://a/"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn difficulty_ordering_is_stable() {
    assert!(Difficulty::L1.rank() < Difficulty::L4.rank());
    assert_eq!(Difficulty::parse("L3"), Some(Difficulty::L3));
    assert_eq!(Difficulty::parse("l3"), Some(Difficulty::L3));
    assert_eq!(Difficulty::parse("L9"), None);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn blocked_unported_apis_carry_explicit_reasons() {
    // 反编译缺口 / 需登录态的家族仍记 unported，但必须给出可解释的 blockedReason，
    // 满足「降级原因 100% 可见」的验收标准。
    // 第 257 轮起 `csp_P115Share` 已原生实现（清单型分享搜索），不再有阻断原因。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_P115Share"), None);
    assert_eq!(
        vbox_core::spider::catvod::blocked_reason("csp_PushShare"),
        None,
        "第 263 轮 csp_PushShare 原生落地后 blockedReason 应为 None"
    );
    assert_eq!(
        vbox_core::spider::catvod::blocked_reason("csp_Panyq"),
        None,
        "第 264 轮 csp_Panyq 原生落地后 blockedReason 应为 None"
    );
    // 第 143 轮起 csp_Czsapp 已原生实现（clean 变体），不再有阻断原因；
    // webview_dependency 的样本改用 csp_Xinshijue 保持覆盖。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_Czsapp"), None);
    assert_eq!(
        vbox_core::spider::catvod::blocked_reason("csp_Xinshijue").as_deref(),
        Some("webview_dependency")
    );
    // 第 258 轮起 `csp_TGYunPan`/`csp_TGYunPanLocal` 已原生实现（tgsearch 伴生服务客户端），
    // decompiler_gap 清零；engine_dependency 样本改用 csp_PushAgent 保持覆盖。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_TGYunPan"), None);
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_TGYunPanLocal"), None);
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_SambaShare"), None);
    // 第 131 轮起 csp_DouDou 并入 douban 家族原生实现，不再有阻断原因。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_DouDou"), None);
    // 第 125 轮起 `csp_AppMao` 已原生实现（appto 框架），不再有阻断原因。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_AppMao"), None);
    assert_eq!(
        vbox_core::spider::catvod::blocked_reason("csp_PushAgent").as_deref(),
        Some("engine_dependency")
    );
    // 第 140 轮起 `csp_NiNi` 已原生实现（DES 签名/解密），不再有阻断原因。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_NiNi"), None);
    // 第 145 轮起 `csp_Kuaikan` 已原生实现（RSA+AES 信封），不再有阻断原因。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_Kuaikan"), None);
    // 第 149 轮起 `csp_Zhaozy` 已原生实现（登录 + 搜索 + Ali 引擎），不再有阻断原因。
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_Zhaozy"), None);
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_Bili"), None);
    assert_eq!(vbox_core::spider::catvod::blocked_reason("csp_Unknown"), None);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn jar_spec_rejects_malformed_md5_instead_of_requesting_bad_url() {
    let err = JarSpec::parse("http://a/b.jar;md5;not-a-md5")
        .expect_err("非法 md5 段必须直接报配置错误");
    let text = err.to_string();
    assert!(text.contains("md5 非法"), "错误应指出 md5 段: {text}");
    assert!(text.contains("not-a-md5"), "错误应保留原始线索: {text}");
    // 合法 md5 仍照常剥离
    let ok = JarSpec::parse("http://a/b.jar;md5;DDAF13D7923EA1E333EA06ED930778A2")
        .unwrap()
        .unwrap();
    assert_eq!(ok.url, "http://a/b.jar");
    assert_eq!(ok.md5.as_deref(), Some("ddaf13d7923ea1e333ea06ed930778a2"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn jar_errors_carry_site_and_spec_identity() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let base = std::env::temp_dir().join(format!(
        "vbox-jar-id-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    let store = JarStore::new(base.join("cache"));
    let client = reqwest::blocking::Client::new();
    let s = site("site-k", "csp_X", "");

    for (raw, code) in [
        ("assets://spider.jar", "catvod.jar.layout"),
        ("weird://x/y.jar", "catvod.jar.layout"),
    ] {
        let err = store.load(raw, &s, &client).unwrap_err().to_string();
        assert!(err.contains(code), "{raw} -> {err}");
        assert!(
            err.contains("jar=") && !err.contains("jar=-"),
            "错误文案必须指回 jar 字段: {err}"
        );
        assert!(err.contains(raw), "应保留原始 spec: {err}");
        assert!(err.contains("site-k"), "应包含站点 key: {err}");
    }

    // 本地文件缺失：错误既带站点身份，也带完整 spec（含 md5 段）
    let missing = base.join("no-such.jar");
    let raw = format!(
        "file://{};md5;00000000000000000000000000000000",
        missing.display()
    );
    let err = store.load(&raw, &s, &client).unwrap_err().to_string();
    assert!(err.contains("catvod.jar.fetch"), "{err}");
    assert!(err.contains(&raw), "jar= 应保留完整 spec: {err}");
    assert!(err.contains("site-k"), "应包含站点 key: {err}");

    // 没有 jar 字段时没有 spec 可带，但仍保留站点身份
    let err = store.load("", &s, &client).unwrap_err().to_string();
    assert!(
        err.contains("catvod.jar.layout") && err.contains("site-k"),
        "{err}"
    );
}
