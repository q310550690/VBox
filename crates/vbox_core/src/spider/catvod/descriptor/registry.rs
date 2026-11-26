//! 内嵌描述注册表。
//!
//! 描述文件随 crate 编译进二进制（`include_str!`），运行时不做文件读取；
//! 新增家族只需加一对 `(api, include_str!)` 与一个测试，不改分发逻辑。

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::Value;

use super::schema::{self, Descriptor};
use super::value;

/// `(api, 描述 JSON)`。api 必须与描述内 `api` 字段一致（大小写不敏感）。
const EMBEDDED: &[(&str, &str)] = &[
    ("csp_Kkys", include_str!("data/kkys.json")),
    ("csp_LiteApple", include_str!("data/liteapple.json")),
    ("csp_Ysj", include_str!("data/ysj.json")),
    // T4 第一批迁移（2026-11-16）：保留手写实现，描述层按入口接管。
    ("csp_Auete", include_str!("data/csp_Auete.json")),
    ("csp_Bttwoo", include_str!("data/csp_Bttwoo.json")),
    ("csp_Czsapp", include_str!("data/csp_Czsapp.json")),
    ("csp_Dm84", include_str!("data/csp_Dm84.json")),
    ("csp_Duanju", include_str!("data/csp_Duanju.json")),
    ("csp_Jianpian", include_str!("data/csp_Jianpian.json")),
    ("csp_Libvio", include_str!("data/csp_Libvio.json")),
    ("csp_Meijumi", include_str!("data/csp_Meijumi.json")),
    ("csp_NanGua", include_str!("data/csp_NanGua.json")),
    ("csp_SixV", include_str!("data/csp_SixV.json")),
    ("csp_TTian", include_str!("data/csp_TTian.json")),
    ("csp_Tangrenjie", include_str!("data/csp_Tangrenjie.json")),
    ("csp_Xb6v", include_str!("data/csp_Xb6v.json")),
    // T4 第二批迁移（2026-11-16）：HTML/JSON 混合家族，保留手写实现，描述层按入口接管。
    ("csp_AppYsV2", include_str!("data/csp_AppYsV2.json")),
    ("csp_Ddrk", include_str!("data/csp_Ddrk.json")),
    ("csp_FirstAid", include_str!("data/csp_FirstAid.json")),
    ("csp_Kanqiu", include_str!("data/csp_Kanqiu.json")),
    ("csp_Mp4Mov", include_str!("data/csp_Mp4Mov.json")),
];

/// 模板里允许出现的内置占位符（其余必须由 `vars` 声明）。
const BUILTIN: &[&str] = &[
    "base",
    "id",
    "cat",
    "page",
    "kw",
    "index",
    "url",
    "error",
    "value",
    "raw",
    "platform.referer",
];

static CACHE: OnceLock<Vec<Descriptor>> = OnceLock::new();

/// 解析 + 校验一份描述；描述是编译期资产，任何问题都直接 panic。
fn build(name: &str, text: &str) -> Descriptor {
    let desc = schema::parse(text).unwrap_or_else(|e| panic!("描述 {name} 解析失败: {e}"));
    assert!(
        desc.api.eq_ignore_ascii_case(name.trim_end_matches(".json")),
        "描述 {name} 的 api 字段是 {}，两者必须一致",
        desc.api
    );
    let errs = desc.validate();
    assert!(errs.is_empty(), "描述 {name} 校验失败: {errs:?}");
    let known = known_placeholders(&desc);
    check_placeholders(name, text, &known);
    desc
}

/// 开发期候选描述目录（环境变量 `VBOX_DESCRIPTOR_DIR`）。
///
/// T4 迁移时先把候选 JSON 放进该目录、以 `VBOX_DESCRIPTOR_STRICT=1` 跑既有 mock，
/// 通过后再移入 `data/` 并登记 `EMBEDDED`——这样迁移期间不必反复改编译期清单。
/// 目录内按文件名排序读取 `*.json`；与内嵌描述 api 重复时以内嵌为准。
fn external_entries() -> Vec<(String, String)> {
    let Some(dir) = std::env::var_os("VBOX_DESCRIPTOR_DIR") else {
        return Vec::new();
    };
    let dir = std::path::PathBuf::from(dir);
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("VBOX_DESCRIPTOR_DIR 不可读（{}）: {e}", dir.display()));
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().map(|ext| ext == "json").unwrap_or(false))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("读取候选描述 {} 失败: {e}", path.display()));
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, text)
        })
        .collect()
}

/// 全部已注册描述（首次调用时解析并校验，失败即 panic——描述是编译期资产）。
pub fn all() -> &'static [Descriptor] {
    CACHE.get_or_init(|| {
        let mut out: Vec<Descriptor> = Vec::with_capacity(EMBEDDED.len());
        for (name, text) in EMBEDDED {
            out.push(build(name, text));
        }
        for (name, text) in external_entries() {
            let desc = build(&name, &text);
            if out.iter().any(|known| known.api.eq_ignore_ascii_case(&desc.api)) {
                continue;
            }
            out.push(desc);
        }
        out
    })
}

/// 变量名 + 内置名。
fn known_placeholders(desc: &Descriptor) -> BTreeSet<String> {
    let mut known: BTreeSet<String> = BUILTIN.iter().map(|s| s.to_string()).collect();
    for decl in &desc.vars {
        known.insert(decl.name.clone());
    }
    known
}

/// 遍历描述 JSON 的所有字符串，检查模板占位符都在白名单内。
///
/// 递归扫描（而非逐字段枚举）保证新增字段不会绕过校验。
fn check_placeholders(name: &str, text: &str, known: &BTreeSet<String>) {
    let value: Value = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("内嵌描述 {name} 解析失败: {e}"));
    let mut bad = BTreeSet::new();
    walk_strings(&value, &mut |s| {
        for placeholder in value::unknown_placeholders(s, known) {
            bad.insert(placeholder);
        }
    });
    assert!(bad.is_empty(), "内嵌描述 {name} 使用了未知占位符: {bad:?}");
}

fn walk_strings(value: &Value, f: &mut impl FnMut(&str)) {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => {
            for item in items {
                walk_strings(item, f);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                walk_strings(item, f);
            }
        }
        _ => {}
    }
}

/// 按 api 查找（大小写不敏感）。
pub fn find(api: &str) -> Option<&'static Descriptor> {
    let api = api.trim();
    all().iter().find(|d| d.api.eq_ignore_ascii_case(api))
}

/// 已覆盖的 api（供测试/报表使用）。
pub fn apis() -> Vec<&'static str> {
    all().iter().map(|d| d.api.as_str()).collect()
}

/// `(api, 描述原文)`，供台账/差分判据核对字节级指纹。
pub fn embedded() -> &'static [(&'static str, &'static str)] {
    EMBEDDED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_embeds_all_migrated_apis() {
        // T1 三个 api + T4 迁移批次；顺序即 `EMBEDDED` 声明顺序。
        assert_eq!(
            apis(),
            vec![
                "csp_Kkys",
                "csp_LiteApple",
                "csp_Ysj",
                "csp_Auete",
                "csp_Bttwoo",
                "csp_Czsapp",
                "csp_Dm84",
                "csp_Duanju",
                "csp_Jianpian",
                "csp_Libvio",
                "csp_Meijumi",
                "csp_NanGua",
                "csp_SixV",
                "csp_TTian",
                "csp_Tangrenjie",
                "csp_Xb6v",
                "csp_AppYsV2",
                "csp_Ddrk",
                "csp_FirstAid",
                "csp_Kanqiu",
                "csp_Mp4Mov",
            ]
        );
        assert!(find("csp_kkys").is_some(), "api 匹配应大小写不敏感");
        assert!(find("  csp_LiteApple ").is_some(), "api 匹配应忽略空白");
        assert!(find("csp_Unknown").is_none());
    }

    #[test]
    fn every_descriptor_has_evidence_and_sha() {
        for desc in all() {
            assert!(!desc.evidence.is_empty(), "{} 缺少证据", desc.api);
            assert!(
                schema::is_sha256(&desc.jar_sha256),
                "{} 的 jar_sha256 不是 64 位小写 hex",
                desc.api
            );
            assert!(!desc.class.is_empty());
            assert!(!desc.jar.is_empty());
        }
    }
}
