//! 判据 33：`docs/*.md` 必须是「自动生成」或「手写」二者之一，且分类齐全。
//!
//! - 自动生成文档：头部必须给出生成工具（`_rea/tools/<tool>.mjs --write`）与生成时间；
//!   且生成工具必须被 `docs/审计索引.md` 收录（改工具能追溯到台账）。
//! - 手写文档：登记在 MANUAL 列表并附用途；新增/删除文档必须同步登记，
//!   防止「自动生成文档被手改」或「手写文档混入生成流程」。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 自动生成文档 → 生成工具（仓库相对路径）
const AUTO: &[( &str, &str)] = &[
    ("docs/站点覆盖报告.md", "_rea/tools/site-coverage.mjs"),
    ("docs/真站回归矩阵.md", "_rea/tools/live-coverage.mjs"),
    ("docs/上游复核.md", "_rea/tools/upstream-audit.mjs"),
    ("docs/阻断复核.md", "_rea/tools/blocked-recheck.mjs"),
    ("docs/审计索引.md", "_rea/tools/audit-index.mjs"),
    ("docs/兼容性审计.md", "_rea/tools/compat-audit.mjs"),
    ("docs/订阅漂移复核.md", "_rea/tools/inventory-drift.mjs"),
    ("docs/jar供应链.md", "_rea/samples/inventory/jar-supply.mjs"),
];

/// 手写文档（需要维护清单：增删文档时同步改这里）
const MANUAL: &[&str] = &[
    "docs/checklist.md",
    "docs/计划-W13-网盘账号登录面板.md",
    "docs/计划-W14-云盘聚合搜索.md",
    "docs/TVBoxOS订阅链接解析与真实定制分析报告.md",
    "docs/VBox实现说明.md",
    "docs/icons.md",
    "docs/发布与自动更新.md",
    "docs/完整开发方案.md",
    "docs/归档摘要-aspirecode-workbuddy.md",
    "docs/方案3-mpv渲染后端.md",
    "docs/着色器与滤镜插件方案.md",
    "docs/计划-type3-REA原生重写.md",
    "docs/计划-type3-原生兼容100%.md",
    "docs/计划-type3-翻译层与引擎工作包.md",
];

#[test]
fn docs_are_classified_and_markers_match() {
    let docs_dir = root().join("docs");
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(&docs_dir).expect("读取 docs").flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            actual.insert(format!("docs/{name}"));
        }
    }

    let auto: BTreeSet<String> = AUTO.iter().map(|(p, _)| p.to_string()).collect();
    let manual: BTreeSet<String> = MANUAL.iter().map(|p| p.to_string()).collect();
    assert_eq!(auto.len(), AUTO.len(), "AUTO 列表存在重复项");
    assert_eq!(manual.len(), MANUAL.len(), "MANUAL 列表存在重复项");
    assert!(
        auto.is_disjoint(&manual),
        "同一文档不能既自动生成又手写"
    );
    let classified: BTreeSet<String> = auto.union(&manual).cloned().collect();
    let missing: Vec<_> = actual.difference(&classified).cloned().collect();
    let extra: Vec<_> = classified.difference(&actual).cloned().collect();
    assert!(
        missing.is_empty(),
        "docs 下有未分类文档（请加入 AUTO 或 MANUAL）：{missing:?}"
    );
    assert!(extra.is_empty(), "清单登记了不存在的文档：{extra:?}");

    let index = fs::read_to_string(root().join("docs/审计索引.md")).expect("读取审计索引");
    for (doc, tool) in AUTO {
        let content = fs::read_to_string(root().join(doc)).expect("读取自动生成文档");
        let head: String = content.lines().take(20).collect::<Vec<_>>().join("\n");
        assert!(
            head.contains(tool) && head.contains("--write"),
            "{doc} 头部未声明生成工具 {tool} --write"
        );
        assert!(head.contains("生成时间"), "{doc} 头部缺少生成时间");
        assert!(
            index.contains(tool),
            "{doc} 的生成工具 {tool} 未被 docs/审计索引.md 收录"
        );
    }
}
