//! XBPQ 原生规则引擎集成测试（本地 mock，无真实网络）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::xbpq;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let base2 = base.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = if url.starts_with("/list/") {
                r#"<html><body><ul>
                  <li><a title="影片A" href="/detail/1.html">
                    <img data-original="/img/a.jpg"/><span class="note">HD</span></a></li>
                  <li><a title="影片B" href="/detail/2.html">
                    <img data-original="/img/b.jpg"/><span class="note">更新至2集</span></a></li>
                </ul></body></html>"#
            } else if url.starts_with("/detail/") {
                r#"<html><body>
                  <div class="line"><a title="线路一">x</a>
                    <div class="eps">
                      <a href="/play/1-1-1.html">第1集</a>
                      <a href="/play/1-1-2.html">第2集</a>
                    </div>
                  </div>
                </body></html>"#
            } else if url.starts_with("/play/") {
                r#"<html><body><script>var player_aaaa={"url":"https:\/\/cdn.example.com\/x.mp4"};</script></body></html>"#
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

fn cfg(base: &str) -> TvConfig {
    let ext = json!({
        "站名": "XBPQ测试",
        "请求头": "User-Agent$手机",
        "主页url": base,
        "分类url": format!("{base}/list/{{cateId}}-{{catePg}}.html"),
        "分类": "电影$1#电视剧$2",
        "起始页": "1",
        "数组": "<li&&</li>",
        "标题": "title=*>&&<",
        "链接": "href=\"&&\"",
        "图片": "data-original=\"&&\"",
        "副标题": "class=\"note\">&&<",
        "线路数组": "class=\"line\"&&</div>",
        "线路标题": "title=*>&&<",
        "播放数组": "class=\"eps\"&&</div>",
        "播放列表": "<a&&</a>",
        "播放标题": ">&&<",
        "播放链接": "href=\"&&\"",
        "直接播放": "0"
    });
    let site = Site {
        key: "xbpq".into(),
        name: "XBPQ".into(),
        kind: 3,
        api: "csp_XBPQ".into(),
        ext: Some(serde_json::Value::String(ext.to_string())),
        ..Default::default()
    };
    TvConfig {
        sites: vec![site],
        ..Default::default()
    }
}

#[test]
fn xbpq_list_detail_play() {
    let base = start_server();
    let cfg = cfg(&base);
    let site = &cfg.sites[0];

    let cats = xbpq::list_categories(&cfg, site).unwrap();
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");

    let items = xbpq::list_videos(&cfg, site, "1", 1).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "影片A");
    assert_eq!(items[0].id, format!("{base}/detail/1.html"));
    assert_eq!(items[0].remark.as_deref(), Some("HD"));

    let detail = xbpq::get_detail(&cfg, site, &format!("{base}/detail/1.html")).unwrap();
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(
        detail.episodes[0].items[1].url,
        format!("{base}/play/1-1-2.html")
    );

    let play = xbpq::player_content(
        &cfg,
        site,
        "线路一",
        &format!("{base}/play/1-1-1.html"),
        &[],
    )
    .unwrap();
    assert_eq!(play["url"], "https://cdn.example.com/x.mp4");
    assert_eq!(play["parse"], 0);
}
