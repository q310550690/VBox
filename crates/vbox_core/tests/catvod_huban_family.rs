//! `csp_Huban`（弹幕|小窗）集成验收：配置引导壳站，全程零网络。
//!
//! 为什么放在集成层：单测只验证各函数；这里从 `sources::spider` 的 5 个公开入口
//! （`list_categories`/`list_videos`/`get_detail`/`search`/`player_content`）走完整分派，
//! 确认真实订阅配置里 `api = "csp_Huban"` 时确实命中这个家族，而不是落到
//! 「需 Android Host」的兜底分支。

use serde_json::json;
use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site() -> Site {
    Site {
        key: "huban".to_string(),
        name: "弹幕|小窗".to_string(),
        kind: 3,
        api: "csp_Huban".to_string(),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn dispatch_routes_huban_to_native_family() {
    assert!(vbox_core::spider::catvod::is_native_ready("csp_Huban"));
    assert!(vbox_core::spider::catvod::blocked_reason("csp_Huban").is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_through_dispatch() {
    let cats = list_categories(&cfg(), &site()).expect("categories");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "小虎斑的口粮");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn action_item_only_on_first_page() {
    let items = list_videos(&cfg(), &site(), "1", 1).expect("page 1");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "弹幕弹窗设置");
    assert_eq!(items[0].name, "弹幕弹窗设置");

    for page in [2u32, 5, 10] {
        assert!(
            list_videos(&cfg(), &site(), "1", page)
                .expect("empty page")
                .is_empty(),
            "page {page} 应为空"
        );
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_exposes_config_line() {
    let d = get_detail(&cfg(), &site(), "弹幕弹窗设置").expect("detail");
    assert_eq!(d.id, "弹幕弹窗设置");
    assert_eq!(d.name, "弹幕弹窗设置");
    assert_eq!(d.episodes.len(), 1);
    let group = &d.episodes[0];
    assert_eq!(group.group, "配置中心");
    assert_eq!(group.items.len(), 1);
    assert_eq!(group.items[0].url, "配置$弹幕弹窗设置");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_rejects_blank_id() {
    let err = get_detail(&cfg(), &site(), " ").expect_err("blank id must fail");
    let text = format!("{err}");
    assert!(text.contains("id 为空"), "unexpected error: {text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_is_empty_by_design() {
    assert!(search(&cfg(), &site(), "任何关键词", false)
        .expect("search")
        .is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_reports_missing_configuration() {
    let v = player_content(&cfg(), &site(), "配置中心", "弹幕弹窗设置", &[]).expect("player");
    assert_eq!(v["parse"], json!(0));
    assert_eq!(v["url"], json!(""));
    assert_eq!(v["msg"], json!("请配置参数"));
    assert_eq!(v["errMsg"], json!("请配置参数"));
}
