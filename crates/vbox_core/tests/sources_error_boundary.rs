//! 源级错误边界回归：cms/xml/remote 三类的裸 `CoreError` 必须被
//! `sources::map_source_error` 收口成 `catvod.*` 稳定码（第 58/59 轮）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{LiveSource, Site, TvConfig};

fn garbage_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(
                Response::from_string("<html><body>not json / not xml</body></html>").with_header(
                    Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap(),
                ),
            );
        }
    });
    base
}

fn cfg_with(site: Site) -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.sites = vec![site];
    cfg
}

fn site(key: &str, kind: i32, api: String) -> Site {
    Site {
        key: key.into(),
        name: key.into(),
        kind,
        api,
        ..Default::default()
    }
}

#[test]
fn cms_config_errors_map_to_catvod_parse() {
    let base = garbage_server();
    let cfg = cfg_with(site("cms1", 1, format!("{base}/api.php/provide/vod/")));
    let err = vbox_core::source::list_categories(&cfg, "cms1").expect_err("垃圾上游应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.result.parse"), "cms 错误未收口：{msg}");
    assert!(!msg.contains("配置无效: 接口返回不是 JSON（url=") || msg.contains("catvod."));
}

#[test]
fn xml_config_errors_map_to_catvod_parse() {
    let base = garbage_server();
    let cfg = cfg_with(site("xml1", 0, format!("{base}/api.php/provide/vod/")));
    let err = vbox_core::source::list_categories(&cfg, "xml1").expect_err("垃圾上游应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.result.parse"), "xml 错误未收口：{msg}");
}

#[test]
fn remote_config_errors_map_to_catvod_parse() {
    let base = garbage_server();
    let cfg = cfg_with(site("remote1", 4, format!("{base}/")));
    let err = vbox_core::source::list_categories(&cfg, "remote1").expect_err("垃圾上游应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.result.parse"), "remote 错误未收口：{msg}");
}

#[test]
fn unknown_kind_keeps_default_message() {
    let cfg = cfg_with(site("bad1", 9, json!("http://127.0.0.1:1/").as_str().unwrap().into()));
    let err = vbox_core::source::list_categories(&cfg, "bad1").expect_err("未知类型应报错");
    assert!(err.to_string().contains("不支持的源类型"));
}

#[test]
fn live_unsupported_kind_maps_to_catvod_live_fetch() {
    let cfg = TvConfig::default();
    let live = LiveSource { kind: 9, name: "bad-live".into(), ..Default::default() };
    let err = vbox_core::live::load_live_source(&cfg, &live).expect_err("未知直播类型应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.live.fetch"), "直播错误未收口：{msg}");
}

#[test]
fn live_connect_error_maps_to_catvod_net_connect() {
    let cfg = TvConfig::default();
    let live = LiveSource {
        kind: 0,
        name: "live-net".into(),
        url: "http://127.0.0.1:1/list.txt".into(),
        ..Default::default()
    };
    let err = vbox_core::live::load_live_source(&cfg, &live).expect_err("连接失败应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.net.connect"), "直播网络错误未收口：{msg}");
}

#[test]
fn empty_play_url_maps_to_catvod_play_parse() {
    let cfg = TvConfig::default();
    let err = vbox_core::source::resolve_play_url(&cfg, " ").expect_err("空地址应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.play.parse"), "解析链错误未收口：{msg}");
}

#[test]
fn epg_connect_error_maps_to_catvod_net_connect() {
    let err = vbox_core::epg::fetch_epg("http://127.0.0.1:1/epg.xml").expect_err("连接失败应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.net.connect"), "EPG 错误未收口：{msg}");
}

#[test]
fn unknown_subtitle_source_maps_to_catvod_aux_fetch() {
    let err = vbox_core::subtitle_online::search("nope", "q", 1).expect_err("未知字幕源应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.aux.fetch"), "字幕错误未收口：{msg}");
}
