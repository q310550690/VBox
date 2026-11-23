use std::path::PathBuf;

/// JVM 车道的错误。错误码与计划附录 B 对应，便于上报与判据统计。
#[derive(Debug, thiserror::Error)]
pub enum JvmError {
    #[error("JVM 运行时缺失：{path}")]
    MissingRuntime { path: PathBuf },

    #[error("JVM 运行时清单损坏：{path}（{reason}）")]
    BadManifest { path: PathBuf, reason: String },

    #[error("spawn 失败：{0}")]
    Io(#[from] std::io::Error),

    #[error("宿主在 {ms} ms 内没有给出可解析的信封（stderr 末尾：{stderr}）")]
    Timeout { ms: u64, stderr: String },

    #[error("宿主以 {code} 退出（stderr 末尾：{stderr}）")]
    Exit { code: i32, stderr: String },

    #[error("协议错误：{line}")]
    Protocol { line: String },

    #[error("蜘蛛失败（{code}）：{detail}")]
    Spider {
        code: String,
        detail: String,
    },

    #[error("dex 转 jar 失败：{detail}")]
    DexConvert { detail: String },

    #[error("JSON 解析失败：{0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, JvmError>;
