//! 网盘搜索簇（csp_PanSou / csp_UpYun）集成测试：走 `sources::spider::search` 分派。
//!
//! 覆盖：PanSou 三基址协议 + `<template>` 文本提取 + 备注清洗；UpYun Base64→JSON；
//! 以及 detail/player 在只返回分享链接的家族上给出 `catvod.host.unsupported`。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

const PANSOU_HTML: &str = r#"
<html><body>
  <van-row><a href="https://pan.example.com/s/abc123">
    <template>流浪地球3 4K</template>
    <template>时间: 2026-11-01 格式: mp4</template>
  </a></van-row>
  <van-row><a href="https://pan.example.com/s/def456">
    <template>流浪地球2</template>
    <template>时间: 2026-10-01 格式: mkv</template>
  </a></van-row>
</body></html>"#;

const UPYUN_JSON: &str = r#"{"result":{"items":[
  {"page_url":"https://pan.quark.cn/s/xyz","title":"测试资源甲","insert_time":"2026-11-01"},
  {"page_url":"https://pan.quark.cn/s/no","title":"不相关","insert_time":"2026-11-02"}
]}}"#;


const FUNLETU_JSON: &str = r#"{"data":[
  {"url":"https://pan.example.com/s/abc123?pwd=abcd","title":" 流浪地球3 4K ","updatetime":"2026-11-01"},
  {"url":"https://pan.example.com/s/def456","title":"流浪地球2","updatetime":"2026-10-01"}
]}"#;

const QUARKPANSO_JSON: &str = r#"{"data":{"list":[
  {"link":"https://pan.quark.cn/s/q1","disk_name":"<em>夸克</em>资源甲","shared_time":"2026-11-05"},
  {"link":"https://pan.quark.cn/s/q2","disk_name":"<em>夸克</em>资源乙","shared_time":"2026-11-06"}
]}}"#;

const UPYUNSO_JSON: &str = r#"{"result":{"items":[
  {"title":"流浪地球3 4K","page_url":"https://www.alipan.com/s/abc123"},
  {"title":"无关资源","page_url":"https://www.alipan.com/s/zzz"},
  {"title":"流浪地球3 夸克","page_url":"https://pan.quark.cn/s/q1"}
]}}"#;

const YISOU_JSON: &str = r#"{"data":{"list":[
  {"fileInfos":[{"fileName":"流浪地球3"}],"gmtCreate":"2026-11-01","url":"dmXE5GoB5Wod+MBGmOnoZqhWGSdQvVFjMPvmKUofS1cr6D/jGXV/RRGUjoFnr/Li"},
  {"fileInfos":[{"fileName":"无关资源"}],"gmtCreate":"2026-11-02","url":"dmXE5GoB5Wod+MBGmOnoZm9RYy6d46rZ62Dr24BH0tP5GPjZ3MmHpwhDRPJrP1mQ"}
]}}"#;

const PIKASO_HTML: &str = r#"<html><body>
  <div class="search-item">
    <a href="https://pan.quark.cn/s/px1"><h2>流浪地球3 4K</h2></a>
    <div class="search-note">文件大小: 数量: 12.5 GB</div>
  </div>
  <div class="search-item">
    <a href="https://example.com/not-pan"><h2>不是网盘</h2></a>
    <div class="search-note">数量: 1</div>
  </div>
  <div class="search-item">
    <a href="https://pan.quark.cn/s/px2"><h2>小文件</h2></a>
    <div class="search-note">文件大小: 900 KB</div>
  </div>
</body></html>"#;

const QIANFAN_HTML: &str = r#"<html><body>
  <div class="search-item">
    <a href="https://pan.quark.cn/s/qf1">流浪地球3 4K</a>
    <div class="search-note">文件大小: 数量: 12.5 GB</div>
  </div>
  <div class="search-item">
    <a href="">空链接</a>
  </div>
</body></html>"#;

const YUNSO_JSON: &str = r#"{"data":"<html><body><div class=\"layui-card\"><div class=\"layui-card-header\"><a id=\"share-abc\" href=\"/d/abc\">流浪地球3</a></div><div class=\"layui-card-body\"><img src=\"/img/1.jpg\" alt=\"阿里\"></div></div></body></html>"}"#;

const ALIPS_HTML: &str = r#"<html><body>
  <van-row><a href="/s/ali111"><template><div>流浪地球3 4K 时间: 2026-11-01</div></template></a></van-row>
  <van-row><a href="/s/ali222"><template><div>无关资源 时间: 2026-11-02</div></template></a></van-row>
</body></html>"#;

const HUNHEPAN_JSON: &str = r#"{"data":{"list":[
  {"link":"https://www.aliyundrive.com/s/hh1","disk_name":"<em>流浪地球3</em> 4K","update_time":"2026-11-01","share_user":"张三"},
  {"link":"https://pan.quark.cn/s/hq1","disk_name":"流浪地球2","update_time":"2026-10-01","share_user":"李四"}
]}}"#;

const DAPANSO_JSON: &str = r#"{"ret":{"records":[
  {"url":"https://pan.xunlei.com/s/dp1","name":"<em>流浪地球3</em>","createdAt":"2026-11-03"},
  {"url":"https://example.com/x","name":"无关","createdAt":""}
]}}"#;


const MIAOSOU_JSON: &str = r#"{"data":{"list":[
  {"url":"https://pan.quark.cn/s/m1","name":"<b>流浪地球3</b> 4K","gmtCreate":"2026-11-01","creatorName":"alice"},
  {"url":"https://example.com/nope","name":"无关","gmtCreate":"2026-10-01","creatorName":"bob"}
]}}"#;

const EIGHTEIGHT_JSON: &str = r#"{"data":[
  {"title":"流浪地球3","url":"https://pan.quark.cn/s/e1"},
  {"title":"无关资源","url":"https://example.com/none"}
]}"#;

const YUNPANONE_JSON: &str = r#"{"data":[
  {"attributeT":{"slug":"disc1","title":"流浪地球3","createdAt":"2026-11-02T08:00:00Z"}},
  {"attributeT":{"slug":"","title":"无 slug"}}
]}"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    let base_for_thread = base.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let (body, ctype) = if url.starts_with("/api/secendsearch") {
                (MIAOSOU_JSON.to_string(), "application/json")
            } else if url.starts_with("/api/get_zy") {
                (EIGHTEIGHT_JSON.to_string(), "application/json")
            } else if url.starts_with("/api/discussions") {
                (YUNPANONE_JSON.to_string(), "application/json")
            } else if url.starts_with("/upyunso/search") {
                (upso_b64(), "application/json")
            } else if url.starts_with("/yisou/api/search") {
                (YISOU_JSON.to_string(), "application/json")
            } else if url.starts_with("/alips/search") {
                (ALIPS_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/qianfan/search/?pan=") {
                (QIANFAN_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/yunso/api/validate/search") {
                let body_ok = req.method().as_str() == "POST" && url.contains("mode=90002");
                (
                    if body_ok { YUNSO_JSON.to_string() } else { "{}".to_string() },
                    "application/json",
                )
            } else if url.starts_with("/search/?pan=") {
                (PIKASO_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/v1/search/disk") {
                (QUARKPANSO_JSON.to_string(), "application/json")
            } else if url == "/search" {
                (FUNLETU_JSON.to_string(), "application/json")
            } else if url.contains("keyword=") {
                (upyun_b64(), "application/json")
            } else if url.contains("k=") {
                (PANSOU_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/hunhepan/v1/search") {
                let ua_ok = req
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("User-Agent") && h.value.as_str().contains("Chrome/122"));
                let post_ok = req.method().as_str() == "POST";
                (
                    if ua_ok && post_ok {
                        HUNHEPAN_JSON.to_string()
                    } else {
                        String::from("bad request")
                    },
                    "application/json",
                )
            } else if url.starts_with("/dapanso/api/resources/m-search") {
                let ts = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Timestamp"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let sign = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Sign"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let expect = vbox_core::spider::catvod::families::pan::sha256_hex(&format!(
                    "/api/resources/m-searchKDNL{ts}"
                ));
                (
                    if !ts.is_empty() && sign == expect {
                        DAPANSO_JSON.to_string()
                    } else {
                        String::from("bad request")
                    },
                    "application/json",
                )
            } else {
                (String::from("not found"), "text/plain")
            };
            let _ = req.respond(
                Response::from_string(body)
                    .with_header(Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap()),
            );
        }
    });
    base_for_thread
}

fn upso_b64() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(UPYUNSO_JSON.as_bytes())
}

fn upyun_b64() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(UPYUN_JSON.as_bytes())
}

fn site(api: &str, base: &str) -> Site {
    Site {
        key: "pan".to_string(),
        name: "网盘搜索".to_string(),
        api: api.to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn pansou_search_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_PanSou", &base), "流浪地球", false)
        .expect("PanSou 搜索");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].name, "流浪地球3 4K");
    assert_eq!(items[0].id, "https://pan.example.com/s/abc123");
    assert_eq!(items[0].remark.as_deref(), Some("阿里 2026-11-01 mp4"));
    assert!(items[0].cover.as_deref().unwrap_or("").contains("gtimg"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn upyun_search_decodes_base64_json() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_UpYun", &base), "测试", false)
        .expect("UpYun 搜索");
    assert_eq!(items.len(), 1, "只保留标题含关键词的条目: {items:?}");
    assert_eq!(items[0].name, "测试资源甲");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/xyz");
    assert_eq!(items[0].remark.as_deref(), Some("2026-11-01"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn pan_family_detail_reports_contract_gap() {
    let base = start_server();
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(
        &cfg,
        &site("csp_PanSou", &base),
        "https://pan.example.com/s/abc123",
    )
    .expect_err("detail 应报契约缺口");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.host.unsupported"), "实际: {msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn funletu_search_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_Funletu", &base), "流浪地球", false)
        .expect("Funletu 搜索");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].name, "流浪地球3 4K");
    assert_eq!(items[0].id, "https://pan.example.com/s/abc123");
    assert_eq!(items[0].remark.as_deref(), Some("2026-11-01"));
    assert!(items[0].cover.as_deref().unwrap_or("").contains("bdstatic"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn quarkpanso_search_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items =
        vbox_core::sources::spider::search(&cfg, &site("csp_QuarkPanso", &base), "夸克", false)
            .expect("QuarkPanso 搜索");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].name, "夸克资源甲", "<em> 标签应被移除");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/q1");
    assert_eq!(items[0].remark.as_deref(), Some("2026-11-05"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn upyunso_search_filters_alipan_and_decodes_base64() {
    let base = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(
        &cfg,
        &site("csp_Upyunso", &format!("{base}/upyunso")),
        "流浪地球3",
        false,
    )
    .expect("search");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "https://www.alipan.com/s/abc123");
    assert_eq!(rows[0].name, "流浪地球3 4K");
    assert_eq!(rows[0].cover.as_deref(), Some("https://inews.gtimg.com/newsapp_bt/0/13263837859/1000"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn yisou_search_decrypts_links() {
    let base = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(
        &cfg,
        &site("csp_Yisou", &format!("{base}/yisou")),
        "流浪地球3",
        false,
    )
    .expect("search");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "https://www.alipan.com/s/yisou123");
    assert_eq!(rows[0].name, "流浪地球3");
    assert_eq!(rows[0].remark.as_deref(), Some("2026-11-01"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn alips_search_prefixes_type_label() {
    let base = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(
        &cfg,
        &site("csp_AliPS", &format!("{base}/alips")),
        "流浪地球3",
        false,
    )
    .expect("search");
    // 两个 t 轮询各命中一条（文件夹 / 视频）
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "/s/ali111");
    assert_eq!(rows[0].name, "[文件夹]流浪地球3 4K 时间: 2026-11-01");
    assert_eq!(rows[0].remark.as_deref(), Some("时间: 2026-11-01"));
    assert_eq!(rows[1].name, "[视频]流浪地球3 4K 时间: 2026-11-01");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn hunhepan_search_classifies_links() {
    let base = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(
        &cfg,
        &site("csp_HunHePan", &format!("{base}/hunhepan")),
        "流浪地球3",
        false,
    )
    .expect("search");
    assert_eq!(rows.len(), 2, "实际: {rows:?}");
    assert_eq!(rows[0].id, "https://www.aliyundrive.com/s/hh1");
    assert_eq!(rows[0].name, "流浪地球3 4K");
    assert_eq!(rows[0].remark.as_deref(), Some("阿里 2026-11-01 张三"));
    assert_eq!(
        rows[0].cover.as_deref(),
        Some("https://inews.gtimg.com/newsapp_bt/0/13263837859/1000")
    );
    assert_eq!(rows[1].remark.as_deref(), Some("夸克 2026-10-01 李四"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dapanso_search_signs_request_with_sha256() {
    let base = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(
        &cfg,
        &site("csp_DaPanSo", &format!("{base}/dapanso")),
        "流浪地球3",
        false,
    )
    .expect("search");
    assert_eq!(rows.len(), 1, "实际: {rows:?}");
    assert_eq!(rows[0].id, "https://pan.xunlei.com/s/dp1###流浪地球3");
    assert_eq!(rows[0].name, "流浪地球3");
    assert_eq!(rows[0].remark.as_deref(), Some("迅雷 2026-11-03"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn miaosou_search_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_MiaoSou", &base), "流浪地球", false)
        .expect("MiaoSou 搜索");
    assert_eq!(items.len(), 1, "非网盘链接应被过滤: {items:?}");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/m1");
    assert_eq!(items[0].name, "流浪地球");
    assert_eq!(items[0].remark.as_deref(), Some("夸克 2026-11-01 alice"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn eighteight_search_via_dispatch_returns_first_page_only() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_EightEight", &base), "流浪地球", false)
        .expect("EightEight 搜索");
    assert_eq!(items.len(), 1, "实际: {items:?}");
    assert_eq!(items[0].remark.as_deref(), Some("夸克"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn yunpanone_search_builds_discussion_ids() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_YunPanOne", &base), "流浪地球", false)
        .expect("YunPanOne 搜索");
    assert_eq!(items.len(), 1, "实际: {items:?}");
    assert_eq!(items[0].name, "流浪地球3");
    assert!(items[0].id.ends_with("/d/disc1"), "实际 id: {}", items[0].id);
    assert_eq!(items[0].remark.as_deref(), Some("2026-11-02T08:00:00Z"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn pikaso_search_via_dispatch_parses_html_rows() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site("csp_PikaSo", &base), "流浪地球", false)
        .expect("PikaSo 搜索");
    assert_eq!(items.len(), 1, "非网盘/小文件应被过滤: {items:?}");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/px1");
    assert_eq!(items[0].name, "流浪地球3 4K");
    assert_eq!(items[0].remark.as_deref(), Some("夸克 /12.5 GB"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn qianfan_search_via_dispatch_parses_rows() {
    let base = start_server();
    let cfg = TvConfig::default();
    let site = site("csp_Qianfan", &format!("{base}/qianfan"));
    let items = vbox_core::sources::spider::search(&cfg, &site, "流浪", false).expect("qianfan search");
    assert_eq!(items.len(), 1, "items: {items:?}");
    assert_eq!(items[0].name, "流浪地球3 4K");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/qf1");
    assert_eq!(items[0].remark.as_deref(), Some("/12.5 GB"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn yunso_search_posts_and_parses_cards() {
    let base = start_server();
    let cfg = TvConfig::default();
    let site = site("csp_YunSo", &format!("{base}/yunso"));
    let items = vbox_core::sources::spider::search(&cfg, &site, "流浪", false).expect("yunso search");
    assert_eq!(items.len(), 1, "items: {items:?}");
    assert_eq!(items[0].name, "流浪地球3");
    assert_eq!(items[0].id, "share-abc###流浪地球3");
    let want_cover = format!("{base}/img/1.jpg");
    assert_eq!(items[0].cover.as_deref(), Some(want_cover.as_str()));
    assert_eq!(items[0].remark.as_deref(), Some("网盘 阿里"));
}
