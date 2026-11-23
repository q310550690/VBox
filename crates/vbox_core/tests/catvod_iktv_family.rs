//! `csp_Iktv` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：固定 3 分类、`ktv2` 曲库列表、`ktv3` 歌手列表、`ktv1` 桌面不可达、
//! 单曲详情（单集「K歌专区」）、歌手详情（`where=singer_names&size=999` 取全曲）、
//! 搜索（`where=name`）、播放（id 即 mkv 直链）。
//!
//! 上游 `vpsdn.leuse.top` 已全部 502、`txysong.mysoto.cc` 403，真站不可达，
//! 因此不设 `#[ignore]` 真站用例（mock 为唯一验收基线）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const SONGS_JSON: &str = r#"[{"number":"123","name":"晴天","singer_names":"周杰伦"},
{"number":"456","name":"稻香","singer_names":"周杰伦"}]"#;

const SINGERS_JSON: &str = r#"[{"id":"55","name":"林俊杰"},{"id":"56","name":"周杰伦"}]"#;

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).expect("header")
}

fn respond_json(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(json_header()));
}

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_handle = Arc::clone(&seen);
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        seen_handle.lock().unwrap().push(url.clone());
        if url.contains("table=singer") {
            respond_json(request, SINGERS_JSON);
        } else if url.contains("where=singer_names") {
            respond_json(request, SONGS_JSON);
        } else if url.contains("where=name") {
            respond_json(request, SONGS_JSON);
        } else if url.contains("/searchmv") {
            respond_json(request, SONGS_JSON);
        } else {
            respond_json(request, "[]");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "iktv".into(),
        name: "Iktv".into(),
        api: "csp_Iktv".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_fixed_three() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    let pairs: Vec<(&str, &str)> = cats
        .iter()
        .map(|c| (c.type_id.as_str(), c.type_name.as_str()))
        .collect();
    assert_eq!(pairs, vec![("ktv3", "歌手"), ("ktv2", "曲库"), ("ktv1", "点歌")]);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn songs_list_parses_number_name_and_singer() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "ktv2", 1).expect("ktv2");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "123$$$晴天$$$周杰伦");
    assert_eq!(items[0].name, "晴天");
    assert_eq!(items[0].remark.as_deref(), Some("周杰伦"));
    assert!(
        items[0]
            .cover
            .as_deref()
            .unwrap_or("")
            .starts_with("https://bpic.51yuansu.com/"),
        "unexpected cover: {:?}",
        items[0].cover
    );
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u == "/searchmv?pg=1"),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn singers_list_uses_singger_id_and_singer_cover() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "ktv3", 1).expect("ktv3");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "林俊杰$$$singger");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("http://txysong.mysoto.cc/singers/55.jpg")
    );
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("table=singer") && u.contains("pg=1")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn ktv1_is_unsupported_without_app_input() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "ktv1", 1)
        .expect_err("ktv1 must be unsupported");
    let text = format!("{err}");
    assert!(
        text.contains("catvod.host.unsupported"),
        "unexpected error: {text}"
    );
    assert!(text.contains("showInput"), "unexpected error: {text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn song_detail_is_single_karaoke_episode() {
    let cfg = cfg();
    let (base, _) = start_server();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "123$$$晴天$$$周杰伦")
        .expect("detail");
    assert_eq!(detail.id, "123$$$晴天$$$周杰伦");
    assert_eq!(detail.name, "晴天");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "K歌专区");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(
        detail.episodes[0].items[0].url,
        "http://txysong.mysoto.cc/songs/123.mkv"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn singer_detail_fetches_all_songs() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "周杰伦$$$singger").expect("detail");
    assert_eq!(detail.name, "周杰伦");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "KTV");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(
        detail.episodes[0].items[0].url,
        "http://txysong.mysoto.cc/songs/123.mkv"
    );
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("where=singer_names") && u.contains("size=999")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_uses_where_name_and_encodes_keyword() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "晴天", false).expect("search");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "123$$$晴天$$$周杰伦");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter()
            .any(|u| u.contains("where=name") && u.contains("keywords=%E6%99%B4%E5%A4%A9")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_passes_url_through_with_user_agent() {
    let cfg = cfg();
    let (base, _) = start_server();
    let out = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "0",
        "http://txysong.mysoto.cc/songs/123.mkv",
        &[],
    )
    .expect("player");
    assert_eq!(out["url"], "http://txysong.mysoto.cc/songs/123.mkv");
    assert_eq!(out["parse"], 0);
    assert!(
        out["header"]["User-Agent"]
            .as_str()
            .unwrap_or("")
            .contains("Chrome/117"),
        "unexpected header: {out}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_with_invalid_id_errors() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "晴天")
        .expect_err("invalid id must fail");
    assert!(format!("{err}").contains("catvod"), "unexpected error: {err}");
}
