//! `csp_SixV` 原生家族集成测试（本地 mock，无真实网络）。
//!
//! 覆盖：首页分类（固定 12 个）、分类分页、详情（magnet 分组 + 中文标签元数据）、
//! 帝国 CMS 表单搜索、magnet 直出播放，以及 `sources::spider` 分派接线。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};
use vbox_core::model::{Site, TvConfig};

const LIST_HTML: &str = r#"
<div id="post_container">
  <div class="post_hover">
    <a class="zoom" href="/voddetail/1/" title="片一"><img src="/img/1.jpg"></a>
    <span rel="category tag">国剧</span>
  </div>
  <div class="post_hover">
    <a class="zoom" href="/voddetail/2/" title="片二"><img src="/img/2.jpg"></a>
    <span rel="category tag">日韩剧</span>
  </div>
</div>"#;

const DETAIL_HTML: &str = r#"
<div class="article_container"><h1>片一 详情</h1></div>
<div id="post_content">
  <img src="/img/1-big.jpg">
  <table><tr>
    <td><a href="magnet:?xt=urn:btih:aaa">第一集</a></td>
    <td><a href="magnet:?xt=urn:btih:bbb">第二集</a></td>
  </tr></table>
  <table><tr><td><a href="magnet:?xt=urn:btih:ccc">1080P</a></td></tr></table>
</div>
<div class="context"><p>
  ◎类　　别　国剧<br>◎年　　代　2024<br>◎产　　地　中国<br>
  ◎上映日期　2024-01-01<br>◎导　　演　张三<br>◎主　　演　李四<br>
  ◎简　　介<br>简介正文<hr>
</p></div>"#;

const SEARCH_HTML: &str = r#"
<div id="post_container">
  <a class="zoom" href="/voddetail/9/" title="片九"><img src="/img/9.jpg"></a>
</div>"#;

/// 启动 mock 站点，返回基址。
fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let method = req.method().clone();
            let (body, ctype) = match (&method, url.as_str()) {
                (&Method::Get, "/dianshiju/guoju") => {
                    (LIST_HTML.to_string(), "text/html; charset=utf-8")
                }
                (&Method::Get, "/xijupian/index_2.html") => (
                    r#"<div id="post_container">
                         <div class="post_hover">
                           <a class="zoom" href="/voddetail/3/" title="片三"><img src="/img/3.jpg"></a>
                           <span rel="category tag">喜剧片</span>
                         </div>
                       </div>"#
                        .to_string(),
                    "text/html; charset=utf-8",
                ),
                (&Method::Get, "/voddetail/1/") => {
                    (DETAIL_HTML.to_string(), "text/html; charset=utf-8")
                }
                (&Method::Post, "/e/search/index.php") => {
                    let mut payload = String::new();
                    let _ = req.as_reader().read_to_string(&mut payload);
                    assert!(
                        payload.contains("keyboard="),
                        "搜索表单缺少 keyboard: {payload}"
                    );
                    assert!(
                        payload.contains("dopost=search"),
                        "搜索表单缺少 dopost: {payload}"
                    );
                    (SEARCH_HTML.to_string(), "text/html; charset=utf-8")
                }
                _ => ("<html></html>".to_string(), "text/html; charset=utf-8"),
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

fn site(base: &str) -> Site {
    Site {
        key: "sixv".to_string(),
        name: "6vdy".to_string(),
        api: "csp_SixV".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
fn categories_are_fixed_twelve_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base))
        .expect("分类不应发请求");
    assert_eq!(cats.len(), 12);
    assert_eq!(cats[0].type_id, "dianshiju/guoju");
    assert_eq!(cats[11].type_name, "动画片");
}

#[test]
fn category_page_lists_items_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::list_videos(
        &cfg,
        &site(&base),
        "dianshiju/guoju",
        1,
    )
    .expect("分类列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].id, "/voddetail/1/");
    assert_eq!(items[0].name, "片一");
    assert_eq!(items[0].cover.as_deref(), Some("/img/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("国剧"));
}

#[test]
fn second_page_uses_index_suffix() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "xijupian", 2)
        .expect("第二页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "片三");
}

#[test]
fn detail_parses_magnets_and_metadata_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "/voddetail/1/")
        .expect("详情");
    assert_eq!(detail.name, "片一 详情");
    assert_eq!(detail.cover.as_deref(), Some("/img/1-big.jpg"));
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.area.as_deref(), Some("中国"));
    assert_eq!(detail.director.as_deref(), Some("张三"));
    assert_eq!(detail.actor.as_deref(), Some("李四"));
    assert!(detail.desc.as_deref().unwrap_or("").contains("简介正文"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "magnet1");
    assert_eq!(detail.episodes[0].items[0].url, "magnet:?xt=urn:btih:aaa");
    assert_eq!(detail.episodes[1].group, "magnet2");
}

#[test]
fn search_posts_form_and_parses_rows() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "全部", false)
        .expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/voddetail/9/");
    assert_eq!(items[0].name, "片九");
}

#[test]
fn player_returns_magnet_directly() {
    let base = start_server();
    let cfg = TvConfig::default();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "magnet1",
        "magnet:?xt=urn:btih:aaa",
        &[],
    )
    .expect("播放信息");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "magnet:?xt=urn:btih:aaa");
}

#[test]
fn missing_ext_reports_clear_error() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "sixv".to_string(),
        api: "csp_SixV".to_string(),
        ..Default::default()
    };
    let err = vbox_core::sources::spider::list_videos(&cfg, &site, "xijupian", 1)
        .expect_err("缺少 ext 必须报错");
    let msg = err.to_string();
    assert!(msg.contains("ext"), "实际: {msg}");
}

/// 真站验证（默认忽略）：`https://www.6vdy.org` 目前在线，用来确认选择器/字段在
/// 真实页面上仍然成立。跑法：
/// `cargo test -p vbox_core --test catvod_sixv_family -- --ignored --nocapture`
#[test]
#[ignore = "需要真实网络（www.6vdy.org）"]
fn live_sixv_parses_real_site() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "sixv-live".to_string(),
        name: "6vdy(真站)".to_string(),
        api: "csp_SixV".to_string(),
        ext: Some(json!("https://www.6vdy.org")),
        ..Default::default()
    };

    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("分类");
    assert_eq!(cats.len(), 12);

    let items = vbox_core::sources::spider::list_videos(&cfg, &site, "dianshiju/guoju", 1)
        .expect("真站分类列表");
    assert!(!items.is_empty(), "真站分类页没有解析出条目（站点改版？）");
    println!("live list: {} 条，首条 = {:?}", items.len(), items[0]);

    let detail = vbox_core::sources::spider::get_detail(&cfg, &site, &items[0].id)
        .expect("真站详情");
    assert!(!detail.name.is_empty(), "详情缺标题");
    println!(
        "live detail: name={:?} episodes={} groups，首组={:?}",
        detail.name,
        detail.episodes.len(),
        detail.episodes.first().map(|g| (&g.group, g.items.len()))
    );
}
