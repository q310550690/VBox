//! 判据 47：REA 附录 ↔ 家族/API 索引（第 164 轮）。
//!
//! 判据 46 只保证「每个家族 api 的字符串在 REA 文档中出现过」，但没有回答「在哪一章」。
//! REA 有 160+ 个附录（A…FM），章节重排、合并、误删都可能让证据静默消失而判据 46 仍绿。
//!
//! 本轮把 `计划-type3-原生兼容100%.md` §165.2 的索引表（90 行）作为机器可读事实：
//! - 每行 `api | 家族 | 类 | REA 附录字母 | 证据方式`；
//! - 用运行时 `families::family_of` 重建映射，与表逐行互锁；
//! - 字母必须在 REA 中存在唯一附录，且该附录正文包含这个 api 字符串；
//! - 「标题」级证据要求附录标题含 api 或类名，「家族章」级证据要求附录标题含家族名。
//!
//! 这样新增/重命名家族时，若不写 REA 索引行，判据 47 会直接失败。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use vbox_core::spider::catvod::{families, registry};

const PLAN_DOC: &str = "docs/type3兼容/type3-实施计划-原生兼容100%.md";
const REA_DOC: &str = "docs/type3兼容/type3-路线-REA原生重写.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("定位仓库根目录失败")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", p.display()))
}

/// 家族 → 该家族接管的 api 集合（运行时 `family_of` 是唯一事实来源）。
fn family_apis() -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for api in registry::profiles().apis.keys() {
        if let Some(family) = families::family_of(api) {
            map.entry(family.to_string()).or_default().insert(api.clone());
        }
    }
    map
}

#[derive(Debug, Clone)]
struct Section {
    letter: String,
    title: String,
    body: String,
}

fn appendix_heading(line: &str, in_appendix: bool) -> Option<(String, String)> {
    fn split(rest: &str) -> Option<(String, String)> {
        let dot = rest.find('.')?;
        let letter = &rest[..dot];
        if letter.is_empty()
            || letter.len() > 3
            || !letter.chars().all(|c| c.is_ascii_uppercase())
        {
            return None;
        }
        let after = &rest[dot + 1..];
        if !after.is_empty() && !after.starts_with(char::is_whitespace) {
            // `### AA.1 来源` 这类小章节编号不是附录标题
            return None;
        }
        Some((letter.to_string(), after.trim().to_string()))
    }
    for prefix in ["## 附录 ", "### 附录 "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return split(rest);
        }
    }
    if in_appendix {
        if let Some(rest) = line.strip_prefix("### ") {
            if let Some((letter, title)) = split(rest) {
                if !title.is_empty() {
                    return Some((letter, title));
                }
            }
        }
    }
    None
}

/// 把 REA 文档切成附录节：
/// - `## 附录 X.` / `### 附录 X.`；
/// - 附录区（`## <数字>. 附录`）内的 `### X.`（早期附录 A–K 的写法）。
fn parse_appendix_sections(text: &str) -> Vec<Section> {
    let norm = text.replace("\r\n", "\n");
    let lines: Vec<&str> = norm.split('\n').collect();
    let mut starts: Vec<(usize, String, String)> = Vec::new();
    let mut in_appendix = false;
    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(dot) = rest.find('.') {
                if !rest[..dot].is_empty() && rest[..dot].chars().all(|c| c.is_ascii_digit()) {
                    in_appendix = rest.contains("附录");
                }
            }
        }
        if let Some((letter, title)) = appendix_heading(line, in_appendix) {
            starts.push((i, letter, title));
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (idx, (start, letter, title)) in starts.iter().enumerate() {
        let end = starts.get(idx + 1).map(|s| s.0).unwrap_or(lines.len());
        out.push(Section {
            letter: letter.clone(),
            title: title.clone(),
            // 含标题行：标题级证据（如 \`### N. csp_Bili 移植记录\`）的 api 名只在标题里出现
            body: lines[*start..end].join("\n"),
        });
    }
    out
}

struct IndexRow {
    api: String,
    family: String,
    class: String,
    letter: String,
    how: String,
}

fn parse_index_rows(plan: &str) -> Vec<IndexRow> {
    let norm = plan.replace("\r\n", "\n");
    let start = norm
        .find("## 165. 第 164 轮")
        .expect("计划文档缺少 §165 REA 附录索引章节");
    let rest = &norm[start..];
    let end = rest.find("\n## ").map(|i| i + 1).unwrap_or(rest.len());
    let section = &rest[..end];
    let mut rows = Vec::new();
    for line in section.split('\n') {
        if !line.starts_with("| `csp_") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
        assert!(
            cells.len() >= 7,
            "§165.2 索引表行列数不足（需 5 列）：{line}"
        );
        rows.push(IndexRow {
            api: cells[1].trim_matches('`').to_string(),
            family: cells[2].to_string(),
            class: cells[3].trim_matches('`').to_string(),
            letter: cells[4].to_string(),
            how: cells[5].to_string(),
        });
    }
    rows
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rea_index_matches_runtime_family_mapping() {
    let plan = read(PLAN_DOC);
    let rea = read(REA_DOC);
    let sections = parse_appendix_sections(&rea);
    assert!(
        sections.len() >= 90,
        "REA 附录节数应不少于 90，实际 {}",
        sections.len()
    );

    let mut by_letter: BTreeMap<String, Section> = BTreeMap::new();
    for section in sections {
        assert!(
            by_letter.insert(section.letter.clone(), section).is_none(),
            "REA 中附录字母重复（同一字母只能有一个附录）"
        );
    }

    let rows = parse_index_rows(&plan);
    assert_eq!(rows.len(), 102, "§165.2 索引表应恰有 102 行");

    let runtime = family_apis();
    assert_eq!(runtime.len(), 77, "运行时家族数应恰为 77");
    let runtime_api_count: usize = runtime.values().map(BTreeSet::len).sum();
    assert_eq!(runtime_api_count, 102, "运行时家族接管 api 应恰为 102");

    let profiles = registry::profiles();
    let mut api_rows: BTreeSet<String> = BTreeSet::new();
    let mut family_rows: BTreeSet<String> = BTreeSet::new();

    for row in &rows {
        assert!(
            api_rows.insert(row.api.clone()),
            "索引表中 api {} 重复",
            row.api
        );
        family_rows.insert(row.family.clone());

        let apis = runtime
            .get(&row.family)
            .unwrap_or_else(|| panic!("索引家族 {} 不在运行时映射中", row.family));
        assert!(
            apis.contains(&row.api),
            "索引把 api {} 归到家族 {}，但运行时 family_of 不是该家族",
            row.api,
            row.family
        );

        let profile = profiles
            .apis
            .get(&row.api)
            .unwrap_or_else(|| panic!("索引 api {} 不在画像中", row.api));
        let runtime_class = profile.class.as_deref().unwrap_or("");
        if row.class != "—" {
            assert_eq!(
                row.class, runtime_class,
                "api {} 的类名与画像不一致",
                row.api
            );
        }

        assert!(
            row.how == "标题" || row.how == "家族章",
            "api {} 的证据方式必须是「标题」或「家族章」，实际 {}",
            row.api,
            row.how
        );

        let section = by_letter
            .get(&row.letter)
            .unwrap_or_else(|| panic!("索引字母 {} 在 REA 中不存在", row.letter));
        let simple = runtime_class.rsplit('.').next().unwrap_or("");

        if row.how == "标题" {
            assert!(
                section.title.contains(&row.api)
                    || (!simple.is_empty() && section.title.contains(simple)),
                "「标题」级证据的附录 {} 标题既不含 api {} 也不含类名 {}",
                row.letter,
                row.api,
                simple
            );
            assert!(
                section.body.contains(&row.api)
                    || (!simple.is_empty() && section.body.contains(simple)),
                "REA 附录 {} 正文既不含 api {} 也不含类名 {}",
                row.letter,
                row.api,
                simple
            );
        } else {
            assert!(
                section.body.contains(&row.api),
                "「家族章」级证据的 REA 附录 {} 正文未包含 api {}",
                row.letter,
                row.api
            );
            assert!(
                section
                    .title
                    .to_lowercase()
                    .contains(&row.family.to_lowercase()),
                "「家族章」级证据的附录 {} 标题不含家族名 {}",
                row.letter,
                row.family
            );
        }
    }

    let runtime_apis: BTreeSet<String> = runtime.values().flatten().cloned().collect();
    assert_eq!(
        api_rows, runtime_apis,
        "索引表 api 集合与运行时家族接管 api 集合不一致"
    );
    let runtime_families: BTreeSet<String> = runtime.keys().cloned().collect();
    assert_eq!(
        family_rows, runtime_families,
        "索引表家族集合与运行时家族集合不一致"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rea_appendices_are_unique_and_well_formed() {
    let plan = read(PLAN_DOC);
    let rea = read(REA_DOC);
    let sections = parse_appendix_sections(&rea);
    let rows = parse_index_rows(&plan);

    // 字母唯一
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for s in &sections {
        assert!(
            s.letter.chars().all(|c| c.is_ascii_uppercase()) && (1..=3).contains(&s.letter.len()),
            "附录字母 {} 形状非法",
            s.letter
        );
        assert!(seen.insert(s.letter.clone()), "附录字母 {} 重复", s.letter);
    }

    // 索引覆盖到的不同字母数：当前 77 个（Wogg 四个 api 共用一个家族附录）
    let mapped: BTreeSet<&str> = rows.iter().map(|r| r.letter.as_str()).collect();
    assert!(
        mapped.len() >= 60,
        "索引涉及的不同附录字母应不少于 60，实际 {}",
        mapped.len()
    );

    // 每个被引用的附录正文非空
    let by_letter: BTreeMap<&str, &Section> =
        sections.iter().map(|s| (s.letter.as_str(), s)).collect();
    for letter in mapped {
        let section = by_letter
            .get(letter)
            .unwrap_or_else(|| panic!("索引字母 {letter} 在 REA 中不存在"));
        assert!(
            section.body.trim().len() >= 40,
            "REA 附录 {letter} 正文过短（{} 字节），证据不成立",
            section.body.trim().len()
        );
    }
}
