//! 直播与 EPG 集成测试（不访问真实网络）
//!
//! 覆盖参考实现里的关键行为：
//! - txt：`组名,#genre#`、`频道,url1#url2`、`url$源名`、同名频道合并；
//! - 密码分组用 `_` 分隔（不是 `#`）；
//! - m3u：`#EXTINF` 的 `group-title`/`tvg-*`、`#EXTHTTP:`/`#EXTVLCOPT:`/`#KODIPROP:`
//!   设置行、行内 `url|header&...`、`x-tvg-url`；
//! - JSON 数组形态（`group`/`name`、`channels`/`channel`）；
//! - `proxy://...&ext=<base64 URL_SAFE|NO_WRAP>` 解码；
//! - `load_live_source` 的 type0（file:// 本地文本，走完整加载链路）与 type3 明确报错；
//! - EPG XMLTV 内联解析与 `{name}/{date}` 模板渲染。

use vbox_core::epg::{parse_epg_xml, render_epg_url};
use vbox_core::live::{
    extract_m3u_epg, load_live_source, parse_txt_or_m3u, resolve_live_address, LiveGroup,
};
use vbox_core::model::{LiveSource, TvConfig};

fn find_group<'a>(groups: &'a [LiveGroup], name: &str) -> &'a LiveGroup {
    groups
        .iter()
        .find(|group| group.name == name)
        .unwrap_or_else(|| {
            panic!(
                "未找到分组 {name}: {:?}",
                groups.iter().map(|g| &g.name).collect::<Vec<_>>()
            )
        })
}

/* ============================================================
 * txt
 * ============================================================ */

#[test]
fn txt_groups_channels_and_multi_source() {
    let text = concat!(
        "央视频道,#genre#\n",
        "#EXTHTTP:{\"User-Agent\":\"okhttp/3.8.0\",\"Referer\":\"http://ref\"}\n",
        "CCTV-1 综合,http://a/cctv1.m3u8#http://b/cctv1.m3u8$备用源\n",
        "CCTV-2 财经,http://a/cctv2.m3u8\n",
        // 同名频道：合并 urls（重复地址去重）
        "CCTV-2 财经,http://a/cctv2.m3u8#http://c/cctv2.m3u8\n",
        "卫视频道,#genre#\n",
        "湖南卫视,rtmp://a/hunan\n",
        "浙江卫视,rtsp://a/zhejiang\n",
    );

    let groups = parse_txt_or_m3u(text, "inline").unwrap();
    assert_eq!(groups.len(), 2, "应解析出两个 #genre# 分组");
    assert_eq!(groups[0].name, "央视频道");
    assert_eq!(groups[0].key, "央视频道");
    assert!(groups[0].password.is_empty());
    assert_eq!(groups[0].channels.len(), 2);
    assert_eq!(groups[1].name, "卫视频道");
    assert_eq!(groups[1].channels.len(), 2);

    let cctv1 = &groups[0].channels[0];
    assert_eq!(cctv1.name, "CCTV-1 综合");
    assert_eq!(cctv1.urls.len(), 2);
    assert_eq!(cctv1.urls[0].url, "http://a/cctv1.m3u8");
    assert_eq!(cctv1.urls[0].name, "源1");
    assert_eq!(cctv1.urls[1].url, "http://b/cctv1.m3u8");
    assert_eq!(cctv1.urls[1].name, "备用源");
    assert_eq!(cctv1.group_key, "央视频道");
    // 设置行只作用于紧随其后的频道
    assert_eq!(
        cctv1.header.get("User-Agent").map(String::as_str),
        Some("okhttp/3.8.0")
    );
    assert_eq!(
        cctv1.header.get("Referer").map(String::as_str),
        Some("http://ref")
    );

    let cctv2 = &groups[0].channels[1];
    assert_eq!(cctv2.urls.len(), 2, "同名频道应合并并去重");
    assert_eq!(cctv2.urls[0].url, "http://a/cctv2.m3u8");
    assert_eq!(cctv2.urls[1].url, "http://c/cctv2.m3u8");
    assert_eq!(cctv2.urls[1].name, "源2");
    assert!(cctv2.header.is_empty(), "设置只应作用于下一个频道");
}

#[test]
fn txt_password_group_uses_underscore() {
    let text = concat!(
        "福利_1234,#genre#\n",
        "加密频道,http://a/1\n",
        "公开组,#genre#\n",
        "公开频道,http://b/1\n",
    );
    let groups = parse_txt_or_m3u(text, "inline").unwrap();
    assert_eq!(groups.len(), 2);

    let locked = find_group(&groups, "福利");
    assert_eq!(locked.key, "福利_1234");
    assert_eq!(locked.password, "1234");
    assert!(locked.is_locked());
    assert_eq!(locked.channels[0].group_key, "福利_1234");

    let open = find_group(&groups, "公开组");
    assert_eq!(open.key, "公开组");
    assert!(open.password.is_empty());
}

#[test]
fn txt_channel_before_genre_uses_default_group() {
    let text = "CCTV-1,http://a/1\n地方台,#genre#\nCCTV-2,http://a/2\n";
    let groups = parse_txt_or_m3u(text, "inline").unwrap();
    // 第一个频道出现在任何 #genre# 之前 → 落到默认组“直播”
    let default_group = find_group(&groups, "直播");
    assert_eq!(default_group.channels.len(), 1);
    assert_eq!(default_group.channels[0].name, "CCTV-1");
    let local = find_group(&groups, "地方台");
    assert_eq!(local.channels.len(), 1);
}

/* ============================================================
 * m3u
 * ============================================================ */

#[test]
fn m3u_attrs_settings_and_url_headers() {
    let text = concat!(
        "#EXTM3U x-tvg-url=\"http://epg.example.com/all.xml\"\n",
        "#EXTVLCOPT:http-referrer=http://ref.example\n",
        "#EXTINF:-1 tvg-id=\"cctv1\" tvg-name=\"CCTV1\" tvg-logo=\"http://logo/1.png\" ",
        "group-title=\"央视\" http-user-agent=\"okhttp/3.15\",CCTV-1 综合\n",
        "http://a/cctv1.m3u8|Referer=http://h5&User-Agent=Mozilla\n",
        "#EXTINF:-1 group-title=\"央视\" catchup=\"append\" catchup-source=\"?playseek={utc}\" ",
        "catchup-replace=\"0\",CCTV-2 财经\n",
        "#KODIPROP:manifest_type=hls\n",
        "http://a/cctv2.m3u8\n",
        // 同名频道再次出现 → 合并 urls
        "#EXTINF:-1 group-title=\"央视\",CCTV-1 综合\n",
        "http://b/cctv1.m3u8\n",
        // 没有 group-title 的频道落到默认组
        "#EXTINF:-1,CCTV-5 体育\n",
        "http://a/cctv5.m3u8\n",
    );

    assert_eq!(
        extract_m3u_epg(text).as_deref(),
        Some("http://epg.example.com/all.xml"),
        "应能提取 x-tvg-url"
    );

    let groups = parse_txt_or_m3u(text, "inline").unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "央视");
    assert_eq!(
        groups[0].epg, "http://epg.example.com/all.xml",
        "m3u 头 x-tvg-url 应挂到组级 epg"
    );
    assert_eq!(groups[0].channels.len(), 2);

    let cctv1 = &groups[0].channels[0];
    assert_eq!(cctv1.name, "CCTV-1 综合");
    assert_eq!(cctv1.logo, "http://logo/1.png");
    assert_eq!(cctv1.epg, "cctv1");
    assert_eq!(cctv1.ua, "okhttp/3.15");
    assert_eq!(cctv1.referer, "http://ref.example");
    assert_eq!(
        cctv1.header.get("Referer").map(String::as_str),
        Some("http://h5")
    );
    assert_eq!(
        cctv1.header.get("User-Agent").map(String::as_str),
        Some("Mozilla")
    );
    assert_eq!(cctv1.urls.len(), 2);
    assert_eq!(cctv1.urls[0].url, "http://a/cctv1.m3u8");
    assert_eq!(cctv1.urls[1].url, "http://b/cctv1.m3u8");

    let cctv2 = &groups[0].channels[1];
    assert_eq!(cctv2.format, "hls", "#KODIPROP manifest_type 应写入 format");
    let catchup = cctv2.catchup.as_ref().expect("catchup 属性应解析");
    assert_eq!(catchup.get("type").and_then(|v| v.as_str()), Some("append"));
    assert_eq!(
        catchup.get("source").and_then(|v| v.as_str()),
        Some("?playseek={utc}")
    );

    let default_group = find_group(&groups, "直播");
    assert_eq!(default_group.channels.len(), 1);
    assert_eq!(default_group.channels[0].name, "CCTV-5 体育");
}

#[test]
fn m3u_one_extinf_with_multiple_address_lines() {
    let text = concat!(
        "#EXTM3U\n",
        "#EXTINF:-1 group-title=\"央视\",CCTV-1\n",
        "http://a/1\n",
        "http://b/2\n",
    );
    let groups = parse_txt_or_m3u(text, "inline").unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].channels.len(), 1);
    assert_eq!(groups[0].channels[0].urls.len(), 2);
}

/* ============================================================
 * JSON
 * ============================================================ */

#[test]
fn json_groups_api() {
    let text = r#"[
        {"group": "央视", "channels": [
            {"name": "CCTV-1", "urls": ["http://a/1", "http://b/1$备用"], "logo": "http://logo",
             "epg": "cctv1", "ua": "ua1", "header": {"Referer": "http://ref"},
             "format": "hls", "parse": "1", "click": "js",
             "catchup": {"type": "default"}},
            {"name": "CCTV-1", "urls": ["http://c/3"]}
        ]},
        {"name": "卫视", "channel": [{"name": "湖南卫视", "url": "rtmp://h/1"}]}
    ]"#;

    let groups = parse_txt_or_m3u(text, "inline").unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "央视");
    assert_eq!(groups[0].channels.len(), 1, "同名频道应合并");

    let cctv1 = &groups[0].channels[0];
    assert_eq!(cctv1.urls.len(), 3);
    assert_eq!(cctv1.urls[1].name, "备用");
    assert_eq!(cctv1.urls[2].url, "http://c/3");
    assert_eq!(cctv1.urls[2].name, "源3");
    assert_eq!(cctv1.logo, "http://logo");
    assert_eq!(cctv1.epg, "cctv1");
    assert_eq!(cctv1.ua, "ua1");
    assert_eq!(cctv1.parse, 1);
    assert_eq!(cctv1.click, "js");
    assert!(cctv1.catchup.is_some());

    // `name` 作为组名、`channel` 作为 channels 别名、单个 `url` 作为 urls 的容错
    assert_eq!(groups[1].name, "卫视");
    assert_eq!(groups[1].channels[0].name, "湖南卫视");
    assert_eq!(groups[1].channels[0].urls[0].url, "rtmp://h/1");
}

#[test]
fn json_serializes_with_camel_case_for_frontend() {
    let groups = parse_txt_or_m3u("福利_1234,#genre#\nCCTV-1,http://a/1$高清\n", "inline").unwrap();
    let value = serde_json::to_value(&groups[0]).unwrap();
    assert_eq!(value["key"], "福利_1234");
    assert_eq!(value["name"], "福利");
    assert_eq!(value["password"], "1234");
    assert_eq!(value["channels"][0]["groupKey"], "福利_1234");
    assert_eq!(value["channels"][0]["urls"][0]["url"], "http://a/1");
    assert_eq!(value["channels"][0]["urls"][0]["name"], "高清");
    // 字段缺失时也保持稳定（camelCase + 默认值）
    assert_eq!(value["channels"][0]["parse"], 0);
    assert!(value["channels"][0]["catchup"].is_null());
}

/* ============================================================
 * proxy:// ext
 * ============================================================ */

#[test]
fn proxy_ext_base64_variants() {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
    use base64::Engine;

    let real = "https://example.com/live/list.txt?token=x";
    for encoded in [
        URL_SAFE.encode(real),
        URL_SAFE_NO_PAD.encode(real),
        STANDARD.encode(real),
    ] {
        let decoded =
            resolve_live_address(&format!("proxy://do=live&type=txt&ext={encoded}")).unwrap();
        assert_eq!(decoded, real);
    }

    // 已经替换成本地代理的形态
    let encoded = URL_SAFE_NO_PAD.encode(real);
    let decoded = resolve_live_address(&format!(
        "http://127.0.0.1:9978/proxy?do=live&type=txt&ext={encoded}"
    ))
    .unwrap();
    assert_eq!(decoded, real);

    // ext 里直接是 http 地址（参考实现允许）
    assert_eq!(
        resolve_live_address("proxy://do=live&type=txt&ext=http%3A%2F%2Fa%2Fb.txt").unwrap(),
        "http://a/b.txt"
    );

    // 普通 http 地址原样返回；clan:// 明确报错
    assert_eq!(
        resolve_live_address("https://a/b.txt").unwrap(),
        "https://a/b.txt"
    );
    assert!(resolve_live_address("clan://localhost/live.txt").is_err());
    assert!(resolve_live_address("proxy://do=live&type=txt&ext=%%%").is_err());
}

/* ============================================================
 * load_live_source
 * ============================================================ */

#[test]
fn load_type0_from_local_file() {
    let content = "本地组,#genre#\n测试频道,http://a/1#http://b/2$备\n";
    let path = std::env::temp_dir().join(format!(
        "vbox_live_source_{}_{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, content).expect("写临时直播源");
    let target = format!("file://{}", path.display().to_string().replace('\\', "/"));

    let cfg = TvConfig::default();
    let live = LiveSource {
        name: "本地".to_string(),
        kind: 0,
        url: target,
        ..Default::default()
    };
    let groups = load_live_source(&cfg, &live).expect("type0 应能加载本地文本");

    let _ = std::fs::remove_file(&path);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "本地组");
    assert_eq!(groups[0].channels[0].urls.len(), 2);
    assert_eq!(groups[0].channels[0].urls[1].name, "备");
}

#[test]
fn load_type0_requires_address() {
    let cfg = TvConfig::default();
    let live = LiveSource {
        name: "空".to_string(),
        kind: 0,
        ..Default::default()
    };
    let err = load_live_source(&cfg, &live).unwrap_err().to_string();
    assert!(err.contains("缺少 url/api"), "{err}");
}

#[test]
fn load_type3_js_uses_spider_live() {
    let cfg = TvConfig::default();
    // 内联 JS（js.rs 支持 inline:，测试不联网）：live(url) 返回直播文本
    let script = r#"function live(url){ return "央视,#genre#\nCCTV-1,http://a/1#http://b/2$备"; }"#;
    let js = LiveSource {
        name: "JS".to_string(),
        kind: 3,
        api: format!("inline:{script}"),
        ..Default::default()
    };
    let groups = load_live_source(&cfg, &js).expect("type3 .js 应走 spider::js::live()");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "央视");
    assert_eq!(groups[0].channels[0].name, "CCTV-1");
    assert_eq!(groups[0].channels[0].urls.len(), 2);
    assert_eq!(groups[0].channels[0].urls[1].name, "备");

    // 脚本没有 live() 时必须报错，不能静默返回空频道列表
    let no_live = LiveSource {
        name: "NoLive".to_string(),
        kind: 3,
        api: "inline:function home(){ return '{}'; }".to_string(),
        ..Default::default()
    };
    assert!(load_live_source(&cfg, &no_live).is_err());
}

#[test]
fn load_type3_py_is_wired_and_jar_is_explicitly_unsupported() {
    let cfg = TvConfig::default();

    // .py 现在会走 Python 运行时；不存在的文件应返回读取/脚本错误，而不是"不支持"
    let py = LiveSource {
        name: "PY".to_string(),
        kind: 3,
        api: "file:///definitely-missing-live.py".to_string(),
        ..Default::default()
    };
    let err = load_live_source(&cfg, &py).unwrap_err().to_string();
    assert!(!err.contains("桌面端不支持"), "{err}");

    let jar = LiveSource {
        name: "JAR".to_string(),
        kind: 3,
        api: "csp_Live".to_string(),
        ..Default::default()
    };
    let err = load_live_source(&cfg, &jar).unwrap_err().to_string();
    assert!(err.contains("jar") || err.contains("dex"), "{err}");

    let unknown = LiveSource {
        name: "T1".to_string(),
        kind: 1,
        url: "http://a".to_string(),
        ..Default::default()
    };
    assert!(load_live_source(&cfg, &unknown).is_err());
}

/* ============================================================
 * EPG（XMLTV）
 * ============================================================ */

#[test]
fn epg_xmltv_inline() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <programme start="20261028120000 +0800" stop="20261028130000 +0800" channel="cctv1">
    <title lang="zh">新闻联播 &amp; 天气预报</title>
    <title lang="en">News</title>
  </programme>
  <programme start="20261028130000 +0800" stop="20261028140000 +0800" channel="cctv2">
    <title><![CDATA[电视剧 <A>]]></title>
  </programme>
  <programme start="20261028140000 +0800" stop="20261028150000 +0800" channel="empty"/>
</tv>"#;
    let programs = parse_epg_xml(xml).unwrap();
    assert_eq!(programs.len(), 2, "无 title 的空节目应跳过");
    assert_eq!(programs[0].channel, "cctv1");
    assert_eq!(programs[0].title, "新闻联播 & 天气预报");
    assert_eq!(programs[0].start, "20261028120000 +0800");
    assert_eq!(programs[0].end, "20261028130000 +0800");
    assert_eq!(programs[1].title, "电视剧 <A>");
}

#[test]
fn epg_url_template_rendering() {
    let url = render_epg_url(
        "http://epg.example.com/api?ch={name}&date={date}",
        "CCTV-1 综合",
        "2026-10-28",
    );
    assert_eq!(
        url,
        "http://epg.example.com/api?ch=CCTV-1%20%E7%BB%BC%E5%90%88&date=2026-10-28"
    );
    // 没有占位符时原样返回
    assert_eq!(
        render_epg_url("http://epg.example.com/all.xml", "CCTV1", "2026-10-28"),
        "http://epg.example.com/all.xml"
    );
}
