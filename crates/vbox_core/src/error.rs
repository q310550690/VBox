//! 核心引擎统一错误类型
//!
//! 用 thiserror 收敛各类错误；在 Tauri 命令边界再转成 `String` 返回给前端。
//! 这样核心库保持错误类型化，前端只看到可读字符串。

use thiserror::Error;

/// 核心库统一 Result 别名
pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("HTTP 请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON 解析失败（位置: {path}）: {msg}")]
    Json { path: String, msg: String },

    #[error("配置无效: {0}")]
    Config(String),

    #[error("未找到站点 key={0}")]
    SiteNotFound(String),

    #[error("源站返回错误: code={code}, msg={msg}")]
    Api { code: i64, msg: String },

    #[error("未找到分类 id={0}")]
    CategoryNotFound(String),

    #[error("没有可用的播放直链（需 Stage 6 接入 parses 链）")]
    NoPlayUrl,

    #[error("文件系统错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 把 serde_path_to_error 的错误转成 CoreError::Json，带出错路径（方便定位脏 JSON 的哪一段）
impl From<serde_path_to_error::Error<serde_json::Error>> for CoreError {
    fn from(e: serde_path_to_error::Error<serde_json::Error>) -> Self {
        CoreError::Json {
            path: e.path().to_string(),
            msg: e.inner().to_string(),
        }
    }
}
