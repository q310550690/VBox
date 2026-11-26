//! catvod 宿主契约层（type3 `csp_*.jar` 的桌面端替身）。
//!
//! Android 版 TVBox 用 `DexClassLoader` 加载 jar、反射调用 `com.github.catvod.spider.*`。
//! 桌面端按以下顺序处理 type3 站点：
//!
//! 1. **加载层**（`loader`）：把配置里的 `jar` 字段变成正确的本地字节
//!    （`;md5;` 校验、`img+` 伪装、TTL 缓存、失败回退旧缓存）；
//! 2. **JVM 车道**（`jvm`）：真 JRE + 兼容层直接跑原始 jar，是带 jar 站点的主力路径；
//! 3. **描述层**（`descriptor`）：T1–T4 抽取的结构化描述，覆盖部分 jar 站的规则化重写；
//! 4. **契约层**（`contract`）：错误分类与 `jar` 字段解析，所有失败路径必须可读、可分类；
//! 5. **归一化层**（`normalize`）：把 jar spider 的 JSON 结果（`$$$`/`#`/`$` 分集、
//!    `parse`/`jx`/`header`）翻译成 VBox 强类型模型。
//!
//! 规则引擎覆盖 `csp_XBPQ*` / `csp_XYQHiker*`，不进入本模块的 jar 路径。

pub mod appto;
pub mod contract;
pub mod descriptor;
pub mod jvm;
pub mod loader;
pub mod normalize;
pub mod xpath_rule;
pub mod xbiu_rule;

pub use contract::{CatvodError, ErrorClass, JarSpec};
pub use loader::{JarStore, LoadedJar};

/// 所有车道（JVM / 描述 / 规则 / 脚本）都未命中时的兜底错误。
///
/// 调用点集中在 `sources::spider` 五个分派函数的末尾；返回带稳定分类码的
/// 配置错误，前端据此展示可读原因，而不是笼统的「不支持」。
pub fn site_error(site: &crate::model::Site) -> crate::error::CoreError {
    CatvodError::new(
        ErrorClass::UnknownApi,
        site,
        "该站点未命中 JVM 兼容层、描述、规则或脚本任一车道",
    )
    .into_core()
}
