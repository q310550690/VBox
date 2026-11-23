//! 判据 35 配套：网盘引擎（`crates/vbox_core/src/pan/`）的**上游契约形状**巡检。
//!
//! 为什么单独建文件：家族测试验证的是「站点协议 → 视频条目」，而 pan 引擎验证的是
//! 「网盘 API 的字段形状/错误形状」——两者失效方式不同。上游若改了字段名（如
//! `data.stoken` → `data.share_token`），家族测试仍可能因 mock 通过而全绿，只有真站
//! 巡检能发现。因此这里放两类用例：
//!   1) 离线：分享链接域名解析与「需登录」错误分类（不依赖网络，随全量测试常跑）；
//!   2) 真站（`#[ignore]`）：匿名夸克分享浏览 + 失效分享的错误形状（自适应上游变化）。
//!
//! 真站用例不写入任何产物，失败即表示「上游契约可能已变化」，需人工复核后同步 provider。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;
use vbox_core::model::Rules;
use vbox_core::spider::catvod::contract::ErrorClass;
use vbox_core::pan::ali::AliClient;
use vbox_core::pan::quark::QuarkClient;
use vbox_core::pan::uc::UcClient;
use vbox_core::pan::{cookie_required, parse_share_link, PanProvider};

fn rules() -> Rules {
    Rules::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn share_link_domains_are_stable() {
    let cases = [
        ("https://pan.quark.cn/s/24f565ff8b77", PanProvider::Quark),
        ("https://www.alipan.com/s/abc123", PanProvider::Ali),
        ("https://www.aliyundrive.com/s/abc123", PanProvider::Ali),
        ("https://alipan.com/s/abc123", PanProvider::Ali),
        ("https://drive.uc.cn/s/abc123", PanProvider::Uc),
        ("https://pc.uc.cn/s/abc123", PanProvider::Uc),
        ("https://115.com/s/abc123", PanProvider::P115),
    ];
    for (raw, want) in cases {
        let link = parse_share_link(raw).unwrap_or_else(|| panic!("解析失败：{raw}"));
        assert_eq!(link.provider, want, "域名映射变化：{raw}");
        assert!(!link.share_id.is_empty(), "分享 id 为空：{raw}");
    }
    assert!(parse_share_link("https://example.com/s/abc").is_none(), "未知域名不应被识别");
    // 带访问码与子目录的链接仍要解析出 passcode / sub_fid。
    let link = parse_share_link("https://pan.quark.cn/s/abc123?pwd=xyz9#/list/share/fid42").unwrap();
    assert_eq!(link.passcode, "xyz9");
    assert_eq!(link.sub_fid, "fid42");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn cookie_required_errors_are_distinct_and_actionable() {
    let mut seen = std::collections::BTreeSet::new();
    for p in [PanProvider::Quark, PanProvider::Ali, PanProvider::Uc, PanProvider::P115] {
        let err = cookie_required(p);
        // 原始 CoreError 是「配置无效: …」，稳定码由 contract::classify_pan_error 归集；
        // 两者都必须成立，否则宿主侧无法把该错误翻译成 catvod.pan.cookie_required。
        let class = ErrorClass::classify_pan_error(&err)
            .unwrap_or_else(|| panic!("{p:?} 未被归集为网盘错误"));
        assert_eq!(class, ErrorClass::PanCookieRequired, "{p:?} 归集错误");
        assert_eq!(class.code(), "catvod.pan.cookie_required");
        let text = err.to_string();
        assert!(seen.insert(text.clone()), "{p:?} 的错误文案与其它 provider 重复（宿主提示无法区分）");
        assert!(text.contains(p.name()), "{p:?} 文案未指明 provider：{text}");
    }
}

/// 真站：匿名夸克分享浏览的字段形状（title / files / fid / name / dir）。
#[test]
#[ignore = "真站：依赖 pan.quark.cn 匿名分享可用性与字段形状，网络/上游变化时人工复核"]
fn quark_anonymous_share_contract_shape() {
    let link = parse_share_link("https://pan.quark.cn/s/24f565ff8b77").expect("分享链接解析");
    let client = QuarkClient::with_rules(&rules(), "").expect("构建匿名客户端");
    let browse = client.browse(&link).expect("匿名浏览公开分享失败（上游契约可能变化）");
    assert_eq!(browse.provider, PanProvider::Quark);
    assert_eq!(browse.share_id, link.share_id);
    assert!(!browse.title.is_empty(), "分享标题为空：data.share.title 字段可能改名");
    assert!(!browse.files.is_empty(), "根目录为空：data.list 字段可能改名或分享已失效");
    for f in &browse.files {
        assert!(!f.fid.is_empty(), "条目 fid 为空：字段可能改名");
        assert!(!f.name.is_empty(), "条目 name 为空：字段可能改名");
    }
}

/// 真站：失效/不存在分享必须返回**结构化错误**（而不是 panic 或静默空列表）。
#[test]
#[ignore = "真站：验证失效分享的错误形状（41006/NotFound 等），上游变化时人工复核"]
fn quark_invalid_share_reports_structured_error() {
    let link = parse_share_link("https://pan.quark.cn/s/ffffffffffff").expect("分享链接解析");
    let client = QuarkClient::with_rules(&rules(), "").expect("构建匿名客户端");
    let err = client.browse(&link).expect_err("不存在的分享不应成功");
    let text = format!("{err}");
    assert!(!text.is_empty(), "错误没有可读信息");
    // 错误必须被归类为「分享失效」，宿主才能映射到 catvod.pan.share_unavailable。
    assert_eq!(
        ErrorClass::classify_pan_error(&err),
        Some(ErrorClass::PanShareUnavailable),
        "失效分享未被归集为 share_unavailable：{err}"
    );
}

/// 真站：阿里云盘失效分享的错误形状（`NotFound.ShareLink` → share_unavailable）。
#[test]
#[ignore = "真站：验证阿里失效分享的错误形状，上游变化时人工复核"]
fn ali_invalid_share_reports_share_unavailable() {
    let link = parse_share_link("https://www.alipan.com/s/ffffffffffffffffffff").expect("分享链接解析");
    let client = AliClient::with_rules(&rules(), "").expect("构建匿名客户端");
    let err = client.browse(&link).expect_err("不存在的阿里云盘分享不应成功");
    assert_eq!(
        ErrorClass::classify_pan_error(&err),
        Some(ErrorClass::PanShareUnavailable),
        "阿里失效分享未被归集为 share_unavailable：{err}"
    );
}

/// 真站：UC 网盘失效分享的错误形状（41006 等 → share_unavailable）。
#[test]
#[ignore = "真站：验证 UC 失效分享的错误形状，上游变化时人工复核"]
fn uc_invalid_share_reports_share_unavailable() {
    let link = parse_share_link("https://drive.uc.cn/s/ffffffffffff").expect("分享链接解析");
    let client = UcClient::with_rules(&rules(), "").expect("构建匿名客户端");
    let err = client.browse(&link).expect_err("不存在的 UC 分享不应成功");
    assert_eq!(
        ErrorClass::classify_pan_error(&err),
        Some(ErrorClass::PanShareUnavailable),
        "UC 失效分享未被归集为 share_unavailable：{err}"
    );
}


// ---------------------------------------------------------------------------
// 字段形状快照：类型化解析「静默成功」时的兜底
// ---------------------------------------------------------------------------

/// 递归收集「JSON 路径: 类型」，与对象键序、数组长度无关（数组元素统一记为 `[]`）。
fn collect_shape(v: &Value, path: &str, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(map) => {
            out.insert(format!("{path}: object"));
            for (k, val) in map {
                collect_shape(val, &format!("{path}.{k}"), out);
            }
        }
        Value::Array(arr) => {
            out.insert(format!("{path}: array"));
            for val in arr {
                collect_shape(val, &format!("{path}[]"), out);
            }
        }
        Value::Null => {
            out.insert(format!("{path}: null"));
        }
        Value::Bool(_) => {
            out.insert(format!("{path}: bool"));
        }
        Value::Number(_) => {
            out.insert(format!("{path}: number"));
        }
        Value::String(_) => {
            out.insert(format!("{path}: string"));
        }
    }
}

fn shape_lines(v: &Value) -> Vec<String> {
    let mut out = BTreeSet::new();
    collect_shape(v, "$", &mut out);
    out.into_iter().collect()
}

fn shape_fixture_path() -> PathBuf {
    // 集成测试 cwd 是 crate 目录，fixtures 放在 `crates/vbox_core/tests/fixtures/`。
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pan_quark_wire_shape.txt")
}

fn read_shape_sections(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut sections: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            let name = name.trim().to_string();
            sections.entry(name.clone()).or_default();
            current = Some(name);
        } else if !line.trim().is_empty() {
            if let Some(cur) = &current {
                if let Some(set) = sections.get_mut(cur) {
                    set.insert(line.trim().to_string());
                }
            }
        }
    }
    sections
}

/// 真站：夸克分享的 **wire 字段形状**快照（token 与目录两段）。
///
/// 为什么需要它：`ShareToken`/`ShareListPage`/`QuarkFile` 的字段全部带 `#[serde(default)]`，
/// 上游若把 `stoken` 改名为 `share_token`，typed 解析只会得到空串、不报错；家族 mock 与
/// 「字段非空」的真站断言都可能因为别处仍可用而漏过。形状快照比对「线上出现过哪些路径与类型」，
/// 缺失/新增都 fatal，改基线必须显式设 `UPDATE_PAN_SHAPE=1`（避免上游漂移被顺手覆盖）。
#[test]
#[ignore = "真站：依赖 pan.quark.cn 匿名分享与字段形状；上游变化时需人工复核后更新基线"]
fn quark_wire_shape_matches_snapshot() {
    let link = parse_share_link("https://pan.quark.cn/s/24f565ff8b77").expect("分享链接解析");
    let client = QuarkClient::with_rules(&rules(), "").expect("构建匿名客户端");
    let token = client
        .share_token_raw(&link.share_id, "")
        .expect("token 请求失败（上游契约可能变化）");
    let stoken = token
        .get("stoken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !stoken.is_empty(),
        "token 响应缺少 data.stoken（字段可能改名）：{token}"
    );
    let page = client
        .list_page_raw(&link.share_id, &stoken, "0", 1)
        .expect("目录请求失败（上游契约可能变化）");

    let mut latest: BTreeMap<String, Vec<String>> = BTreeMap::new();
    latest.insert("share_token".into(), shape_lines(&token));
    latest.insert("list_page".into(), shape_lines(&page));

    let path = shape_fixture_path();
    if std::env::var("UPDATE_PAN_SHAPE").is_ok() {
        let mut text = String::new();
        for (name, lines) in &latest {
            text.push_str(&format!("## {name}\n"));
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("创建 fixtures 目录");
        }
        std::fs::write(&path, text).expect("写入形状基线");
        eprintln!("已更新基线：{}", path.display());
        return;
    }

    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "读取基线 {} 失败：{e}（首次生成请设 UPDATE_PAN_SHAPE=1）",
            path.display()
        )
    });
    let baseline = read_shape_sections(&text);
    for (name, lines) in &latest {
        let base = baseline.get(name).unwrap_or_else(|| {
            panic!("基线缺少段 `{name}`，请设 UPDATE_PAN_SHAPE=1 重新生成")
        });
        // `base` 是集合，`lines` 是本轮快照（已排序），统一转集合后双向比对。
        let latest_set: BTreeSet<String> = lines.iter().cloned().collect();
        let missing: Vec<&String> = base.difference(&latest_set).collect();
        let extra: Vec<&String> = latest_set.difference(base).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "夸克 `{name}` 字段形状与基线不一致（上游契约可能已变化）：\n             缺少（基线有、线上无）：{missing:?}\n             新增（线上有、基线无）：{extra:?}\n             确认上游变更后执行：UPDATE_PAN_SHAPE=1 cargo test -p vbox_core --test catvod_pan_engine_live quark_wire_shape_matches_snapshot -- --ignored"
        );
    }
}
