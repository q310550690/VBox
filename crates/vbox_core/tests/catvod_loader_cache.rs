//! 判据 76：jar 下载缓存的「命中 / 完整性 / 回退 / 伪装裁剪」行为。
//!
//! 为什么需要这个判据：
//! `JarStore` 是所有 jar 站点的字节入口，但既有单测只覆盖本地文件（file://）。
//! 远程路径里的三个不变量没有任何测试锁住：
//! - md5 命中时二次加载**不得再发请求**（缓存即正确字节）；
//! - 下载内容 md5 不符时**不得落盘**，否则错误字节会通过「回退旧缓存」长期存活；
//! - 无 md5 的缓存过期后下载失败，应回退旧缓存并明确 `from_cache=true`；
//! - `img+` 伪装在远程路径也要裁掉前缀，缓存里必须是 zip 字节（否则下次读缓存会失败）。
//!
//! 测试用本地 tiny_http 服务器，不访问外网。

use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tiny_http::{Response, Server};
use vbox_core::model::Site;
use vbox_core::spider::catvod::loader::JarStore;

const ZIP: &[u8] = b"PK\x03\x04";

fn site() -> Site {
    Site {
        key: "loader-probe".into(),
        name: "loader-probe".into(),
        kind: 3,
        api: "csp_LoaderProbe".into(),
        ..Default::default()
    }
}

fn payload(tag: &str) -> Vec<u8> {
    let mut v = ZIP.to_vec();
    v.extend_from_slice(tag.as_bytes());
    v
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vbox-loader-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

struct Mock {
    base: String,
    hits: Arc<AtomicUsize>,
    body: Arc<Mutex<Vec<u8>>>,
    status: Arc<AtomicU16>,
    server: Option<Arc<Server>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Mock {
    fn new(body: Vec<u8>) -> Self {
        let server = Arc::new(Server::http("127.0.0.1:0").expect("启动本地 mock 服务"));
        let addr = server.server_addr().to_ip().expect("mock 监听 TCP");
        let hits = Arc::new(AtomicUsize::new(0));
        let body = Arc::new(Mutex::new(body));
        let status = Arc::new(AtomicU16::new(200));
        let hits_thread = Arc::clone(&hits);
        let body_thread = Arc::clone(&body);
        let status_thread = Arc::clone(&status);
        let server_thread = Arc::clone(&server);
        let handle = std::thread::spawn(move || {
            for request in server_thread.incoming_requests() {
                hits_thread.fetch_add(1, Ordering::SeqCst);
                let bytes = body_thread.lock().map(|b| b.clone()).unwrap_or_default();
                let code = status_thread.load(Ordering::SeqCst);
                let _ = request.respond(Response::from_data(bytes).with_status_code(code));
            }
        });
        Self {
            base: format!("http://{addr}"),
            hits,
            body,
            status,
            server: Some(server),
            handle: Some(handle),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn set_body(&self, bytes: Vec<u8>) {
        *self.body.lock().expect("mock body lock") = bytes;
    }

    fn set_status(&self, code: u16) {
        self.status.store(code, Ordering::SeqCst);
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.unblock();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

fn jar_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .map(|iter| {
            iter.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "jar").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn md5_pinned_jar_is_cached_and_served_without_second_request() {
    let body = payload("cache-a");
    let mock = Mock::new(body.clone());
    let dir = tmp_dir("cache");
    let store = JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60));
    let client = reqwest::blocking::Client::new();
    let raw = format!("{};md5;{}", mock.url("/a.jar"), md5_hex(&body));

    let first = store.load(&raw, &site(), &client).expect("首次下载应成功");
    assert!(!first.from_cache, "首次加载不应来自缓存");
    assert_eq!(first.bytes, body.len() as u64);
    assert_eq!(first.md5, md5_hex(&body));
    assert!(first.path.exists(), "缓存文件应落盘");
    assert!(first.path.starts_with(dir.join("store")), "缓存必须落在配置根内");

    let second = store.load(&raw, &site(), &client).expect("二次加载应命中缓存");
    assert!(second.from_cache, "二次加载应来自缓存");
    assert_eq!(second.path, first.path);
    assert_eq!(mock.hits(), 1, "md5 命中时不应再次发请求");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn md5_mismatch_never_persists_bad_bytes_and_retry_succeeds() {
    let good = payload("good");
    let bad = payload("bad");
    let mock = Mock::new(bad);
    let dir = tmp_dir("integrity");
    let store = JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60));
    let client = reqwest::blocking::Client::new();
    let raw = format!("{};md5;{}", mock.url("/b.jar"), md5_hex(&good));

    let err = store.load(&raw, &site(), &client).expect_err("md5 不符必须失败");
    let text = err.to_string();
    assert!(text.contains("catvod.jar.integrity"), "应报完整性错误: {text}");
    assert!(
        jar_files(&dir.join("store")).is_empty(),
        "md5 不符不得写缓存（否则回退路径会长期使用错误字节）"
    );

    mock.set_body(good.clone());
    let loaded = store.load(&raw, &site(), &client).expect("修正后应成功");
    assert!(!loaded.from_cache);
    assert_eq!(loaded.md5, md5_hex(&good));
    assert_eq!(jar_files(&dir.join("store")).len(), 1);
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn expired_cache_falls_back_when_download_fails() {
    let body = payload("fallback");
    let mock = Mock::new(body.clone());
    let dir = tmp_dir("fallback");
    let store = JarStore::new(dir.join("store")).with_ttl(Duration::ZERO);
    let client = reqwest::blocking::Client::new();
    let raw = mock.url("/f.jar"); // 无 md5：走 TTL 路径

    let first = store.load(&raw, &site(), &client).expect("首次下载应成功");
    assert!(!first.from_cache);
    assert_eq!(mock.hits(), 1);

    mock.set_status(500); // TTL=0 迫使重下，服务端返回失败
    let second = store.load(&raw, &site(), &client).expect("下载失败应回退旧缓存");
    assert_eq!(mock.hits(), 2, "TTL 过期后必须尝试重下");
    assert!(second.from_cache, "回退结果必须标记 from_cache");
    assert_eq!(second.md5, md5_hex(&body));
    assert_eq!(
        second.bytes,
        body.len() as u64,
        "回退的必须是完整旧字节"
    );
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn img_plus_remote_strips_prefix_and_caches_zip_bytes() {
    let zip = payload("disguised");
    let mut disguised = vec![0xFF, 0xD8, 0xFF, 0xE0];
    disguised.extend_from_slice(&zip);
    let mock = Mock::new(disguised);
    let dir = tmp_dir("imgplus");
    let store = JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60));
    let client = reqwest::blocking::Client::new();
    let raw = format!("img+{};md5;{}", mock.url("/i.jpg"), md5_hex(&zip));

    let loaded = store.load(&raw, &site(), &client).expect("伪装 jar 应可裁剪");
    assert_eq!(loaded.stripped_prefix, 4);
    assert_eq!(loaded.bytes, zip.len() as u64);
    let cached = std::fs::read(&loaded.path).expect("读取缓存");
    assert_eq!(&cached[..4], ZIP, "缓存必须是裁剪后的 zip 字节");
    assert_eq!(md5_hex(&cached), md5_hex(&zip));

    let again = store.load(&raw, &site(), &client).expect("二次加载");
    assert!(again.from_cache);
    assert_eq!(again.stripped_prefix, 0, "缓存已是 zip，无需再次裁剪");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn concurrent_loads_use_unique_temp_and_never_corrupt_cache() {
    // 同一 jar 可能被多个站点同时加载。旧实现所有下载都写同一个
    // `<md5>.jar.part`，并发 write 交错后 rename 会落盘损坏字节，
    // 之后每次读缓存都会 md5 失败、反复重下。这里用 4 个线程同时加载，
    // 断言：全部成功、字节/md5 一致、缓存唯一且完整、无 `.part` 残留。
    let body = payload("concurrent");
    let mock = Mock::new(body.clone());
    let dir = tmp_dir("concurrent");
    let store = Arc::new(JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60)));
    let raw = Arc::new(format!("{};md5;{}", mock.url("/c.jar"), md5_hex(&body)));
    let client = Arc::new(reqwest::blocking::Client::new());

    let mut handles = Vec::new();
    for _ in 0..4 {
        let store = Arc::clone(&store);
        let raw = Arc::clone(&raw);
        let client = Arc::clone(&client);
        handles.push(std::thread::spawn(move || store.load(&raw, &site(), &client)));
    }
    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("加载线程不应 panic"))
        .collect();

    let errors: Vec<String> = results
        .iter()
        .filter_map(|r| r.as_ref().err().map(|e| e.to_string()))
        .collect();
    assert!(errors.is_empty(), "并发加载不应失败：{errors:?}");
    for loaded in results.iter().flatten() {
        assert_eq!(loaded.md5, md5_hex(&body), "并发结果的 md5 必须一致");
        assert_eq!(loaded.bytes, body.len() as u64);
    }

    let cache_dir = store.root();
    let jars = jar_files(cache_dir);
    assert_eq!(jars.len(), 1, "同一 jar 只应有一个缓存文件");
    let cached = std::fs::read(&jars[0]).expect("读取缓存文件");
    assert_eq!(cached, body, "缓存字节必须完整（不能是并发写坏的半成品）");
    let leftovers = std::fs::read_dir(cache_dir)
        .expect("读取缓存目录")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .count();
    assert_eq!(leftovers, 0, "不应残留 .part 临时文件");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn oversized_jar_is_rejected_instead_of_truncated_and_cached() {
    // 旧实现用 `resp.take(64 MiB)` 读取：超过上限的响应会被**静默截断**。
    // 对有 md5 的 jar 只是 md5 不符（安全但报错含糊）；对无 md5 的 jar，
    // 截断后的字节会按 TTL 缓存 3 天，后续 dex 解析失败且无法定位。
    let mut body = payload("oversize");
    body.resize(2048, 0);
    let mock = Mock::new(body);
    let dir = tmp_dir("oversize");
    let store = JarStore::new(dir.join("store"))
        .with_ttl(Duration::from_secs(60))
        .with_max_bytes(1024);
    let client = reqwest::blocking::Client::new();

    let err = store
        .load(&mock.url("/big.jar"), &site(), &client)
        .expect_err("超过上限的 jar 必须报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.jar.layout"), "错误码应为 jar.layout：{msg}");
    assert!(msg.contains("上限"), "错误信息应说明超过上限：{msg}");
    assert!(
        jar_files(&store.root()).is_empty(),
        "超限 jar 不得落盘（尤其不能缓存截断字节）"
    );
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn local_jar_over_limit_is_rejected() {
    // `file://` 本地 jar 走的是另一条读取路径，之前完全绕过了 max_bytes：
    // 一个超大本地文件会被无界读入内存，和远程上限的语义不一致。
    let dir = tmp_dir("local-limit");
    let path = dir.join("big.jar");
    let mut body = payload("local-oversize");
    body.resize(2048, 0);
    std::fs::write(&path, &body).unwrap();
    let store = JarStore::new(dir.join("store")).with_max_bytes(1024);
    let client = reqwest::blocking::Client::new();
    let url = format!("file://{}", path.display());

    let err = store.load(&url, &site(), &client).expect_err("本地超限 jar 必须报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod.jar.layout"), "错误码应为 jar.layout：{msg}");
    assert!(msg.contains("上限"), "错误信息应说明超过上限：{msg}");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn oversized_cache_is_ignored_and_redownloaded() {
    // 缓存文件可能被外部替换/损坏为超大文件；读缓存也必须受 max_bytes 约束，
    // 超过上限时视为无效并重新下载，而不是无界读入内存后继续使用。
    let body = payload("cache-small");
    let mock = Mock::new(body.clone());
    let dir = tmp_dir("cache-oversize");
    let client = reqwest::blocking::Client::new();
    let raw = mock.url("/oversize.jar");

    let wide = JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60));
    let first = wide.load(&raw, &site(), &client).expect("首次下载应成功");
    let mut huge = ZIP.to_vec();
    huge.resize(4096, 0x41);
    std::fs::write(&first.path, &huge).expect("写入超大缓存");

    let narrow = JarStore::new(dir.join("store"))
        .with_ttl(Duration::from_secs(60))
        .with_max_bytes(1024);
    let second = narrow.load(&raw, &site(), &client).expect("超大缓存应被忽略并重下");
    assert!(!second.from_cache, "超大缓存不得被当作有效缓存");
    assert_eq!(second.bytes, body.len() as u64);
    assert_eq!(second.md5, md5_hex(&body));
    assert_eq!(mock.hits(), 2, "超大缓存应触发重新下载");
    let cached = std::fs::read(&second.path).expect("读取重写后的缓存");
    assert_eq!(cached, body, "缓存应被重写为服务端字节");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn oversized_cache_is_not_used_as_download_failure_fallback() {
    let body = payload("cache-fallback-small");
    let mock = Mock::new(body.clone());
    let dir = tmp_dir("cache-oversize-fallback");
    let client = reqwest::blocking::Client::new();
    let raw = mock.url("/oversize-fallback.jar");

    let wide = JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60));
    let first = wide.load(&raw, &site(), &client).expect("首次下载应成功");
    let mut huge = ZIP.to_vec();
    huge.resize(4096, 0x42);
    std::fs::write(&first.path, &huge).expect("写入超大缓存");

    mock.set_status(500);
    let narrow = JarStore::new(dir.join("store"))
        .with_ttl(Duration::ZERO)
        .with_max_bytes(1024);
    let err = narrow
        .load(&raw, &site(), &client)
        .expect_err("下载失败且缓存超限时必须报错");
    let text = err.to_string();
    assert!(
        text.contains("jar.fetch") || text.contains("下载 jar 失败"),
        "应归因到下载失败而不是静默使用超大缓存: {text}"
    );
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn cache_root_conflict_is_attributed_and_leaves_no_temp() {
    let body = payload("cache-root-conflict");
    let mock = Mock::new(body);
    let dir = tmp_dir("cache-root");
    let root = dir.join("store");
    std::fs::write(&root, b"not a directory").expect("占位文件");
    let store = JarStore::new(&root);
    let client = reqwest::blocking::Client::new();
    let err = store
        .load(&mock.url("/root-conflict.jar"), &site(), &client)
        .expect_err("缓存目录不可创建时必须失败");
    let text = err.to_string();
    assert!(
        text.contains("catvod.jar.fetch") && text.contains("创建 jar 缓存目录失败"),
        "应归因到缓存目录创建失败: {text}"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "part").unwrap_or(false))
        .collect();
    assert!(leftovers.is_empty(), "失败后不得残留 .part：{leftovers:?}");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn cache_rename_failure_cleans_temp_and_is_attributed() {
    let body = payload("cache-rename");
    let mock = Mock::new(body);
    let dir = tmp_dir("cache-rename");
    let root = dir.join("store");
    std::fs::create_dir_all(&root).expect("创建缓存根");
    let url = mock.url("/rename-conflict.jar");
    // 无 md5 的缓存键是 URL 的 md5；预先在目标路径放一个目录，rename 必失败。
    let cache = root.join(format!("{:x}.jar", md5::compute(url.as_bytes())));
    std::fs::create_dir_all(&cache).expect("目标路径占位目录");
    let store = JarStore::new(&root);
    let client = reqwest::blocking::Client::new();
    let err = store
        .load(&url, &site(), &client)
        .expect_err("目标被目录占用时 rename 必须失败");
    let text = err.to_string();
    assert!(
        text.contains("catvod.jar.fetch") && text.contains("重命名 jar 缓存失败"),
        "应归因到缓存重命名失败: {text}"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "rename 失败后不得残留 .part：{leftovers:?}");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn future_mtime_cache_is_treated_as_fresh_instead_of_missing() {
    let body = payload("future-mtime");
    let mock = Mock::new(body);
    let dir = tmp_dir("future-mtime");
    let store = JarStore::new(dir.join("store"));
    let client = reqwest::blocking::Client::new();
    let spec = store
        .load(&mock.url("/future-mtime.jar"), &site(), &client)
        .expect("首次下载");
    assert!(!spec.from_cache);
    let url = mock.url("/future-mtime.jar");
    let cache = dir
        .join("store")
        .join(format!("{:x}.jar", md5::compute(url.as_bytes())));
    let f = std::fs::File::options().write(true).open(&cache).expect("打开缓存");
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .expect("设置未来 mtime");
    drop(f);
    let second = store
        .load(&mock.url("/future-mtime.jar"), &site(), &client)
        .expect("未来 mtime 仍应命中缓存");
    assert!(second.from_cache, "未来 mtime 不得让缓存失效");
    assert_eq!(mock.hits(), 1, "命中缓存时不得再发第二次请求");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn clan_url_is_fetched_and_cached_like_http() {
    // clan:// 是 TVBox 的局域网文件服务约定：实现退化为 http://<rest>，
    // 但 md5 校验、缓存与二次命中的行为必须和普通 http jar 完全一致。
    let body = payload("clan-cache");
    let mock = Mock::new(body.clone());
    let dir = tmp_dir("clan-cache");
    let store = JarStore::new(dir.join("store")).with_ttl(Duration::from_secs(60));
    let client = reqwest::blocking::Client::new();
    let rest = mock
        .base
        .strip_prefix("http://")
        .expect("mock 基址应为 http");
    let raw = format!("clan://{rest}/clan.jar;md5;{}", md5_hex(&body));

    let first = store.load(&raw, &site(), &client).expect("clan 首次下载应成功");
    assert!(!first.from_cache, "首次加载不应来自缓存");
    assert_eq!(first.md5, md5_hex(&body));
    assert!(first.path.exists(), "clan 下载也应落缓存");

    let second = store.load(&raw, &site(), &client).expect("clan 二次加载应命中缓存");
    assert!(second.from_cache, "clan 二次加载必须来自缓存");
    assert_eq!(mock.hits(), 1, "clan 命中缓存时不得再发请求");
}

#[test]
#[ignore = "沙箱环境无法创建临时目录（os error 5 拒绝访问）；可写环境本机可跑"]
fn clan_url_respects_max_bytes_and_reports_original_spec() {
    let mut body = ZIP.to_vec();
    body.resize(2048, 0x5a);
    let mock = Mock::new(body);
    let dir = tmp_dir("clan-oversize");
    let store = JarStore::new(dir.join("store")).with_max_bytes(1024);
    let client = reqwest::blocking::Client::new();
    let rest = mock
        .base
        .strip_prefix("http://")
        .expect("mock 基址应为 http");
    let raw = format!("clan://{rest}/big.jar");
    let err = store
        .load(&raw, &site(), &client)
        .expect_err("clan 超大 jar 必须显式拒绝");
    let text = err.to_string();
    assert!(
        text.contains("catvod.jar.layout") && text.contains("上限"),
        "clan 路径应复用远程上限与归因: {text}"
    );
    assert!(
        text.contains("clan://"),
        "错误必须指回 clan:// 原文而不是退化后的 http 地址: {text}"
    );
    assert!(
        jar_files(&dir.join("store")).is_empty(),
        "超限的 clan jar 不得落缓存"
    );
}
