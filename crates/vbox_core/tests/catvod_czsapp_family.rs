//! `csp_Czsapp`（厂长资源）家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、首页分类（跳过「首页」）、分类列表、
//! 详情元数据 + 播放列表、搜索、播放（AES/CBC 密文页 + 改版后 iframe
//! `url=` 直链兜底）、以及无媒体地址的错误路径。
//!
//! 上游现状：ext 指向的 `www.czzy55.com` 只剩跳转页，新站为 `www.4kcz.com`；
//! 站点已改版为 iframe 播放器且搜索被 SafeLine WAF 拦截，故真站用例只做
//! `#[ignore]` 的分类/详情冒烟（播放页无密文，走 iframe 直链兜底）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const HOME_HTML: &str = r#"<html><body>
  <ul class="navlist">
    <li><a href="/">首页</a></li>
    <li><a href="/dbtop250/">电影</a></li>
    <li><a href="/dianshiju/">电视剧</a></li>
  </ul>
  <div class="bt_img mi_ne_kd"><ul>
    <li><h3 class="dytit"><a href="/movie/20294.html">峡谷</a></h3>
        <img data-original="/pic/1.jpg"><div class="jidi"><span>HD</span></div></li>
  </ul></div>
</body></html>"#;

const LIST_HTML: &str = r#"<html><body>
  <div class="bt_img mi_ne_kd"><ul>
    <li><h3 class="dytit"><a href="/movie/2.html">第二部</a></h3>
        <img src="/pic/2.jpg"><div class="jidi"><span>更新至 12 集</span></div></li>
  </ul></div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<html><body>
  <div class="moviedteail_tt"><h1>峡谷</h1></div>
  <div class="dyimg"><img src="/pic/123.jpg"></div>
  <div class="yp_context">简介文本</div>
  <ul class="moviedteail_list">
    <li>类型：电影</li><li>地区：美国</li><li>年份：2025</li>
    <li>又名：The Gorge</li><li>上映：2025-02-14</li>
    <li><a href="/actor/1">安雅·泰勒</a></li><li>编剧：编剧甲</li>
    <li><a href="/actor/2">迈尔斯·特勒</a></li><li>语言：英语</li><li>时长：127分钟</li>
  </ul>
  <div class="paly_list_btn">
    <a href="/v_play/abc.html">1080P-1</a>
    <a href="/v_play/def.html">1080P-2</a>
  </div>
</body></html>"#;

const SEARCH_HTML: &str = r#"<html><body>
  <div class="bt_img mi_ne_kd"><ul>
    <li><h3 class="dytit"><a href="/movie/20294.html">峡谷</a></h3>
        <img src="/pic/1.jpg"><div class="jidi"><span>HD</span></div></li>
  </ul></div>
</body></html>"#;

fn respond_html(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond html");
}

fn encrypted_page(b64: &str) -> String {
    format!(
        r#"<html><body><div class="videoplay"><script>
var u="{b64}";var dncry=function dncry(a){{return a+md5.enc.Utf8.parse("0123456789abcdef")+md5.enc.Utf8.parse(1234567890123456)}}
</script></div></body></html>"#
    )
}

fn start_server(enc_b64: String) -> String {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base = format!("http://{}", server.server_addr());
    let ret = base.clone();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            if url == "/" {
                respond_html(request, HOME_HTML);
            } else if url.starts_with("/dbtop250/page/") {
                respond_html(request, LIST_HTML);
            } else if url == "/movie/20294.html" {
                respond_html(request, DETAIL_HTML);
            } else if url == "/v_play/enc.html" {
                respond_html(request, &encrypted_page(&enc_b64));
            } else if url == "/v_play/plain.html" {
                respond_html(
                    request,
                    r#"<html><body><div class="videoplay">
                        <iframe src="https://plaa.test/player?code=cs&url=https://cdn.test/direct.m3u8"></iframe>
                    </div></body></html>"#,
                );
            } else if url.starts_with("/xsss1O1") {
                respond_html(request, SEARCH_HTML);
            } else {
                request
                    .respond(Response::from_string("not found").with_status_code(404))
                    .expect("404");
            }
        }
    });
    ret
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 5_000;
    cfg
}

fn site(base: &str) -> Site {
    Site {
        key: "czsapp".into(),
        name: "厂长资源".into(),
        api: "csp_Czsapp".into(),
        kind: 3,
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn aes_payload() -> (String, &'static str) {
    use base64::Engine as _;
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    let key = "0123456789abcdef";
    let iv = "1234567890123456";
    let plain = r#"video: {url: "https://cdn.test/enc.m3u8"}"#;
    let enc = cbc::Encryptor::<aes::Aes128>::new_from_slices(key.as_bytes(), iv.as_bytes())
        .expect("cipher")
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    (base64::engine::general_purpose::STANDARD.encode(enc), key)
}

#[test]
fn dispatch_and_registry_recognise_czsapp() {
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_Czsapp"),
        Some("czsapp")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready(
        "csp_Czsapp"
    ));
    assert!(vbox_core::spider::catvod::registry::blocked_reason("csp_Czsapp").is_none());
}

#[test]
fn categories_skip_home_entry() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let cats = spider::list_categories(&config(), &site(&base)).expect("分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "dbtop250");
    assert_eq!(cats[0].type_name, "电影");
}

#[test]
fn category_page_lists_videos() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let items = spider::list_videos(&config(), &site(&base), "dbtop250", 2).expect("列表");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第二部");
    assert_eq!(items[0].id, format!("{base}/movie/2.html"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至 12 集"));
}

#[test]
fn detail_has_metadata_and_playlist() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let detail = spider::get_detail(&config(), &site(&base), &format!("{base}/movie/20294.html"))
        .expect("详情");
    assert_eq!(detail.name, "峡谷");
    assert_eq!(detail.year.as_deref(), Some("2025"));
    assert_eq!(detail.area.as_deref(), Some("美国"));
    assert_eq!(detail.director.as_deref(), Some("安雅·泰勒"));
    assert_eq!(detail.actor.as_deref(), Some("迈尔斯·特勒"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "厂长资源");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].url, format!("{base}/v_play/abc.html"));
}

#[test]
fn search_returns_rows() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let items = spider::search(&config(), &site(&base), "峡谷", true).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "峡谷");
    assert!(spider::search(&config(), &site(&base), "  ", true)
        .expect("空关键词")
        .is_empty());
}

#[test]
fn player_decrypts_legacy_page() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let value = spider::player_content(
        &config(),
        &site(&base),
        "厂长资源",
        &format!("{base}/v_play/enc.html"),
        &[],
    )
    .expect("播放");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/enc.m3u8");
    assert!(value["header"]["User-Agent"]
        .as_str()
        .unwrap_or("")
        .contains("Android"));
}

#[test]
fn player_falls_back_to_iframe_embedded_media() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let value = spider::player_content(
        &config(),
        &site(&base),
        "厂长资源",
        &format!("{base}/v_play/plain.html"),
        &[],
    )
    .expect("播放");
    assert_eq!(value["url"], "https://cdn.test/direct.m3u8");
}

#[test]
fn player_errors_when_no_media() {
    let (b64, _) = aes_payload();
    let base = start_server(b64);
    let err = spider::player_content(
        &config(),
        &site(&base),
        "厂长资源",
        &format!("{base}/v_play/missing.html"),
        &[],
    )
    .expect_err("无媒体应报错");
    assert!(format!("{err}").contains("catvod."));
}

#[test]
#[ignore = "真站用例：4kcz.com 分类/详情可用；搜索被 WAF 拦截、播放依赖外部 iframe"]
fn live_site_categories_and_detail() {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 30_000;
    let s = Site {
        key: "czsapp".into(),
        name: "厂长资源".into(),
        api: "csp_Czsapp".into(),
        kind: 3,
        ext: Some(json!("https://www.4kcz.com/")),
        ..Default::default()
    };
    let cats = match spider::list_categories(&cfg, &s) {
        Ok(v) => v,
        Err(e) if e.to_string().contains("HTTP 403") => {
            eprintln!("跳过：4kcz 对当前 TLS/HTTP 指纹返回 403（浏览器 UA 直连为 200）: {e}");
            return;
        }
        Err(e) => panic!("真站分类: {e:?}"),
    };
    assert!(cats.len() >= 3, "分类数 {}", cats.len());
    let items = spider::list_videos(&cfg, &s, &cats[0].type_id, 1).expect("真站列表");
    assert!(!items.is_empty(), "列表为空");
    let detail = spider::get_detail(&cfg, &s, &items[0].id).expect("真站详情");
    assert!(!detail.name.is_empty());
    assert!(!detail.episodes.is_empty(), "详情没有播放列表");
}
