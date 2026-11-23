//! `csp_Nmys`（农民/糯米 XPathMac 家族）集成验收。
//!
//! 原类 `Nmys extends XPathMacFilter extends XPathMac extends XPath`：
//! - 列表/详情/搜索由 XPath 规则驱动（规则 JSON 来自 ext，规则内部可带 XPathMac 配置键）；
//! - `detailContent` 把每条线路的剧集倒序，并可用 `dcShow2Vip` / `pCfgJs` 把线路显示名映射回 key；
//! - `playerContent` 抓 MacCMS 播放页，取 `mac_url` 线路串定位密文，三分支解码
//!   （`asHh`→nmwz 远程、`dsjh`→nmsvip 远程、否则 `getPlayUrl` 交织 Base64）。
//!
//! 上游规则源 `https://agit.ai/.../ext/nmys.json` 已停放，故本文件只做 mock 验收；
//! 远程解析分支的域名（`api.cnmcom.com`）硬编码在常量里，无法用 mock 覆盖，只测本地解码分支与错误路径。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{
    get_detail, list_categories, list_videos, player_content,
};

/// 规则模板：XPath 常规键 + XPathMac 的 `dcShow2Vip` 线路映射。
const RULE_TEMPLATE: &str = r##"{
  "ua": "UA-Nmys/1.0",
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
  "dtFromNode": "//ul[@class='froms']/li",
  "dtFromName": "/text()",
  "dtUrlNode": "//ul[@class='lists']",
  "dtUrlSubNode": "/li/a",
  "dtUrlId": "/@href",
  "dtUrlIdR": "/p/(\\S+)\\.html",
  "dtUrlName": "/text()",
  "playUrl": "__BASE__/play/{playUrl}.html",
  "dcShow2Vip": {"keyA": "线路一", "keyB": "线路二"}
}"##;

const HOME_BODY: &str = r##"<!doctype html><html><body>
<ul class="nav"><li><a href="/list-1.html">电影</a></li><li><a href="/list-2.html">剧集</a></li></ul>
</body></html>"##;

const LIST_BODY: &str = r##"<!doctype html><html><body>
<ul class="vlist">
  <li><a href="/v/abc.html">第一部</a><img src="/img/abc.jpg"><span>更新至 8 集</span></li>
  <li><a href="/v/def.html">第二部</a><img src="/img/def.jpg"><span>全 24 集</span></li>
</ul>
</body></html>"##;

/// 两条线路，各三集；`detail` 之后应倒序且线路名被 `dcShow2Vip` 改成 key。
const DETAIL_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <h1>农民剧</h1>
  <div class="cover"><img src="/cover.jpg"></div>
  <ul class="froms"><li>线路一</li><li>线路二</li></ul>
  <ul class="lists"><li><a href="/p/e1.html">第1集</a><a href="/p/e2.html">第2集</a><a href="/p/e3.html">第3集</a></li></ul>
  <ul class="lists"><li><a href="/p/f1.html">第1集</a><a href="/p/f2.html">第2集</a></li></ul>
</div>
</body></html>"##;

/// 播放页：mac_url 只有一条线路一集，集内 `name$$密文`。
/// 密文 = 前 37 字符交错了 `http://cdn.test/v/a` 前缀，尾部是 `bc.m3u8` 的
/// Base64（`+`→`o000o`、`/`→`oo00o`、`=`→`O0O0O`），解码结果 `http://cdn.test/v/abc.m3u8`。
const PLAYER_BODY: &str = r##"<!doctype html><html><body>
<script>var mac_url='第1集$$hxtxtxpx:x/x/xcxdxnx.xtxexsxtx/xvx/xaYmMubTN1OAO0O0OO0O0O';</script>
</body></html>"##;

const PLAYER_NO_MAC_BODY: &str = r##"<!doctype html><html><body><p>no mac here</p></body></html>"##;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str, rule: &str) -> Site {
    Site {
        key: "nmys-mock".into(),
        name: "Nmys".into(),
        kind: 3,
        api: "csp_Nmys".into(),
        ext: Some(json!(rule.replace("__BASE__", base))),
        ..Default::default()
    }
}

fn respond_text(request: tiny_http::Request, body: &str) {
    let header =
        Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn start_server(rule: String) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let rule = rule.replace("__BASE__", &base);
    let runner = server.clone();
    thread::spawn(move || {
        for request in runner.incoming_requests() {
            let url = request.url().to_string();
            let body = if url.starts_with("/rule.json") {
                rule.clone()
            } else if url.starts_with("/play/") {
                if url.contains("src-2") {
                    PLAYER_NO_MAC_BODY.to_string()
                } else {
                    PLAYER_BODY.to_string()
                }
            } else if url.starts_with("/list-") {
                LIST_BODY.to_string()
            } else if url.starts_with("/v/") {
                DETAIL_BODY.to_string()
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
fn dispatch_and_registry_recognise_nmys() {
    use vbox_core::spider::catvod::families::family_of;
    use vbox_core::spider::catvod::registry::{blocked_reason, is_native_ready};
    assert_eq!(family_of("csp_Nmys"), Some("nmys"));
    assert_eq!(family_of("csp_nmys"), Some("nmys"), "api 匹配应大小写不敏感");
    assert!(is_native_ready("csp_Nmys"));
    assert!(blocked_reason("csp_Nmys").is_none(), "画像应已迁移为 rulepack");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_come_from_rule_home() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let categories = list_categories(&cfg(), &site).expect("categories");
    assert_eq!(categories.len(), 2, "{categories:?}");
    assert_eq!(categories[0].type_id, "1");
    assert_eq!(categories[0].type_name, "电影");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_use_rule_cate_template() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let items = list_videos(&cfg(), &site, "1", 1).expect("videos");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, "abc");
    assert_eq!(items[0].name, "第一部");
    assert_eq!(items[1].remark.as_deref(), Some("全 24 集"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_renames_lines_and_reverses_episodes() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let detail = get_detail(&cfg(), &site, "abc").expect("detail");
    assert_eq!(detail.name, "农民剧");
    assert_eq!(detail.episodes.len(), 2, "{:?}", detail.episodes);
    // 线路名被 dcShow2Vip 映射为 key。
    assert_eq!(detail.episodes[0].group, "keyA");
    assert_eq!(detail.episodes[1].group, "keyB");
    // 每线路内部剧集倒序（原类 detailContent 行为）。
    let titles: Vec<&str> = detail.episodes[0]
        .items
        .iter()
        .map(|e| e.title.as_str())
        .collect();
    assert_eq!(titles, vec!["第3集", "第2集", "第1集"]);
    assert_eq!(detail.episodes[0].items[0].url, "e3");
    assert_eq!(detail.episodes[1].items[0].url, "f2");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_decodes_interleaved_base64() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let value = player_content(
        &cfg(),
        &site,
        "keyA",
        "vod-play-id-1-src-1-num-1",
        &[],
    )
    .expect("play");
    assert_eq!(value["url"], json!("http://cdn.test/v/abc.m3u8"));
    assert_eq!(value["parse"], json!("0"));
    assert_eq!(value["jx"], json!("1"));
    assert_eq!(value["playUrl"], json!(""));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_id_without_src_num() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let err = player_content(&cfg(), &site, "keyA", "abc", &[]).expect_err("应报错");
    let text = format!("{err}");
    assert!(text.contains("src/num"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_reports_missing_mac_url() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let site = site(&base, RULE_TEMPLATE);
    let err = player_content(
        &cfg(),
        &site,
        "keyA",
        "vod-play-id-1-src-2-num-1",
        &[],
    )
    .expect_err("应报错");
    let text = format!("{err}");
    assert!(text.contains("mac_url"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rule_can_be_fetched_from_http_ext() {
    let base = start_server(RULE_TEMPLATE.to_string());
    let mut site = site(&base, RULE_TEMPLATE);
    site.ext = Some(json!(format!("{base}/rule.json")));
    let categories = list_categories(&cfg(), &site).expect("categories");
    assert_eq!(categories.len(), 2);
}
