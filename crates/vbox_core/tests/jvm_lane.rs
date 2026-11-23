//! jar 蜘蛛 JVM 车道的端到端集成测试（不依赖真实站点/网络）。
//!
//! 用 `jvm/fixtures` 里的 conformance 蜘蛛验证完整链路：宿主拉起 → `__ping__` →
//! home/category/detail/search/player → normalize。没有 JRE 或夹具 jar 时打印原因并跳过，
//! 以免在没跑过 `scripts/build-jvm.mjs --fixtures` 的机器上误报失败。

use std::path::{Path, PathBuf};

use serde_json::json;
use vbox_core::model::TvConfig;
use vbox_core::model::Site;
use vbox_core::spider::catvod::jvm;

fn fixture_jar() -> Option<PathBuf> {
    let jar = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../jvm/build/fixtures/test-spider.jar");
    jar.is_file().then_some(jar)
}

fn file_url(path: &Path) -> String {
    format!("file:///{}", path.display().to_string().replace('\\', "/"))
}

fn fixture_site(jar: &Path) -> Site {
    Site {
        key: "fixture".into(),
        name: "夹具蜘蛛".into(),
        api: "csp_TestSpider".into(),
        kind: 3,
        jar: file_url(jar),
        ext: Some(json!("")),
        ..Default::default()
    }
}

#[test]
fn jvm_lane_runs_fixture_spider_end_to_end() {
    if vbox_jvm::Runtime::locate().is_none() {
        eprintln!("跳过：没有 JRE 运行时（先 npm run build:jvm && npm run fetch:jre）");
        return;
    }
    let Some(jar) = fixture_jar() else {
        eprintln!("跳过：夹具 jar 未构建（node scripts/build-jvm.mjs --fixtures）");
        return;
    };
    let cfg = TvConfig::default();
    let site = fixture_site(&jar);

    // homeContent → 分类
    let cats = jvm::list_categories(&cfg, &site).expect("list_categories 失败");
    assert!(!cats.is_empty(), "homeContent 没有返回分类");
    let first = cats[0].type_id.clone();

    // categoryContent → 视频列表
    let videos = jvm::list_videos(&cfg, &site, &first, 1).expect("list_videos 失败");
    assert!(!videos.is_empty(), "categoryContent 没有返回视频");

    // detailContent → 详情 + 选集
    let video_id = videos[0].id.clone();
    let detail = jvm::get_detail(&cfg, &site, &video_id).expect("get_detail 失败");
    assert_eq!(detail.id, video_id);
    assert!(!detail.episodes.is_empty(), "详情里没有选集");

    // searchContent → 搜索结果
    let found = jvm::search(&cfg, &site, "测试", true).expect("search 失败");
    assert!(!found.is_empty(), "searchContent 没有返回结果");

    // playerContent → 播放信息
    let play = jvm::player_content(&cfg, &site, "m3u8", &video_id, &[]).expect("player_content 失败");
    let url = play.get("url").and_then(|v| v.as_str()).unwrap_or_default();
    assert!(!url.is_empty(), "playerContent 没有返回 url");

    jvm::shutdown_all();
}
