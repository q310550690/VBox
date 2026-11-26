//! 判据 67：错误码「用户文案三元组」（标题 / 建议动作 / 可重试）在 Rust 契约与前端
//! 文案表之间逐条一致。
//!
//! 为什么需要：既有 `catvod_error_copy` 只校验「每个码在前端表出现一次」以及
//! 用 260 字符窗口找 `retryable:` —— 窗口会跨越到下一个条目，真实漂移（如
//! `catvod.jar.missing` 前端为可重试、Rust 为不可重试）被掩盖。用户最终看到的是
//! 前端文案，文案与契约不一致时，界面行为与错误分类语义会分叉，且无编译错误。
//!
//! 本判据逐条解析前端对象（code → title/action/retryable），与
//! `ErrorClass::title()/action()/retryable()` 全量比对；并校验
//! `describe_error()` 输出的三元组与 `ErrorClass` 同源。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use vbox_core::spider::catvod::contract::{describe_error, ErrorClass};

fn frontend_entries() -> BTreeMap<String, (String, String, bool)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/utils/catvodErrors.ts");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取前端错误文案表失败 {}: {e}", path.display()));
    let start = text
        .find("CATVOD_ERROR_COPY")
        .expect("前端文案表缺少 CATVOD_ERROR_COPY");
    let body = &text[start..];

    let mut map = BTreeMap::new();
    for chunk in body.split("'catvod.").skip(1) {
        let code_end = chunk.find('\'').expect("码字面量未闭合");
        let code = format!("catvod.{}", &chunk[..code_end]);
        let entry_end = chunk.find('}').expect("文案对象未闭合");
        let entry = &chunk[..entry_end];

        let field = |name: &str| -> String {
            let key = format!("{name}: '");
            let i = entry
                .find(&key)
                .unwrap_or_else(|| panic!("{code} 缺少字段 {name}"));
            let rest = &entry[i + key.len()..];
            let j = rest.find('\'').unwrap_or_else(|| panic!("{code}.{name} 未闭合"));
            rest[..j].to_string()
        };

        let retryable = if entry.contains("retryable: true") {
            true
        } else if entry.contains("retryable: false") {
            false
        } else {
            panic!("{code} 缺少可重试标记");
        };

        let prev = map.insert(code.clone(), (field("title"), field("action"), retryable));
        assert!(prev.is_none(), "前端文案表出现重复码 {code}");
    }
    map
}

#[test]
fn frontend_copy_matches_rust_contract_exactly() {
    let entries = frontend_entries();
    let rust_codes: BTreeMap<String, ErrorClass> = ErrorClass::ALL
        .iter()
        .map(|c| (c.code().to_string(), *c))
        .collect();

    assert_eq!(
        entries.len(),
        rust_codes.len(),
        "前端文案条目数 {} 与契约错误码数 {} 不一致",
        entries.len(),
        rust_codes.len()
    );

    for (code, class) in &rust_codes {
        let (title, action, retryable) = entries
            .get(code)
            .unwrap_or_else(|| panic!("前端缺少错误码 {code} 的文案"));
        assert_eq!(
            title,
            class.title(),
            "错误码 {code} 的标题两端口径不一致"
        );
        assert_eq!(
            action,
            class.action(),
            "错误码 {code} 的建议动作两端口径不一致"
        );
        assert_eq!(
            retryable,
            &class.retryable(),
            "错误码 {code} 的可重试标记两端口径不一致"
        );
        assert!(!title.trim().is_empty(), "错误码 {code} 标题为空");
        assert!(!action.trim().is_empty(), "错误码 {code} 建议动作为空");
    }

    for code in entries.keys() {
        assert!(
            rust_codes.contains_key(code),
            "前端存在契约之外的错误码 {code}"
        );
    }
}

#[test]
fn describe_error_uses_contract_copy_and_strips_code() {
    for class in ErrorClass::ALL {
        let code = class.code();

        let text = describe_error(&format!("{code}：详情 42")).unwrap_or_else(|| {
            panic!("{code} 应能被 describe_error 识别");
        });
        assert_eq!(text.code, code);
        assert_eq!(text.title, class.title());
        assert_eq!(text.action, class.action());
        assert_eq!(text.retryable, class.retryable());
        assert_eq!(text.detail, "详情 42");

        let colon = describe_error(&format!("前缀 {code}:detail")).expect("半角冒号也应识别");
        assert_eq!(colon.code, code);
        assert_eq!(colon.detail, "detail");

        let bare = describe_error(code).expect("裸码也应识别");
        assert_eq!(bare.code, code);
        assert!(bare.detail.is_empty(), "裸码不应产生技术细节");
    }

    assert!(describe_error("普通错误，没有稳定码").is_none());
}
