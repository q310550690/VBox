//! `csp_PushShare`（推送资源清单）集成测试。
//!
//! 用 tiny_http 起一个本地 mock 网关：`share/snap` / `share/download` 走 115 分享协议，
//! `/list.txt` 返回文本清单。测试直接调用家族函数（本任务不改 `family_of` / 分派表）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::families::pushshare;

const SHARE: &str = "https://115.com/s/abc123";
const QUARK_SHARE: &str = "https://pan.quark.cn/s/abcdef123456";
const SNAP: &str = r#"{"state":true,"data":{"count":2,"list":[{"cid":"f1","n":"ep1.mp4","s":1024,"pc":"0"},{"cid":"d1","n":"合集","pc":"0","is_dir":true}]}}"#;
const DOWNLOAD: &str = r#"{"data":{"url":"https://cdn.test/ep1.mp4"}}"#;
const HTTP_MANIFEST: &str = "https://115.com/s/abc123 HTTP 剧集\n";

fn respond(request: tiny_http::Request, content_type: &str, body: &str) {
    let header = Header::from_bytes("Content-Type", content_type).expect("content type");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond");
}

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    let handle_base = base.clone();
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            if url.contains("share/snap") {
                respond(request, "application/json", SNAP);
            } else if url.contains("share/download") {
                respond(request, "application/json", DOWNLOAD);
            } else if url.contains("list.txt") {
                respond(request, "text/plain; charset=utf-8", HTTP_MANIFEST);
            } else {
                respond(request, "application/json", "{}");
            }
        }
        drop(handle_base);
    });
    base
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 5_000;
    cfg
}

fn site_with(ext: serde_json::Value) -> Site {
    Site {
        key: "pushshare-test".to_string(),
        name: "推送资源测试".to_string(),
        api: "csp_PushShare".to_string(),
        kind: 3,
        ext: Some(ext),
        ..Default::default()
    }
}

/// 文本清单站点：两条条目（115 + Quark），mock 基址 + 115 cookie。
fn text_site(base: &str) -> Site {
    site_with(json!({
        "list_source": format!("# 推送资源清单\n{SHARE} 剧集一\n{QUARK_SHARE} 电影 二"),
        "p115_base": format!("{base}/"),
        "cookie": "UID=1_A1_1",
    }))
}

/// 仅 115 一条条目的站点（浏览/详情/播放用）。
fn single_site(base: &str) -> Site {
    site_with(json!({
        "list_source": format!("{SHARE} 剧集一"),
        "p115_base": format!("{base}/"),
        "cookie": "UID=1_A1_1",
    }))
}

#[test]
fn api_name_and_handles_are_case_insensitive() {
    assert_eq!(pushshare::API, "csp_PushShare");
    assert!(pushshare::handles("csp_PushShare"));
    assert!(pushshare::handles("CSP_PUSHSHARE"));
    assert!(pushshare::handles("  csp_pushshare  "));
    assert!(!pushshare::handles("csp_P115Share"));
    assert!(!pushshare::handles(""));
}

#[test]
fn categories_from_text_manifest() {
    let base = start_server();
    let cfg = config();
    let site = text_site(&base);
    let cats = pushshare::categories(&cfg, &site).expect("分类应该从文本清单解析出来");
    assert_eq!(cats.len(), 2, "{cats:?}");
    assert_eq!(cats[0].type_id, SHARE);
    assert_eq!(cats[0].type_name, "剧集一");
    assert_eq!(cats[1].type_id, QUARK_SHARE);
    assert_eq!(cats[1].type_name, "电影 二");
}

#[test]
fn missing_manifest_source_is_parse_error() {
    let base = start_server();
    let cfg = config();
    let site = site_with(json!({ "p115_base": format!("{base}/") }));
    let err = pushshare::categories(&cfg, &site).expect_err("缺少清单源应该报 Parse 错误");
    let text = format!("{err}");
    assert!(text.contains("清单源"), "{text}");
    assert!(text.contains("catvod.result.parse"), "{text}");
}

#[test]
fn manifest_from_http_list_url() {
    let base = start_server();
    let cfg = config();
    let site = site_with(json!({
        "list_url": format!("{base}/list.txt"),
        "p115_base": format!("{base}/"),
    }));
    let cats = pushshare::categories(&cfg, &site).expect("HTTP 清单应抓取成功");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_name, "HTTP 剧集");
    assert_eq!(cats[0].type_id, SHARE);
}

#[test]
fn manifest_from_second_segment() {
    let cfg = config();
    let site = site_with(json!(format!("own-ext$$${SHARE} 段清单")));
    let cats = pushshare::categories(&cfg, &site).expect("$$$ 第二段应作为清单源");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_name, "段清单");
    assert_eq!(cats[0].type_id, SHARE);
}

#[test]
fn manifest_from_json_list_source() {
    let cfg = config();
    let site = site_with(json!({
        "list_source": r#"[{"url":"https://115.com/s/abc123","name":"JSON 剧集"}]"#,
    }));
    let cats = pushshare::categories(&cfg, &site).expect("JSON 清单应解析成功");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_name, "JSON 剧集");
    assert_eq!(cats[0].type_id, SHARE);
}

#[test]
fn unsupported_share_link_is_parse_error() {
    let cfg = config();
    let site = site_with(json!({
        "list_source": "https://example.com/v/1 非法分享",
    }));
    let err = pushshare::videos(&cfg, &site, "https://example.com/v/1", 1)
        .expect_err("不支持的分享类型应该报 Parse 错误");
    let text = format!("{err}");
    assert!(text.contains("不支持的分享类型"), "{text}");

    let err = pushshare::player_content(&cfg, &site, "", "https://example.com/v/1###f1")
        .expect_err("不支持的分享类型播放也应报错");
    let text = format!("{err}");
    assert!(text.contains("不支持的分享类型"), "{text}");
}

#[test]
fn videos_without_category_pages_manifest() {
    let base = start_server();
    let cfg = config();
    let site = text_site(&base);
    let items = pushshare::videos(&cfg, &site, "", 1).expect("空分类应回退到清单");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, SHARE);
    assert_eq!(items[0].name, "剧集一");
    assert_eq!(items[0].remark.as_deref(), Some("分享清单"));
}

#[test]
fn videos_browse_root_of_115_share() {
    let base = start_server();
    let cfg = config();
    let site = single_site(&base);
    let items = pushshare::videos(&cfg, &site, SHARE, 1).expect("浏览 115 分享根目录");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, format!("{SHARE}###f1"));
    assert_eq!(items[0].name, "ep1.mp4");
    assert!(items[0].remark.is_none(), "视频文件不是目录");
    assert_eq!(items[1].id, format!("{SHARE}###d1"));
    assert_eq!(items[1].name, "合集");
    assert_eq!(items[1].remark.as_deref(), Some("文件夹"));
}

#[test]
fn detail_expands_root_videos_to_episodes() {
    let base = start_server();
    let cfg = config();
    let site = single_site(&base);
    let detail = pushshare::detail(&cfg, &site, SHARE).expect("根目录详情");
    assert_eq!(detail.id, SHARE);
    assert_eq!(detail.name, "剧集一");
    assert_eq!(detail.episodes.len(), 1);
    let group = &detail.episodes[0];
    assert_eq!(group.group, "推送资源");
    assert_eq!(group.items.len(), 1);
    assert_eq!(group.items[0].title, "ep1.mp4");
    assert_eq!(group.items[0].url, format!("{SHARE}###f1"));
}

#[test]
fn detail_file_ref_returns_single_episode() {
    let base = start_server();
    let cfg = config();
    let site = single_site(&base);
    let id = format!("{SHARE}###f1");
    let detail = pushshare::detail(&cfg, &site, &id).expect("文件 fid 应降级为单集");
    assert_eq!(detail.episodes.len(), 1);
    let items = &detail.episodes[0].items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "ep1.mp4");
    assert_eq!(items[0].url, id);
}

#[test]
fn detail_empty_dir_is_parse_error() {
    let base = start_server();
    let cfg = config();
    let site = single_site(&base);
    let err = pushshare::detail(&cfg, &site, &format!("{SHARE}###d1"))
        .expect_err("空目录详情应报 Parse 错误");
    let text = format!("{err}");
    assert!(text.contains("为空"), "{text}");
}

#[test]
fn player_content_115_returns_direct_url() {
    let base = start_server();
    let cfg = config();
    let site = single_site(&base);
    let value =
        pushshare::player_content(&cfg, &site, "", &format!("{SHARE}###f1")).expect("115 播放直链");
    assert_eq!(value["parse"].as_i64(), Some(0));
    assert_eq!(value["playUrl"].as_str(), Some(""));
    assert_eq!(value["url"].as_str(), Some("https://cdn.test/ep1.mp4"));
    let ua = value["header"]["User-Agent"]
        .as_str()
        .expect("播放返回应带 User-Agent");
    assert!(!ua.is_empty());
    assert!(value["header"]["Referer"].as_str().is_some());
}

#[test]
fn player_without_cookie_reports_standard_error() {
    let base = start_server();
    let cfg = config();
    let site = site_with(json!({
        "list_source": format!("{SHARE} 剧集一"),
        "p115_base": format!("{base}/"),
    }));
    let err = pushshare::player_content(&cfg, &site, "", &format!("{SHARE}###f1"))
        .expect_err("无 cookie 播放应报标准错误");
    let text = format!("{err}");
    assert!(text.contains("115"), "{text}");
    assert!(text.contains("Cookie"), "{text}");
}

#[test]
fn search_matches_manifest_display_names() {
    let base = start_server();
    let cfg = config();
    let site = text_site(&base);
    let hits = pushshare::search(&cfg, &site, "剧").expect("按显示名搜索");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].id, SHARE);
    assert_eq!(hits[0].name, "剧集一");

    assert!(
        pushshare::search(&cfg, &site, "")
            .expect("空关键词")
            .is_empty(),
        "空关键词应返回空结果"
    );
    let movie = pushshare::search(&cfg, &site, "电影").expect("搜索电影");
    assert_eq!(movie.len(), 1);
    assert_eq!(movie[0].id, QUARK_SHARE);
}
