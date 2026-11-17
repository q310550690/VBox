// VBox 桌面端二进制入口
//
// 真正的应用逻辑放在 lib.rs 的 `run()`（crate 名为 vbox_desktop_lib），
// 这样便于单测与 Tauri 2 推荐的代码组织方式。
//
// release 构建切到 Windows 子系统：双击启动时不再附带一个 CMD 控制台窗口；
// debug 构建（tauri dev / cargo run）保留控制台，方便看后端日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    vbox_desktop_lib::run()
}
