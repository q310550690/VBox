//! `csp_Kuaikan` 家族集成测试（全离线）。
//!
//! 覆盖：分派/registry 识别、上级 base 连不通时的错误分类（`catvod.*`）、
//! 非法播放 token 的错误路径。协议级端到端（登录 `{body,ticket}` 信封、
//! `genres/video/play/config/analysis` 全链路、AES+RSA 轮转）已在
//! `kuaikan.rs` 的模块内 mock 测试中覆盖（需要 crate 私有加密函数）。
//!
//! 上游现状：`ctlook.facaishiyi.com` 在 2026-11 已不可达，故无真站
//! `#[ignore]` 用例；无线上游时错误分类仍可验证（连接失败 → `catvod.net.*`）。

use serde_json::json;

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const API: &str = "csp_Kuaikan";

fn config() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "kuaikan-test".to_string(),
        name: "快看".to_string(),
        kind: 3,
        api: API.to_string(),
        ext: Some(json!(json!({ "base": base, "um_id": "it-umid" }).to_string())),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_kuaikan() {
    assert_eq!(families::family_of(API), Some("kuaikan"));
    assert_eq!(families::family_of(" csp_kuaikan "), Some("kuaikan"));
    assert!(registry::is_native_ready(API));
    assert!(registry::blocked_reason(API).is_none(), "画像迁移后应无 blocked_reason");
    assert_ne!(families::family_of("csp_NotAKuaikan"), Some("kuaikan"));
}

#[test]
fn categories_maps_connect_error_to_catvod_code() {
    // 127.0.0.1:9 默认 discard 端口，连接会被拒绝 → 统一映射为 catvod.* 错误。
    let s = site("http://127.0.0.1:9/api/v1");
    let err = spider::list_categories(&config(), &s).expect_err("upstream unreachable");
    let text = format!("{err}");
    assert!(text.contains("catvod."), "错误码: {text}");
}

#[test]
fn detail_maps_connect_error_to_catvod_code() {
    let s = site("http://127.0.0.1:9/api/v1");
    let err = spider::get_detail(&config(), &s, "17").expect_err("upstream unreachable");
    let text = format!("{err}");
    assert!(text.contains("catvod."), "错误码: {text}");
}

#[test]
fn player_rejects_illegal_token_via_dispatch() {
    let s = site("http://127.0.0.1:9/api/v1");
    let err = spider::player_content(&config(), &s, "", "!!!not-base64!!!", &[])
        .expect_err("illegal token");
    let text = format!("{err}");
    assert!(text.contains("catvod"), "错误码: {text}");
}
