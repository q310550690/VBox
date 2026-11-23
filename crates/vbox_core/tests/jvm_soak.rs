//! JVM 车道切源压测：连续 20 次在同一个夹具站点上切换分类/详情，验证
//! 宿主池只起 1 个宿主、无失败、`shutdown_all()` 后无残留。
//!
//! 默认 `#[ignore]`（会真的拉起 JVM，约 1–3 s），用下面命令跑：
//! `cargo test -p vbox_core --test jvm_soak -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use serde_json::json;
use vbox_core::model::Site;
use vbox_core::model::TvConfig;
use vbox_core::spider::catvod::jvm;

fn fixture_jar() -> Option<PathBuf> {
    let jar =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../jvm/build/fixtures/test-spider.jar");
    jar.is_file().then_some(jar)
}

fn file_url(path: &Path) -> String {
    format!("file:///{}", path.display().to_string().replace('\\', "/"))
}

fn fixture_site(jar: &Path) -> Site {
    Site {
        key: "soak".into(),
        name: "压测夹具".into(),
        api: "csp_TestSpider".into(),
        kind: 3,
        jar: file_url(jar),
        ext: Some(json!("")),
        ..Default::default()
    }
}

#[test]
#[ignore = "会真拉起 JVM，按需用 --ignored 跑"]
fn soak_twenty_rounds_reuses_one_host() {
    let Some(jar) = fixture_jar() else {
        eprintln!("跳过：没有夹具 jar（先 node scripts/build-jvm.mjs --fixtures）");
        return;
    };
    if vbox_jvm::Runtime::locate().is_none() {
        eprintln!("跳过：没有 JRE 运行时（先 npm run build:jvm && npm run fetch:jre）");
        return;
    }
    let cfg = TvConfig::default();
    let site = fixture_site(&jar);
    jvm::shutdown_all();

    let started = std::time::Instant::now();
    for round in 1..=20 {
        let cats = jvm::list_categories(&cfg, &site).unwrap_or_else(|e| {
            panic!("第 {round} 轮 list_categories 失败：{e:?}");
        });
        assert!(!cats.is_empty(), "第 {round} 轮分类为空");
        let videos = jvm::list_videos(&cfg, &site, &cats[0].type_id, 1)
            .unwrap_or_else(|e| panic!("第 {round} 轮 list_videos 失败：{e:?}"));
        assert!(!videos.is_empty(), "第 {round} 轮列表为空");
        let _ = jvm::get_detail(&cfg, &site, &videos[0].id)
            .unwrap_or_else(|e| panic!("第 {round} 轮 get_detail 失败：{e:?}"));
    }
    let elapsed = started.elapsed();

    let stats = jvm::stats();
    eprintln!(
        "压测完成：20 轮 / {:.0} ms；池 hosts={} spawns={} hits={} calls={} failures={}",
        elapsed.as_secs_f64() * 1000.0,
        stats.hosts,
        stats.spawns,
        stats.hits,
        stats.calls,
        stats.failures
    );
    assert_eq!(stats.failures, 0, "压测期间不应有失败调用");
    assert!(stats.hosts <= 1, "同一个 jar+ext 只应常驻 1 个宿主，实际 {}", stats.hosts);
    assert!(stats.calls >= 60, "20 轮 × 3 次调用，实际 calls={}", stats.calls);

    jvm::shutdown_all();
    let after = jvm::stats();
    assert_eq!(after.hosts, 0, "shutdown_all 之后宿主池应为空，实际 {}", after.hosts);
    assert_eq!(jvm::session_count(), 0, "shutdown_all 后不应有播放会话");
}
