//! 缺类诊断：把宿主返回的 `class_not_found` 汇总成「缺哪些类」清单。
//!
//! 兼容层的类覆盖是逐个样本暴露出来的（`android.util.LruCache`、`android.util.Base64` …），
//! 以前只能靠用户撞一个补一个。这里把 `ClassNotFoundException` / `NoClassDefFoundError`
//! 里的类名抽出来计数，`jvm_status` 与设置页直接展示，补类时按热度从上往下加。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 一条缺类记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingClass {
    /// 全限定类名，如 `android.util.LruCache`。
    pub class: String,
    /// 累计命中次数。
    pub hits: u64,
}

fn registry() -> &'static Mutex<BTreeMap<String, u64>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, u64>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 从宿主错误详情里抽出缺失的类名。
///
/// 认这几种形态（宿主 `classify()` 的 detail 里常见）：
/// - `java.lang.ClassNotFoundException: android.util.LruCache`
/// - `java.lang.NoClassDefFoundError: android/util/LruCache`
/// - `java.lang.NoClassDefFoundError: Could not initialize class android.util.LruCache`
pub fn extract_missing_class(detail: &str) -> Option<String> {
    for marker in [
        "java.lang.ClassNotFoundException: ",
        "ClassNotFoundException: ",
        "java.lang.NoClassDefFoundError: ",
        "NoClassDefFoundError: ",
    ] {
        if let Some(pos) = detail.find(marker) {
            let rest = &detail[pos + marker.len()..];
            // 「Could not initialize class X」是初始化失败，不是缺类，跳过。
            if rest.starts_with("Could not initialize class ") {
                return None;
            }
            let token = rest
                .trim_start()
                .split(|c: char| c.is_whitespace() || c == '\r' || c == '\n' || c == ';')
                .next()
                .unwrap_or("");
            let class = token.trim().replace('/', ".");
            if class.is_empty() || !class.contains('.') {
                return None;
            }
            // 去掉常见的尾部噪声（`at ...`、`(xxx`）。
            let class = class
                .split(|c: char| c == '(' || c == ')')
                .next()
                .unwrap_or("")
                .to_string();
            if class.is_empty() {
                return None;
            }
            return Some(class);
        }
    }
    None
}

/// 记一次失败：只有 `class_not_found` 且能抽出类名时入库。
pub fn note_failure(code: &str, detail: &str) {
    if code != "class_not_found" {
        return;
    }
    let Some(class) = extract_missing_class(detail) else {
        return;
    };
    let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
    *map.entry(class).or_insert(0) += 1;
}

/// `note_failure` 的公开别名（vbox_core 用这个名字调用，语义更直白）。
pub fn note_missing_class(code: &str, detail: &str) {
    note_failure(code, detail);
}

/// 当前缺类清单，按命中次数降序（同次数按类名升序）。
pub fn missing_classes() -> Vec<MissingClass> {
    let map = registry().lock().unwrap_or_else(|e| e.into_inner());
    let mut rows: Vec<MissingClass> = map
        .iter()
        .map(|(class, hits)| MissingClass {
            class: class.clone(),
            hits: *hits,
        })
        .collect();
    rows.sort_by(|a, b| b.hits.cmp(&a.hits).then_with(|| a.class.cmp(&b.class)));
    rows
}

/// 清空（测试用；产品里没有入口）。
pub fn reset() {
    let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
    map.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_dotted_and_slashed_names() {
        assert_eq!(
            extract_missing_class("java.lang.ClassNotFoundException: android.util.LruCache").as_deref(),
            Some("android.util.LruCache")
        );
        assert_eq!(
            extract_missing_class("java.lang.NoClassDefFoundError: android/util/Base64").as_deref(),
            Some("android.util.Base64")
        );
        assert_eq!(
            extract_missing_class("java.lang.ClassNotFoundException: com.foo.Bar\n\tat x.y.Z").as_deref(),
            Some("com.foo.Bar")
        );
    }

    #[test]
    fn ignores_initialization_failures_and_other_errors() {
        assert_eq!(
            extract_missing_class(
                "java.lang.NoClassDefFoundError: Could not initialize class com.foo.Bar"
            ),
            None
        );
        assert_eq!(extract_missing_class("java.lang.NullPointerException"), None);
    }

    #[test]
    fn counts_only_class_not_found() {
        reset();
        note_failure("class_not_found", "java.lang.ClassNotFoundException: android.util.LruCache");
        note_failure("class_not_found", "java.lang.ClassNotFoundException: android.util.LruCache");
        note_failure("spider_threw", "java.lang.ClassNotFoundException: not.counted.Cls");
        let rows = missing_classes();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hits, 2);
        reset();
        assert!(missing_classes().is_empty());
    }
}
