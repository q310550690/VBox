// VBox 桌面端 Tauri 应用主体
mod capture;
mod commands;
mod commands_cloud;
mod commands_pan;
mod download;
mod embedded_tools;
mod enhance;
mod favorites;
mod ffmpeg_prefs;
mod history;
mod hls_proxy;
mod hls_util;
mod local_server;
mod logs;
#[cfg(target_os = "macos")]
mod macos_video;
mod menu;
mod pan_credentials;
mod playback_cache;
mod player;
mod player_prefs;
mod plugins;
mod progress;
mod search_prefs;
mod source_health;
mod subtitle_online;
mod vod_cache;
mod window_effect;

use tauri::Manager;

/// 调试构建下的无界面抓包自检：`VBOX_SNIFF_PROBE=<播放页 URL>` 启动后，
/// 创建一个隐藏 WebView 跑完整的抓包链路，把结果打到 stderr 然后退出。
///
/// 用途：在没有 UI 交互的环境（CI/远程/锁屏）验证嗅探链路，例如：
/// `VBOX_SNIFF_PROBE=https://play.hhuus.com/play/xxx target/debug/vbox-desktop.exe`
///
/// 仅 `debug_assertions` 生效，发布构建里不存在这段逻辑。
#[cfg(debug_assertions)]
fn maybe_run_sniff_probe(app: tauri::AppHandle) {
    let Ok(url) = std::env::var("VBOX_SNIFF_PROBE") else {
        return;
    };
    let timeout_ms = std::env::var("VBOX_SNIFF_PROBE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(25_000);
    eprintln!("[probe] start sniff: {url} (timeout {timeout_ms}ms)");
    tauri::async_runtime::spawn(async move {
        let sink = vbox_core::progress::ProgressSink::new(std::sync::Arc::new(
            |event: vbox_core::progress::ProgressEvent| {
                eprintln!("[probe] {} {} {}", event.phase, event.status, event.title);
            },
        ));
        let result =
            capture::sniff_async(&app, &url, Default::default(), None, timeout_ms, &sink).await;
        match result {
            Ok(play) => {
                eprintln!("[probe] OK url={}", play.url);
                eprintln!("[probe] headers={:?}", play.headers);
                eprintln!("[probe] trace={:#?}", play.trace);
            }
            Err(err) => eprintln!("[probe] FAIL {err}"),
        }
        app.exit(0);
    });
}

pub fn run() {
    let player_hub = player::PlayerHub::default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(player_hub)
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let _ = std::fs::create_dir_all(&data_dir);

            // macOS 视频输出（方案 2）：在 contentView 底部挂专用 CAMetalLayer 视图，
            // player_open 会把它作为 mpv 的 wid。失败只记日志：播放仍会退回旧的
            // ns_view 兜底，保证音频与播放状态可用，不会整条链路起不来。
            #[cfg(target_os = "macos")]
            if let Err(err) = macos_video::install(app.handle()) {
                logs::push("error", format!("macOS 视频视图创建失败：{err}"));
            }

            let state = commands::AppState::new(data_dir.clone());

            // 下载库先建：HLS 边下边播的缓存目录挂在下载缓存下（<下载目录>/cache/hls），
            // 这样用户清理下载缓存时能一并清掉。
            let document_dir = app.path().document_dir().ok();
            let download_store = download::DownloadStore::new(&data_dir, document_dir);

            // 播放缓存保留策略：<app_data_dir>/playback-cache.json（大小上限 / 保留天数，
            // 0 = 不限制）。缓存跨重启保留，启动/会话结束/设置变更时按策略清理。
            let playback_cache =
                std::sync::Arc::new(playback_cache::PlaybackCacheStore::new(&data_dir));

            // HLS 本地代理：真正边下边播（mpv 播 127.0.0.1，按需拉段 + 落盘复用）；
            // 缓存目录跨重启保留，由保留策略控制清理。
            let hls_proxy = std::sync::Arc::new(hls_proxy::HlsProxy::new(
                download_store.cache_dir().join("hls"),
                playback_cache.clone(),
            ));

            // 本地 /file、/proxy、/hls 服务（等价 TVBox 9978 的最小集合 + HLS 代理）
            match local_server::start(data_dir.join("clan"), hls_proxy.clone()) {
                Ok(server) => {
                    if let Ok(mut slot) = state.local_base.lock() {
                        *slot = Some(server.base_url.clone());
                    }
                    eprintln!(
                        "[vbox] local server: {} (clan root: {})",
                        server.base_url,
                        data_dir.join("clan").display()
                    );
                    app.manage(server);
                }
                Err(err) => {
                    eprintln!("[vbox] local server 启动失败（clan:// 与 HLS 代理将不可用）: {err}");
                }
            }

            app.manage(state);
            app.manage(hls_proxy);
            app.manage(playback_cache);

            // 观看历史：SQLite 库落在 <app_data_dir>/history.db。
            // 打开失败时 HistoryStore 内部降级为「不可用」，相关命令返回错误而不是 panic。
            app.manage(history::HistoryStore::new(&data_dir));

            // 收藏：SQLite 库落在 <app_data_dir>/favorites.db（按 site_key + vod_id 唯一）。
            app.manage(favorites::FavoriteStore::new(&data_dir));

            // 播放设置：<app_data_dir>/player.json（硬解 / 上次音量 / 默认倍速），
            // 缺失或损坏时回退默认值，不影响播放。
            app.manage(player_prefs::PlayerPrefsStore::new(&data_dir));

            // 插件：<app_data_dir>/plugins/<id>/plugin.json（GLSL 着色器 / lavfi 滤镜），
            // 启用状态与顺序存在 <app_data_dir>/plugins.json；目录缺失时自动创建。
            app.manage(plugins::PluginStore::new(&data_dir));

            // 内置 Anime4K 也以插件形式随包分发：启动时安装/校正到插件目录，
            // 之后和用户插件一样扫描、校验、启用、选预设。
            let _ = plugins::install_bundled_plugin(app.handle());

            // 搜索设置：<app_data_dir>/search.json（跨源搜索线程数），
            // 缺失或损坏时回退默认值（8 线程）。
            app.manage(search_prefs::SearchPrefsStore::new(&data_dir));

            // 源健康度：<app_data_dir>/source_health.db（站点/线路连续失败计数，
            // 供前端变灰、禁用与自动切源）。
            app.manage(source_health::SourceHealthStore::new(&data_dir));

            // 点播分类标签缓存：<app_data_dir>/vod-cache.json（按线路+站点分区，
            // 30 分钟内命中时跳过网络，首屏直接渲染旧分类）。
            app.manage(vod_cache::VodCacheStore::new(&data_dir));

            // 外部工具 ffmpeg：<app_data_dir>/ffmpeg.json（路径），启动时应用到全局解析。
            // 解析顺序：用户指定 → 已下载集成 → 随安装包集成 → VBOX_FFMPEG → PATH，
            // 所以发布包/一键集成后用户不需要自己安装 ffmpeg。
            let resource_dir = app.path().resource_dir().ok();
            app.manage(ffmpeg_prefs::FfmpegPrefsStore::new(&data_dir, resource_dir));

            // 随包 Python / Node 运行时（打包时可选内置）：注册给 vbox_core，
            // 让 .py spider / drpy 在「用户没装 Python/Node」时也能直接跑。
            embedded_tools::init(app.handle());

            // 下载：SQLite 库落在 <app_data_dir>/downloads.db；默认下载目录
            // = <系统文档目录>/VBox（没有文档目录时回退 <app_data_dir>/downloads）。
            // 后台工作线程在 setup 里就启动：重启后 queue 中未完成的任务会自动续传。
            app.manage(download_store.clone());
            download_store.start_worker(app.handle().clone());

            #[cfg(debug_assertions)]
            maybe_run_sniff_probe(app.handle().clone());

            // 调试用：启动即打开独立日志窗口（VBOX_LOG_WINDOW_PROBE=1）
            #[cfg(debug_assertions)]
            if std::env::var_os("VBOX_LOG_WINDOW_PROBE").is_some() {
                if let Err(err) = logs::open_log_window(app.handle().clone()) {
                    eprintln!("[probe] open log window failed: {err}");
                }
            }
            // 窗口毛玻璃（仅 Windows）：窗口本身 `transparent: true`，这里叠加原生 Acrylic，
            // 让自定义标题栏 / 侧栏透出桌面（类似微信 4.0 的亚克力）；内容区仍由 CSS 保持不透明。
            // 失败（旧系统 / 远程桌面）时静默回退，前端不会加 acrylic-on，外观与之前一致。
            let acrylic = app
                .get_webview_window("main")
                .map(|window| window_effect::apply(&window))
                .unwrap_or(false);
            app.manage(window_effect::WindowBackdrop { acrylic });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            window_effect::window_backdrop,
            commands::get_subscriptions,
            commands::get_enabled,
            commands::update_subscription,
            commands::export_subscriptions,
            commands::import_subscriptions,
            commands::subscription_prefs,
            commands::set_subscription_refresh,
            commands::get_loaded,
            commands::add_subscription,
            commands::remove_subscription,
            commands::get_addresses,
            commands::set_enabled,
            commands::restore_enabled,
            commands::reload_enabled,
            commands::load_config,
            commands::list_categories,
            commands::list_categories_cached,
            commands::list_videos,
            commands::get_detail,
            commands::search,
            commands::search_page,
            commands::local_media_url,
            commands::resolve,
            commands::list_parses,
            commands::set_default_parse,
            commands::cancel_parse,
            commands::python_status,
            commands::set_python_path,
            commands::refresh_python_status,
            commands::node_status,
            commands::set_node_path,
            commands::refresh_node_status,
            commands::network_log,
            commands::network_clear,
            logs::logs_recent,
            logs::logs_clear,
            logs::logs_export,
            logs::open_log_window,
            commands::douban_home,
            commands::douban_home_cached,
            commands::sniff,
            commands::live_sources,
            commands::live_groups,
            commands::live_epg,
            commands::play,
            commands::player_control,
            history::history_list,
            history::history_upsert,
            history::history_remove,
            history::history_clear,
            history::history_settings,
            history::history_set_settings,
            favorites::favorite_list,
            favorites::favorite_add,
            favorites::favorite_remove,
            favorites::favorite_check,
            download::download_list,
            download::download_start,
            download::download_control,
            download::download_remove,
            download::download_settings,
            download::download_set_dir,
            download::download_set_auto_save,
            download::download_set_network,
            download::download_set_auto_save_policy,
            download::download_pick_dir,
            download::download_open_dir,
            download::download_open_file_location,
            download::download_local_file,
            download::download_storage_status,
            download::download_cleanup_parts,
            download::download_cleanup_completed,
            player::player_open,
            player::player_close,
            player::player_save_current,
            player::player_save_pause,
            player::player_save_resume,
            player::player_arm_autosave,
            player::player_play,
            player::player_pause,
            player::player_toggle_pause,
            player::player_seek,
            player::player_seek_relative,
            player::player_set_volume,
            player::player_set_mute,
            player::player_set_rate,
            player::player_set_enhance,
            plugins::plugin_overview,
            plugins::plugin_rescan,
            plugins::plugin_set_enabled,
            plugins::plugin_set_order,
            plugins::plugin_set_preset,
            plugins::plugin_open_dir,
            plugins::plugin_open_url,
            player::player_set_audio_track,
            player::player_set_subtitle_track,
            player::player_add_subtitle,
            player::player_pick_subtitle,
            subtitle_online::subtitle_sources,
            subtitle_online::subtitle_search,
            subtitle_online::subtitle_download,
            player::player_set_rect,
            player::player_set_visible,
            player::player_state,
            player_prefs::player_prefs,
            player_prefs::player_set_prefs,
            playback_cache::playback_cache_prefs,
            playback_cache::playback_cache_set_prefs,
            playback_cache::playback_cache_status,
            playback_cache::playback_cache_clear,
            search_prefs::search_prefs,
            search_prefs::search_set_prefs,
            source_health::source_health_list,
            source_health::source_health_record,
            source_health::source_health_reset,
            source_health::source_health_reset_all,
            source_health::source_health_code_stats,
            source_health::source_health_recent_events,
            source_health::source_health_daily,
            ffmpeg_prefs::ffmpeg_status,
            ffmpeg_prefs::ffmpeg_set_path,
            ffmpeg_prefs::ffmpeg_provision,
            ffmpeg_prefs::ffmpeg_pick,
            ffmpeg_prefs::ffmpeg_open_download,
            // W13：网盘账号登录（协议登录 + 网页兜底 + 账号管理）
            commands_cloud::cloud_search,
            commands_cloud::cloud_hot_searches,
            commands_cloud::cloud_source_status,
            commands_cloud::cloud_source_set,
            commands_cloud::cloud_source_test,
            commands_cloud::cloud_sources_update,
            commands_pan::login_start,
            commands_pan::login_poll,
            commands_pan::login_cancel,
            commands_pan::login_manual,
            commands_pan::accounts_list,
            commands_pan::credentials_list,
            commands_pan::account_remove,
            commands_pan::credentials_clear,
            commands_pan::credentials_verify,
            commands_pan::login_providers,
            commands_pan::open_web_login,
        ])
        .build(tauri::generate_context!())
        .expect("启动 VBox 失败")
        .run(|app, event| {
            // 退出应用前记住用户最后一次调节的音量（下次启动恢复）。
            // 关闭播放页时 player_close 已经写回一次，这里覆盖「播放中直接退出应用」的情况。
            if matches!(event, tauri::RunEvent::Exit) {
                player::remember_volume(app);
            }
        });
}
