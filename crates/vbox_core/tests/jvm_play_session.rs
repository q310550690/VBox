//! P2 验收：播放会话把宿主钉住 → 播放器能读到字节 → 释放会话；连续切源不泄漏。
//!
//! 用 `StreamSpider` 夹具 + 测试进程内的本地 HTTP 服务，不依赖任何外网。
//! 跑法：`cargo test -p vbox_core --test jvm_play_session`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::jvm;

fn fixture_jar() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../jvm/build/fixtures/test-spider.jar")
}

/// 起一个只会「慢慢吐字节」的 HTTP 服务，返回端口。
fn spawn_stream_server() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("本地端口");
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming().take(64) {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let head = "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(head.as_bytes());
                // 分块慢慢写，模拟直播流：播放器读的时候宿主必须还活着。
                for _ in 0..4 {
                    let _ = stream.write_all(&[0u8; 512]);
                    let _ = stream.flush();
                    std::thread::sleep(Duration::from_millis(150));
                }
            });
        }
    });
    (port, handle)
}

fn site_for(port: u16) -> Site {
    let jar = fixture_jar();
    let mut site = Site::default();
    site.key = "play-fixture".into();
    site.name = "PlayFixture".into();
    site.api = "csp_StreamSpider".into();
    site.kind = 3;
    site.jar = format!("file:///{}", jar.display().to_string().replace('\\', "/"));
    site.ext = Some(json!(format!("{{\"base\":\"http://127.0.0.1:{port}\"}}")));
    site.timeout = 10;
    site
}

fn read_first_bytes(url: &str) -> std::io::Result<Vec<u8>> {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (addr, _path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(format!("GET /stream HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())?;
    let mut out = vec![0u8; 64];
    let n = stream.read(&mut out)?;
    out.truncate(n);
    Ok(out)
}

#[test]
fn play_session_pins_host_until_released() {
    if !jvm::available() || !fixture_jar().is_file() {
        eprintln!("跳过：缺少 JRE 或夹具 jar（先跑 node scripts/build-jvm.mjs --fixtures）");
        return;
    }
    let (port, _server) = spawn_stream_server();
    let cfg = TvConfig::default();
    let site = site_for(port);

    let play = jvm::player_content(&cfg, &site, "m3u8", "live", &[]).expect("playerContent 应当成功");
    let url = play.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    assert!(url.contains(&format!("127.0.0.1:{port}")), "播放地址应指向测试服务：{url}");
    assert_eq!(jvm::session_count(), 1, "播放后应挂一个会话，把宿主租住");
    let pp = jvm::session_proxy_port(&site, "live");
    eprintln!("[play] url={url} pp={pp:?} stats={:?}", jvm::stats());

    // 宿主还活着时才能读到流；读到 HTTP 头就说明取流链路通。
    let head = read_first_bytes(&url).expect("读取播放地址");
    assert!(
        String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"),
        "应读到 200 响应头：{:?}",
        String::from_utf8_lossy(&head)
    );

    assert!(jvm::release_session(&site, "live"), "释放会话应成功");
    assert_eq!(jvm::session_count(), 0, "释放后不应有会话");

    // 连续切源 20 次：会话不累积、宿主数不超上限。
    for i in 0..20 {
        let play = jvm::player_content(&cfg, &site, "m3u8", "live", &[]).expect("重复 playerContent");
        assert!(play.get("url").and_then(|v| v.as_str()).is_some());
        assert_eq!(jvm::session_count(), 1, "第 {i} 次切源后应恰好一个会话");
        assert!(jvm::stats().hosts <= jvm::stats().max_hosts, "宿主数不得超过上限");
        assert!(jvm::release_session(&site, "live"), "第 {i} 次释放");
        assert_eq!(jvm::session_count(), 0, "第 {i} 次释放后会话应清零");
    }

    jvm::shutdown_all();
    assert_eq!(jvm::stats().hosts, 0, "shutdown_all 后不应残留宿主");
}

/// P2 验收加强版：**播放 30 秒不断流**（`#[ignore]`，实测约 30 s）。
///
/// 服务端每 250 ms 写 1 KB，共写 30 s；客户端持续读 30 s 并检查「没有超过 8 s 的静默」。
/// 跑法：`cargo test -p vbox_core --test jvm_play_session -- --ignored --nocapture play_stream_survives_30s`
#[test]
#[ignore = "30 秒真实流，按需用 --ignored 跑"]
fn play_stream_survives_30s() {
    if !jvm::available() || !fixture_jar().is_file() {
        eprintln!("跳过：缺少 JRE 或夹具 jar");
        return;
    }
    const TOTAL: Duration = Duration::from_secs(30);
    let listener = TcpListener::bind("127.0.0.1:0").expect("本地端口");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let head = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(head.as_bytes());
                let deadline = std::time::Instant::now() + TOTAL;
                while std::time::Instant::now() < deadline {
                    if stream.write_all(&[7u8; 1024]).and_then(|_| stream.flush()).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        }
    });

    let site = site_for(port);
    let cfg = TvConfig::default();
    let play = jvm::player_content(&cfg, &site, "m3u8", "live", &[]).expect("playerContent");
    let url = play.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    assert_eq!(jvm::session_count(), 1, "播放开始应挂会话");

    let rest = url.strip_prefix("http://").unwrap_or(&url);
    let (addr, _path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut stream = TcpStream::connect(addr).expect("连接取流地址");
    stream.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    stream
        .write_all(format!("GET /stream HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
        .unwrap();

    let start = std::time::Instant::now();
    let mut total = 0usize;
    let mut buf = [0u8; 4096];
    let mut zero_reads = 0usize;
    while start.elapsed() < TOTAL {
        match stream.read(&mut buf) {
            Ok(0) => {
                zero_reads += 1;
                if zero_reads > 3 {
                    break;
                }
            }
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                panic!("流中断：{e}");
            }
            Err(e) => panic!("读流失败：{e}"),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let elapsed = start.elapsed();
    eprintln!("[play30s] 读取 {total} B / {elapsed:?} / sessions={}", jvm::session_count());
    assert!(elapsed >= Duration::from_secs(25), "应持续读满 30 s，实际 {elapsed:?}");
    assert!(total >= 24 * 1024, "30 s 应至少读到 24 KB，实际 {total} B");
    assert_eq!(jvm::session_count(), 1, "流期间会话应一直挂住宿主");

    assert!(jvm::release_session(&site, "live"), "释放会话");
    assert_eq!(jvm::session_count(), 0);
    jvm::shutdown_all();
    assert_eq!(jvm::stats().hosts, 0);
}
