//! `csp_Ysj` 原生家族集成测试（本地 mock + 可选真站）。
//!
//! 覆盖：首页分类、分类列表（URL 拼装含 class）、详情（年份/地区/主演/导演/播放线路）、
//! 搜索、播放页直出，以及 `sources::spider` 分派接线。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"
<div id="hl02"><div class="scroller"><ul class="screen_list">
  <li><a href="/index.php/vod/show/id/20.html">全部</a></li>
  <li><a href="/index.php/vod/show/class/%E5%A5%87%E5%B9%BB/id/20.html">奇幻</a></li>
  <li><a href="/index.php/vod/show/class/%E6%81%8B%E7%88%B1/id/20.html">恋爱</a></li>
</ul></div></div>
<ul class="vodlist">
  <li class="vodlist_item">
    <a class="vodlist_thumb" href="/index.php/vod/detail/id/1618.html" title="暗杀教室 第二季" data-original="/upload/vod/a.jpg">
      <span class="pic_text text_right">已完结</span>
    </a>
  </li>
</ul>"#;

const LIST_HTML: &str = r#"
<ul class="vodlist">
  <li class="vodlist_item">
    <a class="vodlist_thumb" href="/index.php/vod/detail/id/1618.html" title="暗杀教室 第二季" data-original="/upload/vod/a.jpg">
      <span class="pic_text text_right">已完结</span>
    </a>
  </li>
  <li class="vodlist_item">
    <a class="vodlist_thumb" href="/index.php/vod/detail/id/1617.html" title="暗杀教室" data-original="/upload/vod/b.jpg">
      <span class="pic_text text_right">更新至12集</span>
    </a>
  </li>
</ul>"#;

const DETAIL_HTML: &str = r#"
<h2 class="title">暗杀教室 第二季</h2>
<a class="vodlist_thumb" href="/index.php/vod/play/id/1618/sid/1/nid/1.html" title="暗杀教室 第二季" data-original="/upload/vod/a.jpg"></a>
<ul>
  <li class="data">
    <span>年份：</span><a href="/index.php/vod/search/year/2016.html">2016</a>
    <span>地区：</span><a href="/index.php/vod/search/area/%E6%97%A5%E6%9C%AC.html">日本</a>
    <span>类型：</span><a href="/index.php/vod/search/class/%E6%90%9E%E7%AC%91.html">搞笑</a>
  </li>
  <li class="data"><span>状态：</span><span class="data_style">已完结</span>&nbsp;/&nbsp;<em>09-10</em></li>
  <li class="data"><span>主演：</span>未知</li>
  <li class="data"><span>导演：</span><a href="/index.php/vod/search/director/x.html">ラルケ</a></li>
</ul>
<div class="content_desc context"><span><p>剧情简介文本</p></span></div>
<div class="play_source_tab"><a href="javascript:void(0);">主播放线路</a></div>
<div class="play_list_box"><ul class="content_playlist">
  <li><a href="/index.php/vod/play/id/1618/sid/1/nid/1.html">第1集</a></li>
  <li><a href="/index.php/vod/play/id/1618/sid/1/nid/2.html">第2集</a></li>
</ul></div>"#;

const SEARCH_HTML: &str = r#"
<ul class="vodlist">
  <li class="searchlist_item">
    <a class="vodlist_thumb" href="/index.php/vod/detail/id/3451.html" title="身为暗杀者的我" data-original="/upload/vod/c.jpg"></a>
  </li>
</ul>"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = if url.contains("/vod/show/id/20.html") {
                HOME_HTML.to_string()
            } else if url.contains("/vod/show/class/") {
                LIST_HTML.to_string()
            } else if url.contains("/vod/detail/id/") {
                DETAIL_HTML.to_string()
            } else if url.contains("/vod/search.html") {
                SEARCH_HTML.to_string()
            } else {
                String::from("<html></html>")
            };
            let _ = req.respond(
                Response::from_string(body).with_header(
                    Header::from_bytes(&b"Content-Type"[..], b"text/html; charset=utf-8").unwrap(),
                ),
            );
        }
    });
    base
}

fn site(base: &str) -> Site {
    Site {
        key: "ysj".to_string(),
        name: "dmmiku".to_string(),
        api: "csp_Ysj".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_from_home_page() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&TvConfig::default(), &site(&base))
        .expect("分类");
    assert_eq!(cats.len(), 2, "实际: {cats:?}");
    assert_eq!(cats[0].type_id, "%E5%A5%87%E5%B9%BB");
    assert_eq!(cats[0].type_name, "奇幻");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn list_videos_passes_class_in_url() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(
        &TvConfig::default(),
        &site(&base),
        "%E5%A5%87%E5%B9%BB",
        1,
    )
    .expect("列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].id, "1618.html");
    assert_eq!(items[0].name, "暗杀教室 第二季");
    assert_eq!(items[0].remark.as_deref(), Some("已完结"));
    assert!(items[0].cover.as_deref().unwrap().starts_with(&base));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_parses_metadata_and_episodes() {
    let base = start_server();
    let d = vbox_core::sources::spider::get_detail(
        &TvConfig::default(),
        &site(&base),
        "1618.html",
    )
    .expect("详情");
    assert_eq!(d.name, "暗杀教室 第二季");
    assert_eq!(d.year.as_deref(), Some("2016"));
    assert_eq!(d.area.as_deref(), Some("日本"));
    assert_eq!(d.actor.as_deref(), Some("未知"));
    assert_eq!(d.director.as_deref(), Some("ラルケ"));
    assert_eq!(d.desc.as_deref(), Some("剧情简介文本"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "主播放线路");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(d.episodes[0].items[1].url, "1618/sid/1/nid/2.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_returns_rows() {
    let base = start_server();
    let items = vbox_core::sources::spider::search(
        &TvConfig::default(),
        &site(&base),
        "暗杀",
        false,
    )
    .expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "3451.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_play_page_url() {
    let base = start_server();
    let v = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &site(&base),
        "主播放线路",
        "1618/sid/1/nid/1.html",
        &[],
    )
    .expect("播放");
    assert_eq!(v["parse"], 1);
    let url = v["url"].as_str().unwrap_or("");
    assert!(url.ends_with("/index.php/vod/play/id/1618/sid/1/nid/1.html"), "实际: {url}");
}

#[test]
#[ignore = "需要真实网络：www.dmmiku.com"]
fn live_dmmiku_categories_and_list() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "ysj_live".to_string(),
        name: "dmmiku".to_string(),
        api: "csp_Ysj".to_string(),
        ..Default::default()
    };
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("真站分类");
    assert!(!cats.is_empty(), "真站分类为空");
    println!("真站分类 {} 个，首个: {:?}", cats.len(), cats[0]);
    let first = cats[0].type_id.clone();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &first, 1).expect("真站列表");
    assert!(!items.is_empty(), "真站列表为空");
    println!("分类 {} 首条: {:?}", first, items[0]);
    let d = vbox_core::sources::spider::get_detail(&cfg, &site, &items[0].id).expect("真站详情");
    assert!(!d.name.is_empty(), "真站详情无标题");
    assert!(!d.episodes.is_empty(), "真站详情无播放线路");
    println!(
        "详情: name={} year={:?} area={:?} actor={:?} director={:?} 线路={:?} 集数={}",
        d.name,
        d.year,
        d.area,
        d.actor,
        d.director,
        d.episodes.iter().map(|g| g.group.clone()).collect::<Vec<_>>(),
        d.episodes.iter().map(|g| g.items.len()).sum::<usize>()
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn garbage_upstream_returns_stable_catvod_error() {
    let server = Server::http("127.0.0.1:0").expect("mock");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(
                Response::from_string("<html><body>garbage, not ysj</body></html>")
                    .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()),
            );
        }
    });
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "bogus")
        .expect_err("垃圾响应应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod."), "错误缺少稳定码: {msg}");
}
