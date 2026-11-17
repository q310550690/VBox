//! type=1 苹果CMS JSON 的集成测试
//!
//! 覆盖这次「豪华资源」问题的三个回归点：
//! - 订阅 `categories` 的字符串写法没有 id，必须用接口 `class` 的真实 `type_id` 映射；
//! - 列表接口用 `ac=videolist`（`ac=list` 不返回 `vod_pic`，会表现为没有封面）；
//! - 详情 `vod_play_url` 是「线路 $$$ 分隔、线路内 # 分集、首个 $ 分集名/地址」，
//!   不能把集名当播放地址。
//!
//! 所有请求都发往本机 tiny_http mock，不访问真实网络。

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tiny_http::{Header, Response, Server};
use url::Url;

use vbox_core::model::{Category, Rules, Site, TvConfig};
use vbox_core::sources::cms;

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
        self.query
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// PHP 的 `$_GET` 对重复键取最后一个；站点的 api 自带 `?ac=list` 时会用到
    fn last_query_value(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
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

fn ok_json(body: &str) -> (u16, &'static str, String) {
    (200, JSON_CT, body.to_string())
}

fn not_found() -> (u16, &'static str, String) {
    (404, JSON_CT, "not found".to_string())
}

fn cfg_site(api: String) -> (TvConfig, Site) {
    let site = Site {
        key: "hhzy".to_string(),
        name: "豪华资源".to_string(),
        kind: 1,
        api,
        ..Site::default()
    };
    let mut cfg = TvConfig::empty();
    cfg.http = Rules::default();
    cfg.sites.push(site.clone());
    (cfg, site)
}

/* ============================================================
 * 分类
 * ============================================================ */

#[test]
fn cms_categories_use_api_ids_not_preset_names() {
    // 真实接口形态：api 自带 ?ac=list，返回 class；订阅里的 categories 只有名称
    let body = r#"{"code":1,"msg":"数据列表","page":1,"list":[],
        "class":[{"type_id":1,"type_name":"电视剧"},{"type_id":20,"type_name":"内地剧"},{"type_id":3,"type_name":"欧美剧"}]}"#;
    let mock = Mock::new(move |_path, _query| ok_json(body));
    let (cfg, mut site) = cfg_site(mock.url("/api.php/provide/vod/?ac=list"));
    site.categories = vec![
        Category::new("内地剧", "内地剧"),
        Category::new("欧美剧", "欧美剧"),
        Category::new("台湾剧", "台湾剧"),
    ];

    let categories = cms::list_categories(&cfg, &site).expect("分类应解析成功");
    assert_eq!(categories.len(), 3, "保持订阅里的顺序与白名单");
    assert_eq!(categories[0].type_id, "20", "名称必须映射成接口真实 type_id");
    assert_eq!(categories[0].type_name, "内地剧");
    assert_eq!(categories[1].type_id, "3");
    // 接口没有的预置分类保留原样（不会凭空造 id，也不会让整栏消失）
    assert_eq!(categories[2].type_id, "台湾剧");

    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "有 class 时不应再补 ac=list 请求");
}

#[test]
fn cms_categories_fall_back_to_preset_when_api_fails() {
    let mock = Mock::new(|_path, _query| not_found());
    let (cfg, mut site) = cfg_site(mock.url("/api.php/provide/vod/"));
    site.categories = vec![Category::new("内地剧", "内地剧")];

    let categories = cms::list_categories(&cfg, &site).expect("接口失败应回退预置分类");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].type_name, "内地剧");
    assert!(!mock.requests().is_empty(), "应先尝试接口再回退");
}

/* ============================================================
 * 列表（封面）
 * ============================================================ */

#[test]
fn cms_list_videos_use_videolist_and_keep_covers() {
    let body = r#"{"code":1,"msg":"数据列表","page":1,"total":6916,"list":[
        {"vod_id":182325,"vod_name":"嫁金钗","vod_pic":"https://hhmage.com/cover/a.jpg","vod_remarks":"更新至第21集"}]}"#;
    let mock = Mock::new(move |_path, _query| ok_json(body));
    let (cfg, site) = cfg_site(mock.url("/api.php/provide/vod/"));

    let items = cms::list_videos(&cfg, &site, "20", 1).expect("列表应解析成功");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "嫁金钗");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://hhmage.com/cover/a.jpg")
    );
    assert_eq!(items[0].remark.as_deref(), Some("更新至第21集"));

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("ac"), Some("videolist"));
    assert_eq!(request.query_value("t"), Some("20"));
    assert_eq!(request.query_value("pg"), Some("1"));
}

#[test]
fn cms_list_videos_support_all_tab_with_empty_category() {
    // 前端「全部」标签传 category=""，对应 t= 空值；接口按最新更新返回
    let body = r#"{"code":1,"total":115703,"list":[
        {"vod_id":1,"vod_name":"最新一集","vod_pic":"https://hhmage.com/cover/b.jpg"}]}"#;
    let mock = Mock::new(move |_path, _query| ok_json(body));
    let (cfg, site) = cfg_site(mock.url("/api.php/provide/vod/"));

    let items = cms::list_videos(&cfg, &site, "", 1).expect("「全部」应解析成功");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://hhmage.com/cover/b.jpg")
    );

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("ac"), Some("videolist"));
    assert_eq!(request.query_value("t"), Some(""));
    assert_eq!(request.query_value("pg"), Some("1"));
}

#[test]
fn cms_list_videos_fall_back_to_ac_list_when_videolist_empty() {
    let legacy = r#"{"code":1,"list":[{"vod_id":7,"vod_name":"老接口内容"}]}"#;
    let mock = Mock::new(move |_path, query| {
        let ac = query
            .iter()
            .rev()
            .find(|(k, _)| k == "ac")
            .map(|(_, v)| v.as_str());
        if ac == Some("list") {
            ok_json(legacy)
        } else {
            ok_json(r#"{"code":1,"total":0,"list":[]}"#)
        }
    });
    let (cfg, site) = cfg_site(mock.url("/api.php/provide/vod/?ac=list"));

    let items = cms::list_videos(&cfg, &site, "20", 1).expect("应回退 ac=list 成功");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "老接口内容");

    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].last_query_value("ac"), Some("videolist"));
    assert_eq!(requests[1].last_query_value("ac"), Some("list"));
}

/* ============================================================
 * 详情 / 剧集
 * ============================================================ */

#[test]
fn cms_detail_parses_multi_line_episode_pairs() {
    // 真实「豪华资源」详情：两条线路，各一集，集名「全集完结」
    let body = r#"{"code":1,"list":[{
        "vod_id":182325,
        "vod_name":"小姨随礼二十块，我回敬二十一块",
        "vod_pic":"https://hhmage.com/cover/a.jpg",
        "vod_play_from":"hhyun$$$hhm3u8",
        "vod_play_note":"$$$",
        "vod_play_url":"全集完结$https://play.hhuus.com/play/bYEQJMOb$$$全集完结$https://play.hhuus.com/play/bYEQJMOb/index.m3u8"
    }]}"#;
    let mock = Mock::new(move |_path, _query| ok_json(body));
    let (cfg, site) = cfg_site(mock.url("/api.php/provide/vod/"));

    let detail = cms::get_detail(&cfg, &site, "182325").expect("详情应解析成功");
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "hhyun");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].title, "全集完结");
    assert_eq!(
        detail.episodes[0].items[0].url,
        "https://play.hhuus.com/play/bYEQJMOb"
    );
    assert_eq!(detail.episodes[1].group, "hhm3u8");
    assert_eq!(
        detail.episodes[1].items[0].url,
        "https://play.hhuus.com/play/bYEQJMOb/index.m3u8"
    );

    let request = &mock.requests()[0];
    assert_eq!(request.query_value("ac"), Some("detail"));
    assert_eq!(request.query_value("ids"), Some("182325"));
}
