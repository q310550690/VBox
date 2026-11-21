//! 判据 33：`docs/*.md` 必须是「自动生成」或「手写」二者之一，且分类齐全。
//!
//! - 自动生成文档：头部必须给出生成工具（`_rea/tools/<tool>.mjs --write`）与生成时间；
//!   且生成工具必须被 `docs/审计报告/审计索引.md` 收录（改工具能追溯到台账）。
//! - 手写文档：登记在 MANUAL 列表并附用途；新增/删除文档必须同步登记，
//!   防止「自动生成文档被手改」或「手写文档混入生成流程」。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// 自动生成文档 → 生成工具（仓库相对路径）
const AUTO: &[(&str, &str)] = &[
    ("docs/审计报告/站点覆盖报告.md", "_rea/tools/site-coverage.mjs"),
    ("docs/审计报告/真站回归矩阵.md", "_rea/tools/live-coverage.mjs"),
    ("docs/审计报告/上游复核.md", "_rea/tools/upstream-audit.mjs"),
    ("docs/审计报告/阻断复核.md", "_rea/tools/blocked-recheck.mjs"),
    ("docs/审计报告/审计索引.md", "_rea/tools/audit-index.mjs"),
    ("docs/审计报告/兼容性审计.md", "_rea/tools/compat-audit.mjs"),
    ("docs/审计报告/订阅漂移复核.md", "_rea/tools/inventory-drift.mjs"),
    ("docs/审计报告/jar供应链.md", "_rea/samples/inventory/jar-supply.mjs"),
];

/// 手写文档（需要维护清单：增删文档时同步改这里；子目录按分类存放）
const MANUAL: &[&str] = &[
    "docs/README.md",
    "docs/checklist.md",
    "docs/产品方案/完整开发方案-初版2026-10-26.md",
    "docs/产品方案/VBox实现说明.md",
    "docs/产品方案/TVBoxOS订阅链路分析报告.md",
    "docs/功能设计/播放器-mpv渲染后端方案.md",
    "docs/功能设计/播放器-着色器与滤镜插件.md",
    "docs/功能设计/发布与自动更新.md",
    "docs/功能设计/图标体系.md",
    "docs/type3兼容/type3-路线-REA原生重写.md",
    "docs/type3兼容/type3-实施计划-原生兼容100%.md",
    "docs/type3兼容/type3-翻译层与引擎工作包.md",
    "docs/研发计划/W13-网盘账号登录面板.md",
    "docs/研发计划/W14-云盘聚合搜索.md",
    "docs/设计规范/VBox-UI美化计划.md",
    "docs/审计报告/REA原生站点实测.md",
    "docs/归档/归档摘要-aspirecode-workbuddy-2026-11-06.md",
    "docs/归档/早期分析草稿/draft-1-config.md",
    "docs/归档/早期分析草稿/draft-2-sites.md",
    "docs/归档/早期分析草稿/draft-3-parse-live.md",
];

/// 递归收集 docs 下的 markdown（跳过 .git 与插件示例；返回 docs/ 相对路径）
fn collect_docs(root: &std::path::Path) -> BTreeSet<String> {
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == "plugin-examples" {
                continue;
            }
            if path.is_dir() {
                walk(&path, base, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let rel = path.strip_prefix(base).expect("docs 相对路径");
                out.insert(format!("docs/{}", rel.to_string_lossy().replace('\\', "/")));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(&root.join("docs"), &root.join("docs"), &mut out);
    out
}

#[test]
fn docs_are_classified_and_markers_match() {
    let actual = collect_docs(&root());

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

    let index = fs::read_to_string(root().join("docs/审计报告/审计索引.md")).expect("读取审计索引");
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
            "{doc} 的生成工具 {tool} 未被 docs/审计报告/审计索引.md 收录"
        );
    }
}
