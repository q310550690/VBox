//! macOS 视频输出层（方案 2）：专用的 layer-backed `NSView` + `CAMetalLayer`。
//!
//! 背景：直接把窗口 `contentView` 的指针交给 libmpv 当 `wid` 时，mpv 的 cocoa VO
//! 不会自己创建/挂载 Metal layer，实测表现为「音频、播放状态正常，但窗口里没有画面」。
//! 这里在 `contentView` 底部挂一个专用 `NSView`（`wantsLayer = true`，backing layer
//! 用 `CAMetalLayer`），再把它的指针作为 `wid` 交给 mpv；mpv 把视频画在这层上，
//! 透明的 WKWebView 通过 `clip-path` 挖洞把它露出来。
//!
//! 视频在窗口内的位置仍由现有的 `video-margin-ratio-*` 控制（见 `player::apply_margins`），
//! 所以这个 view 直接铺满 `contentView` 即可，autoresizing 会跟随窗口 / 全屏变化，
//! 不需要每帧同步原生 frame，也避免了 point / pixel 与 DPI 的换算误差。
//!
//! 线程约束：AppKit 必须在主线程操作，本模块的 `install` 只应在 tauri `setup`
//! （主线程）里调用。

use std::sync::OnceLock;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSWindowOrderingMode};
use objc2_quartz_core::CAMetalLayer;
use tauri::{AppHandle, Manager};

/// 专用视频视图指针（`NSView*`）。
///
/// `contentView` 会持有 subview，进程生命周期内指针有效；这里只存地址供 `wid` 使用。
static VIDEO_VIEW: OnceLock<usize> = OnceLock::new();

/// 在 setup（主线程）里创建并挂载视频视图，返回 mpv `wid` 要用的指针。
pub fn install(app: &AppHandle) -> Result<usize, String> {
    if let Some(ptr) = VIDEO_VIEW.get() {
        return Ok(*ptr);
    }

    let mtm =
        MainThreadMarker::new().ok_or_else(|| "创建 macOS 视频视图必须在主线程执行".to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    let content_ptr = window
        .ns_view()
        .map_err(|e| format!("读取主窗口 NSView 失败: {e}"))?;
    if content_ptr.is_null() {
        return Err("主窗口 NSView 为空".to_string());
    }

    // SAFETY: `ns_view()` 返回窗口的 contentView；setup 期间窗口存活，且当前在主线程。
    let content: &NSView = unsafe { &*(content_ptr as *const NSView) };
    let frame = content.bounds();
    let scale = content
        .window()
        .map(|window| window.backingScaleFactor())
        .unwrap_or(1.0);

    // 主线程分配 NSView / CAMetalLayer，并把 subview 挂到 contentView 底部。
    let view = NSView::initWithFrame(mtm.alloc::<NSView>(), frame);
    let layer = CAMetalLayer::layer();
    layer.setContentsScale(scale);
    // 不要设 opaque：窗口是透明的，WebView 只靠 clip-path 挖一个洞；视频层铺满
    // contentView 时，洞外的区域必须继续透出下层（毛玻璃/桌面），否则整窗被黑底盖住。
    layer.setOpaque(false);
    view.setWantsLayer(true);
    view.setLayer(Some(&layer));
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // relativeTo = None + Below：放到 subview 列表最底部，位于 WKWebView 之下。
    content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, None);

    let ptr = Retained::as_ptr(&view) as usize;
    let _ = VIDEO_VIEW.set(ptr);
    crate::logs::push(
        "info",
        format!(
            "macOS 视频视图已创建：NSView=0x{ptr:x}，frame={}x{} @({}, {})，scale={scale}，CAMetalLayer 已挂载",
            frame.size.width, frame.size.height, frame.origin.x, frame.origin.y
        ),
    );
    Ok(ptr)
}

/// 已创建的视频视图指针；未创建时返回 `None`。
pub fn view_ptr() -> Option<usize> {
    VIDEO_VIEW.get().copied()
}
