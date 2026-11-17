//! `csp_PanSearch` 原生家族集成测试（本地 mock，无真实网络）。
//!
//! 覆盖两步 Next.js 协议：首页取 `__NEXT_DATA__.buildId` → 拉 `_next/data/<id>/search.json`。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::families::pansearch;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let (body, ctype) = if url == "/" {
                (
                    r#"<html><head>
                      <script id="__NEXT_DATA__" type="application/json">{"props":{},"buildId":"bid-123"}</script>
                    </head><body>pansearch</body></html>"#
                        .to_string(),
                    "text/html; charset=utf-8",
                )
            } else if url.starts_with("/_next/data/bid-123/search.json") {
                assert!(url.contains("pan=aliyundrive"), "缺少 pan 参数: {url}");
                assert!(url.contains("keyword="), "缺少 keyword 参数: {url}");
                (
                    json!({
                        "pageProps": {"data": {"data": [
                            {
                                "content": "<a href=\"https://pan.quark.cn/s/abc\">阿凡达：水之道</a>\n磁力链接…",
                                "image": "https://img.example.com/a.jpg",
                                "time": "2024-05-01"
                            },
                            {
                                "content": "<a href=\"https://pan.baidu.com/s/def\">流浪地球2</a>",
                                "image": "",
                                "time": ""
                            }
                        ]}}
                    })
                    .to_string(),
                    "application/json",
                )
            } else {
                ("<html></html>".to_string(), "text/html; charset=utf-8")
            };
            let _ = req.respond(
                Response::from_string(body).with_header(
                    Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap(),
                ),
            );
        }
    });
    base
}

fn site() -> Site {
    Site {
        key: "pansou".to_string(),
        name: "盘搜".to_string(),
        api: "csp_PanSearch".to_string(),
        ..Default::default()
    }
}

#[test]
fn search_follows_next_data_two_step_protocol() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = pansearch::search_with_base(&cfg, &site(), "阿凡达", &base)
        .expect("mock 搜索应成功");

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "阿凡达：水之道");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/abc");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.example.com/a.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("2024-05-01"));
    assert_eq!(items[1].name, "流浪地球2");
    assert!(items[1].cover.is_none());
}

#[test]
fn search_reports_parse_error_when_build_id_missing() {
    // 未起服务前先验证错误分支：用一个不存在的端口会走 Http 错误，因此这里只测纯函数。
    assert!(pansearch::extract_build_id("<html>no next data</html>").is_none());
}

#[test]
fn non_search_entries_return_explicit_host_api_error() {
    let cfg = TvConfig::default();
    let site = site();

    assert!(pansearch::handles(&site.api));
    assert!(pansearch::categories().is_empty());
    assert!(pansearch::videos().is_empty());

    let detail_err = pansearch::detail(&cfg, &site, "https://pan.quark.cn/s/abc")
        .expect_err("PanSearch 没有详情");
    let detail_msg = detail_err.to_string();
    assert!(detail_msg.contains("catvod.host.unsupported"), "实际: {detail_msg}");

    let play_err = pansearch::player_content(&cfg, &site).expect_err("PanSearch 不能直接播放");
    let play_msg = play_err.to_string();
    assert!(play_msg.contains("catvod.host.unsupported"), "实际: {play_msg}");
}
