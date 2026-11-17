//! drpy Node sidecar 集成测试：用 fake engine 验证 Node 桥接/协议/结果解析。
//! 真正 drpy2 引擎依赖 drpy-core-lite（体积大），不放进仓库；设置了 Node 的用户可自行验证。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use vbox_core::model::{Site, TvConfig};
use vbox_core::node;
use vbox_core::sources::drpy;

static SEQ: AtomicU64 = AtomicU64::new(0);

fn ensure_node() -> bool {
    if node::node_status().available {
        return true;
    }
    eprintln!("跳过：本机没有可用的 Node.js（VBOX_NODE 可指定路径）");
    false
}

fn make_site() -> (Site, PathBuf) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vbox_drpy_test_{}_{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).unwrap();
    let engine = dir.join("drpy2.min.js");
    std::fs::write(
        &engine,
        r#"
export default {
  init(rule) { globalThis.__rule = rule },
  home(filter) { return JSON.stringify({ class: [{ type_id: '1', type_name: '电影' }] }) },
  category(tid, pg, filter, extend) {
    return JSON.stringify({ list: [{ vod_id: 'id1', vod_name: '影片A', vod_pic: 'http://x/a.jpg', vod_remarks: 'HD' }] })
  },
  detail(id) { return JSON.stringify({ list: [{ vod_name: '影片A', vod_year: '2026', vod_play_from: '线路一', vod_play_url: '第1集$http://x/1.m3u8#第2集$http://x/2.m3u8' }] }) },
  search(wd, quick) { return JSON.stringify({ list: [{ vod_id: 's1', vod_name: '搜索', vod_pic: '', vod_remarks: '' }] }) },
  play(flag, id, flags) { return JSON.stringify({ url: 'http://x/1.m3u8', parse: 0 }) }
}
"#,
    )
    .unwrap();
    // fake core：host 需要一个路径；fake engine 不实际使用
    std::fs::write(
        dir.join("drpy-core-lite.min.js"),
        "export const cheerio = {};\n",
    )
    .unwrap();
    let rule = dir.join("rule.mjs");
    std::fs::write(&rule, "export default { title: 'fake' };\n").unwrap();
    let site = Site {
        key: "drpy".into(),
        name: "DRPY".into(),
        kind: 3,
        api: format!("file:///{}", engine.to_string_lossy().replace('\\', "/")),
        ext: Some(serde_json::Value::String(format!(
            "file:///{}",
            rule.to_string_lossy().replace('\\', "/")
        ))),
        ..Default::default()
    };
    (site, dir)
}

#[test]
fn drpy_fake_engine_roundtrip() {    if !ensure_node() {
        return;
    }
    let (site, dir) = make_site();
    let cfg = TvConfig {
        sites: vec![site],
        ..Default::default()
    };
    let site = &cfg.sites[0];

    let cats = drpy::list_categories(&cfg, site).unwrap();
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_name, "电影");

    let list = drpy::list_videos(&cfg, site, "1", 1).unwrap();
    assert_eq!(list[0].name, "影片A");

    let detail = drpy::get_detail(&cfg, site, "id1").unwrap();
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert_eq!(detail.episodes[0].items.len(), 2);

    let play = drpy::player_content(&cfg, site, "线路一", "id1", &[]).unwrap();
    assert_eq!(play["url"], "http://x/1.m3u8");

    let _ = std::fs::remove_dir_all(dir);
}

/// 真实 drpy2 站点联调（默认忽略）：
/// VBOX_TEST_DRPY_API=... VBOX_TEST_DRPY_EXT=... cargo test -p vbox_core --test drpy_node_test -- --ignored real_drpy
#[test]
#[ignore = "需要真实站点：设置 VBOX_TEST_DRPY_API / VBOX_TEST_DRPY_EXT"]
fn drpy_real_engine_lists_categories() {
    if !ensure_node() {
        return;
    }
    let Ok(api) = std::env::var("VBOX_TEST_DRPY_API") else {
        eprintln!("skip: VBOX_TEST_DRPY_API 未设置");
        return;
    };
    let Ok(ext) = std::env::var("VBOX_TEST_DRPY_EXT") else {
        eprintln!("skip: VBOX_TEST_DRPY_EXT 未设置");
        return;
    };
    let site = Site {
        key: "real".into(),
        name: "real".into(),
        kind: 3,
        api,
        ext: Some(serde_json::Value::String(ext)),
        ..Default::default()
    };
    let cfg = TvConfig {
        sites: vec![site],
        ..Default::default()
    };
    let cats = drpy::list_categories(&cfg, &cfg.sites[0]).expect("drpy 分类列表失败");
    println!(
        "categories={:?}",
        cats.iter().map(|c| c.type_name.clone()).collect::<Vec<_>>()
    );
    assert!(!cats.is_empty());
}
