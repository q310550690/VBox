//! drpy 规则解析与执行
//!
//! drpy 是 TVBox 生态常见的爬虫规则格式（含 pdf/pdfh 等 DSL 与数十个内置函数）。
//! 实现需要 rquickjs 跑 JS，并在沙箱里提供受控 API。这是工作量最大的一块，见 Stage 7。
//! 当前仅声明入口函数签名，返回“未实现”占位。

use crate::error::{CoreError, Result};
use crate::model::Site;

/// 执行某条 drpy 规则。
///
/// - `site`：站点配置（其 api 字段通常是规则代码的下载地址或内联规则）
/// - `func`：要调用的规则函数名（如 "home"、"category"、"detail"、"play"）
/// - `input`：传入的参数（如分类 id、视频 id、播放 URL）
///
/// 返回规则执行后的字符串结果（通常是 JSON 或 HTML 片段）。
pub fn run_drpy_rule(_site: &Site, _func: &str, _input: &str) -> Result<String> {
    Err(CoreError::Config(
        "drpy 规则引擎尚未实现，见 Stage 7".into(),
    ))
}
