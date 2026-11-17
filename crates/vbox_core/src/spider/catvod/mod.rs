//! catvod 宿主契约层（type3 `csp_*.jar` 的桌面端替身）。
//!
//! Android 版 TVBox 用 `DexClassLoader` 加载 jar、反射调用 `com.github.catvod.spider.*`。
//! 桌面端不加载 dex、不执行 jar，只做三件事（计划 §3.1 / §5）：
//!
//! 1. **加载层**（`loader`）：把配置里的 `jar` 字段变成正确的本地字节
//!    （`;md5;` 校验、`img+` 伪装、TTL 缓存、失败回退旧缓存）；
//! 2. **契约层**（`contract` / `registry`）：`api` → `class` → 处置结论，
//!    已画像但未移植/受保护/脚本家族都返回**带分类码的错误**，不再谎称「需 Android Host」；
//! 3. **归一化层**（`normalize`）：把 jar spider 的 JSON 结果（`$$$`/`#`/`$` 分集、
//!    `parse`/`jx`/`header`）翻译成 VBox 强类型模型，供后续家族实现复用。
//!
//! 移植优先级（画像表 `profiles.json` 的 difficulty）：
//! `csp_Bili` L2 → `csp_Anime1`/`csp_Ysj`/`csp_PanSearch` L1 …
//! 规则引擎已覆盖 `csp_XBPQ*` / `csp_XYQHiker*`，不进入本模块的 jar 路径。

pub mod appto;
pub mod contract;
pub mod descriptor;
pub mod families;
pub mod loader;
pub mod normalize;
pub mod registry;
pub mod xpath_rule;
pub mod xbiu_rule;

pub use contract::{CatvodError, Difficulty, ErrorClass, JarSpec, Strategy};
pub use loader::{JarStore, LoadedJar};
pub use families::family_of;
pub use registry::{blocked_reason, classify, is_native_ready, site_error, Classification};

/// 站点是否已被原生路径覆盖（规则引擎或已移植家族）。
pub fn covers(site: &crate::model::Site) -> bool {
    is_native_ready(site.api.trim())
}

/// 一次性返回画像规模，便于 `/about` 或 doctor 输出展示。
pub fn profile_totals() -> &'static registry::Totals {
    &registry::profiles().totals
}
