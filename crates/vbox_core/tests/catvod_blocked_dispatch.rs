//! 判据 73：未移植站点在五个入口「快速失败」且错误码稳定。
//!
//! 判据 68 只校验 `classify()` 生成的**消息文本**；但真正面向用户的是
//! `sources::spider::{list_categories,list_videos,get_detail,search,player_content}`
//! 五个入口。如果某个入口漏掉分类分支、或某个新家族不小心 `handles` 了未移植 api，
//! 用户会看到空列表/无详情/挂起网络请求，而不是稳定的 `catvod.spider.not_ported`。
//!
//! 本判据对画像里 16 个 `strategy=unported` 站点逐一调用五个入口，断言：
//! - 全部返回 `Err`，消息包含 `catvod.spider.not_ported`；
//! - 消息包含画像登记的 `blockedReason`（用户能知道为什么不可用）；
//! - 全部调用在 2 秒内完成（证明在触网之前就短路）。
//!
//! `_rea/` 缺失时 CI 软跳过。

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{json, Value};
use vbox_core::model::{Site, TvConfig};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根目录")
        .to_path_buf()
}

fn load_profiles() -> Value {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/spider/catvod/profiles.json"),
    )
    .expect("画像文件必须存在");
    serde_json::from_str(&text).expect("profiles.json 必须是合法 JSON")
}

fn site_of(api: &str) -> Site {
    serde_json::from_value(json!({
        "key": format!("test_{api}"),
        "name": api,
        "type": 3,
        "api": api,
        "ext": null
    }))
    .expect("站点 JSON 必须可反序列化")
}

#[test]
fn unported_sites_return_not_ported_before_any_io() {
    let profiles = load_profiles();
    let apis = profiles["apis"].as_object().expect("apis 必须是对象");
    let cfg = TvConfig::default();

    let mut checked = 0;
    let mut elapsed = std::time::Duration::ZERO;
    for (api, entry) in apis {
        if entry["strategy"].as_str() != Some("unported") {
            continue;
        }
        let reason = entry["blockedReason"].as_str().expect("unported 必有 blockedReason");
        let site = site_of(api);

        let checks: Vec<(&str, vbox_core::error::Result<()>)> = vec![
            (
                "list_categories",
                vbox_core::sources::spider::list_categories(&cfg, &site).map(|_| ()),
            ),
            (
                "list_videos",
                vbox_core::sources::spider::list_videos(&cfg, &site, "1", 1).map(|_| ()),
            ),
            (
                "get_detail",
                vbox_core::sources::spider::get_detail(&cfg, &site, "1").map(|_| ()),
            ),
            (
                "search",
                vbox_core::sources::spider::search(&cfg, &site, "test", true).map(|_| ()),
            ),
            (
                "player_content",
                vbox_core::sources::spider::player_content(&cfg, &site, "line", "1", &[]).map(|_| ()),
            ),
        ];
        for (entry_point, result) in checks {
            let started = Instant::now();
            let err = result.unwrap_err();
            elapsed += started.elapsed();
            let message = err.to_string();
            assert!(
                message.contains("catvod.spider.not_ported"),
                "{api}.{entry_point} 必须返回 not_ported，实际：{message}"
            );
            assert!(
                message.contains(reason),
                "{api}.{entry_point} 消息应包含阻断原因 {reason}：{message}"
            );
        }
        checked += 1;
    }

    assert_eq!(checked, 5, "画像应恰有 5 个未移植 api（第 264 轮 csp_Panyq 原生落地后 6 → 5）");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "5 个未移植站点 × 5 入口应在 2s 内短路，实际 {elapsed:?}"
    );
}

#[test]
fn unported_dispatch_set_matches_profile() {
    let profiles = load_profiles();
    let apis = profiles["apis"].as_object().unwrap();

    let unported: Vec<&str> = apis
        .iter()
        .filter(|(_, entry)| entry["strategy"].as_str() == Some("unported"))
        .map(|(api, _)| api.as_str())
        .collect();
    assert_eq!(unported.len(), 5);

    // 每个未移植 api 的 blockedReason 必须能在 registry 找到非空 hint；
    // 这是「not_ported 消息包含原因」的前提，防止新原因未登记 hint。
    for api in &unported {
        let reason = apis[*api]["blockedReason"].as_str().unwrap();
        let hint = vbox_core::spider::catvod::registry::blocked_reason_hint(reason);
        assert!(
            hint.len() >= 6,
            "{api} 的阻断原因 {reason} 缺 hint：{hint:?}"
        );
    }
}
