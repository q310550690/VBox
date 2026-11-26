//! 弹幕：按播放地址 / 标题拉取 B 站弹幕并生成 ASS 字幕，交给 mpv 作为外挂字幕加载。
//!
//! 前端在播放开始 / 切集（且弹幕开关为开）时调用 `danmaku_fetch`，拿到本地 ASS 路径后
//! 复用 `player_add_subtitle` 进入 mpv。缓存目录与在线字幕同根（`<cache>/danmaku`），
//! 便于统一清理。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// 拉取弹幕并生成 ASS，返回本地文件路径（None = 当前视频没有找到弹幕）。
#[tauri::command]
pub async fn danmaku_fetch(
    app: AppHandle,
    url: Option<String>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    let dir = danmaku_cache_dir(&app);
    let page_url = url.unwrap_or_default();
    let title = title.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        let mut items =
            vbox_core::danmaku::fetch_bilibili(&page_url).map_err(|err| err.to_string())?;
        if items.is_empty() && !title.trim().is_empty() {
            items = vbox_core::danmaku::fetch_by_title(&title).map_err(|err| err.to_string())?;
        }
        if items.is_empty() {
            return Ok(None);
        }
        let ass = vbox_core::danmaku::to_ass(&items, 1920, 1080);
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let key = cache_key(&page_url, &title);
        let path = dir.join(format!("danmaku-{key}.ass"));
        std::fs::write(&path, ass).map_err(|err| err.to_string())?;
        Ok(Some(path.to_string_lossy().to_string()))
    })
    .await
    .map_err(|err| format!("弹幕任务失败: {err}"))?
}

/// 弹幕缓存目录：`<下载目录>/cache/danmaku`（与在线字幕同根）。
fn danmaku_cache_dir(app: &AppHandle) -> PathBuf {
    if let Some(store) = app.try_state::<crate::download::DownloadStore>() {
        return store.cache_dir().join("danmaku");
    }
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("danmaku"))
        .unwrap_or_else(|_| std::env::temp_dir().join("vbox-danmaku"))
}

/// 用「播放地址 + 标题」生成稳定的缓存文件名（同一集重复播放复用同一份 ASS）。
fn cache_key(url: &str, title: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    title.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
