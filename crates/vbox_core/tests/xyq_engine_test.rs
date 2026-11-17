//! XYQHiker 原生规则引擎集成测试：本地 tiny_http mock + 真实规则 schema。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::xyq;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let base2 = base.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = if url == "/" || url.starts_with("/list/") {
                r#"
                <html><body>
                  <ul class="stui-vodlist">
                    <li><h4><a href="/detail/1.html">电影A</a></h4>
                        <img class="lazyload" data-original="/img/a.jpg"/>
                        <span class="pic-text">HD</span></li>
                    <li><h4><a href="/detail/2.html">电影B</a></h4>
                        <img class="lazyload" data-original="/img/b.jpg"/>
                        <span class="pic-text">更新至2集</span></li>
                  </ul>
                </body></html>"#
            } else if url.starts_with("/detail/") {
                r#"
                <html><body>
                  <span>类型：</span><a>电影</a>
                  <span>年份：</span><a>2026</a>
                  <span>地区：</span><a>中国</a>
                  <span>主演：</span><p>演员甲</p>
                  <span>简介：</span><p>剧情简介</p>
                  <div class="playlist"><h3>线路一</h3>
                    <ul class="stui-content__playlist">
                      <li><a href="/play/1-1-1.html">第1集</a></li>
                      <li><a href="/play/1-1-2.html">第2集</a></li>
                    </ul>
                  </div>
                </body></html>"#
            } else if url.starts_with("/play/") {
                r#"<html><body><script>var player_aaaa={"url":"https:\/\/cdn.example.com\/x.m3u8","from":"m3u8"};</script></body></html>"#
            } else if url.starts_with("/search") {
                r#"
                <html><body><ul class="stui-vodlist__media">
                  <li><h3><a href="/detail/9.html">搜索结果</a></h3>
                      <img class="lazyload" data-original="/img/9.jpg"/>
                      <span class="pic-text">HD</span></li>
                </ul></body></html>"#
            } else if url.starts_with("/json") {
                r#"jsonp({"data":{"items":[{"name":"JSON影片","id":"123","pic":"/img/j.jpg","remark":"HD"}]}});"#
            } else {
                "<html></html>"
            };
            let _ = req.respond(Response::from_string(body).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
        }
    });
    base2
}

fn site(base: &str) -> Site {
    let rules = json!({
        "规则名": "测试规则",
        "请求头参数": "手机",
        "分类起始页码": "1",
        "分类链接": format!("{base}/list/{{cateId}}-{{catePg}}.html"),
        "分类名称": "电影&电视剧",
        "分类名称替换词": "1&2",
        "分类列表数组规则": ".stui-vodlist&&li",
        "分类片单是否Jsoup写法": "1",
        "分类片单标题": "h4&&a&&Text",
        "分类片单链接": "h4&&a&&href",
        "分类片单图片": ".lazyload&&data-original",
        "分类片单副标题": ".pic-text&&Text",
        "分类片单链接加前缀": base,
        "搜索请求头参数": "手机",
        "搜索链接": format!("{base}/search?wd={{wd}}"),
        "搜索列表数组规则": ".stui-vodlist__media&&li",
        "搜索片单是否Jsoup写法": "1",
        "搜索片单标题": "h3&&a&&Text",
        "搜索片单链接": "h3&&a&&href",
        "搜索片单图片": ".lazyload&&data-original",
        "搜索片单副标题": ".pic-text&&Text",
        "搜索片单链接加前缀": base,
        "链接是否直接播放": "0",
        "详情是否Jsoup写法": "0",
        "类型详情": "类型：</span>&&</a>",
        "年代详情": "年份：</span>&&</a>",
        "地区详情": "地区：</span>&&</a>",
        "演员详情": "主演：</span>&&</p>",
        "简介详情": "简介：</span>&&</p>",
        "线路列表数组规则": "body&&.playlist",
        "线路标题": "h3&&Text",
        "播放列表数组规则": ".stui-content__playlist",
        "选集列表数组规则": "li",
        "选集标题": "a&&Text",
        "选集链接": "a&&href",
        "选集链接加前缀": base,
        "分析MacPlayer": "0",
        "是否开启手动嗅探": "1",
        "手动嗅探视频链接关键词": ".m3u8#.mp4",
        "手动嗅探视频链接过滤词": ".html"
    });
    Site {
        key: "xyq".into(),
        name: "XYQ".into(),
        kind: 3,
        api: "csp_XYQHiker".into(),
        ext: Some(serde_json::Value::String(rules.to_string())),
        ..Default::default()
    }
}

fn cfg(base: &str) -> TvConfig {
    TvConfig {
        sites: vec![site(base)],
        ..Default::default()
    }
}

#[test]
fn xyq_home_category_detail_search_play() {
    let base = start_server();
    let cfg = cfg(&base);
    let site = &cfg.sites[0];

    let cats = xyq::list_categories(&cfg, site).unwrap();
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_name, "电影");

    let list = xyq::list_videos(&cfg, site, "1", 1).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "电影A");
    assert_eq!(list[0].id, format!("{base}/detail/1.html"));
    assert_eq!(list[0].remark.as_deref(), Some("HD"));

    let detail = xyq::get_detail(&cfg, site, &format!("{base}/detail/1.html")).unwrap();
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(
        detail.episodes[0].items[1].url,
        format!("{base}/play/1-1-2.html")
    );

    let hits = xyq::search(&cfg, site, "测试", false).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "搜索结果");

    let play = xyq::player_content(
        &cfg,
        site,
        "线路一",
        &format!("{base}/play/1-1-1.html"),
        &[],
    )
    .unwrap();
    assert_eq!(play["url"], "https://cdn.example.com/x.m3u8");
    assert_eq!(play["parse"], 0);
}

// 保持 Arc 导入使用（server 线程生命周期）
#[allow(dead_code)]
fn _touch(_: Arc<()>) {}

#[test]
fn xyq_json_list_mode() {
    let base = start_server();
    let rules = json!({
        "规则名": "JSON模式",
        "请求头参数": "手机",
        "分类起始页码": "1",
        "分类链接": format!("{base}/json?type={{cateId}}&page={{catePg}}"),
        "分类名称": "电影",
        "分类名称替换词": "1",
        "分类截取模式": "0",
        "分类Json数据二次截取": "jsonp(&&);",
        "分类列表数组规则": "data.items",
        "分类片单标题": "name",
        "分类片单链接": "id",
        "分类片单图片": "pic",
        "分类片单副标题": "remark",
        "分类片单链接加前缀": format!("{base}/d/"),
        "链接是否直接播放": "0"
    });
    let site = Site {
        key: "xyq-json".into(),
        name: "JSON".into(),
        kind: 3,
        api: "csp_XYQHiker".into(),
        ext: Some(serde_json::Value::String(rules.to_string())),
        ..Default::default()
    };
    let cfg = TvConfig {
        sites: vec![site],
        ..Default::default()
    };
    let items = xyq::list_videos(&cfg, &cfg.sites[0], "1", 1).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "JSON影片");
    assert_eq!(items[0].id, format!("{base}/d/123"));
}
