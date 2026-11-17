//! 配置协议集成测试：用本地 tiny_http mock 验证
//! - 明文 / ;pk; AES-ECB / 2423 AES-CBC / Base64 标记
//! - 线路合集（urls-only）自动跟随第一条线路
//! - 相对路径修正
//! - 配置缓存命中
//! - 循环引用保护

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;

use aes::Aes128;
use base64::Engine;
use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit, KeyIvInit};
use tiny_http::{Header, Response, Server};

use vbox_core::config::{inspect_config, load_config_with_options, ConfigKind, LoadOptions};

fn right_pad(s: &str, n: usize) -> String {
    let mut out = s.trim().to_string();
    while out.chars().count() < n {
        out.push('0');
    }
    out
}

fn pk_ecb_hex(plain: &str, key: &str) -> String {
    let key = right_pad(key, 16);
    let cipher = ecb::Encryptor::<Aes128>::new_from_slice(key.as_bytes())
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    hex::encode(cipher)
}

fn cbc_2423(plain: &str, key: &str, iv13: &str) -> String {
    let key16 = right_pad(key, 16);
    let iv16 = right_pad(iv13, 16);
    let cipher = cbc::Encryptor::<Aes128>::new_from_slices(key16.as_bytes(), iv16.as_bytes())
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
    format!(
        "{}{}{}",
        hex::encode(format!("$#{key16}#$").as_bytes()),
        hex::encode(cipher),
        hex::encode(iv13.as_bytes())
    )
}

fn start_server() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let base2 = base.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            seen2.lock().unwrap().push(url.clone());
            let body = if url.starts_with("/plain.json") {
                r#"{"sites":[{"key":"a","name":"A","type":1,"api":"http://x"}],"parses":[]}"#
                    .to_string()
            } else if url.starts_with("/child.json") {
                r#"{"sites":[{"key":"child","name":"Child","type":1,"api":"http://x"}],"parses":[{"name":"p","type":1,"url":"http://x/"}]}"#.to_string()
            } else if url.starts_with("/lines.json") {
                format!(
                    r#"{{"urls":[{{"name":"A","url":"{base2}/child.json"}},{{"name":"B","url":"{base2}/plain.json"}}]}}"#
                )
            } else if url.starts_with("/rel.json") {
                r#"{"sites":[{"key":"r","name":"R","type":1,"api":"http://x","ext":"./img/cover.png"}]}"#.to_string()
            } else if url.starts_with("/loop-a.json") {
                format!(r#"{{"urls":[{{"name":"B","url":"{base2}/loop-b.json"}}]}}"#)
            } else if url.starts_with("/loop-b.json") {
                format!(r#"{{"urls":[{{"name":"A","url":"{base2}/loop-a.json"}}]}}"#)
            } else if url.starts_with("/pk.json") {
                pk_ecb_hex(
                    r#"{"sites":[{"key":"pk","name":"PK","type":1,"api":"http://x"}]}"#,
                    "secret",
                )
            } else if url.starts_with("/cbc.json") {
                cbc_2423(
                    r#"{"sites":[{"key":"cbc","name":"CBC","type":1,"api":"http://x"}]}"#,
                    "0123456789abcdef",
                    "abcdefghijklm",
                )
            } else if url.starts_with("/b64.json") {
                format!(
                    "junk AbCd1234**{}",
                    base64::engine::general_purpose::STANDARD.encode(
                        r#"{"sites":[{"key":"b64","name":"B64","type":1,"api":"http://x"}]}"#
                    )
                )
            } else if url.starts_with("/big.png") {
                // 不是配置：用于验证错误
                "not-json".to_string()
            } else {
                "404".to_string()
            };
            let header =
                Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..])
                    .unwrap();
            let _ = req.respond(Response::from_string(body).with_header(header));
        }
    });
    (base, seen)
}

fn opts(cache: &std::path::Path) -> LoadOptions {
    LoadOptions {
        use_cache: true,
        cache_dir: Some(cache.to_path_buf()),
        cache_ttl_secs: 3600,
        local_base: None,
        force_refresh: false,
        max_depth: 3,
        preferred_line: None,
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vbox-config-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn plain_and_cache() {
    let (base, _seen) = start_server();
    let dir = temp_dir("plain");
    let url = format!("{base}/plain.json");
    let first = load_config_with_options(&url, &opts(&dir)).unwrap();
    assert_eq!(first.kind, ConfigKind::Single);
    assert_eq!(first.config.sites[0].key, "a");
    assert!(!first.cache_hit);
    let second = load_config_with_options(&url, &opts(&dir)).unwrap();
    assert!(second.cache_hit, "第二次应命中配置缓存");
}

#[test]
fn line_collection_follows_first_line() {
    let (base, _seen) = start_server();
    let dir = temp_dir("lines");
    let url = format!("{base}/lines.json");
    let loaded = load_config_with_options(&url, &opts(&dir)).unwrap();
    assert_eq!(loaded.kind, ConfigKind::Lines);
    assert_eq!(loaded.lines.len(), 2);
    assert!(loaded.active_line.ends_with("/child.json"));
    assert_eq!(loaded.config.sites[0].key, "child");
    assert_eq!(loaded.config.urls.len(), 2);
}

#[test]
fn line_collection_can_prefer_second_line() {
    let (base, _seen) = start_server();
    let dir = temp_dir("lines-pref");
    let url = format!("{base}/lines.json");
    let mut o = opts(&dir);
    o.preferred_line = Some(format!("{base}/plain.json"));
    let loaded = load_config_with_options(&url, &o).unwrap();
    assert_eq!(loaded.kind, ConfigKind::Lines);
    assert!(loaded.active_line.ends_with("/plain.json"));
    assert_eq!(loaded.config.sites[0].key, "a");
}

#[test]
fn inspect_does_not_follow_lines() {
    let (base, _seen) = start_server();
    let dir = temp_dir("inspect");
    let url = format!("{base}/lines.json");
    let ins = inspect_config(&url, &opts(&dir)).unwrap();
    assert_eq!(ins.kind, ConfigKind::Lines);
    assert_eq!(ins.lines.len(), 2);
}

#[test]
fn relative_paths_fixed() {
    let (base, _seen) = start_server();
    let dir = temp_dir("rel");
    let url = format!("{base}/rel.json");
    let loaded = load_config_with_options(&url, &opts(&dir)).unwrap();
    let ext = loaded.config.sites[0].ext.clone().unwrap();
    let text = ext.to_string();
    assert!(
        text.contains(&format!("{base}/img/cover.png")),
        "相对路径应基于配置 URL 修正，实际: {text}"
    );
}

#[test]
fn pk_ecb_decoded() {
    let (base, _seen) = start_server();
    let dir = temp_dir("pk");
    let url = format!("{base}/pk.json;pk;secret");
    let loaded = load_config_with_options(&url, &opts(&dir)).unwrap();
    assert_eq!(loaded.format, "aes-ecb-pk");
    assert_eq!(loaded.config.sites[0].key, "pk");
}

#[test]
fn aes_cbc_2423_decoded() {
    let (base, _seen) = start_server();
    let dir = temp_dir("cbc");
    let url = format!("{base}/cbc.json");
    let loaded = load_config_with_options(&url, &opts(&dir)).unwrap();
    assert_eq!(loaded.format, "aes-cbc-2423");
    assert_eq!(loaded.config.sites[0].key, "cbc");
}

#[test]
fn base64_marker_decoded() {
    let (base, _seen) = start_server();
    let dir = temp_dir("b64");
    let url = format!("{base}/b64.json");
    let loaded = load_config_with_options(&url, &opts(&dir)).unwrap();
    assert_eq!(loaded.format, "base64-marker");
    assert_eq!(loaded.config.sites[0].key, "b64");
}

#[test]
fn loop_detected() {
    let (base, _seen) = start_server();
    let dir = temp_dir("loop");
    let url = format!("{base}/loop-a.json");
    let err = load_config_with_options(&url, &opts(&dir)).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("循环") || msg.contains("深度"),
        "实际错误: {msg}"
    );
}

#[test]
fn invalid_config_errors() {
    let (base, _seen) = start_server();
    let dir = temp_dir("bad");
    let url = format!("{base}/big.png");
    assert!(load_config_with_options(&url, &opts(&dir)).is_err());
}

// 让测试编译器知道这些依赖在集成测试里确实被用到
#[allow(dead_code)]
fn _touch(_: HashMap<String, String>) {}
