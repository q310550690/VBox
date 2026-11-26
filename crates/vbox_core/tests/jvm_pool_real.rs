//! P2：真实 JRE + 真实样本下的宿主池复用与回收（默认 `--ignored`，需要本机样本与 JRE）。
//!
//! 样本放在 `jvm/samples/`（不入库，见 .gitignore）。
//!
//! 跑法：`cargo test -p vbox_core --test jvm_pool_real -- --ignored --nocapture`

use std::path::PathBuf;

use serde_json::json;
use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::jvm;

fn sample_jar() -> PathBuf {
    if let Ok(p) = std::env::var("VBOX_SAMPLE_JAR") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../jvm/samples/dffec63f-pg.jar")
}

fn site_for(api: &str, jar: &PathBuf) -> Site {
    let mut site = Site::default();
    site.key = "pool-real".into();
    site.name = "PoolReal".into();
    site.api = api.into();
    site.kind = 3;
    site.jar = format!("file:///{}", jar.display().to_string().replace('\\', "/"));
    site.ext = Some(json!(""));
    site
}

#[test]
#[ignore = "需要本机 JRE 17 + jvm/samples 样本 jar"]
fn real_host_is_reused_and_reclaimed() {
    let jar = sample_jar();
    if !jar.is_file() {
        eprintln!("跳过：样本 jar 不存在 {}", jar.display());
        return;
    }
    if !jvm::available() {
        eprintln!("跳过：没有 JRE 运行时");
        return;
    }
    let config = TvConfig::default();
    let site = site_for("csp_Jianpian", &jar);
    assert!(jvm::can_handle(&site), "样本应当能被 JVM 车道接管");

    let first = jvm::list_categories(&config, &site).expect("第一次取分类");
    assert!(!first.is_empty(), "Jianpian 首页应有分类");
    let stats1 = jvm::stats();
    assert_eq!(stats1.hosts, 1, "第一次之后应只有一个宿主");

    let second = jvm::list_categories(&config, &site).expect("第二次取分类");
    assert_eq!(second.len(), first.len());
    let stats2 = jvm::stats();
    assert_eq!(stats2.hosts, 1, "同 jar+ext 必须复用同一个宿主");
    assert!(stats2.hits >= 1, "第二次应命中池：{:?}", stats2);
    assert!(stats2.spawns <= 1, "不应该重复 spawn：{:?}", stats2);

    jvm::shutdown_all();
    let after = jvm::stats();
    assert_eq!(after.hosts, 0, "shutdown_all 之后不应残留宿主");
    eprintln!("[pool] stats={:?}", stats2);
}
