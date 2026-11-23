//! `csp_TGYunPan` / `csp_TGYunPanLocal` 家族模块测试。
//!
//! 两个 api 共用同一模块（仅默认基址与端点不同）。测试全程离线：用 tiny_http
//! 起一个假的「tgsearch」伴生服务，通过 ext JSON 的 `tgsearch_url` 注入基址，
//! 断言请求构造（Base64 JSON body、`/` 与 `/s/1` 端点）与响应解析
//! （Base64 行 / 明文行、`##`/`$$` 分隔、magnet 截断、类别封面与备注）。

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::families::tgyunpan;

/// 起一个 tiny_http mock 服务：`handler(path, body)` 返回响应正文。
fn start_server<F>(handler: F) -> String
where
    F: Fn(&str, &str) -> String + Send + 'static,
{
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let text = handler(&url, &body);
            let header =
                Header::from_bytes("Content-Type", "text/plain; charset=utf-8").expect("ctype");
            let _ = request.respond(Response::from_string(text).with_header(header));
        }
    });
    base
}

/// mock 收到的 `(path, body)` 记录（用于断言请求构造）。
type Requests = Arc<Mutex<Vec<(String, String)>>>;

fn recorder() -> Requests {
    Arc::new(Mutex::new(Vec::new()))
}

fn captured(requests: &Requests) -> Vec<(String, String)> {
    requests.lock().expect("requests lock").clone()
}

fn config() -> TvConfig {
    let mut config = TvConfig::default();
    config.http.timeout = 5_000;
    config
}

/// ext 为字符串形态（等价上游 `$$$` 文本里塞一个 JSON 段）。
fn site(api: &str, base: &str) -> Site {
    site_with_ext(api, json!(format!(r#"{{"tgsearch_url":"{base}"}}"#)))
}

fn site_with_ext(api: &str, ext: Value) -> Site {
    Site {
        key: format!("tgyun_{api}"),
        name: "TG 频道搜索".to_string(),
        api: api.to_string(),
        kind: 3,
        ext: Some(ext),
        ..Default::default()
    }
}

/// 解出请求体里的 Base64 JSON。
fn decode_request_body(body: &str) -> String {
    let bytes = STANDARD.decode(body.trim()).expect("请求体应为 Base64");
    String::from_utf8(bytes).expect("请求体应为 UTF-8")
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn handles_matches_both_apis() {
    assert!(tgyunpan::handles("csp_TGYunPan"));
    assert!(tgyunpan::handles("csp_TGYunPanLocal"));
    assert!(tgyunpan::handles("  csp_tgyunpan  "));
    assert!(tgyunpan::handles(" csp_tgyunpanlocal "));
    assert!(!tgyunpan::handles("csp_Pan"));
    assert!(!tgyunpan::handles("csp_TGYunPanLocalExtra"));
    assert!(!tgyunpan::handles(""));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_fixed_jar_search() {
    let base = start_server(|_url, _body| String::new());
    let categories = tgyunpan::categories(&config(), &site(tgyunpan::API, &base)).expect("分类");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].type_id, "JARSEARCH");
    assert_eq!(categories[0].type_name, "搜索");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_jar_search_pages_are_empty_without_request() {
    let requests = recorder();
    let record = Arc::clone(&requests);
    let base = start_server(move |url, body| {
        record
            .lock()
            .expect("lock")
            .push((url.to_string(), body.to_string()));
        String::new()
    });
    let config = config();
    let site = site(tgyunpan::API, &base);
    assert!(tgyunpan::videos(&config, &site, "JARSEARCH", 1)
        .expect("第一页")
        .is_empty());
    assert!(tgyunpan::videos(&config, &site, "JARSEARCH", 2)
        .expect("第二页")
        .is_empty());
    let err = tgyunpan::videos(&config, &site, "JARSEARCHFOLDER", 1).expect_err("弹窗分类不支持");
    let message = err.to_string();
    assert!(message.contains("JARSEARCHFOLDER"), "{message}");
    assert!(message.contains("catvod.host.unsupported"), "{message}");
    assert!(captured(&requests).is_empty(), "JARSEARCH 分类不应发请求");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_base64_quark_line() {
    // 消息行：`chat\tmsgId\ttext\tmedia`，整行再 Base64。
    let inner = "testchat\t1001\t<strong>测试资源</strong> https://pan.quark.cn/s/abcd1234\t";
    let response = STANDARD.encode(inner.as_bytes());
    let base = start_server(move |_url, _body| response.clone());
    let items = tgyunpan::search(&config(), &site(tgyunpan::API, &base), "测试").expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "https://pan.quark.cn/s/abcd1234");
    assert_eq!(items[0].name, "测试资源");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://bkimg.cdn.bcebos.com/pic/6d81800a19d8bc3e93bd4e218c8ba61ea9d345a7")
    );
    assert_eq!(items[0].remark.as_deref(), Some("1.夸克/testchat/1001"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_plaintext_magnet_line() {
    let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Test%20Movie&tr=udp%3A%2F%2Ftracker.test%3A80";
    let line = format!("testchat 1002 <strong>磁力资源</strong> {magnet}");
    let base = start_server(move |_url, _body| line.clone());
    let items = tgyunpan::search(&config(), &site(tgyunpan::API, &base), "测试").expect("搜索");
    assert_eq!(items.len(), 1);
    // id 截断到前 60 字符（恰好保留 btih、丢弃 tracker）。
    assert_eq!(
        items[0].id,
        "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(items[0].name, "Test Movie 磁力资源");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://img-blog.csdnimg.cn/img_convert/09f13ae9df32c90920a200f82e7538b0.png")
    );
    assert_eq!(items[0].remark.as_deref(), Some("1.磁力/testchat/1002"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_local_uses_channel_endpoint_and_page_string() {
    let requests = recorder();
    let record = Arc::clone(&requests);
    let base = start_server(move |url, body| {
        record
            .lock()
            .expect("lock")
            .push((url.to_string(), body.to_string()));
        String::new()
    });
    let config = config();
    let ext = json!(format!(
        r#"{{"tgsearch_url":"{base}","channellist":"hao115"}}"#
    ));
    let local = site_with_ext(tgyunpan::API_LOCAL, ext);
    let remote = site(tgyunpan::API, &base);
    assert!(tgyunpan::videos(&config, &local, "测试", 2)
        .expect("频道搜索")
        .is_empty());
    assert!(tgyunpan::search(&config, &remote, "测试")
        .expect("全站搜索")
        .is_empty());
    let captured = captured(&requests);
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].0, "/s/1");
    assert_eq!(
        decode_request_body(&captured[0].1),
        r#"{"page":"2","encode":"1","keyword":"测试","channelUsername":"hao115"}"#
    );
    assert_eq!(captured[1].0, "/");
    assert_eq!(
        decode_request_body(&captured[1].1),
        r#"{"page":"1","encode":"1","keyword":"测试","channelUsername":""}"#
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_empty_keyword_makes_no_request() {
    let requests = recorder();
    let record = Arc::clone(&requests);
    let base = start_server(move |url, body| {
        record
            .lock()
            .expect("lock")
            .push((url.to_string(), body.to_string()));
        String::new()
    });
    let config = config();
    let site = site(tgyunpan::API, &base);
    assert!(tgyunpan::search(&config, &site, "   ")
        .expect("空关键词")
        .is_empty());
    assert!(captured(&requests).is_empty(), "空关键词不应发请求");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_empty_response_is_empty_list() {
    let config = config();
    // 上游空响应等价「没有结果」（`extractAllMessageTexts("")` 解析为空）。
    let base = start_server(|_url, _body| String::new());
    assert!(
        tgyunpan::search(&config, &site(tgyunpan::API, &base), "测试")
            .expect("空响应")
            .is_empty()
    );
    let blank_base = start_server(|_url, _body| "\n\n".to_string());
    assert!(
        tgyunpan::search(&config, &site(tgyunpan::API, &blank_base), "测试")
            .expect("纯空行响应")
            .is_empty()
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_rejects_non_json_response() {
    let config = config();
    let base = start_server(|_url, _body| r#"{"code":500,"msg":"tgsearch 未启动"}"#.to_string());
    let err = tgyunpan::search(&config, &site(tgyunpan::API, &base), "测试")
        .expect_err("JSON 响应应报错");
    let message = err.to_string();
    assert!(message.contains("非预期内容"), "{message}");
    assert!(message.contains("catvod.result.parse"), "{message}");

    let html_base = start_server(|_url, _body| "<html>502 Bad Gateway</html>".to_string());
    let err = tgyunpan::search(&config, &site(tgyunpan::API, &html_base), "测试")
        .expect_err("HTML 响应应报错");
    assert!(err.to_string().contains("非预期内容"), "{err}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_uses_ext_object_base_and_115_cover() {
    let line = "testchat 1003 <strong>115 资源</strong> https://115.com/s/swn8xyz?password=abc4";
    let base = start_server(move |_url, _body| line.to_string());
    // ext 用 JSON 对象形态（而不是 JSON 字符串）。
    let site = site_with_ext(tgyunpan::API, json!({ "tgsearch_url": base }));
    let items = tgyunpan::search(&config(), &site, "测试").expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "https://115.com/s/swn8xyz?password=abc4");
    assert_eq!(items[0].name, "115 资源");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://bkimg.cdn.bcebos.com/pic/f2deb48f8c5494eeb95e781a24f5e0fe99257eb0")
    );
    assert_eq!(items[0].remark.as_deref(), Some("1.115/testchat/1003"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_base64_media_image_line() {
    // `media` 为 `123456:I` 时，图片取 `{base}/i/{msgId}/123456`。
    let inner = "imgchat\t2001\t<strong>图片资源</strong> https://pan.quark.cn/s/imgtest\t123456:I";
    let response = STANDARD.encode(inner.as_bytes());
    let base = start_server(move |_url, _body| response.clone());
    let items = tgyunpan::search(&config(), &site(tgyunpan::API, &base), "测试").expect("搜索");
    assert_eq!(items.len(), 1);
    let expected_cover = format!("{base}/i/2001/123456");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/imgtest");
    assert_eq!(items[0].cover.as_deref(), Some(expected_cover.as_str()));
    assert_eq!(items[0].remark.as_deref(), Some("1.夸克/imgchat/2001"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_filters_plain_web_links() {
    let line = "testchat 1004 <strong>普通网页</strong> https://example.com/page";
    let base = start_server(move |_url, _body| line.to_string());
    let items = tgyunpan::search(&config(), &site(tgyunpan::API, &base), "测试").expect("搜索");
    assert!(items.is_empty(), "无网盘/magnet 链接的消息应被过滤");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_content_shape() {
    let base = start_server(|_url, _body| String::new());
    let config = config();
    let site = site(tgyunpan::API, &base);
    let id = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
    let value = tgyunpan::player_content(&config, &site, "磁力", id).expect("播放信息");
    assert_eq!(
        value,
        json!({"parse": 0, "playUrl": "", "url": id, "header": {}})
    );
    let err = tgyunpan::player_content(&config, &site, "", "  ").expect_err("空 id");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_returns_single_episode_for_magnet() {
    let base = start_server(|_url, _body| String::new());
    let id = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Test%20Movie";
    let detail = tgyunpan::detail(&config(), &site(tgyunpan::API, &base), id).expect("详情");
    assert_eq!(detail.id, id);
    assert_eq!(detail.name, "Test Movie");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "TG 搜索");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].url, id);
    assert_eq!(detail.episodes[0].items[0].title, "Test Movie");
    let err = tgyunpan::detail(&config(), &site(tgyunpan::API, &base), " ").expect_err("空 id");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
}
