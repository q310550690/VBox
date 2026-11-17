//! 窗口外观：Windows 原生亚克力 / 模糊背景
//!
//! 主窗口本身是 `transparent: true`（内嵌 libmpv 需要），这里在此基础上叠加
//! 系统级 Acrylic 毛玻璃，让自定义标题栏 / 侧栏能透出桌面（类似微信 4.0）。
//! 内容区仍由前端 CSS 保持不透明，只在 html.acrylic-on 时让外壳半透明。
//!
//! 失败（旧系统 / 远程桌面 / 非 Windows）时静默回退，前端查询 `window_backdrop`
//! 得到 false，保持原来的不透明外观。

/// 窗口原生毛玻璃是否成功应用（由 setup 在启动时写入）。
#[derive(Clone, Copy)]
pub struct WindowBackdrop {
    pub acrylic: bool,
}

/// 查询窗口毛玻璃是否生效（浏览器预览 / 非 Windows 恒为 false）。
#[tauri::command]
pub fn window_backdrop(state: tauri::State<'_, WindowBackdrop>) -> bool {
    state.acrylic
}

/// 尝试给窗口应用原生毛玻璃，返回是否成功。
pub fn apply(window: &tauri::WebviewWindow) -> bool {
    #[cfg(target_os = "windows")]
    {
        // 深色中性底，前端再按主题叠一层半透明主题色，浅色主题也不会发黑。
        let tint = Some((18, 18, 18, 100));
        if window_vibrancy::apply_acrylic(window, tint).is_ok() {
            eprintln!("[vbox] 窗口毛玻璃已启用：acrylic");
            return true;
        }
        // Win7 / 某些 Win10 版本没有 Acrylic，退回普通 blur。
        let blurred = window_vibrancy::apply_blur(window, tint).is_ok();
        if blurred {
            eprintln!("[vbox] 窗口毛玻璃已启用：blur（无 acrylic 支持）");
        } else {
            eprintln!("[vbox] 窗口毛玻璃不可用：保持不透明外壳");
        }
        blurred
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        false
    }
}
