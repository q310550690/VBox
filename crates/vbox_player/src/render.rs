//! 渲染目标抽象
//!
//! libmpv 需要把一个窗口（HWND / NSView / X11 Window）作为渲染表面。
//! 这里抽象成跨平台句柄，Stage 8 传给 libmpv 的 `wid` 参数。
//! 降级方案：独立无边框 mpv 窗口（方案第 2 节架构图提及）。

/// 跨平台渲染目标
#[derive(Debug, Clone)]
pub struct RenderTarget {
    /// 系统窗口句柄（Windows=HWND, macOS=NSView*, Linux=X11 Window id）
    /// 用 usize 承载，跨平台统一；0 表示“降级为独立 mpv 窗口”
    pub handle: usize,
    /// 宽度（像素），用于初始尺寸
    pub width: u32,
    /// 高度（像素）
    pub height: u32,
}

impl RenderTarget {
    /// 绑定到指定窗口句柄
    pub fn from_handle(handle: usize, width: u32, height: u32) -> Self {
        Self {
            handle,
            width,
            height,
        }
    }

    /// 降级：不绑定父窗口，由 mpv 自建独立窗口
    pub fn standalone(width: u32, height: u32) -> Self {
        Self {
            handle: 0,
            width,
            height,
        }
    }
}
