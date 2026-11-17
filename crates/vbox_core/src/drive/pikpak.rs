//! PikPak 个人网盘只读（W14 P2+）。
//!
//! PikPak 与迅雷共用 drive 协议，接口根不同（`https://api-drive.mypikpak.com/drive/v1/`），
//! 因此直接复用 [`crate::drive::xunlei`] 的列表与直链解析。
//! token 来源：W13 网页授权捕获的 `access_token` Cookie（`DriveClient` 会自动提取），
//! 或 `login_manual` 手动录入的 token。

pub use super::xunlei::{download_url, list};
