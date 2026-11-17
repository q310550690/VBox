//! 集成测试：订阅配置加载
//!
//! 使用项目级示例数据（VBox/tests/fixtures/config.json），验证 load_config
//! 能正确解析 JSON、识别各源类型、收集直播源 / 解析器 / 多仓地址。

use vbox_core::load_config;
use vbox_core::model::TvConfig;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/config.json"
);

#[test]
fn test_load_fixture_config() {
    let cfg: TvConfig = load_config(FIXTURE).expect("应能解析示例配置");
    // 至少包含 3 个站点（demo/xml/spider）
    assert!(
        cfg.sites.len() >= 3,
        "站点数应 >= 3，实际 {}",
        cfg.sites.len()
    );
    // 含直播源与解析器
    assert!(!cfg.lives.is_empty(), "应解析出直播源");
    assert!(!cfg.parses.is_empty(), "应解析出解析器");
    // 顶层 parses 非空时，配置层应注入 type=4 "超级解析" 并放在 index 0（TVBox 行为）
    assert_eq!(cfg.parses[0].kind, 4, "默认解析应为超级解析");
    assert_eq!(cfg.parses[0].name, "超级解析");
    // 多仓地址应保留（有 sites 时只作为元数据，不参与合并）
    assert!(!cfg.urls.is_empty(), "应收集 urls 多仓地址");
}

#[test]
fn test_sites_kind_parsed() {
    let cfg: TvConfig = load_config(FIXTURE).unwrap();
    let kinds: Vec<i32> = cfg.sites.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&1), "应包含 type=1 苹果CMS");
    assert!(kinds.contains(&0), "应包含 type=0 XML");
    assert!(kinds.contains(&3), "应包含 type=3 Spider");
}
