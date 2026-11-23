//! `csp_Hdh`（Hdhive 影巢）家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、登录（JSON + Bearer 头 + token 复用）、固定 5 分类、
//! 列表的 movies/tv 二元路径与分页、TMDb 搜索聚合（`media_type` → 站内条目）、
//! 详情（资源列表首项元信息 + 阿里引擎展开 + 磁力线路）、播放（磁力直出 / 阿里剧集直链）、
//! 空关键词不发请求。
//!
//! 上游现状：`www.hdhive.org` 全路由返回 SPA「Redirecting…」，因此只有 mock 验收，
//! 无真站 `#[ignore]` 用例（与附录 EZ 偏差 6 一致）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const LOGIN_JSON: &str = r#"{"meta":{"access_token":"tok-h"}}"#;

const LIST_MOVIES: &str = r#"{"data":[
  {"title":"沙丘2","type":"movie","id":"693134","poster_url":"https://img.test/dune2.jpg","video_resolution":["4K","1080P"]},
  {"name":"备用标题","type":"movie","id":"2","poster_url":"https://img.test/b.jpg","video_resolution":["HD"]}
]}"#;

const LIST_TV: &str = r#"{"data":[
  {"title":"三体","type":"anime","id":"9","poster_url":"https://img.test/3t.jpg","video_resolution":["1080P"]}
]}"#;

const TMDB_SEARCH: &str = r#"{"data":[{"id":"693134","media_type":"movie"}]}"#;

const TMDB_ENTRY: &str = r#"{"data":{"title":"沙丘2","type":"movie","id":"693134","poster_url":"https://img.test/dune2.jpg","updated_at":"2026-01-02"}}"#;

const RESOURCES_JSON: &str = r#"{"data":[
  {"title":"沙丘2","movie_id":693134,"tv_id":-1,"remark":"HD","source":["官方"],
   "movie":{"poster_url":"https://img.test/dune2.jpg"},
   "url":"https://www.alipan.com/s/AAA111"},
  {"title":"沙丘2 加长版","movie_id":693134,"url":"magnet:?xt=urn:btih:xyz"},
  {"title":"无关条目","movie_id":99,"url":"https://www.alipan.com/s/ZZZ"},
  {"title":"沙丘2","movie_id":693134,"url":"https://pan.quark.cn/s/QQQ"}
]}"#;

const ANON_JSON: &str = r#"{"share_name":"影巢分享","share_id":"AAA111","file_infos":[]}"#;
const TOKEN_JSON: &str = r#"{"share_token":"tok-h-ali","expire_time":4102416000}"#;
const LIST_JSON: &str = r#"{"items":[
  {"file_id":"f1","file_name":"S01E01.mp4","type":"file","size":1024},
  {"file_id":"f2","file_name":"S01E02.mp4","type":"file","size":1024}
],"next_marker":""}"#;
const DOWNLOAD_JSON: &str = r#"{"url":"https://cdn.test/hdh/e1.mp4"}"#;

fn respond(request: tiny_http::Request, body: &str, ctype: &str) {
    let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond");
}

/// 返回 (root, 请求日志)。日志元素形如 `POST /api/v1/login auth`。
fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").expect("server");
    let root = format!("http://{}", server.server_addr());
    let log = Arc::new(Mutex::new(Vec::new()));
    let ret = root.clone();
    let log2 = Arc::clone(&log);
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let method = request.method().to_string();
            let url = request.url().to_string();
            let has_auth = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Authorization") && h.value.as_str().contains("Bearer tok-h"));
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            log2.lock().expect("log").push(format!(
                "{method} {url}{}",
                if has_auth { " auth" } else { " no-auth" }
            ));
            if url.contains("/api/v1/login") {
                respond(request, LOGIN_JSON, "application/json");
            } else if url.contains("/api/v1/customer/resources") {
                respond(request, RESOURCES_JSON, "application/json");
            } else if url.contains("/api/v1/public/tmdb/search/multi") {
                respond(request, TMDB_SEARCH, "application/json");
            } else if url.contains("/api/v1/public/movies?share_num_gt=0")
                || url.contains("/api/v1/public/tv?share_num_gt=0")
            {
                respond(request, TMDB_ENTRY, "application/json");
            } else if url.contains("/api/v1/public/movies?") {
                respond(request, LIST_MOVIES, "application/json");
            } else if url.contains("/api/v1/public/tv?") {
                respond(request, LIST_TV, "application/json");
            } else if url.contains("get_share_by_anonymous") {
                respond(request, ANON_JSON, "application/json");
            } else if url.contains("get_share_token") {
                respond(request, TOKEN_JSON, "application/json");
            } else if url.contains("adrive/v3/file/list") {
                respond(request, LIST_JSON, "application/json");
            } else if url.contains("get_share_link_download_url") {
                respond(request, DOWNLOAD_JSON, "application/json");
            } else {
                respond(request, "{}", "application/json");
            }
        }
    });
    (ret, log)
}

fn site_with_key(root: &str, key: &str) -> Site {
    let pan = json!({ "ali_base": format!("{root}/ali/") }).to_string();
    let ext = format!("{pan}$$$https://img.proxy.test/?url=$$$1$$${root}");
    Site {
        key: key.to_string(),
        name: "影巢".into(),
        api: "csp_Hdh".into(),
        kind: 3,
        searchable: true,
        ext: Some(json!(ext)),
        ..Site::default()
    }
}

fn site(root: &str) -> Site {
    site_with_key(root, "hdh-default")
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_hdhive() {
    let api = "csp_Hdh";
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(api),
        Some("hdhive")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready(api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(api).is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_are_fixed_five() {
    let (root, _log) = start_server();
    let cats = spider::list_categories(&cfg(), &site(&root)).expect("categories");
    assert_eq!(cats.len(), 5);
    assert_eq!(cats[0].type_id, "movie");
    assert_eq!(cats[3].type_name, "综艺");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn login_runs_once_and_reuses_bearer_token() {
    let (root, log) = start_server();
    let s = site_with_key(&root, "hdh-login");
    spider::list_videos(&cfg(), &s, "movie", 1).expect("first list");
    spider::list_videos(&cfg(), &s, "movie", 1).expect("second list");
    let entries = log.lock().expect("log").clone();
    let logins = entries.iter().filter(|e| e.contains("/api/v1/login")).count();
    assert_eq!(logins, 1, "login 应只发生一次: {entries:?}");
    assert!(
        entries.iter().any(|e| e.contains("/api/v1/public/movies?") && e.contains(" auth")),
        "列表请求应带 Bearer: {entries:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_movie_uses_movies_path() {
    let (root, log) = start_server();
    let rows = spider::list_videos(&cfg(), &site_with_key(&root, "hdh-movie"), "movie", 1).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "movie_id=693134");
    assert_eq!(rows[0].name, "沙丘2");
    assert_eq!(rows[0].remark.as_deref(), Some("4K"));
    assert_eq!(rows[1].name, "备用标题");
    let entries = log.lock().expect("log").clone();
    assert!(
        entries.iter().any(|e| e.contains("/api/v1/public/movies?")
            && e.contains("per_page=30&page=1")
            && e.contains("record_type=movie")),
        "{entries:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_non_movie_uses_tv_path() {
    let (root, log) = start_server();
    let rows = spider::list_videos(&cfg(), &site_with_key(&root, "hdh-anime"), "anime", 1).expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "anime_id=9");
    let entries = log.lock().expect("log").clone();
    assert!(
        entries.iter().any(|e| e.contains("/api/v1/public/tv?") && e.contains("record_type=anime")),
        "{entries:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_pass_page_number() {
    let (root, log) = start_server();
    let _ = spider::list_videos(&cfg(), &site_with_key(&root, "hdh-page"), "movie", 2).expect("list");
    let entries = log.lock().expect("log").clone();
    assert!(entries.iter().any(|e| e.contains("page=2")), "{entries:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_aggregates_tmdb_hits() {
    let (root, log) = start_server();
    let rows = spider::search(&cfg(), &site_with_key(&root, "hdh-search"), "沙丘2", false).expect("search");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "movie_id=693134");
    assert_eq!(rows[0].name, "沙丘2");
    assert_eq!(rows[0].remark.as_deref(), Some("2026-01-02"));
    let entries = log.lock().expect("log").clone();
    assert!(
        entries
            .iter()
            .any(|e| e.contains("/api/v1/public/tmdb/search/multi?query=%E6%B2%99%E4%B8%982")),
        "关键字应 URL 编码: {entries:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_expands_ali_and_keeps_magnet_line() {
    let (root, _log) = start_server();
    let detail = spider::get_detail(&cfg(), &site_with_key(&root, "hdh-detail"), "movie_id=693134")
        .expect("detail");
    assert_eq!(detail.name, "沙丘2");
    assert_eq!(detail.cover.as_deref(), Some("https://img.test/dune2.jpg"));
    assert_eq!(detail.desc.as_deref(), Some("HD [官方]"));
    let ali = detail
        .episodes
        .iter()
        .find(|g| g.group == "阿里云盘")
        .expect("阿里线路");
    assert_eq!(ali.items.len(), 2);
    assert!(ali.items[0].url.contains("###f1"));
    let magnet = detail
        .episodes
        .iter()
        .find(|g| g.group == "磁力1")
        .expect("磁力线路");
    assert_eq!(magnet.items[0].url, "magnet:?xt=urn:btih:xyz");
    // 第三个条目 id 不匹配 → 中断，夸克线路不应出现。
    assert!(!detail.episodes.iter().any(|g| g.group == "夸克网盘"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_rejects_blank_id() {
    let (root, _log) = start_server();
    let err = spider::get_detail(&cfg(), &site(&root), " ").expect_err("blank id");
    assert!(format!("{err}").contains("catvod.result.parse"), "{err}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_magnet_is_direct() {
    let (root, _log) = start_server();
    let value = spider::player_content(&cfg(), &site(&root), "磁力1", "magnet:?xt=urn:btih:xyz", &[])
        .expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "magnet:?xt=urn:btih:xyz");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_resolves_ali_episode() {
    let (root, _log) = start_server();
    let value = spider::player_content(
        &cfg(),
        &site_with_key(&root, "hdh-player"),
        "阿里云盘",
        "https://www.alipan.com/s/AAA111###f1",
        &[],
    )
    .expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/hdh/e1.mp4");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_blank_keyword_makes_no_request() {
    let (root, log) = start_server();
    let rows = spider::search(&cfg(), &site_with_key(&root, "hdh-blank"), "  ", false).expect("search");
    assert!(rows.is_empty());
    assert!(log.lock().expect("log").is_empty());
}
