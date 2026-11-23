//! 判据 T1：描述层三件套的完整性与引擎直连回归。
//!
//! 与既有 `catvod_*_family.rs` 的区别：那些文件走 `sources/spider.rs` 分发，
//! 描述执行失败时会回退手写家族，测试通过并不能证明描述引擎真的在工作；
//! 这里直接调用 `descriptor::engine::*`，完全绕过分发与兜底。
//!
//! 覆盖：
//! * 全部内嵌描述（T1 三份 + T4 迁移批次）能加载、schema 合法、证据文件存在、
//!   jar SHA-256 与 `profiles.json` 一致；
//! * LiteApple 直连五个入口：动态请求头（timestamp/hash）确实随请求发送，
//!   分类/列表/详情/搜索/播放的字段映射与手写实现一致。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::descriptor::{engine, registry};

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn embedded_descriptors_are_complete_and_traceable() {
    let all = registry::all();
    let mut apis: Vec<String> = all.iter().map(|d| d.api.clone()).collect();
    apis.sort();
    assert_eq!(
        apis,
        vec![
            "csp_AppYsV2",
            "csp_Auete",
            "csp_Bttwoo",
            "csp_Czsapp",
            "csp_Ddrk",
            "csp_Dm84",
            "csp_Duanju",
            "csp_FirstAid",
            "csp_Jianpian",
            "csp_Kanqiu",
            "csp_Kkys",
            "csp_Libvio",
            "csp_LiteApple",
            "csp_Meijumi",
            "csp_Mp4Mov",
            "csp_NanGua",
            "csp_SixV",
            "csp_TTian",
            "csp_Tangrenjie",
            "csp_Xb6v",
            "csp_Ysj"
        ],
        "内嵌描述清单（T1 3 份 + T4 已迁移批次；新增描述必须同步登记）"
    );

    let profiles: Value = serde_json::from_str(include_str!("../src/spider/catvod/profiles.json"))
        .expect("profiles.json 可解析");
    let jars = profiles["jars"].as_array().expect("profiles.jars 是数组");

    for desc in all {
        assert_eq!(desc.schema, 1, "{} 的 schema 版本", desc.api);
        assert!(desc.validate().is_empty(), "{} 校验失败: {:?}", desc.api, desc.validate());
        assert!(!desc.evidence.is_empty(), "{} 缺少反编译证据", desc.api);
        for evidence in &desc.evidence {
            let path = root().join(&evidence.file);
            assert!(path.is_file(), "{} 证据文件不存在: {}", desc.api, evidence.file);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("读取证据 {} 失败: {e}", evidence.file));
            let line_count = source.lines().count();
            let range = evidence
                .lines
                .as_deref()
                .unwrap_or_else(|| panic!("{} 的 {} 缺少行号区间", desc.api, evidence.method));
            let (start, end) = range
                .split_once('-')
                .unwrap_or_else(|| panic!("{} 行号区间格式错误: {range}", desc.api));
            let start: usize = start.parse().unwrap_or_else(|_| panic!("行号非数字: {range}"));
            let end: usize = end.parse().unwrap_or_else(|_| panic!("行号非数字: {range}"));
            assert!(
                start >= 1 && start <= end && end <= line_count,
                "{} 的 {} 行号区间 {range} 超出 {} 的 {line_count} 行",
                desc.api,
                evidence.method,
                evidence.file
            );
        }
        let profile = jars
            .iter()
            .find(|jar| jar["name"].as_str() == Some(desc.jar.as_str()))
            .unwrap_or_else(|| panic!("profiles.json 缺少 jar {}", desc.jar));
        assert_eq!(
            profile["sha256"].as_str(),
            Some(desc.jar_sha256.as_str()),
            "{} 的 jar SHA-256 与 profiles.json 不一致",
            desc.api
        );
    }
}

#[derive(Clone, Debug)]
struct Hit {
    url: String,
    headers: Vec<(String, String)>,
}

fn start_server() -> (String, Arc<Mutex<Vec<Hit>>>) {
    let server = Server::http("127.0.0.1:0").expect("mock server 可启动");
    let base = format!("http://{}", server.server_addr());
    let hits = Arc::new(Mutex::new(Vec::new()));
    let sink = hits.clone();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let headers: Vec<(String, String)> = request
                .headers()
                .iter()
                .map(|h| {
                    (
                        h.field.as_str().to_string().to_ascii_lowercase(),
                        h.value.as_str().to_string(),
                    )
                })
                .collect();
            sink.lock().unwrap().push(Hit { url: url.clone(), headers });
            let payload = if url.contains("androidtypes") {
                json!({"data": [
                    {"type_id": "1", "type_name": "电影"},
                    {"type_id": "2", "type_name": "剧集"}
                ]})
            } else if url.contains("androidfilter") {
                json!({"data": [
                    {"id": "10", "name": "影片A", "pic": "http://img/10.jpg", "updateInfo": "更新"},
                    {"id": "11", "name": "", "pic": "", "updateInfo": ""}
                ]})
            } else if url.contains("androiddetail2") {
                json!({"data": {
                    "name": "影片A", "pic": "http://img/10.jpg", "content": " 剧情  简介 ",
                    "year": "2024", "area": "CN", "director": "导演", "actor": "演员",
                    "className": "电影", "updateInfo": "HD",
                    "urls": [{"key": "第1集", "url": "http://x/1"}, {"key": "", "url": "http://x/2"}]
                }})
            } else if url.contains("androidsearch") {
                json!({"data": [
                    {"id": "21", "name": "目标影片", "pic": "", "updateInfo": ""},
                    {"id": "22", "name": "别的影片", "pic": "", "updateInfo": ""}
                ]})
            } else {
                json!({})
            };
            let body = payload.to_string();
            let _ = request.respond(
                Response::from_string(body).with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
                ),
            );
        }
    });
    (base, hits)
}

fn liteapple() -> &'static vbox_core::spider::catvod::descriptor::schema::Descriptor {
    registry::find("csp_LiteApple").expect("LiteApple 描述已注册")
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn liteapple_engine_direct_covers_all_entries_and_dynamic_headers() {
    let (base, hits) = start_server();
    let site: Site = serde_json::from_value(json!({
        "key": "liteapple", "name": "LiteApple", "type": 3, "api": "csp_LiteApple",
        "ext": base
    }))
    .expect("site");
    let cfg = TvConfig::default();
    let desc = liteapple();

    let cats = engine::categories(desc, &cfg, &site).expect("分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");

    let items = engine::videos(desc, &cfg, &site, "1", 2).expect("列表");
    assert_eq!(items.len(), 1, "空 id/name 的行应被丢弃");
    assert_eq!(items[0].id, "10");
    assert_eq!(items[0].remark.as_deref(), Some("更新"));

    let detail = engine::detail(desc, &cfg, &site, "10").expect("详情");
    assert_eq!(detail.name, "影片A");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "LiteApple");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].title, "http://x/2", "标题为空时回退 url");
    let desc_text = detail.desc.unwrap_or_default();
    assert!(desc_text.contains("剧情 简介"), "{desc_text}");
    assert!(desc_text.contains("分类：电影"), "{desc_text}");
    assert!(desc_text.contains("更新：HD"), "{desc_text}");

    let found = engine::search(desc, &cfg, &site, "目标").expect("搜索");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "21");

    let play = engine::player_content(desc, &cfg, &site, "tok1").expect("播放");
    assert_eq!(play["url"], json!("http://c.xpgtv.net/m3u8/tok1.m3u8"));
    let header: Value = serde_json::from_str(play["header"].as_str().expect("header 是字符串"))
        .expect("header 是 JSON 字符串");
    assert_eq!(header["user_id"], json!("XPGBOX"));
    assert_eq!(header["screenx"], json!("2331"));
    let timestamp = header["timestamp"].as_str().expect("timestamp");
    assert!(timestamp.parse::<u64>().is_ok(), "timestamp={timestamp}");
    assert_eq!(header["hash"].as_str().map(str::len), Some(4), "hash 取 md5[8..12]");

    let hits = hits.lock().unwrap();
    let list_hit = hits.iter().find(|h| h.url.contains("androidfilter")).expect("列表请求");
    assert!(list_hit.url.contains("page=2"), "{}", list_hit.url);
    let header_value = |name: &str| {
        list_hit
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(header_value("user_id").as_deref(), Some("XPGBOX"));
    assert_eq!(header_value("version").as_deref(), Some("XPGBOX com.phoenix.tv1.5.5"));
    assert_eq!(header_value("screeny").as_deref(), Some("1121"));
    assert_eq!(
        header_value("user-agent").as_deref(),
        Some("okhttp/3.12.11"),
        "描述 headers 应随请求发送"
    );
    let hash = header_value("hash").expect("动态 hash 头");
    assert_eq!(hash.len(), 4, "hash={hash}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn unknown_api_has_no_descriptor() {
    assert!(registry::find("csp_NotRegistered").is_none());
}
