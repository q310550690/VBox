//! `csp_AppLd`（率率 App）集成验收。
//!
//! 上游 `http://103.85.85.197:8899/api.php/app/` 已超时，故用 tiny_http mock
//! 走完整分派链路：nav / index_video / video / video_detail / search 五端点，
//! 并覆盖 `lvdou+` AES/CBC/PKCS5 播放串解密（key/iv 来自 ext.mackey）。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};
use vbox_core::spider::catvod::families::appld;

fn respond(request: tiny_http::Request, body: &str, status: u16) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        if url.contains("/nav?token=") {
            respond(request, r#"{"list":[{"type_id":"1","type_name":"电影"},{"type_id":2,"type_name":"电视剧"}]}"#, 200);
        } else if url.contains("/index_video?token=") {
            respond(request, r#"{"list":[{"vlist":[{"vod_id":9,"vod_name":"首页一","vod_pic":"/img/h.jpg","vod_remarks":"HD"}]}]}"#, 200);
        } else if url.contains("/video?tid=") {
            respond(request, r#"{"list":[{"vod_id":11,"vod_name":"列一","vod_pic":"https://cdn/a.jpg","vod_remarks":"更新至 3 集"},{"vod_id":"12","vod_name":"列二","vod_pic":"","vod_remarks":""}],"pagecount":7}"#, 200);
        } else if url.contains("/video_detail?id=") {
            respond(request, r#"{"data":{"vod_id":11,"vod_name":"列一","vod_pic":"https://cdn/d.jpg","vod_class":"动作","vod_year":"2024","vod_area":"大陆","vod_actor":"演员甲","vod_director":"导演乙","vod_content":"<p>剧情&nbsp;简介</p>","vod_url_with_player":[{"name":"线路A","url":"https://v/a.m3u8"},{"code":"线路B","url":"https://page.example.com/play/1"}]}}"#, 200);
        } else if url.contains("/search?text=") {
            respond(request, r#"{"list":[{"vod_id":21,"vod_name":"搜一","vod_pic":"https://cdn/s.jpg","vod_remarks":"全 1 集"}]}"#, 200);
        } else {
            respond(request, "{\"error\":\"not found\"}", 404);
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "appld".to_string(),
        name: "率率".to_string(),
        api: "csp_AppLd".to_string(),
        kind: 3,
        ext: Some(json!({
            "server": format!("{base}/api.php/app/"),
            "mackey": "yingshicherwQ4Zi|PHRBxdt7PppocRJ5"
        })),
        ..Default::default()
    }
}

fn encrypt_play_id(plain: &str) -> String {
    use aes::Aes128;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    let enc = cbc::Encryptor::<Aes128>::new_from_slices(
        b"yingshicherwQ4Zi",
        b"PHRBxdt7PppocRJ5",
    )
    .expect("encryptor");
    let mut buf = vec![0u8; plain.len() + 16];
    buf[..plain.len()].copy_from_slice(plain.as_bytes());
    let cipher = enc
        .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
        .expect("encrypt")
        .to_vec();
    format!("lvdou+{}", B64.encode(&cipher))
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_appld_to_native_family() {
    assert!(appld::handles("csp_AppLd"));
    assert!(appld::handles("CSP_APPLD"));
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_AppLd"),
        Some("appld")
    );
    let base = start_server();
    let s = site(&base);
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&s.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&s.api).is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_through_dispatch() {
    let base = start_server();
    let cats = list_categories(&cfg(), &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_id, "2");
    assert_eq!(cats[1].type_name, "电视剧");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_through_dispatch() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 2).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "11");
    assert_eq!(items[0].name, "列一");
    assert_eq!(items[0].remark.as_deref(), Some("更新至 3 集"));
    assert_eq!(items[1].id, "12");
    assert_eq!(items[1].cover, None);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn home_videos_flattens_vlist() {
    let base = start_server();
    let items = appld::home_videos(&cfg(), &site(&base)).expect("home");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "9");
    assert_eq!(items[0].name, "首页一");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_maps_play_lines_and_metadata() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "11").expect("detail");
    assert_eq!(detail.id, "11");
    assert_eq!(detail.name, "列一");
    assert_eq!(detail.cover.as_deref(), Some("https://cdn/d.jpg"));
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.actor.as_deref(), Some("演员甲"));
    assert_eq!(detail.director.as_deref(), Some("导演乙"));
    assert_eq!(detail.desc.as_deref(), Some("剧情 简介"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路A");
    assert_eq!(detail.episodes[0].items[0].url, "https://v/a.m3u8");
    assert_eq!(detail.episodes[1].group, "线路B");
    assert_eq!(detail.episodes[1].items[0].url, "https://page.example.com/play/1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_through_dispatch() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "海 边", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "21");
    assert_eq!(items[0].name, "搜一");
    assert_eq!(items[0].remark.as_deref(), Some("全 1 集"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_decodes_encrypted_and_plain_ids() {
    let base = start_server();
    let s = site(&base);
    let enc = encrypt_play_id("https://cdn.example.com/play/index.m3u8");
    let out = player_content(&cfg(), &s, "线路A", &enc, &[]).expect("encrypted player");
    assert_eq!(out["parse"], 0);
    assert_eq!(out["url"], "https://cdn.example.com/play/index.m3u8");

    let out = player_content(&cfg(), &s, "线路B", "https://page.example.com/x", &[])
        .expect("plain player");
    assert_eq!(out["parse"], 1);
    assert_eq!(out["jx"], "1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rejects_ext_without_server() {
    let s = Site {
        key: "appld".to_string(),
        name: "率率".to_string(),
        api: "csp_AppLd".to_string(),
        kind: 3,
        ext: Some(json!({ "mackey": "k|v" })),
        ..Default::default()
    };
    let err = list_categories(&cfg(), &s).expect_err("must fail");
    assert!(
        err.to_string().contains("server") || format!("{err:?}").contains("Parse"),
        "err = {err}"
    );
}
