//! type=0 XML / type=4 HTTP 远程接口的集成测试
//!
//! 全部请求都发往本机 tiny_http mock（`127.0.0.1:0`），不访问真实网络。覆盖：
//! - XML：分类（文本 / name 属性两种 <ty>）、列表（根 <list> 与 <rss><list>）、
//!   详情（CDATA/实体/dl-dd 多线路）、搜索、ac=list 失败回退 ac=videolist；
//! - type=4：filter/extend 参数、`data` 包装、宽松字段、$$$ 多线路详情、
//!   搜索参数、播放信息、extend 为 http 时先抓取。

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};
use url::Url;

use vbox_core::model::{Category, Rules, Site, TvConfig};
use vbox_core::sources::{remote, xml};

const XML_CT: &str = "text/xml; charset=utf-8";
const JSON_CT: &str = "application/json; charset=utf-8";

/* ============================================================
 * 本地 mock HTTP 服务
 * ============================================================ */

#[derive(Clone, Debug)]
struct RecordedRequest {
    path: String,
    query: Vec<(String, String)>,
}

impl RecordedRequest {
    fn query_value(&self, key: &str) -> Option<&str> {
        query_value(&self.query, key)
    }
}

fn query_value<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

type MockHandler = dyn Fn(&str, &[(String, String)]) -> (u16, &'static str, String) + Send + Sync;

struct Mock {
    base: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    server: Option<Arc<Server>>,
    handle: Option<JoinHandle<()>>,
}

impl Mock {
    fn new<F>(handler: F) -> Self
    where
        F: Fn(&str, &[(String, String)]) -> (u16, &'static str, String) + Send + Sync + 'static,
    {
        let server = Arc::new(Server::http("127.0.0.1:0").expect("启动本地 mock HTTP 服务失败"));
        let addr = server
            .server_addr()
            .to_ip()
            .expect("mock 服务应监听 TCP 地址");
        let base = format!("http://{addr}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_in_thread = Arc::clone(&requests);
        let server_in_thread = Arc::clone(&server);
        let handler: Arc<MockHandler> = Arc::new(handler);

        let handle = std::thread::spawn(move || {
            for request in server_in_thread.incoming_requests() {
                let recorded = parse_request_url(request.url());
                if let Ok(mut list) = requests_in_thread.lock() {
                    list.push(recorded.clone());
                }
                let (status, content_type, body) = handler(&recorded.path, &recorded.query);
                let header = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
                    .expect("合法 Content-Type");
                let response = Response::from_string(body)
                    .with_status_code(status)
                    .with_header(header);
                let _ = request.respond(response);
            }
        });

        Self {
            base,
            requests,
            server: Some(server),
            handle: Some(handle),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.unblock();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn parse_request_url(raw: &str) -> RecordedRequest {
    let url = Url::parse(&format!("http://mock.invalid{raw}")).expect("请求 URL 可解析");
    RecordedRequest {
        path: url.path().to_string(),
        query: url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
    }
}

fn ok_xml(body: &str) -> (u16, &'static str, String) {
    (200, XML_CT, body.to_string())
}

fn ok_json(body: &str) -> (u16, &'static str, String) {
    (200, JSON_CT, body.to_string())
}

fn not_found() -> (u16, &'static str, String) {
    (404, XML_CT, "not found".to_string())
}

/// 构造一个只含单个站点的配置；站点与配置都返回，调用方按需改 `site`
fn cfg_site(kind: i32, api: String) -> (TvConfig, Site) {
    let site = Site {
        key: "mock".to_string(),
        name: "Mock".to_string(),
        kind,
        api,
        ..Site::default()
    };
    let mut cfg = TvConfig::empty();
    cfg.http = Rules::default();
    cfg.sites.push(site.clone());
    (cfg, site)
}

/* ============================================================
 * type=0 XML
 * ============================================================ */

#[test]
fn xml_categories_support_text_and_attr_name() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
        <rss version="5.1">
          <class>
            <ty id="1">电影</ty>
            <ty id="2" name="电视剧"/>
            <ty id="3"><![CDATA[动漫]]></ty>
          </class>
        </rss>"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/xml" {
            ok_xml(body)
        } else {
            not_found()
        }
    });
    let (cfg, site) = cfg_site(0, mock.url("/xml"));

    let categories = xml::list_categories(&cfg, &site).expect("XML 分类应解析成功");
    assert_eq!(categories.len(), 3);
    assert_eq!(categories[0].type_id, "1");
    assert_eq!(categories[0].type_name, "电影");
    // 兼容 `<ty id="2" name="电视剧"/>`
    assert_eq!(categories[1].type_id, "2");
    assert_eq!(categories[1].type_name, "电视剧");
    assert_eq!(categories[2].type_name, "动漫");

    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "分类首选裸 api，不应多发请求");
    assert_eq!(requests[0].path, "/xml");
    assert!(requests[0].query.is_empty(), "裸 api 不应带查询参数");
}

#[test]
fn xml_categories_map_preset_names_to_api_ids() {
    // 订阅里 categories 是字符串数组时 type_id 就是名称；真实 id 必须来自接口 <class>
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
        <rss version="5.1">
          <class>
            <ty id="1">电影</ty>
            <ty id="9">动漫</ty>
          </class>
        </rss>"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/xml" {
            ok_xml(body)
        } else {
            not_found()
        }
    });
    let (cfg, mut site) = cfg_site(0, mock.url("/xml"));
    site.categories = vec![Category::new("动漫", "动漫")];

    let categories = xml::list_categories(&cfg, &site).expect("分类应解析成功");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].type_id, "9", "名称应映射成接口真实 id");
    assert_eq!(categories[0].type_name, "动漫");
    assert_eq!(mock.requests().len(), 1, "只应请求一次裸 api");
}

#[test]
fn xml_categories_fall_back_to_preset_when_api_fails() {
    let mock = Mock::new(|_path, _query| not_found());
    let (cfg, mut site) = cfg_site(0, mock.url("/never"));
    site.categories = vec![Category::new("9", "动漫")];

    let categories = xml::list_categories(&cfg, &site).expect("接口失败时应回退预置分类");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].type_id, "9");
    assert!(!mock.requests().is_empty(), "应先尝试接口再回退");
}

#[test]
fn xml_list_videos_parses_root_list() {
    // 根节点就是 <list>（没有 <rss> 包装）
    let body = r#"<list page="1" pagecount="8" pagesize="20" recordcount="160">
        <video>
          <id>101</id>
          <name>庆余年</name>
          <pic>http://img/101.jpg</pic>
          <note>更新至 10 集</note>
        </video>
        <video><id>102</id><name>雪中悍刀行</name><note>全 38 集</note></video>
      </list>"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/xml-list" {
            ok_xml(body)
        } else {
            not_found()
        }
    });
    let (cfg, site) = cfg_site(0, mock.url("/xml-list"));

    let items = xml::list_videos(&cfg, &site, "1", 2).expect("XML 列表应解析成功");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "101");
    assert_eq!(items[0].name, "庆余年");
    assert_eq!(items[0].cover.as_deref(), Some("http://img/101.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至 10 集"));
    assert_eq!(items[1].name, "雪中悍刀行");
    assert_eq!(items[1].cover, None);

    let request = &mock.requests()[0];
    assert_eq!(request.path, "/xml-list");
    assert_eq!(request.query_value("ac"), Some("videolist"));
    assert_eq!(request.query_value("t"), Some("1"));
    assert_eq!(request.query_value("pg"), Some("2"));
}

#[test]
fn xml_list_videos_falls_back_to_list() {
    // 少数改版接口只认 ac=list：videolist 返回 JSON（解析失败）时自动回退
    let xml = r#"<rss><list><video><id>7</id><name>回退成功</name></video></list></rss>"#;
    let mock = Mock::new(move |_path, query| {
        if query_value(query, "ac") == Some("list") {
            ok_xml(xml)
        } else {
            ok_json(r#"{"code":1,"list":[]}"#)
        }
    });
    let (cfg, site) = cfg_site(0, mock.url("/xml-fallback"));

    let items = xml::list_videos(&cfg, &site, "2", 1).expect("应回退 ac=list 成功");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "回退成功");

    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].query_value("ac"), Some("videolist"));
    assert_eq!(requests[1].query_value("ac"), Some("list"));
}

#[test]
fn xml_detail_parses_dl_dd_lines_and_entities() {
    let body = r#"<rss><list><video>
        <id>42</id>
        <name><![CDATA[庆余年 & 番外]]></name>
        <pic>http://img/42.jpg</pic>
        <des>普通 &amp; 文本</des>
        <year>2019</year>
        <area>中国大陆</area>
        <director>孙皓</director>
        <actor>张若昀,李沁</actor>
        <dl>
          <dd flag="m3u8">第1集$http://v/1.m3u8#第2集$http://v/2.m3u8</dd>
          <dd flag="mp4">http://v/1.mp4</dd>
        </dl>
      </video></list></rss>"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/xml-detail" {
            ok_xml(body)
        } else {
            not_found()
        }
    });
    let (cfg, site) = cfg_site(0, mock.url("/xml-detail"));

    let detail = xml::get_detail(&cfg, &site, "42").expect("XML 详情应解析成功");
    assert_eq!(detail.id, "42");
    assert_eq!(detail.name, "庆余年 & 番外");
    assert_eq!(detail.cover.as_deref(), Some("http://img/42.jpg"));
    assert_eq!(detail.desc.as_deref(), Some("普通 & 文本"));
    assert_eq!(detail.year.as_deref(), Some("2019"));
    assert_eq!(detail.area.as_deref(), Some("中国大陆"));
    assert_eq!(detail.director.as_deref(), Some("孙皓"));
    assert_eq!(detail.actor.as_deref(), Some("张若昀,李沁"));

    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "m3u8");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].title, "第1集");
    assert_eq!(detail.episodes[0].items[0].url, "http://v/1.m3u8");
    assert_eq!(detail.episodes[0].items[1].url, "http://v/2.m3u8");
    // 无 name 的条目用「第N集」
    assert_eq!(detail.episodes[1].items[0].title, "第1集");
    assert_eq!(detail.episodes[1].items[0].url, "http://v/1.mp4");

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("ac"), Some("detail"));
    assert_eq!(request.query_value("ids"), Some("42"));
}

#[test]
fn xml_search_url_encodes_keyword() {
    let body = r#"<rss><list><video><id>5</id><name>搜索结果</name></video></list></rss>"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/xml-search" {
            ok_xml(body)
        } else {
            not_found()
        }
    });
    let (cfg, site) = cfg_site(0, mock.url("/xml-search"));

    let items = xml::search(&cfg, &site, "庆余年", false).expect("XML 搜索应解析成功");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "5");

    let request = &mock.requests()[0];
    assert_eq!(request.path, "/xml-search");
    assert_eq!(request.query_value("wd"), Some("庆余年"));
    assert_eq!(request.query_value("ac"), None, "type=0 搜索不带 ac");
}

/* ============================================================
 * type=4 HTTP 远程接口
 * ============================================================ */

#[test]
fn remote_categories_send_filter_and_extend() {
    let body = r#"{"code":1,"class":[
        {"type_id":"1","type_name":"电影"},
        {"type_id":2,"type_name":"电视剧"}
      ],"list":[{"vod_id":"9","vod_name":"首页推荐"}]}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, mut site) = cfg_site(4, mock.url("/remote"));
    site.ext = Some(json!({"token": "abc"}));

    let categories = remote::list_categories(&cfg, &site).expect("type=4 分类应解析成功");
    assert_eq!(categories.len(), 2);
    assert_eq!(categories[0].type_id, "1");
    assert_eq!(categories[0].type_name, "电影");
    // 数字 type_id 也要能收成字符串
    assert_eq!(categories[1].type_id, "2");

    let request = &mock.requests()[0];
    assert_eq!(request.path, "/remote");
    assert_eq!(request.query_value("filter"), Some("true"));
    assert_eq!(
        request.query_value("extend"),
        Some(r#"{"token":"abc"}"#),
        "站点 ext 应作为 extend 透传"
    );
}

#[test]
fn remote_categories_support_data_wrapper_and_omit_empty_extend() {
    let body = r#"{"data":{"class":[{"type_id":1,"type_name":"电影"}]}}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, site) = cfg_site(4, mock.url("/remote"));

    let categories = remote::list_categories(&cfg, &site).expect("data 包装应解析成功");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].type_id, "1");
    assert_eq!(categories[0].type_name, "电影");

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("filter"), Some("true"));
    assert_eq!(
        request.query_value("extend"),
        None,
        "ext 为空时不应带 extend"
    );
}

#[test]
fn remote_list_videos_parse_data_wrapper() {
    let body = r#"{"data":{"list":[
        {"vod_id":101,"vod_name":"甲","vod_pic":"http://img/a.jpg","vod_remarks":"更新中"},
        {"vod_id":"102","vod_name":"乙"}
      ]}}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote-list" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, mut site) = cfg_site(4, mock.url("/remote-list"));
    site.ext = Some(json!({"token": "abc"}));

    let items = remote::list_videos(&cfg, &site, "5", 3).expect("type=4 列表应解析成功");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "101");
    assert_eq!(items[0].name, "甲");
    assert_eq!(items[0].cover.as_deref(), Some("http://img/a.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新中"));
    assert_eq!(items[1].id, "102");

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("ac"), Some("list"));
    assert_eq!(request.query_value("t"), Some("5"));
    assert_eq!(request.query_value("pg"), Some("3"));
    assert_eq!(request.query_value("extend"), Some(r#"{"token":"abc"}"#));
    assert_eq!(request.query_value("filter"), None, "列表请求不带 filter");
}

#[test]
fn remote_detail_parses_multi_line_episodes() {
    let body = r#"{"code":1,"list":[{
        "vod_id":7,
        "vod_name":"测试剧",
        "vod_pic":"http://img/7.jpg",
        "vod_content":"剧情简介",
        "vod_year":"2024",
        "vod_area":"中国大陆",
        "vod_director":"导演",
        "vod_actor":"演员",
        "vod_play_from":"线路一$$$线路二",
        "vod_play_url":"第1集$http://v/11.m3u8#第2集$http://v/12.m3u8$$$第1集$http://v/21.m3u8"
      }]}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote-detail" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, mut site) = cfg_site(4, mock.url("/remote-detail"));
    site.ext = Some(json!({"uid": "u1"}));

    let detail = remote::get_detail(&cfg, &site, "7").expect("type=4 详情应解析成功");
    assert_eq!(detail.id, "7");
    assert_eq!(detail.name, "测试剧");
    assert_eq!(detail.desc.as_deref(), Some("剧情简介"));
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].url, "http://v/12.m3u8");
    assert_eq!(detail.episodes[1].group, "线路二");
    assert_eq!(detail.episodes[1].items[0].url, "http://v/21.m3u8");

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("ac"), Some("detail"));
    assert_eq!(request.query_value("ids"), Some("7"));
    assert_eq!(request.query_value("extend"), Some(r#"{"uid":"u1"}"#));
}

#[test]
fn remote_detail_maps_api_error() {
    let body = r#"{"code":404,"msg":"找不到"}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote-detail" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, site) = cfg_site(4, mock.url("/remote-detail"));

    let err = remote::get_detail(&cfg, &site, "999").expect_err("业务错误码应转成错误");
    let message = err.to_string();
    assert!(
        message.contains("404") && message.contains("找不到"),
        "错误信息应带 code/msg，实际: {message}"
    );
}

#[test]
fn remote_search_sends_wd_ac_detail_quick_false() {
    let body = r#"{"data":{"list":[
        {"vod_id":"s1","vod_name":"命中","vod_pic":"http://img/s1.jpg"}
      ]}}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote-search" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, mut site) = cfg_site(4, mock.url("/remote-search"));
    site.ext = Some(json!({"token": "abc"}));

    let items = remote::search(&cfg, &site, "庆 余年", false).expect("type=4 搜索应解析成功");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "s1");
    assert_eq!(items[0].name, "命中");

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("wd"), Some("庆 余年"));
    assert_eq!(request.query_value("ac"), Some("detail"));
    assert_eq!(request.query_value("quick"), Some("false"));
    assert_eq!(request.query_value("extend"), Some(r#"{"token":"abc"}"#));
}

#[test]
fn remote_play_returns_raw_json_fields() {
    let body = r#"{"url":"https://cdn.example/v.m3u8","header":{"Referer":"https://a/"},
        "parse":0,"jx":1,"playUrl":"https://p/"}"#;
    let mock = Mock::new(move |path, _query| {
        if path == "/remote-play" {
            ok_json(body)
        } else {
            not_found()
        }
    });
    let (cfg, mut site) = cfg_site(4, mock.url("/remote-play"));
    site.ext = Some(json!({"token": "abc"}));

    let play =
        remote::get_play(&cfg, &site, "线路一", "https://v/ep1").expect("播放信息应解析成功");
    assert_eq!(
        play.get("url").and_then(Value::as_str),
        Some("https://cdn.example/v.m3u8")
    );
    assert_eq!(play.get("parse").and_then(Value::as_i64), Some(0));
    assert_eq!(play.get("jx").and_then(Value::as_i64), Some(1));
    assert_eq!(
        play.get("playUrl").and_then(Value::as_str),
        Some("https://p/")
    );
    assert_eq!(
        play.get("header")
            .and_then(|v| v.get("Referer"))
            .and_then(Value::as_str),
        Some("https://a/")
    );

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("play"), Some("https://v/ep1"));
    assert_eq!(request.query_value("flag"), Some("线路一"));
    assert_eq!(request.query_value("extend"), Some(r#"{"token":"abc"}"#));
}

#[test]
fn remote_extend_http_url_is_fetched_first() {
    let categories_body = r#"{"class":[{"type_id":"1","type_name":"电影"}]}"#;
    let extend_body = r#"{"region":"cn"}"#;
    let mock = Mock::new(move |path, _query| match path {
        "/ext" => ok_json(extend_body),
        "/remote" => ok_json(categories_body),
        _ => not_found(),
    });
    let (cfg, mut site) = cfg_site(4, mock.url("/remote"));
    // ext 本身是 http 地址：应拉取成真实 extend（getFixUrl 行为）
    site.ext = Some(Value::String(mock.url("/ext")));

    let categories = remote::list_categories(&cfg, &site).expect("extend 拉取后分类应成功");
    assert_eq!(categories.len(), 1);

    let requests = mock.requests();
    assert!(
        requests.iter().any(|request| request.path == "/ext"),
        "应先把 ext 当作 URL 拉取"
    );
    let remote_request = requests
        .iter()
        .find(|request| request.path == "/remote")
        .expect("应有远端分类请求");
    assert_eq!(
        remote_request.query_value("extend"),
        Some(r#"{"region":"cn"}"#),
        "extend 应为拉取到的紧凑 JSON"
    );
}
