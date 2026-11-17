//! Python spider（CPython sidecar）集成测试
//!
//! - 被测脚本全部是**无第三方依赖**的内联 / 临时文件 Python spider；
//! - 不访问真实网络（只测 `base.spider` 兼容层 + 协议 + JSON 解析）；
//! - 本机没有可用 Python 解释器时，测试打印跳过原因后直接返回（不算失败）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value as Json};
use vbox_core::model::{Site, TvConfig};
use vbox_core::python;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/* ============================================================
 * 夹具
 * ============================================================ */

/// 没有 Python 时跳过（返回 true 表示可以继续）
fn ensure_python() -> bool {
    if python::python_available() {
        return true;
    }
    eprintln!("跳过：本机没有可用的 Python 解释器（可设置 VBOX_PYTHON 指向 python 可执行文件）");
    false
}

/// 临时目录里的 .py 脚本，Drop 时清理
struct TempSpider {
    dir: PathBuf,
    path: PathBuf,
}

impl TempSpider {
    fn new(name: &str, content: &str) -> Self {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "vbox_py_spider_test_{}_{}_{}",
            std::process::id(),
            nanos,
            seq
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let path = dir.join(format!("{name}.py"));
        std::fs::write(&path, content.as_bytes()).expect("写入临时 Python 脚本");
        Self { dir, path }
    }

    fn file_url(&self) -> String {
        format!("file:///{}", self.path.to_string_lossy().replace('\\', "/"))
    }
}

impl Drop for TempSpider {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn site_with(api: &str, ext: Option<Json>) -> Site {
    Site {
        key: "test".into(),
        name: "Python 测试站".into(),
        kind: 3,
        api: api.into(),
        ext,
        ..Default::default()
    }
}

fn file_site(spider: &TempSpider) -> Site {
    site_with(&spider.file_url(), None)
}

/* ============================================================
 * 测试用 Python 脚本
 * ============================================================ */

const BASE_SPIDER: &str = r##"# coding: utf-8
import sys
from base.spider import Spider

print("import-noise-should-go-to-stderr")

class Spider(Spider):
    def getName(self):
        return "测试源"

    def init(self, extend=""):
        self.extend = extend or ""
        print("init-print-noise")

    def isVideoFormat(self, url):
        return url.endswith(".m3u8")

    def homeContent(self, filter):
        return {"class": [
            {"type_id": "1", "type_name": "电影"},
            {"type_id": "2", "type_name": "电视剧"},
        ]}

    def categoryContent(self, tid, pg, filter, extend):
        if filter is not True:
            raise RuntimeError("filter 参数应为 True")
        return {"list": [
            {"vod_id": "%s-%s" % (tid, pg), "vod_name": "片" + str(tid),
             "vod_pic": "https://img/1.jpg", "vod_remarks": "更新至2集"},
        ], "page": int(pg), "pagecount": 99}

    def detailContent(self, ids):
        vid = ids[0]
        return {"list": [{
            "vod_id": vid, "vod_name": "详情片", "vod_pic": "https://img/d.jpg",
            "vod_content": "简介", "vod_year": "2024", "vod_area": "内地",
            "vod_director": "导演", "vod_actor": "演员",
            "vod_play_from": "线路A$$$线路B",
            "vod_play_url": "第1集$https://a/1.m3u8#第2集$https://a/2.m3u8$$$第1集$https://b/1.mp4",
        }]}

    def searchContent(self, key, quick, pg="1"):
        return {"list": [{"vod_id": "s-" + key, "vod_name": key,
                          "vod_remarks": "快" if quick else "全"}]}

    def playerContent(self, flag, id, vipFlags):
        return {"url": "https://a/" + id + ".m3u8",
                "header": {"User-Agent": "UA"}, "parse": 0,
                "flag": flag, "vip": len(vipFlags)}

    def liveContent(self, url):
        return "直播,#genre#\nCCTV1," + url
"##;

/* ============================================================
 * 1. 分类 / 列表
 * ============================================================ */

#[test]
fn categories_and_videos() {
    if !ensure_python() {
        return;
    }
    let spider = TempSpider::new("base", BASE_SPIDER);
    let site = file_site(&spider);
    let cfg = TvConfig::default();

    let cats = python::categories(&cfg, &site).expect("homeContent");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[1].type_name, "电视剧");

    let items = python::list_videos(&cfg, &site, "2", 3).expect("categoryContent");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "2-3");
    assert_eq!(items[0].name, "片2");
    assert_eq!(items[0].cover.as_deref(), Some("https://img/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至2集"));
}

/* ============================================================
 * 2. 详情 / 多线路选集
 * ============================================================ */

#[test]
fn detail_and_episodes() {
    if !ensure_python() {
        return;
    }
    let spider = TempSpider::new("base", BASE_SPIDER);
    let site = file_site(&spider);

    let detail = python::get_detail(&TvConfig::default(), &site, "42").expect("detailContent");
    assert_eq!(detail.id, "42");
    assert_eq!(detail.name, "详情片");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路A");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].title, "第2集");
    assert_eq!(detail.episodes[0].items[1].url, "https://a/2.m3u8");
    assert_eq!(detail.episodes[1].items[0].url, "https://b/1.mp4");
}

/* ============================================================
 * 3. 搜索 / 播放 / 直播
 * ============================================================ */

#[test]
fn search_play_live() {
    if !ensure_python() {
        return;
    }
    let spider = TempSpider::new("base", BASE_SPIDER);
    let site = file_site(&spider);
    let cfg = TvConfig::default();

    let hits = python::search(&cfg, &site, "关键词", true).expect("searchContent");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "s-关键词");
    assert_eq!(hits[0].remark.as_deref(), Some("快"));

    let play = python::player_content(&cfg, &site, "线路A", "ep1", &["flag1".to_string()])
        .expect("playerContent");
    assert_eq!(
        play.get("url").and_then(|v| v.as_str()),
        Some("https://a/ep1.m3u8")
    );
    assert_eq!(play.get("flag").and_then(|v| v.as_str()), Some("线路A"));
    assert_eq!(play.get("vip").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        play.get("header")
            .and_then(|v| v.get("User-Agent"))
            .and_then(|v| v.as_str()),
        Some("UA")
    );

    let live = python::live_content(&cfg, &site, "https://live/1.m3u8").expect("liveContent");
    assert!(live.contains("CCTV1"));
    assert!(live.contains("https://live/1.m3u8"));
}

/* ============================================================
 * 4. 缺方法 / 脚本异常
 * ============================================================ */

#[test]
fn missing_methods_degrade_as_designed() {
    if !ensure_python() {
        return;
    }
    let script = r##"# coding: utf-8
from base.spider import Spider

class Spider(Spider):
    def homeContent(self, filter):
        return {"class": []}
"##;
    let spider = TempSpider::new("partial", script);
    let site = file_site(&spider);
    let cfg = TvConfig::default();

    // 缺失 searchContent → 空数组，不算错误
    assert!(python::search(&cfg, &site, "x", false)
        .expect("缺少 searchContent 应返回空")
        .is_empty());
    // 缺失 detailContent / playerContent → 明确错误
    let err = python::get_detail(&cfg, &site, "1").expect_err("缺少 detailContent 应报错");
    assert!(
        err.to_string().contains("未实现 detailContent"),
        "实际: {err}"
    );
    let err =
        python::player_content(&cfg, &site, "f", "1", &[]).expect_err("缺少 playerContent 应报错");
    assert!(
        err.to_string().contains("未实现 playerContent"),
        "实际: {err}"
    );
}

#[test]
fn script_exception_reports_message() {
    if !ensure_python() {
        return;
    }
    let script = r##"# coding: utf-8
from base.spider import Spider

class Spider(Spider):
    def homeContent(self, filter):
        raise ValueError("boom-home")
"##;
    let spider = TempSpider::new("boom", script);
    let err = python::categories(&TvConfig::default(), &file_site(&spider))
        .expect_err("脚本抛异常应报错");
    let text = err.to_string();
    assert!(text.contains("boom-home"), "实际: {text}");
}

/* ============================================================
 * 5. 站点形态：py_ 前缀 + ext、inline、ext 对象
 * ============================================================ */

#[test]
fn py_prefix_resolves_script_from_ext() {
    if !ensure_python() {
        return;
    }
    let spider = TempSpider::new("base", BASE_SPIDER);
    let file_url = spider.file_url();
    let cfg = TvConfig::default();

    // ext 是纯字符串
    let site = site_with("py_demo", Some(Json::String(file_url.clone())));
    assert!(python::is_python_site(&site));
    let cats = python::categories(&cfg, &site).expect("py_ + ext 字符串");
    assert_eq!(cats.len(), 2);

    // ext 是对象 {url: ...}
    let site = site_with("py_demo", Some(json!({"url": file_url})));
    assert!(python::is_python_site(&site));
    let cats = python::categories(&cfg, &site).expect("py_ + ext 对象");
    assert_eq!(cats[0].type_id, "1");
}

#[test]
fn inline_script_is_supported() {
    if !ensure_python() {
        return;
    }
    let api = format!("inline:{BASE_SPIDER}");
    let site = site_with(&api, None);
    assert!(python::is_python_site(&site), "inline Python 脚本应被识别");

    let cats = python::categories(&TvConfig::default(), &site).expect("inline 脚本");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[1].type_name, "电视剧");
}

/// init(ext) 拿到的是站点 ext 的字符串形态（TVBox 的传参约定）
#[test]
fn init_receives_ext_string() {
    if !ensure_python() {
        return;
    }
    let script = r##"# coding: utf-8
from base.spider import Spider

class Spider(Spider):
    def init(self, extend=""):
        self.extend = extend

    def homeContent(self, filter):
        return {"class": [{"type_id": "1", "type_name": self.extend}]}
"##;
    let api = format!("inline:{script}");
    let site = site_with(&api, Some(json!({"token": "abc"})));
    let cats = python::categories(&TvConfig::default(), &site).expect("init ext");
    assert!(
        cats[0].type_name.contains("abc"),
        "init 应收到 ext 字符串，实际: {}",
        cats[0].type_name
    );
}

/* ============================================================
 * 6. 远程脚本下载（tiny_http mock，不访问真实网络）
 * ============================================================ */

struct MockServer {
    base: String,
    server: std::sync::Arc<tiny_http::Server>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    fn start(script: String) -> Self {
        let server =
            std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("启动 mock 服务"));
        let addr = server.server_addr().to_ip().expect("mock 服务 IP");
        let base = format!("http://{addr}");
        let listener = server.clone();
        let handle = std::thread::spawn(move || {
            for request in listener.incoming_requests() {
                let response = tiny_http::Response::from_string(script.clone()).with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"text/x-python; charset=utf-8"[..],
                    )
                    .expect("合法响应头"),
                );
                let _ = request.respond(response);
            }
        });
        Self {
            base,
            server,
            handle: Some(handle),
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn script_downloaded_from_http() {
    if !ensure_python() {
        return;
    }
    let server = MockServer::start(BASE_SPIDER.to_string());
    let site = site_with(&format!("{}/spider.py", server.base), None);
    let cfg = TvConfig::default();

    // 第一次下载；第二次应命中临时缓存（不再请求 mock）
    let cats = python::categories(&cfg, &site).expect("下载脚本后执行 homeContent");
    assert_eq!(cats.len(), 2);

    let items = python::list_videos(&cfg, &site, "1", 2).expect("缓存脚本后执行 categoryContent");
    assert_eq!(items[0].id, "1-2");
}

/* ============================================================
 * 7. 经 source 分发层（type=3 → sources/spider.rs → python）
 * ============================================================ */

#[test]
fn routes_through_source_dispatch() {
    if !ensure_python() {
        return;
    }
    let spider = TempSpider::new("base", BASE_SPIDER);
    let site = file_site(&spider);
    let cfg = TvConfig {
        sites: vec![site],
        ..Default::default()
    };

    let cats = vbox_core::source::list_categories(&cfg, "test").expect("source 分发分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");

    let items = vbox_core::source::list_videos(&cfg, "test", "1", 2).expect("source 分发列表");
    assert_eq!(items[0].id, "1-2");

    let detail = vbox_core::source::get_detail(&cfg, "test", "42").expect("source 分发详情");
    assert_eq!(detail.name, "详情片");

    let play = vbox_core::source::get_play(&cfg, "test", "线路A", "42", "", None)
        .expect("source 分发播放");
    assert_eq!(play.url, "https://a/42.m3u8");
}

/* ============================================================
 * 8. 纯识别逻辑 + 解释器探测（无 Python 也能跑）
 * ============================================================ */

#[test]
fn python_status_reports_dependencies() {
    if !ensure_python() {
        return;
    }
    let status = python::python_status();
    assert!(status.available, "状态应显示可用: {status:?}");
    assert!(status.label.is_some());
    assert!(!status.dependencies.is_empty(), "应返回依赖检查结果");
    assert!(
        status.dependencies.iter().any(|d| d.name == "requests"),
        "依赖列表应包含 requests"
    );
}

#[test]
fn detection_helpers() {
    assert!(python::is_python_site(&site_with(
        "http://cdn.example/a.py",
        None
    )));
    assert!(python::is_python_site(&site_with("./py/libvio.py", None)));
    assert!(python::is_python_site(&site_with("py_BLZB", None)));
    assert!(python::is_python_site(&site_with(
        "py_BLZB",
        Some(json!({"script": "https://x/y.py"}))
    )));
    assert!(python::is_python_site(&site_with(
        "inline:from base.spider import Spider\nclass Spider(Spider): pass",
        None
    )));
    assert!(!python::is_python_site(&site_with("csp_XYQHiker", None)));
    assert!(!python::is_python_site(&site_with("https://x/a.js", None)));
    assert!(!python::is_python_site(&site_with(
        "inline:function home(){return {}}",
        None
    )));

    // 探测结果（无解释器时 python_label() 为 None，不影响断言）
    if let Some(label) = python::python_label() {
        assert!(!label.trim().is_empty());
    }
}
