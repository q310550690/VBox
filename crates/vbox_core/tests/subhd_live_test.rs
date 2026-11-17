//! 在线字幕（SubHD）联网验证：默认 `#[ignore]`，需要外网时手动跑：
//!
//! ```text
//! set VBOX_TEST_SUBHD=1
//! cargo test -p vbox_core --test subhd_live_test -- --ignored --nocapture
//! ```
//!
//! 验证内容：搜索「沙丘」→ 取一条 ASS/SRT 结果 → 解析真实下载直链 → 落盘。
//! 站点域名/反爬经常变化，这条测试只作为人工冒烟，不进默认测试集。

use std::time::Duration;

#[test]
#[ignore = "需要联网访问 SubHD（设置 VBOX_TEST_SUBHD=1 后手动运行）"]
fn subhd_search_and_download_real_subtitle() {
    if std::env::var("VBOX_TEST_SUBHD").is_err() {
        eprintln!("skip: 未设置 VBOX_TEST_SUBHD=1");
        return;
    }

    let items = vbox_core::subtitle_online::search("subhd", "沙丘", 1).expect("搜索 SubHD 失败");
    assert!(!items.is_empty(), "搜索「沙丘」没有结果");
    eprintln!("搜索到 {} 条结果，第一条: {} / {}", items.len(), items[0].title, items[0].name);

    // 优先选文本字幕（ASS/SRT），避免 SUP 这类二进制位图字幕干扰断言；
    // 前几条直链可能是 .rar 字幕包，这里只验证「能解析出直链并下到文件」。
    let candidates: Vec<_> = items
        .iter()
        .filter(|item| item.format == "ASS" || item.format == "SRT")
        .take(4)
        .collect();
    assert!(!candidates.is_empty(), "没有 ASS/SRT 结果");

    let mut resolved: Option<(&str, String)> = None;
    for item in &candidates {
        eprintln!("尝试: id={} format={} size={}", item.id, item.format, item.size);
        match vbox_core::subtitle_online::resolve_download("subhd", &item.id) {
            Ok(url) => {
                eprintln!("直链: {url}");
                resolved = Some((item.id.as_str(), url));
                break;
            }
            Err(err) => eprintln!("解析失败（换下一条）: {err}"),
        }
    }
    let (id, url) = resolved.expect("所有候选都解析不出下载直链");
    assert!(url.starts_with("http"), "直链格式异常: {url}");

    let ext = url
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "bin".to_string());
    let dest = std::env::temp_dir().join(format!(
        "vbox-subhd-live-{}-{}.{}",
        std::process::id(),
        id,
        ext
    ));
    let _ = std::fs::remove_file(&dest);
    vbox_core::subtitle_online::download_to(&url, &dest).expect("下载字幕文件失败");
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    eprintln!("已下载 {} 字节 -> {}", size, dest.display());
    assert!(size > 100, "下载的字幕文件过小: {size}");

    // 字幕文件应该是文本（ASS/SRT）或二进制（SUP），至少不能是 HTML 错误页
    let head = std::fs::read(&dest).expect("读取字幕文件失败");
    let is_html = head.starts_with(b"<!DOCTYPE") || head.starts_with(b"<html");
    assert!(!is_html, "下载到的是网页而不是字幕文件");

    std::thread::sleep(Duration::from_millis(10));
    let _ = std::fs::remove_file(&dest);
}
