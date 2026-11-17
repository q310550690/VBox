// 应用菜单（占位）
//
// Tauri 2 用 `tauri::menu` 构建。框架阶段提供占位函数，Stage 1 起填充具体菜单项
// （如文件/打开订阅、编辑/偏好、帮助/关于）。

#[allow(dead_code)]
pub fn build_menu() -> String {
    // TODO(Stage 1): 用 tauri::menu::{Menu, MenuItem} 构建并返回 Menu
    "菜单将在 Stage 1 构建".into()
}
