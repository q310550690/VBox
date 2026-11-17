//! 本地缓存（SQLx + SQLite）
//!
//! 用途：缓存订阅配置、播放历史、源健康度，减少重复网络请求、提升冷启动速度。
//! Stage 3 落地。框架阶段只声明 Cache 结构与入口，不引入 sqlx 依赖，
//! 以免在未安装 SQLite 的环境下阻塞编译。

use crate::error::{CoreError, Result};

/// 缓存句柄（Stage 3 将包裹 sqlx::SqlitePool）
pub struct Cache {
    /// 数据库文件路径（占位，Stage 3 用于打开连接）
    #[allow(dead_code)]
    path: String,
}

impl Cache {
    /// 打开（或创建）缓存数据库。占位实现，仅记录路径。
    pub fn open(path: &str) -> Result<Self> {
        Ok(Self {
            path: path.to_string(),
        })
    }

    /// 写入一条播放历史。占位，返回未实现错误（Stage 3 接入 SQLite）。
    pub fn record_history(&self, _key: &str, _value: &str) -> Result<()> {
        Err(CoreError::Config("缓存写入尚未实现，见 Stage 3".into()))
    }
}
