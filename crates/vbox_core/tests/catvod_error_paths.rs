//! 跨家族「垃圾上游 / 网络失败」错误路径回归。
//!
//! 1) 垃圾上游：一个对任意请求都返回 `<html>garbage…</html>` 的 mock，按各家族最小
//!    合法 ext/id 组合经 `sources::spider::get_detail` 走完整分派，断言错误含**具体**
//!    稳定码（`catvod.result.parse`）。
//! 2) 网络失败：连接被拒 → `catvod.net.connect`；DNS 失败 → `catvod.net.dns`；
//!    服务端慢响应 + 短超时 → `catvod.net.timeout`，而不是裸 `HTTP 请求失败: …`。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

fn garbage_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(
                Response::from_string("<html><body>garbage, not a real site</body></html>")
                    .with_header(
                        Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap(),
                    ),
            );
        }
    });
    base
}

fn site(api: &str, ext: serde_json::Value) -> Site {
    Site {
        key: api.to_ascii_lowercase(),
        name: api.to_string(),
        api: api.to_string(),
        ext: Some(ext),
        searchable: true,
        ..Default::default()
    }
}

fn assert_code(api: &str, ext: serde_json::Value, id: &str, code: &str) {
    let cfg = TvConfig::default();
    let s = site(api, ext);
    match vbox_core::sources::spider::get_detail(&cfg, &s, id) {
        Ok(detail) => panic!(
            "{api}: 垃圾上游不应返回 Ok（name={:?} episodes={}）",
            detail.name,
            detail.episodes.len()
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(msg.contains(code), "{api}: 期望稳定码 {code}，实际：{msg}");
        }
    }
}

const PARSE: &str = "catvod.result.parse";

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn tzfile_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_TZFile", json!(format!("{base}/")), "119749.html", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn auete_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Auete", json!(format!("{base}/")), "Dm/donghua/baxian_/", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn meijumi_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Meijumi", json!(base), "45785.html", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn tangrenjie_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Tangrenjie", json!(format!("{base}/")), "SCCCCKyi", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn moli_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code(
        "csp_Moli",
        json!(format!("./lib/token.json$$${base}/$$$null$$$1")),
        "movie/index3638.html",
        PARSE,
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn ddrk_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Ddrk", json!(format!("{base}/")), "huanshi", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn xb6v_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Xb6v", json!(format!("{base}/")), "12345", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn zxzj_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Zxzj", json!(format!("{base}/")), "1", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn jpys_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Jpys", json!(format!("{base}/")), "1@1", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn ysj_garbage_upstream_errors_first_page() {
    let base = garbage_server();
    assert_code("csp_Ysj", json!(format!("{base}/")), "1", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn iktv_garbage_upstream_errors() {
    // csp_Iktv 的 detail 由 id 直接拼直链（不访问上游），因此用会访问上游的 list_videos 验证错误路径。
    let base = garbage_server();
    let cfg = TvConfig::default();
    let s = site("csp_Iktv", json!(format!("{base}/")));
    let err = vbox_core::sources::spider::list_videos(&cfg, &s, "ktv2", 1).expect_err("垃圾上游应报错");
    let msg = err.to_string();
    assert!(msg.contains(PARSE), "csp_Iktv: 期望稳定码 {PARSE}，实际：{msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn yinyuetai_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Yinyuetai", json!(format!("{base}/")), "123456", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn appysv2_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_AppYsV2", json!(format!("{base}/api.php/app/")), "1", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn alist_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code("csp_Alist", json!({ "garbage": format!("{base}/") }), "garbage/x.mp4", PARSE);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn share_garbage_upstream_errors() {
    let base = garbage_server();
    let ext = format!(
        "{{\"drive_base\":\"{base}/1/clouddrive/\",\"cookie\":\"mock-cookie\"}}$$$[{{\"id\":\"https://pan.quark.cn/s/s1\",\"name\":\"测试\"}}]"
    );
    assert_code(
        "csp_QuarkShare",
        json!(ext),
        "https://pan.quark.cn/s/s1###f1",
        PARSE,
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn quark_garbage_upstream_errors() {
    let base = garbage_server();
    assert_code(
        "csp_Quark",
        json!(format!("{{\"cookie\":\"mock-cookie\",\"drive_base\":\"{base}/1/clouddrive/\"}}")),
        "ffffffffffff+++1",
        PARSE,
    );
}

/// 网络层失败：连接被拒绝时应收口成 `catvod.net.http`，而不是裸 `HTTP 请求失败: …`。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn connect_refused_maps_to_catvod_net_connect() {
    let cfg = TvConfig::default();
    let s = site("csp_Xb6v", json!("http://127.0.0.1:1/"));
    let err = vbox_core::sources::spider::get_detail(&cfg, &s, "12345").expect_err("连接失败应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("catvod.net.connect"),
        "连接被拒未映射为 catvod.net.connect：{msg}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn dns_failure_maps_to_catvod_net_dns() {
    let cfg = TvConfig::default();
    let s = site("csp_Xb6v", json!("http://nonexistent.invalid/"));
    let err = vbox_core::sources::spider::get_detail(&cfg, &s, "12345").expect_err("DNS 失败应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("catvod.net.dns"),
        "DNS 失败未映射为 catvod.net.dns：{msg}"
    );
}

/// 网盘引擎返回 41006（分享不存在/失效）→ `catvod.pan.share_unavailable`。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn share_unavailable_maps_to_catvod_pan_share_unavailable() {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("pan mock"));
    let base = format!("http://{}", server.server_addr());
    let handle = server.clone();
    thread::spawn(move || {
        for req in handle.incoming_requests() {
            let _ = req.respond(
                Response::from_string(
                    r#"{"status":404,"code":41006,"message":"分享不存在","data":null}"#,
                )
                .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
            );
        }
    });
    let ext = format!(
        "{{\"drive_base\":\"{base}/1/clouddrive/\",\"cookie\":\"mock-cookie\"}}$$$[{{\"id\":\"s1\",\"name\":\"测试\"}}]"
    );
    let s = site("csp_QuarkShare", json!(ext));
    let err = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &s,
        "",
        "https://pan.quark.cn/s/s1###f1",
        &[],
    )
    .expect_err("分享失效应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("catvod.pan.share_unavailable"),
        "41006 未映射为 catvod.pan.share_unavailable：{msg}"
    );
}

/// 网盘直链缺少 Cookie → `catvod.pan.cookie_required`。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn missing_cookie_maps_to_catvod_pan_cookie_required() {
    let s = site("csp_Quark", json!(r#"{"drive_base":"http://127.0.0.1:1/"}"#));
    let err = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &s,
        "",
        "s1++f1",
        &[],
    )
    .expect_err("缺 Cookie 应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("catvod.pan.cookie_required"),
        "缺 Cookie 未映射为 catvod.pan.cookie_required：{msg}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn timeout_maps_to_catvod_net_timeout() {
    let slow = Server::http("127.0.0.1:0").expect("slow server");
    let base = format!("http://{}", slow.server_addr());
    let slow = Arc::new(slow);
    let handle = slow.clone();
    thread::spawn(move || {
        for req in handle.incoming_requests() {
            thread::sleep(Duration::from_secs(2));
            let _ = req.respond(Response::from_string("<html>late</html>"));
        }
    });
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 300;
    cfg.http.retry = 0;
    let s = site("csp_Xb6v", json!(format!("{base}/")));
    let err = vbox_core::sources::spider::get_detail(&cfg, &s, "12345").expect_err("超时应报错");
    let msg = err.to_string();
    assert!(
        msg.contains("catvod.net.timeout"),
        "超时未映射为 catvod.net.timeout：{msg}"
    );
}
