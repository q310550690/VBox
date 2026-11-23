//! `csp_Bttwoo`（BT之家影视）集成验收。
//!
//! 上游 `https://www.bttwo.net/` 已不可达（`fetch failed`），因此用 tiny_http mock
//! 走完整分派链路：首页 nav+推荐、分类页、详情页、搜索页、播放页（AES/CBC 三段脚本）。
//! 家族侧通过 `ext` 覆盖基址（为可测试性新增的偏差，见文档 §129.4）。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};
use vbox_core::spider::catvod::families::bttwoo;

const HOME_BODY: &str = r#"<!doctype html><html><body>
<ul class="navlist">
  <li class="menu-item-type-custom"><a href="/zuixin/">最新电影</a></li>
  <li class="menu-item-type-custom"><a href="/guochanju/">国产剧</a></li>
</ul>
<div class="leibox"><ul>
  <li><h3 class="dytit"><a href="/movie/21.html">首页推荐片</a></h3>
      <a href="/movie/21.html"><img data-original="/p/21.jpg"></a>
      <div class="jidi"><span>HD中字</span></div></li>
</ul></div>
</body></html>"#;

const LIST_BODY: &str = r#"<!doctype html><html><body>
<div class="bt_img"><ul>
  <li><h3 class="dytit"><a href="/movie/7.html">示例剧集</a></h3>
      <a href="/movie/7.html"><img data-original="https://img.test/7.jpg"></a>
      <div class="jidi"><span>更新至3集</span></div></li>
  <li><h3 class="dytit"><a href="/movie/8.html">另一部剧</a></h3>
      <a href="/movie/8.html"><img src="/p/8.jpg"></a>
      <div class="jidi"><span>全24集</span></div></li>
</ul></div>
<div class="pagenavi_txt"><a class="extend" href="/zuixin/page/9">下一页</a></div>
</body></html>"#;

const DETAIL_BODY: &str = r#"<!doctype html><html><head>
<meta name="description" content="示例剧集简介">
</head><body>
<div class="moviedteail_tt"><h1>示例剧集</h1></div>
<div class="dyimg"><img src="/p/7.jpg"></div>
<ul class="moviedteail_list">
  <li>类型：剧情</li><li>年份：2021</li><li>地区：大陆</li>
  <li>导演：王五</li><li>主演：赵六</li>
</ul>
<div class="mi_paly_box"><div><div class="ypxingq_t">线路一</div></div></div>
<div class="paly_list_btn">
  <a href="/v_play/7-1-1.html">第1集</a>
  <a href="/v_play/7-1-2.html">第2集</a>
</div>
</body></html>"#;

const SEARCH_BODY: &str = r#"<!doctype html><html><body>
<div class="mi_ne_kd"><ul>
  <li><a href="/movie/31.html"><img data-original="/p/31.jpg" alt="搜索片名"></a>
      <div class="jidi"><span>HD</span></div></li>
</ul></div>
</body></html>"#;

const EMPTY_BODY: &str = r#"<!doctype html><html><body><p>没有找到您想要的结果哦</p></body></html>"#;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "bttwoo-mock".into(),
        name: "BT之家".into(),
        kind: 3,
        api: bttwoo::API.into(),
        searchable: true,
        quick_search: true,
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn respond_text(request: tiny_http::Request, body: &str, status: u16) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("header");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn respond_json(request: tiny_http::Request, body: &str, status: u16) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn start_server(player_body: String) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("mock server"));
    let addr = format!("http://{}", server.server_addr());
    let server_for_thread = Arc::clone(&server);
    thread::spawn(move || {
        for mut request in server_for_thread.incoming_requests() {
            let url = request.url().to_string();
            let method = request.method().clone();
            let mut body = String::new();
            let _ = request.as_reader().take(64 * 1024).read_to_string(&mut body);
            if method == Method::Get && url == "/" {
                respond_text(request, HOME_BODY, 200);
            } else if method == Method::Get && url.starts_with("/zuixin/page/") {
                respond_text(request, LIST_BODY, 200);
            } else if method == Method::Get && url.starts_with("/movie/") {
                respond_text(request, DETAIL_BODY, 200);
            } else if method == Method::Get && url.starts_with("/xssearch") {
                if url.contains("q=%E6%B2%A1%E6%9C%89") {
                    respond_text(request, EMPTY_BODY, 200);
                } else {
                    respond_text(request, SEARCH_BODY, 200);
                }
            } else if method == Method::Get && url.starts_with("/v_play/") {
                let _ = respond_text(request, &player_body, 200);
            } else {
                respond_json(request, r#"{"error":"not found"}"#, 404);
            }
        }
    });
    addr
}

fn encrypted_player_body() -> String {
    use aes::Aes128;
    use base64::Engine;
    use cbc::Encryptor;
    use cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};

    let key = b"1234567890abcdef";
    let iv = b"abcdef1234567890";
    let plain = r#"var url: "https://cdn.test/movie.m3u8";"#;
    let mut buf = vec![0u8; plain.len() + 16];
    buf[..plain.len()].copy_from_slice(plain.as_bytes());
    let encrypted = Encryptor::<Aes128>::new_from_slices(key, iv)
        .expect("key/iv")
        .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
        .expect("加密")
        .to_vec();
    let cipher = base64::engine::general_purpose::STANDARD.encode(encrypted);
    format!(
        r#"<html><body><script>var dncry="{}";var key=parse("{}");var iv=md5.enc.Utf8.parse("{}");var decrypted;</script></body></html>"#,
        cipher,
        std::str::from_utf8(key).unwrap(),
        std::str::from_utf8(iv).unwrap()
    )
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_bttwoo_to_native_family() {
    let site = site("https://mock.invalid/");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_come_from_home_nav() {
    let base = start_server(encrypted_player_body());
    let site = site(&base);
    let cats = list_categories(&cfg(), &site).expect("分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "zuixin");
    assert_eq!(cats[0].type_name, "最新电影");
    assert_eq!(cats[1].type_id, "guochanju");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn home_videos_parse_leibox_rows() {
    let base = start_server(encrypted_player_body());
    let site = site(&base);
    let items = bttwoo::home_videos(&cfg(), &site).expect("首页推荐");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "21");
    assert_eq!(items[0].name, "首页推荐片");
    assert_eq!(items[0].remark.as_deref(), Some("HD中字"));
    assert_eq!(items[0].cover.as_deref(), Some(&format!("{base}/p/21.jpg")[..]));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_videos_parse_rows_and_page_meta() {
    let base = start_server(encrypted_player_body());
    let site = site(&base);
    let items = list_videos(&cfg(), &site, "zuixin", 1).expect("分类列表");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "7");
    assert_eq!(items[0].name, "示例剧集");
    assert_eq!(items[0].remark.as_deref(), Some("更新至3集"));
    assert_eq!(items[1].id, "8");

    let page = bttwoo::videos_page(&cfg(), &site, "zuixin", 1).expect("分页 JSON");
    assert_eq!(page["pageCount"], 9);
    assert_eq!(page["limit"], 25);
    assert_eq!(page["list"].as_array().expect("list").len(), 2);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_maps_fields_and_play_groups() {
    let base = start_server(encrypted_player_body());
    let site = site(&base);
    let detail = get_detail(&cfg(), &site, "7").expect("详情");
    assert_eq!(detail.name, "示例剧集");
    assert_eq!(detail.year.as_deref(), Some("2021"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.director.as_deref(), Some("王五"));
    assert_eq!(detail.actor.as_deref(), Some("赵六"));
    assert_eq!(detail.desc.as_deref(), Some("示例剧集简介"));
    assert_eq!(
        detail.cover.as_deref(),
        Some(&format!("{base}/p/7.jpg")[..])
    );
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].url, "7-1-2");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_rows_and_empty_page() {
    let base = start_server(encrypted_player_body());
    let site = site(&base);
    let items = search(&cfg(), &site, "片名", false).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "31");
    assert_eq!(items[0].name, "搜索片名");
    assert_eq!(items[0].remark.as_deref(), Some("HD"));

    let empty = search(&cfg(), &site, "没有结果", false).expect("空搜索");
    assert!(empty.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_decrypts_dncry_and_keeps_original_output_shape() {
    let base = start_server(encrypted_player_body());
    let site = site(&base);
    let value = player_content(&cfg(), &site, "线路一", "7-1-1", &[]).expect("播放");
    assert_eq!(value["url"], "https://cdn.test/movie.m3u8");
    assert_eq!(value["parse"], "0");
    assert_eq!(value["palyUrl"], "");
    let ua = value["header"]["User-Agent"].as_str().expect("UA");
    assert!(ua.starts_with(" Mozilla/5.0"), "原类 UA 带前导空格: {ua:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_without_dncry_reports_host_error() {
    let base = start_server("<html><body>no player here</body></html>".to_string());
    let site = site(&base);
    let err = player_content(&cfg(), &site, "线路一", "9-9-9", &[]).expect_err("应报错");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.host.unsupported"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn empty_category_page_reports_parse_error() {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("mock server"));
    let base = format!("http://{}", server.server_addr());
    let server_for_thread = Arc::clone(&server);
    thread::spawn(move || {
        for mut request in server_for_thread.incoming_requests() {
            respond_text(request, "<html><body></body></html>", 200);
        }
    });
    let site = site(&base);
    let err = list_videos(&cfg(), &site, "zuixin", 1).expect_err("应报错");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
}
