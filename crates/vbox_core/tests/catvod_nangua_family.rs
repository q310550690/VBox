//! `csp_NanGua` 原生家族集成测试（本地 mock；上游域名为停放页，无真站用例）。
//!
//! 覆盖：5 个写死分类、分类列表（含签名/App 头校验）、详情（线路与分集）、搜索（去 `<em>`）、
//! 播放（二次接口 `data.url` + `data.header`）、错误分级，以及 `sources::spider` 分派接线。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

/// 签名盐，与 `families::nangua` 内的常量一致（协议锚点，故意在测试里重复一份）。
const SIGN_SALT: &str = "#uBFszdEM0oL0JRn@";
const IMEI: &str = "131251157296170";

const LIST_JSON: &str = r#"{
  "data": {
    "list": [
      {"id": 12, "name": "逆天至尊", "img": "http://img.test/a.jpg", "msg": "更新至10集"},
      {"id": 28, "name": "无上神帝", "img": "http://img.test/b.jpg", "msg": "更新至656集"}
    ],
    "page": 2,
    "total": 2
  }
}"#;

const DETAIL_JSON: &str = r#"{
  "data": {
    "name": "逆天至尊",
    "img": "http://img.test/a.jpg",
    "type": "动漫",
    "year": "2021",
    "msg": "更新至10集",
    "info": "  整个宇宙分为域内宇宙和域外宇宙  ",
    "player_info": [
      {"show": "南瓜线路", "video_info": [
        {"name": "第1集", "url": ["/play/1"]},
        {"name": "第2集", "url": ["/play/2"]}
      ]}
    ]
  }
}"#;

const SEARCH_JSON: &str = r#"{
  "list": [
    {"id": "9", "video_name": "<em>海贼</em>王", "img": "http://img.test/y.jpg", "qingxidu": "1080P"}
  ]
}"#;

const PLAY_JSON: &str = r#"{
  "data": {
    "url": "http://cdn.test/video.m3u8",
    "header": {"User-Agent": "okhttp/3.12.0", "Referer": "http://ys.changmengyun.com/"}
  }
}"#;

fn expected_sign(ts: u128) -> String {
    let payload = format!("{IMEI}{SIGN_SALT}{ts}");
    format!("{:x}", md5::compute(payload.as_bytes()))
}

/// 校验签名与 App 头；不合法返回 false（测试因此失败，说明请求构造错误）。
fn signed(req: &tiny_http::Request) -> bool {
    let url = req.url().to_string();
    let Some(ts) = url
        .split("&time=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .and_then(|s| s.parse::<u128>().ok())
    else {
        return false;
    };
    let header = |name: &'static str| {
        req.headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_string())
    };
    let sign_ok = header("sign").as_deref() == Some(expected_sign(ts).as_str());
    let ua_ok = header("user-agent").as_deref() == Some("okhttp/3.12.0");
    let ver_ok = header("version_name").as_deref() == Some("1.0.9");
    sign_ok && ua_ok && ver_ok
}

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let is_api = url.starts_with("/api.php/provide/");
            let (status, body) = if is_api && !signed(&req) {
                (403, json!({"code": 403, "msg": "bad sign"}).to_string())
            } else if url.starts_with("/api.php/provide/vod_list") {
                (200, LIST_JSON.to_string())
            } else if url.starts_with("/api.php/provide/vod_detail") {
                if url.contains("id=bad") {
                    (200, json!({"data": {}}).to_string())
                } else {
                    (200, DETAIL_JSON.to_string())
                }
            } else if url.starts_with("/api.php/provide/search_result") {
                (200, SEARCH_JSON.to_string())
            } else if url.starts_with("/play/empty") {
                (200, json!({"data": {}}).to_string())
            } else if url.starts_with("/play/") {
                (200, PLAY_JSON.to_string())
            } else {
                (404, json!({"code": 404}).to_string())
            };
            let _ = req.respond(
                Response::from_string(body)
                    .with_status_code(status)
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], b"application/json").unwrap(),
                    ),
            );
        }
    });
    base
}

fn site(base: &str) -> Site {
    Site {
        key: "nangua".to_string(),
        name: "南瓜".to_string(),
        api: "csp_NanGua".to_string(),
        ext: Some(json!(format!("{base}$$$1"))),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_are_the_five_static_ones() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&TvConfig::default(), &site(&base))
        .expect("分类");
    assert_eq!(cats.len(), 5, "实际: {cats:?}");
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[4].type_id, "46");
    assert_eq!(cats[4].type_name, "外剧");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_uses_signed_app_api() {
    let base = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&TvConfig::default(), &site(&base), "1", 2)
            .expect("列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].id, "12");
    assert_eq!(items[0].name, "逆天至尊");
    assert_eq!(items[0].remark.as_deref(), Some("更新至10集"));
    assert_eq!(items[0].cover.as_deref(), Some("http://img.test/a.jpg"));
    assert_eq!(items[1].id, "28");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_builds_groups_and_episodes() {
    let base = start_server();
    let d = vbox_core::sources::spider::get_detail(&TvConfig::default(), &site(&base), "12")
        .expect("详情");
    assert_eq!(d.name, "逆天至尊");
    assert_eq!(d.year.as_deref(), Some("2021"));
    assert_eq!(d.desc.as_deref(), Some("整个宇宙分为域内宇宙和域外宇宙"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "南瓜线路");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(d.episodes[0].items[0].title, "第1集");
    // 分集地址是相对路径，播放时按基址补全
    assert_eq!(d.episodes[0].items[0].url, "/play/1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_strips_em_tags() {
    let base = start_server();
    let items = vbox_core::sources::spider::search(
        &TvConfig::default(),
        &site(&base),
        "海贼",
        false,
    )
    .expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "9");
    assert_eq!(items[0].name, "海贼王");
    assert_eq!(items[0].remark.as_deref(), Some("1080P"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_follows_episode_url_and_headers() {
    let base = start_server();
    let s = site(&base);
    let play_url = format!("{base}/play/1");
    let out = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &s,
        "南瓜线路",
        &play_url,
        &[],
    )
    .expect("播放");
    assert_eq!(out["url"], "http://cdn.test/video.m3u8");
    assert_eq!(out["parse"], 0);
    assert_eq!(out["header"]["Referer"], "http://ys.changmengyun.com/");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_garbage_reports_parse_error() {
    let base = start_server();
    let err = vbox_core::sources::spider::get_detail(&TvConfig::default(), &site(&base), "bad")
        .expect_err("应当失败");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_without_url_reports_parse_error() {
    let base = start_server();
    let s = site(&base);
    let play_url = format!("{base}/play/empty");
    let err = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &s,
        "南瓜线路",
        &play_url,
        &[],
    )
    .expect_err("应当失败");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
}
