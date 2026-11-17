//! libmpv 冒烟测试：验证 DLL 与基础 API 可用（不需要窗口）。
//!
//! - 第一个/第二个测试只依赖 libmpv-2.dll，不需要网络与窗口；
//! - `libmpv_can_play_network_hls` 需要设置环境变量 `VBOX_TEST_PLAY_URL`（m3u8/mp4 直链），
//!   用 `vo=null` / `ao=null` 无窗口播放，验证 HLS 解析与网络拉流。
//!   DLL 找不到时所有测试打印 skip 直接通过（方便无播放器环境跑测试）。

use std::time::{Duration, Instant};

use vbox_player::ffi::{MpvApi, MPV_EVENT_END_FILE};

fn api_or_skip() -> Option<MpvApi> {
    match MpvApi::load_default() {
        Ok(api) => Some(api),
        Err(err) => {
            eprintln!("skip: {err}");
            None
        }
    }
}

#[test]
fn libmpv_can_load_and_initialize() {    let Some(api) = api_or_skip() else { return };
    let handle = unsafe { (api.create)() };
    assert!(!handle.is_null(), "mpv_create 返回空句柄");
    let rc = unsafe { (api.initialize)(handle) };
    let message = api.error_text(rc);
    unsafe { (api.terminate_destroy)(handle) };
    assert!(rc >= 0, "mpv_initialize 失败: {message}");
}

#[test]
fn libmpv_reports_version() {
    let Some(api) = api_or_skip() else { return };
    let handle = unsafe { (api.create)() };
    assert!(!handle.is_null());
    let rc = unsafe { (api.initialize)(handle) };
    assert!(rc >= 0, "mpv_initialize 失败: {}", api.error_text(rc));
    let version = api.get_string(handle, "mpv-version");
    eprintln!("mpv-version: {version:?}");
    unsafe { (api.terminate_destroy)(handle) };
    assert!(
        version.as_deref().unwrap_or("").contains("mpv"),
        "读不到 mpv-version: {version:?}"
    );
}

/// 无窗口播放网络流：验证 libmpv 能加载 m3u8/mp4、能拿到 duration。
#[test]
fn libmpv_can_play_network_hls() {
    let Ok(url) = std::env::var("VBOX_TEST_PLAY_URL") else {
        eprintln!("skip: 设置 VBOX_TEST_PLAY_URL 才会跑网络播放测试");
        return;
    };
    let Some(api) = api_or_skip() else { return };
    let handle = unsafe { (api.create)() };
    assert!(!handle.is_null());
    let log_path = std::env::temp_dir().join("vbox-mpv-test.log");
    let _ = std::fs::remove_file(&log_path);
    for (name, value) in [
        ("log-file", log_path.to_str().unwrap_or("vbox-mpv-test.log")),
        ("log-level", "debug"),
        ("tls-verify", "no"),
        ("ytdl", "no"),
        ("vo", "null"),
        ("ao", "null"),
        ("idle", "yes"),
        ("terminal", "no"),
        ("config", "no"),
    ] {
        let _ = api.set_option(handle, name, value);
    }
    let rc = unsafe { (api.initialize)(handle) };
    assert!(rc >= 0, "mpv_initialize 失败: {}", api.error_text(rc));

    api.command(handle, &["loadfile", &url, "replace"])
        .expect("loadfile 失败");

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut duration = 0.0f64;
    let mut loaded = false;
    while Instant::now() < deadline {
        let event = unsafe { (api.wait_event)(handle, 0.5) };
        if !event.is_null() {
            let event = unsafe { &*event };
            if event.event_id == MPV_EVENT_END_FILE {
                panic!(
                    "播放提前结束（error={}）：{}",
                    event.error,
                    api.error_text(event.error)
                );
            }
        }
        loaded = api.get_flag(handle, "file-loaded").unwrap_or(false);
        duration = api.get_double(handle, "duration").unwrap_or(0.0);
        let position = api.get_double(handle, "time-pos").unwrap_or(0.0);
        if duration > 0.0 && position > 0.5 {
            break;
        }
    }
    let position = api.get_double(handle, "time-pos").unwrap_or(0.0);
    eprintln!("loaded={loaded} duration={duration:.2} time-pos={position:.2}");
    unsafe { (api.terminate_destroy)(handle) };
    let _ = loaded;
    assert!(duration > 0.0, "拿不到 duration（流可能非法）");
}

/// 生成 30 秒 8kHz / 16bit / 单声道静音 WAV（当本地媒体用，不依赖网络与编解码器）。
fn silent_wav() -> Vec<u8> {
    let sample_rate: u32 = 8_000;
    let data_len: u32 = sample_rate * 2 * 30; // 16bit mono × 30s
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(out.len() + data_len as usize, 0);
    out
}

/// 外挂字幕：媒体加载后 `sub-add <file> select` 应产生一条 `external=yes` 的字幕轨。
///
/// 用本地静音 WAV 当媒体（vo=null / ao=null 无窗口），不依赖网络；DLL 缺失时跳过。
#[test]
fn libmpv_can_add_external_subtitle() {
    let Some(api) = api_or_skip() else { return };
    let dir = std::env::temp_dir().join(format!("vbox-sub-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let media = dir.join("media.wav");
    let subtitle = dir.join("sample.srt");
    let log_path = dir.join("mpv.log");
    std::fs::write(&media, silent_wav()).expect("写入 WAV 失败");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:01,000\nhello subtitle\n",
    )
    .expect("写入字幕失败");

    let handle = unsafe { (api.create)() };
    assert!(!handle.is_null(), "mpv_create 返回空句柄");
    for (name, value) in [
        ("vo", "null"),
        ("ao", "null"),
        ("idle", "yes"),
        ("terminal", "no"),
        ("config", "no"),
        ("sub-auto", "no"),
        ("log-file", log_path.to_str().unwrap_or("vbox-sub-smoke.log")),
        ("log-level", "debug"),
    ] {
        let _ = api.set_option(handle, name, value);
    }
    let rc = unsafe { (api.initialize)(handle) };
    assert!(rc >= 0, "mpv_initialize 失败: {}", api.error_text(rc));

    api.command(handle, &["loadfile", &media.to_string_lossy(), "replace"])
        .expect("loadfile 失败");
    let loaded_deadline = Instant::now() + Duration::from_secs(10);
    let mut started = false;
    while Instant::now() < loaded_deadline {
        let event = unsafe { (api.wait_event)(handle, 0.2) };
        if !event.is_null() {
            let event = unsafe { &*event };
            if event.event_id == MPV_EVENT_END_FILE {
                eprintln!(
                    "媒体提前结束（reason/error={}）：{}",
                    event.error,
                    api.error_text(event.error)
                );
                break;
            }
        }
        // 用 time-pos/duration 判断已起播（`file-loaded` 属性在本机 libmpv 上读不到）
        let position = api.get_double(handle, "time-pos").unwrap_or(0.0);
        let duration = api.get_double(handle, "duration").unwrap_or(0.0);
        if position > 0.05 || duration > 0.0 {
            started = true;
            break;
        }
    }
    if !started {
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            eprintln!("--- mpv log tail ---");
            for line in log.lines().rev().take(40).collect::<Vec<_>>().iter().rev() {
                eprintln!("{line}");
            }
        }
        unsafe { (api.terminate_destroy)(handle) };
        panic!("本地 WAV 未在 10s 内起播（libmpv 可能缺少 demuxer）");
    }

    api.command(
        handle,
        &["sub-add", &subtitle.to_string_lossy(), "select"],
    )
    .expect("sub-add 失败");

    let mut external_sub = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let count = api.get_i64(handle, "track-list/count").unwrap_or(0);
        for index in 0..count {
            let prefix = format!("track-list/{index}/");
            let kind = api
                .get_string(handle, &format!("{prefix}type"))
                .unwrap_or_default();
            let external = api
                .get_flag(handle, &format!("{prefix}external"))
                .unwrap_or(false);
            if kind == "sub" && external {
                external_sub = true;
                let title = api
                    .get_string(handle, &format!("{prefix}title"))
                    .unwrap_or_default();
                eprintln!("external subtitle track: id={index} title={title}");
            }
        }
        if external_sub {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    unsafe { (api.terminate_destroy)(handle) };
    if !external_sub {
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            let tail: Vec<_> = log.lines().rev().take(40).collect();
            eprintln!("--- mpv log tail ---");
            for line in tail.iter().rev() {
                eprintln!("{line}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(external_sub, "sub-add 后未在 track-list 里看到外挂字幕轨");
}
