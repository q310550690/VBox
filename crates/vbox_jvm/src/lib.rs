//! vbox_jvm —— jar 蜘蛛的 JVM 车道（P0：运行时定位 + 探针）。
//!
//! 设计要点（对应 `docs/type3兼容/type3-JVM兼容层实施计划.md`）：
//! - 真 JRE 17 在进程外运行，宿主 `SpiderHost` 与兼容层 `stubs.jar` 一起进 classpath；
//! - 本 crate 只暴露**阻塞** API（vbox_core 不带 async runtime）；
//! - P0 只实现 `--probe`：验证运行时装配（java 版本、class 版本、stubs/libs 可见性）；
//! - P1 再加 `--serve` 长驻进程、请求信封与五入口分派。

mod dex;
pub mod diag;
mod errors;
mod host;
mod pool;
pub mod shell;
pub mod telemetry;
pub mod shell_family;
mod proto;
mod runtime;

pub use dex::{
    active_conversions, class_total, convert_progress, ensure_loadable, extract_zip,
    needs_conversion, progress_begin, progress_finish, progress_line, sha256_file,
    ConvertProgress, DexOptions,
};
pub use diag::{extract_missing_class, missing_classes, note_missing_class, MissingClass};
pub use errors::{JvmError, Result};
pub use host::{Call, HostOptions, HostProcess, Reply};
pub use shell_family::{fingerprint as shell_fingerprint, ShellFingerprint};
pub use shell::{
    describe as describe_payload, install_payload, payload_root, resolve as resolve_payload,
    unsupported_hint, Payload,
};
pub use pool::{HostItem, Lease, Pool, PoolConfig, PoolKey, PoolSpawnError, PoolStats, Slot};
pub use proto::{Envelope, Probe};
pub use runtime::{java_major, read_java_version, JavaRuntime, Runtime, RuntimeManifest};

/// 与 `scripts/build-jvm.mjs` / `scripts/fetch-jre.ps1` 约定的产物名。
pub const STUBS_JAR: &str = "stubs.jar";
pub const HOST_JAR: &str = "spider-host.jar";
pub const LIBS_DIR: &str = "libs";
pub const JRE_DIR: &str = "jre";
pub const TOOLS_JAR: &str = "jvm-tools.jar";
/// 纯 Java 影子 `DexNative`（加固壳解密出内层 dex 后的快路径，见 host.rs 的 VBOX_SHELL_SHIM）。
pub const SHELL_SHIM_JAR: &str = "shell-shim.jar";
pub const NATIVE_BRIDGE_JAR: &str = "native-bridge.jar";
pub const UNIDBG_DIR: &str = "unidbg";
pub const DEX2JAR_DIR: &str = "dex2jar";
pub const SHIMS_MANIFEST: &str = "manifest.json";
pub const JRE_MARKER: &str = "marker.json";
