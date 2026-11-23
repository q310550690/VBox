//! 加固壳载荷索引（P4）。
//!
//! 守卫 jar 在 `init` 阶段用 ARM native 解密出自己的真实实现，x86 JVM 跑不了那一步。
//! 这里不模拟 ARM，而是定义**载荷供给协议**：谁（离线工具、厂商包、将来的 ARM 桥）
//! 先把解密结果放到 `<cache>/payloads/<jar_sha256>/` 里，宿主就按索引把它排进 classpath。
//!
//! 索引文件 `index.json`：
//! ```json
//! {
//!   "jar": "<源 jar 的 sha256>",
//!   "classes": { "com.foo.Bar": ["libs/real.jar", "classes/"] },
//!   "any": ["libs/real.jar"]
//! }
//! ```
//! - `classes` 精确命中目标类 → 用它；
//! - `any` 是「不知道类名时也可能可用」的兜底（预热场景），只在没有精确项时使用；
//! - 目录项按给定顺序排在最前（后加载的覆盖前面的同名类，与 TVBox 的 shim 语义一致）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::{JvmError, Result};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawIndex {
    #[serde(default)]
    jar: String,
    #[serde(default)]
    classes: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    any: Vec<String>,
}

/// 某个 (jar, class) 解析出的载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub jar_sha: String,
    /// 要排到 classpath 最前的目录/jar（已转成绝对路径）。
    pub paths: Vec<PathBuf>,
}

/// 载荷根目录：`<dex 缓存>/payloads`。
pub fn payload_root(cache_dir: &Path) -> PathBuf {
    cache_dir.join("payloads")
}

/// 读取索引；索引不存在或坏掉都返回 `None`（加固壳只是「暂不可用」，不是运行时报错）。
pub(crate) fn read_index(cache_dir: &Path, jar_sha: &str) -> Option<RawIndex> {
    let file = payload_root(cache_dir).join(jar_sha).join("index.json");
    let text = std::fs::read_to_string(&file).ok()?;
    serde_json::from_str(&text).ok()
}

/// 按 (jar_sha, class) 解析载荷；`class` 为 `None` 时只看 `any`（预热）。
pub fn resolve(cache_dir: &Path, jar_sha: &str, class: Option<&str>) -> Option<Payload> {
    let idx = read_index(cache_dir, jar_sha)?;
    // 索引里写的 jar 与调用方不一致时不采信（防止把 A 的载荷挂到 B 上）。
    if !idx.jar.is_empty() && idx.jar != jar_sha {
        return None;
    }
    let base = payload_root(cache_dir).join(jar_sha);
    let mut rels: Vec<String> = Vec::new();
    if let Some(cls) = class {
        if let Some(list) = idx.classes.get(cls) {
            rels.extend(list.iter().cloned());
        }
    }
    if rels.is_empty() {
        rels.extend(idx.any.iter().cloned());
    }
    if rels.is_empty() {
        return None;
    }
    let mut paths = Vec::new();
    for rel in rels {
        let p = base.join(&rel);
        if p.exists() {
            paths.push(p);
        }
    }
    if paths.is_empty() {
        return None;
    }
    Some(Payload { jar_sha: jar_sha.to_string(), paths })
}

/// 把一个「已经解密并转好」的载荷 jar 安装到 `payloads/<jar_sha>/inner.jar`，并写索引。
///
/// 典型用法：加固壳 jar 先用原生桥跑一次（`runtime-dex/<md5>.r2.jar` 就是解密 + 转好的
/// 内层实现），然后：
/// ```text
/// vbox_cli jvm-shell --jar aowu.png --payload runtime-dex/0b58....r2.jar
/// ```
/// 之后同一只 jar 的宿主会**自动**改用 `shell-shim.jar` 的纯 Java 快路径（不再启动 unidbg），
/// 除非显式 `VBOX_SHELL_SHIM=0`。
pub fn install_payload(cache_dir: &Path, jar_sha: &str, payload_jar: &Path) -> Result<PathBuf> {
    if !payload_jar.is_file() {
        return Err(JvmError::MissingRuntime { path: payload_jar.to_path_buf() });
    }
    let dir = payload_root(cache_dir).join(jar_sha);
    std::fs::create_dir_all(&dir)?;
    let inner = dir.join("inner.jar");
    std::fs::copy(payload_jar, &inner)?;
    // 类索引：`inner.jar` 里的每个 .class → 该类名映射到 inner.jar；同时写进 any。
    let mut classes = std::collections::BTreeMap::new();
    for name in crate::dex::entry_names(&inner)? {
        if !name.ends_with(".class") || name.starts_with("META-INF/") {
            continue;
        }
        let cls = name.trim_end_matches(".class").replace('/', ".");
        classes.insert(cls, vec!["inner.jar".to_string()]);
    }
    let index = serde_json::json!({
        "jar": jar_sha,
        "classes": classes,
        "any": ["inner.jar"],
        "installedAt": chrono_free_now(),
        "size": std::fs::metadata(&inner)?.len(),
    });
    std::fs::write(dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
    Ok(inner)
}

/// 不引入 chrono：用 epoch 秒即可（只用于人工诊断）。
fn chrono_free_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 给用户看的诊断：为什么这个守卫 jar 没跑起来。
pub fn unsupported_hint(jar_sha: &str) -> String {
    format!(
        "该 jar 是加固壳（init 阶段需要 ARM native 解密），当前环境没有可用载荷。\
         可把解密产物放到 payloads/{jar_sha}/ 并按 index.json 建索引，或用 `VBOX_JVM_MODE=fallback` 走原生家族。"
    )
}

/// 读取/校验索引（供 CLI 诊断用）。
pub fn describe(cache_dir: &Path, jar_sha: &str) -> Result<Option<serde_json::Value>> {
    let file = payload_root(cache_dir).join(jar_sha).join("index.json");
    if !file.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&file)?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| JvmError::BadManifest { path: file.clone(), reason: e.to_string() })?;
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp/vbox_jvm_shell")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn exact_class_wins_and_missing_paths_are_skipped() {
        let cache = tmp("exact");
        let sha = "abc123";
        let dir = payload_root(&cache).join(sha);
        std::fs::create_dir_all(dir.join("classes")).unwrap();
        std::fs::write(
            dir.join("index.json"),
            r#"{"jar":"abc123","classes":{"com.foo.Bar":["classes/","gone.jar"]},"any":["classes/"]}"#,
        )
        .unwrap();
        let p = resolve(&cache, sha, Some("com.foo.Bar")).expect("payload");
        assert_eq!(p.paths.len(), 1, "{:?}", p.paths);
        assert!(p.paths[0].ends_with("classes"));
        // 未知类 → 落到 any
        let p2 = resolve(&cache, sha, Some("com.foo.Other")).expect("any payload");
        assert_eq!(p2.paths.len(), 1);
        // 没有任何条目 → None
        assert!(resolve(&cache, "nope", Some("X")).is_none());
    }

    #[test]
    fn broken_index_is_none_not_error() {
        let cache = tmp("broken");
        let dir = payload_root(&cache).join("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.json"), "{not json").unwrap();
        assert!(resolve(&cache, "bad", None).is_none());
        assert!(describe(&cache, "bad").is_err());
    }
}
