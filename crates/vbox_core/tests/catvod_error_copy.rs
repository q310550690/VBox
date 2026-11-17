//! 宿主侧（前端）错误文案与契约层 `ErrorClass` 的一致性回归。
//!
//! 目标：`catvod.*` 稳定码的 UI 文案不允许悄悄漂移。前端映射表
//! `src/utils/catvodErrors.ts` 必须覆盖契约层的全部 20 个分类，且
//! 标题 / 建议动作 / 可重试标记与 Rust 侧一致。

use vbox_core::spider::catvod::contract::ErrorClass;

fn frontend_table() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/utils/catvodErrors.ts");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取前端错误文案表失败 {}: {e}", path.display()))
}

#[test]
fn frontend_covers_every_error_class() {
    let table = frontend_table();
    for class in ErrorClass::ALL {
        assert!(
            table.contains(class.code()),
            "前端文案缺少错误码 {}（{}）",
            class.code(),
            class.title()
        );
        assert!(
            table.contains(class.title()),
            "前端文案缺少标题 {}（对应 {}）",
            class.title(),
            class.code()
        );
        assert!(
            table.contains(class.action()),
            "前端文案缺少建议动作 {}（对应 {}）",
            class.action(),
            class.code()
        );
    }
}

#[test]
fn frontend_table_has_exactly_all_classes() {
    let table = frontend_table();
    for class in ErrorClass::ALL {
        let occurrences = table.matches(class.code()).count();
        assert_eq!(
            occurrences, 1,
            "错误码 {} 在前端表中出现 {} 次（应为 1 次）",
            class.code(),
            occurrences
        );
    }
    assert_eq!(ErrorClass::ALL.len(), 20);
}

#[test]
fn frontend_retryable_flags_match_contract() {
    let table = frontend_table();
    for class in ErrorClass::ALL {
        let code = class.code();
        let start = table.find(code).unwrap_or_else(|| panic!("缺少 {code}"));
        let block: String = table[start..].chars().take(260).collect();
        let expected = if class.retryable() {
            "retryable: true"
        } else {
            "retryable: false"
        };
        assert!(
            block.contains(expected),
            "错误码 {code} 的可重试标记与 Rust 侧不一致（应为 {expected}）"
        );
    }
}
