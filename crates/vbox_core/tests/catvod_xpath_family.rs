//! `csp_XPath`（TVBox 标准 XPath 规则引擎）集成验收。
//!
//! 规则实例来自 `dc(xhztv)` 的「动漫直链」站（`https://www.yingm.cc/dm/dm.json` 指向
//! `./json/动漫直链.json`，目标站 `dm84.tv` 当前仍在线）。集成测试用 tiny_http mock
//! 复刻同构的规则 JSON + 页面结构，覆盖：规则 URL 加载、分类（`cateNode`）、
//! 列表（`cateUrl` 模板）、详情（线路/剧集）、搜索（HTML 与 `json:` JSONPath）、播放模板。
//!
//! 真站用例见 `#[ignore]`：用真实规则 URL 拉取并请求 `dm84.tv`（`cargo test -- --ignored`）。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{
    get_detail, list_categories, list_videos, player_content, search,
};

const RULE_TEMPLATE: &str = r##"{
  "ua": "UA-Test/1.0",
  "homeUrl": "__BASE__/home.html",
  "cateNode": "//ul[@class='nav']/li/a",
  "cateName": "/text()",
  "cateId": "/@href",
  "cateIdR": "/list-(\\d+)\\.html",
  "cateUrl": "__BASE__/list-{cateId}-{catePg}.html",
  "cateVodNode": "//ul[@class='vlist']/li",
  "cateVodName": "/a/text()",
  "cateVodId": "/a/@href",
  "cateVodIdR": "/v/(\\w+)\\.html",
  "cateVodImg": "/img/@src",
  "cateVodMark": "/span/text()",
  "dtUrl": "__BASE__/v/{vid}.html",
  "dtNode": "//div[@class='content']",
  "dtName": "/h1/text()",
  "dtImg": "/div[@class='cover']/img/@src",
  "dtYear": "/p[@class='year']/text()",
  "dtDesc": "/div[@class='desc']/text()",
  "dtFromNode": "//ul[@class='froms']/li",
  "dtFromName": "/text()",
  "dtUrlNode": "//ul[@class='lists']",
  "dtUrlSubNode": "/li/a",
  "dtUrlId": "/@href",
  "dtUrlIdR": "/p/(\\S+)\\.html",
  "dtUrlName": "/text()",
  "playUrl": "__BASE__/p/{playUrl}.html",
  "playUa": "UA-Play/2.0",
  "playReferer": "__BASE__/play/",
  "searchUrl": "__BASE__/s?wd={wd}",
  "scVodNode": "//div[@class='result']",
  "scVodName": "/a/text()",
  "scVodId": "/a/@href",
  "scVodIdR": "/v/(\\w+)\\.html",
  "scVodImg": "/img/@src",
  "scVodMark": "/span/text()"
}"##;

/// `json:` 前缀规则：搜索结果从 JSONPath `data>rows` 取行。
const JSON_RULE_TEMPLATE: &str = r##"{
  "homeUrl": "__BASE__/home.html",
  "cateManual": {"电影": "1"},
  "searchUrl": "__BASE__/api/search?kw={wd}",
  "scVodNode": "json:data>rows",
  "scVodName": "scVodName",
  "scVodId": "scVodId",
  "scVodIdR": "(\\d+)"
}"##;

const HOME_BODY: &str = r##"<!doctype html><html><body>
<ul class="nav">
  <li><a href="/list-1.html">电影</a></li>
  <li><a href="/list-2.html">电视剧</a></li>
</ul>
</body></html>"##;

const LIST_BODY: &str = r##"<!doctype html><html><body>
<ul class="vlist">
  <li><a href="/v/abc.html">第一部</a><img src="/img/abc.jpg"><span>更新至 8 集</span></li>
  <li><a href="/v/def.html">第二部</a><img src="/img/def.jpg"><span>全 24 集</span></li>
</ul>
</body></html>"##;

const DETAIL_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <h1>测试剧</h1>
  <div class="cover"><img src="/cover.jpg"></div>
  <p class="year">2024</p>
  <div class="desc">一段简介</div>
  <ul class="froms"><li>线路一</li><li>线路二</li></ul>
  <ul class="lists"><li><a href="/p/aaa.html">第1集</a><a href="/p/bbb.html">第2集</a></li></ul>
  <ul class="lists"><li><a href="/p/ccc.html">第3集</a></li></ul>
</div>
</body></html>"##;

const SEARCH_BODY: &str = r##"<!doctype html><html><body>
<div class="result"><a href="/v/xyz.html">命中一</a><img src="/s1.jpg"><span>2023</span></div>
<div class="result"><a href="/v/qqq.html">命中二</a><img src="/s2.jpg"></div>
</body></html>"##;

const JSON_SEARCH_BODY: &str =
    r#"{"data":{"rows":[{"scVodId":"101","scVodName":"甲"},{"scVodId":"202","scVodName":"乙"}]}}"#;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str, rule: &str) -> Site {
    Site {
        key: "xpath-mock".into(),
        name: "XPath".into(),
        kind: 3,
        api: "csp_XPath".into(),
        ext: Some(json!(rule.replace("__BASE__", base))),
        ..Default::default()
    }
}

fn respond_text(request: tiny_http::Request, body: &str) {
    let header =
        Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

/// 启动 mock：`rule` 是规则模板，`json_rule` 是 JSONPath 规则模板。
fn start_server(rule: String, json_rule: String) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let rule = rule.replace("__BASE__", &base);
    let json_rule = json_rule.replace("__BASE__", &base);
    let runner = server.clone();
    thread::spawn(move || {
        for request in runner.incoming_requests() {
            let url = request.url().to_string();
            let body = if url.starts_with("/rule.json") {
                rule.clone()
            } else if url.starts_with("/rule-json.json") {
                json_rule.clone()
            } else if url.starts_with("/api/search") {
                JSON_SEARCH_BODY.to_string()
            } else if url.starts_with("/list-") {
                LIST_BODY.to_string()
            } else if url.starts_with("/v/") {
                DETAIL_BODY.to_string()
            } else if url.starts_with("/s?") || url.starts_with("/s") {
                SEARCH_BODY.to_string()
            } else {
                HOME_BODY.to_string()
            };
            respond_text(request, &body);
        }
    });
    base
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_xpath() {
    use vbox_core::spider::catvod::families::family_of;
    use vbox_core::spider::catvod::registry::{blocked_reason, is_native_ready};
    assert_eq!(family_of("csp_XPath"), Some("xpath"));
    assert!(is_native_ready("csp_XPath"));
    assert!(blocked_reason("csp_XPath").is_none(), "画像应已迁移为 rulepack");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_from_rule_home() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let categories = list_categories(&cfg(), &site).expect("categories");
    assert_eq!(categories.len(), 2, "{categories:?}");
    assert_eq!(categories[0].type_id, "1");
    assert_eq!(categories[0].type_name, "电影");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_use_cate_and_page_template() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let items = list_videos(&cfg(), &site, "1", 2).expect("videos");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, "abc");
    assert_eq!(items[0].name, "第一部");
    assert_eq!(items[0].cover.as_deref(), Some(format!("{base}/img/abc.jpg").as_str()));
    assert_eq!(items[0].remark.as_deref(), Some("更新至 8 集"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_reads_fields_and_two_lines() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let detail = get_detail(&cfg(), &site, "abc").expect("detail");
    assert_eq!(detail.name, "测试剧");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.desc.as_deref(), Some("一段简介"));
    assert_eq!(detail.episodes.len(), 2, "{:?}", detail.episodes);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].title, "第1集");
    assert_eq!(detail.episodes[0].items[0].url, "aaa");
    assert_eq!(detail.episodes[1].group, "线路二");
    assert_eq!(detail.episodes[1].items[0].url, "ccc");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_html_rows() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let items = search(&cfg(), &site, "命中", true).expect("search");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, "xyz");
    assert_eq!(items[0].remark.as_deref(), Some("2023"));
    assert_eq!(items[1].id, "qqq");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rule_url_is_fetched_from_http_ext() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let mut s = site(&base, RULE_TEMPLATE);
    s.ext = Some(json!(format!("{base}/rule.json")));
    let categories = list_categories(&cfg(), &s).expect("categories");
    assert_eq!(categories.len(), 2);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_json_path_rows() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let mut s = site(&base, RULE_TEMPLATE);
    s.ext = Some(json!(format!("{base}/rule-json.json")));
    let categories = list_categories(&cfg(), &s).expect("categories");
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].type_name, "电影");
    let items = search(&cfg(), &s, "甲", true).expect("search");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, "101");
    assert_eq!(items[0].name, "甲");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_uses_template_and_headers() {
    let base = start_server(RULE_TEMPLATE.to_string(), JSON_RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let value = player_content(&cfg(), &site, "线路一", "aaa", &[]).expect("play");
    assert_eq!(value["parse"], json!(0));
    assert_eq!(value["url"], json!(format!("{base}/p/aaa.html")));
    assert_eq!(value["header"]["User-Agent"], json!("UA-Play/2.0"));
    assert_eq!(value["header"]["Referer"], json!(format!("{base}/play/")));
}

const DM84_SEARCH_BODY: &str = r##"<!doctype html><html><body>
<div class="nav"><ul class="nav_row"><li><a href="/list-1.html">国漫</a></li></ul></div>
<div class="card"><ul class="v_list">
  <li><div class="item"><a href="/v/71.html" class="cover lazy" data-bg="http://img.test/71.jpg" title="海贼王"></a><a class="title" href="/v/71.html" title="海贼王">海贼王</a><span class="desc">第1180话</span></div></li>
  <li><div class="item"><a href="/v/719.html" class="cover lazy" data-bg="http://img.test/719.jpg" title="海贼王女"></a><a class="title" href="/v/719.html" title="海贼王女">海贼王女</a><span class="desc">完结</span></div></li>
</ul></div>
</body></html>"##;

const DM84_DETAIL_BODY: &str = r##"<!doctype html><html><body>
<div class="v_info">
  <h1 class="v_title"><a>海贼王</a></h1>
  <div class="cover"><img src="http://img.test/71-big.jpg"></div>
  <div class="v_content">蒙奇·D·路飞</div>
</div>
<ul class="tab_control play_from"><li>线路一</li><li>线路二</li></ul>
<ul class="play_list"><li><a href="/p/6329-1-1.html">第1集</a><a href="/p/6329-1-2.html">第2集</a></li></ul>
<ul class="play_list"><li><a href="/p/6329-2-1.html">第1集</a></li></ul>
</body></html>"##;

/// 用真实规则快照 + dm84 的搜索/详情页面结构做离线回归：
/// 锁定 scVod*（ul.v_list）与 dt*（v_content / play_from / play_list）选择器，
/// 不再只依赖简化模板 div.result 或真站可达性。
#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dm84_fixture_selectors_match_real_search_markup() {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let runner = server.clone();
    thread::spawn(move || {
        for request in runner.incoming_requests() {
            let url = request.url().to_string();
            let body = if url.starts_with("/v/") {
                DM84_DETAIL_BODY
            } else {
                DM84_SEARCH_BODY
            };
            respond_text(request, body);
        }
    });
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/xpath_dm84_rule.json"
    );
    let rule = std::fs::read_to_string(fixture)
        .expect("规则快照存在")
        .replace("https://dm84.tv", &base);
    let site = site(&base, &rule);

    let hits = search(&cfg(), &site, "海贼王", true).expect("search");
    assert_eq!(hits.len(), 2, "{hits:?}");
    let hit = hits.iter().find(|item| item.id == "71").expect("id=71");
    assert_eq!(hit.name, "海贼王");
    assert_eq!(hit.remark.as_deref(), Some("第1180话"));
    assert!(hit.cover.as_deref().unwrap_or_default().contains("71.jpg"));

    let detail = get_detail(&cfg(), &site, "71").expect("detail");
    assert_eq!(detail.name, "海贼王");
    assert_eq!(detail.episodes.len(), 2, "{:?}", detail.episodes);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[1].group, "线路二");
    assert_eq!(detail.episodes[1].items.len(), 1);
}

/// 真站验收：用仓库内 fixture `tests/fixtures/xpath_dm84_rule.json`（REA 阶段从 `dc(xhztv)` 的「动漫直链」配置拉取的规则快照）打 `dm84.tv`。
/// 运行：`cargo test -p vbox_core --test catvod_xpath_family -- --ignored live_`
#[test]
#[ignore]
fn live_rule_instance_lists_categories_detail_and_play() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/xpath_dm84_rule.json"
    );
    let rule = std::fs::read_to_string(path).expect("规则快照存在");
    let site = site("https://dm84.tv", &rule);
    let categories = list_categories(&cfg(), &site).expect("categories");
    assert!(categories.len() >= 3, "{categories:?}");
    let items = list_videos(&cfg(), &site, &categories[0].type_id, 1).expect("videos");
    assert!(!items.is_empty(), "{items:?}");
    let detail = get_detail(&cfg(), &site, &items[0].id).expect("detail");
    assert!(!detail.name.is_empty());
    assert!(!detail.episodes.is_empty(), "{:?}", detail.episodes);
    let episode = &detail.episodes[0].items[0];
    let play = player_content(&cfg(), &site, &detail.episodes[0].group, &episode.url, &[]).expect("play");
    let url = play["url"].as_str().unwrap_or_default();
    assert!(url.contains("dm84.tv"), "{url}");

    // 搜索页与详情页共用同一套 id 形态（/v/{id}.html）；搜索规则 scVod* 必须
    // 能直接从该页面取到条目，并让 id 继续喂给详情入口。
    let hits = search(&cfg(), &site, "海贼王", true).expect("search");
    assert!(!hits.is_empty(), "{hits:?}");
    let hit = hits
        .iter()
        .find(|item| item.name == "海贼王")
        .unwrap_or_else(|| panic!("搜索结果缺少海贼王：{:?}", hits.iter().map(|i| &i.name).collect::<Vec<_>>()));
    let hit_detail = get_detail(&cfg(), &site, &hit.id).expect("search detail");
    assert_eq!(hit_detail.name, "海贼王");
}
