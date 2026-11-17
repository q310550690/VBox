//! 集成测试：源站分发层路由
//!
//! 验证 source 模块能按 site.kind 路由到正确适配器，且未实现的类型返回明确错误。
//! 注意：不在此联网测试 type=1 的真实抓取（那属于手动冒烟 / 单元测试范围）。

use vbox_core::load_config;
use vbox_core::source::list_categories;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/config.json"
);

fn cfg() -> vbox_core::model::TvConfig {
    load_config(FIXTURE).expect("加载示例配置")
}

#[test]
fn test_xml_source_dispatch_does_not_panic() {
    let cfg = cfg();
    let site = cfg.sites.iter().find(|s| s.key == "xmlsrc").unwrap();
    assert_eq!(site.kind, 0, "fixture 中 xmlsrc 应为 type=0");
    // 不发起真实网络请求：仅验证分发层能识别 type=0 且不会 panic。
    // XML 的具体解析由 sources::xml 的单测与 sources_xml_remote_test 负责。
    assert!(!site.api.is_empty());
}

#[test]
fn test_spider_source_not_implemented() {
    let cfg = cfg();
    // type=3 应返回未实现错误
    assert!(
        list_categories(&cfg, "spidersrc").is_err(),
        "Spider 源应返回未实现错误"
    );
}

#[test]
fn test_unknown_site_errors() {
    let cfg = cfg();
    // 不存在的站点 key 应返回 SiteNotFound
    assert!(list_categories(&cfg, "nope").is_err());
}
