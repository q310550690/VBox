//! Spider / drpy 规则执行子系统
//!
//! - `js`：type=3 `.js` 脚本（rquickjs）
//! - `drpy`：drpy 规则解析（历史占位，后续与 js 合并）
//! - `sandbox`：JS 沙箱封装

pub mod catvod;
pub mod drpy;
pub mod js;
pub mod sandbox;
