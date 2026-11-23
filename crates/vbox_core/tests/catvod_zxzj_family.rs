//! `csp_Zxzj` 家族集成测试（mock 站点 + 1 项真站回归）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families::zxzj;

const HOME: &str = r#"<!DOCTYPE html><html><body>
<ul class="stui-header__menu">
  <li><a href="/vodtype/1.html">电影</a></li>
  <li><a href="/vodtype/2.html">美剧</a></li>
  <li><a href="/vodtype/9.html">综艺</a></li>
</ul></body></html>"#;

const LIST: &str = r#"<ul class="stui-vodlist">
<li><div class="stui-vodlist__box">
  <a class="stui-vodlist__thumb" href="/voddetail/4601.html" title="铁血战士"
     data-original="https://img1.doubanio.com/a.jpg">
    <span class="pic-text text-right">已完结</span></a>
  <div class="stui-vodlist__detail"><h4 class="title"><a href="/voddetail/4601.html">铁血战士</a></h4></div>
</div></li>
<li><div class="stui-vodlist__box">
  <a class="stui-vodlist__thumb" href="/voddetail/4596.html" title="惊天魔盗团3"
     data-original="//img2.doubanio.com/b.jpg">
    <span class="pic-text text-right">更新至03集</span></a>
</div></li></ul>"#;

const LIST_PAGE2: &str = r#"<ul class="stui-vodlist">
<li><div class="stui-vodlist__box">
  <a class="stui-vodlist__thumb" href="/voddetail/4500.html" title="第二页影片"
     data-original="https://img3.doubanio.com/c.jpg"><span class="pic-text">HD</span></a>
</div></li></ul>"#;

const DETAIL: &str = r#"<div class="stui-content__thumb"><a><img data-original="https://img1.doubanio.com/p.jpg"></a></div>
<div class="stui-content__detail">
  <h1 class="title">努力克服自卑的我们</h1>
  <p class="data">类型：其他 / 地区：韩国 / 年份：2026</p>
  <p class="data">主演：具教焕,高允贞</p>
  <p class="data">导演：车荣勋</p>
  <p class="data">更新：2026-06-14 08:06:37</p>
</div>
<span class="detail-content">该剧以电影界为背景的简介正文。</span>
<div class="stui-vodlist__head"><h3>播放线路5</h3></div>
<ul class="stui-content__playlist">
  <li><a href="/vodplay/4601-1-1.html">第01集</a></li>
  <li><a href="/vodplay/4601-1-2.html">第02集</a></li>
</ul>
<div class="stui-vodlist__head"><h3>播放线路3</h3></div>
<ul class="stui-content__playlist">
  <li><a href="/vodplay/4601-2-1.html">HD</a></li>
</ul>"#;

const DETAIL_EMPTY: &str = r#"<div class="stui-content__detail"><h1>无线路影片</h1></div>"#;

const SEARCH: &str = r#"<ul class="stui-vodlist">
<li><div class="stui-vodlist__box">
  <a class="stui-vodlist__thumb" href="/voddetail/4616.html" title="海贼王真人版第二季"
     data-original="https://img9.doubanio.com/s.jpg"><span class="pic-text">已完结</span></a>
</div></li></ul>"#;

const PLAY: &str = r#"<script type="text/javascript">var player_aaaa={"flag":"play","encrypt":3,"url":"https:\/\/ex.zxzja.com:9876\/player-v2\/common\/1996901707220652032\/E58AAA.m3u8","from":"line5"};</script>"#;

const PLAY_BAD: &str = r#"<html><body>无 player_aaaa</body></html>"#;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn site(base: &str) -> Site {
    Site {
        key: "zxzj".into(),
        name: "周星驰影视".into(),
        api: "csp_Zxzj".into(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let hits2 = hits.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            hits2.lock().unwrap().push(url.clone());
            let (status, body) = if url == "/" {
                (200, HOME)
            } else if url == "/vodshow/1-----------.html" {
                (200, LIST)
            } else if url == "/vodshow/2--------2---.html" {
                (200, LIST_PAGE2)
            } else if url == "/voddetail/4601.html" {
                (200, DETAIL)
            } else if url == "/voddetail/bad.html" {
                (200, DETAIL_EMPTY)
            } else if url.starts_with("/vodsearch/") {
                (200, SEARCH)
            } else if url == "/vodplay/4601-1-1.html" {
                (200, PLAY)
            } else if url == "/vodplay/bad.html" {
                (200, PLAY_BAD)
            } else {
                (404, "<html>not found</html>")
            };
            let resp = Response::from_string(body)
                .with_status_code(status)
                .with_header(header("Content-Type", "text/html; charset=utf-8"));
            let _ = req.respond(resp);
        }
    });
    (base, hits)
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_come_from_nav() {
    let (base, hits) = start_server();
    let cfg = TvConfig::default();
    let cats = spider::list_categories(&cfg, &site(&base)).expect("categories");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["电影", "美剧"]);
    assert_eq!(cats[0].type_id, "1");
    assert!(hits.lock().unwrap().iter().any(|u| u == "/"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_use_twelve_segment_route() {
    let (base, hits) = start_server();
    let cfg = TvConfig::default();
    let rows = spider::list_videos(&cfg, &site(&base), "1", 1).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "4601");
    assert_eq!(rows[0].name, "铁血战士");
    assert_eq!(rows[0].remark.as_deref(), Some("已完结"));
    assert_eq!(rows[1].cover.as_deref(), Some("https://img2.doubanio.com/b.jpg"));
    let hits = hits.lock().unwrap();
    assert!(hits.iter().any(|u| u == "/vodshow/1-----------.html"), "{hits:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_second_page_sets_page_segment() {
    let (base, hits) = start_server();
    let cfg = TvConfig::default();
    let rows = spider::list_videos(&cfg, &site(&base), "2", 2).expect("list2");
    assert_eq!(rows.len(), 1);
    let hits = hits.lock().unwrap();
    assert!(hits.iter().any(|u| u == "/vodshow/2--------2---.html"), "{hits:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_parses_meta_and_groups() {
    let (base, _hits) = start_server();
    let cfg = TvConfig::default();
    let d = spider::get_detail(&cfg, &site(&base), "4601").expect("detail");
    assert_eq!(d.name, "努力克服自卑的我们");
    assert_eq!(d.year.as_deref(), Some("2026"));
    assert_eq!(d.area.as_deref(), Some("韩国"));
    assert_eq!(d.actor.as_deref(), Some("具教焕,高允贞"));
    assert_eq!(d.director.as_deref(), Some("车荣勋"));
    assert!(d.desc.as_deref().unwrap_or_default().contains("简介正文"));
    assert_eq!(d.episodes.len(), 2);
    assert_eq!(d.episodes[0].group, "播放线路5");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(
        d.episodes[1].items[0].url,
        format!("{base}/vodplay/4601-2-1.html")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_without_playlist_reports_parse_error() {
    let (base, _hits) = start_server();
    let cfg = TvConfig::default();
    let err = spider::get_detail(&cfg, &site(&base), "bad").expect_err("should fail");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "{msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_uses_vodsearch_route() {
    let (base, hits) = start_server();
    let cfg = TvConfig::default();
    let rows = spider::search(&cfg, &site(&base), "海贼", false).expect("search");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "4616");
    let hits = hits.lock().unwrap();
    assert!(hits.iter().any(|u| u.starts_with("/vodsearch/")), "{hits:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_returns_url_and_referer() {
    let (base, _hits) = start_server();
    let cfg = TvConfig::default();
    let value = spider::player_content(
        &cfg,
        &site(&base),
        "播放线路5",
        &format!("{base}/vodplay/4601-1-1.html"),
        &[],
    )
    .expect("player");
    assert_eq!(
        value["url"].as_str().unwrap(),
        "https://ex.zxzja.com:9876/player-v2/common/1996901707220652032/E58AAA.m3u8"
    );
    assert_eq!(value["parse"].as_i64().unwrap(), 0);
    assert_eq!(value["header"]["Referer"].as_str().unwrap(), format!("{base}/"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_without_config_reports_contract_gap() {
    let (base, _hits) = start_server();
    let cfg = TvConfig::default();
    let err = spider::player_content(&cfg, &site(&base), "线路", "bad", &[])
        .expect_err("should fail");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.host.unsupported"), "{msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn family_handles_api_name() {
    assert!(zxzj::handles("csp_Zxzj"));
    assert!(!zxzj::handles("csp_SixV"));
}

#[test]
#[ignore = "真站回归：需要网络"]
fn live_zxzj_smoke() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "zxzj_live".into(),
        name: "周星驰影视".into(),
        api: "csp_Zxzj".into(),
        ..Default::default()
    };
    let cats = spider::list_categories(&cfg, &site).expect("live categories");
    assert!(!cats.is_empty());
    let cat = cats[0].type_id.clone();
    let rows = spider::list_videos(&cfg, &site, &cat, 1).expect("live list");
    assert!(!rows.is_empty(), "live list empty");
    println!("live first row: {:?}", rows[0]);
    let d = spider::get_detail(&cfg, &site, &rows[0].id).expect("live detail");
    assert!(!d.episodes.is_empty());
    println!("live detail: {} / {} 组", d.name, d.episodes.len());
    let first = &d.episodes[0].items[0];
    let play = spider::player_content(&cfg, &site, &d.episodes[0].group, &first.url, &[])
        .expect("live play");
    println!("live play url: {:?}", play["url"].as_str());
    assert!(play["url"].as_str().unwrap_or_default().starts_with("http"));
}
