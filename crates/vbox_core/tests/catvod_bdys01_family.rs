//! `csp_Bdys01` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、固定分类、列表页、详情页（线路+剧集）、
//! 播放 mp4 直链分支、播放 m3u8 密文分支（3354 字节头 + GZIP + `.ts` 前缀
//! + `data:` URI）、缺 pid 错误路径。
//!
//! 站点上游当前是 JS 挑战壳（`bdys01.com`）与 404（`bdys03.com`），故没有
//! 真站 `#[ignore]` 用例；原类里的 Google 搜索依赖外网，也只在单测里覆盖
//! `first_search_hit` 的解析逻辑。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const LIST_HTML: &str = r#"
<div class="col-lg-8">
  <h3 class="mb-0">测试片一</h3>
  <img class="w-100" src="https://img.test/1.jpg">
  <p class="mb-0">更新至01集</p>
  <a class="d-block" href="/mv/11.html;JSESSIONID=abc">播放</a>
</div>
"#;

const DETAIL_HTML: &str = r#"
<div class="col-md-auto"><img src="https://img.test/d.jpg"></div>
<h1 class="d-none d-md-block">测试详情片</h1>
<div class="card collapse"><div class="card-body">剧情简介</div></div>
<span class="badge bg-purple-lt">更新时间：2024-08-01</span>
<div class="col mb-2">
  <p><strong>类型：</strong><a>电影</a></p>
  <p><strong>地区：</strong>[日本]</p>
  <p><strong>豆瓣：</strong>8.1</p>
  <p><strong>导演：</strong><a>某导演</a></p>
  <p><strong>主演：</strong><a>甲</a><a>乙</a></p>
</div>
<a class="btn btn-square" href="/play/1-1-1.html">第1集</a>
<a class="btn btn-square" href="/play/1-1-2.html">第2集</a>
"#;

const PLAY_HTML: &str = "<html><script>var pid = 'pid123';\nvar time = 1;</script></html>";

fn play_html_no_pid() -> &'static str {
    "<html><script>var time = 1;</script></html>"
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(bytes).expect("gzip write");
    enc.finish().expect("gzip finish")
}

/// 3354 字节垃圾前缀 + GZIP(m3u8 文本)，模拟原类“跳过头部再解压”。
fn m3u8_payload() -> Vec<u8> {
    let mut out = vec![0u8; 3354];
    out.extend_from_slice(&gzip(b"#EXTM3U\n#EXTINF:10,\nseg1.ts\nseg2.ts\n#EXT-X-ENDLIST\n"));
    out
}

fn respond_text(request: tiny_http::Request, body: String) {
    let header = Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("ctype");
    request
        .respond(Response::from_string(body).with_header(header))
        .expect("respond text");
}

fn respond_bytes(request: tiny_http::Request, body: Vec<u8>, ctype: &str) {
    let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
    request
        .respond(Response::from_data(body).with_header(header))
        .expect("respond bytes");
}

fn start_server(with_mp4: bool) -> String {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base = format!("http://{}", server.server_addr());
    let ret = base.clone();
    let base = Arc::new(base);
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let mut sink = Vec::new();
            let _ = request.as_reader().read_to_end(&mut sink);
            let b = base.clone();
            if url.contains("/lines?") {
                let lines = if with_mp4 {
                    format!(r#"{{"data":{{"url3":"{b}/movie.mp4"}}}}"#)
                } else {
                    format!(r#"{{"data":{{"url3":"{b}/redir.m3u8"}}}}"#)
                };
                respond_text(request, lines);
            } else if url.contains("/zzzzz") {
                let header = Header::from_bytes("Set-Cookie", "w=cookie123; Path=/").expect("ck");
                request
                    .respond(Response::from_string("ok").with_header(header))
                    .expect("cookie");
            } else if url.contains("/s/all/") {
                respond_text(request, LIST_HTML.to_string());
            } else if url.contains("/mv/11.html") {
                respond_text(request, DETAIL_HTML.to_string());
            } else if url.contains("/play/1-1-1.html") {
                respond_text(request, PLAY_HTML.to_string());
            } else if url.contains("/play/bad") {
                respond_text(request, play_html_no_pid().to_string());
            } else if url.contains("/redir.m3u8") || url.contains("/real.m3u8") {
                respond_bytes(request, m3u8_payload(), "application/octet-stream");
            } else {
                respond_text(request, "<html></html>".to_string());
            }
        }
    });
    ret
}

fn site(base: &str) -> Site {
    Site {
        key: "bdys01".into(),
        name: "哔滴".into(),
        api: "csp_Bdys01".into(),
        ext: Some(serde_json::Value::String(base.into())),
        ..Default::default()
    }
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 5_000;
    cfg
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_bdys01() {
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_Bdys01"),
        Some("bdys01")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready("csp_Bdys01"));
    assert!(vbox_core::spider::catvod::registry::blocked_reason("csp_Bdys01").is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_are_fixed_three() {
    let base = start_server(false);
    let cfg = config();
    let cats = spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 3);
    assert_eq!(cats[0].type_id, "a");
    assert_eq!(cats[1].type_name, "电影");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_page_parses_rows() {
    let base = start_server(false);
    let cfg = config();
    let items = spider::list_videos(&cfg, &site(&base), "a", 1).expect("videos");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/mv/11.html");
    assert_eq!(items[0].name, "测试片一");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_parses_meta_and_episodes() {
    let base = start_server(false);
    let cfg = config();
    let d = spider::get_detail(&cfg, &site(&base), "/mv/11.html").expect("detail");
    assert_eq!(d.name, "测试详情片");
    assert_eq!(d.year.as_deref(), Some("2024"));
    assert_eq!(d.area.as_deref(), Some("日本"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "播放列表");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(d.episodes[0].items[0].title, "第1集");
    assert_eq!(d.episodes[0].items[0].url, "/play/1-1-1.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_prefers_mp4_candidate() {
    let base = start_server(true);
    let cfg = config();
    // /lines 里 m3u8 字段是 .mp4 → 直链分支（url3 的 m3u8 会被优先命中后判断）
    let value = spider::player_content(&cfg, &site(&base), "播放列表", "/play/1-1-1.html", &[])
        .expect("player");
    assert_eq!(value["parse"], 0);
    assert!(value["url"].as_str().unwrap_or_default().contains(".mp4"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_decodes_gzip_m3u8() {
    let base = start_server(false);
    let cfg = config();
    let value = spider::player_content(&cfg, &site(&base), "播放列表", "/play/1-1-1.html", &[])
        .expect("player m3u8");
    assert_eq!(value["parse"], 0);
    let url = value["url"].as_str().unwrap_or_default();
    let b64 = url.strip_prefix("data:application/vnd.apple.mpegurl;base64,").expect("data uri");
    use base64::Engine as _;
    let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64).expect("b64"))
        .expect("utf8");
    assert!(text.contains("#EXTM3U"), "got {text}");
    assert!(text.contains("https://vod.bdys.me/seg1.ts"), "got {text}");
    assert!(text.contains("https://vod.bdys.me/seg2.ts"), "got {text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_missing_pid_reports_parse_error() {
    let base = start_server(false);
    let cfg = config();
    let err = spider::player_content(&cfg, &site(&base), "播放列表", "/play/bad", &[])
        .expect_err("缺少 pid 必须报错");
    let msg = format!("{err}");
    assert!(msg.contains("catvod"), "got {msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_against_local_google_is_empty() {
    // 搜索依赖 Google（外网）；本机 mock 只覆盖站内路径，因此这里断言不 panic。
    let base = start_server(false);
    let cfg = config();
    let rows = spider::search(&cfg, &site(&base), "", true).expect("empty keyword ok");
    assert!(rows.is_empty());
}
