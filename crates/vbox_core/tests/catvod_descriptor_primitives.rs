//! 描述引擎原语单测：schema v1 白名单指令逐条覆盖（计划 §13.1-5）。
//!
//! `catvod_descriptor_engine.rs` 证明的是「三份真实描述能跑通」；本文件证明的是
//! 「引擎把每条声明式指令都实现成了手写语义」——用可控的 mock 服务器（回显 URL /
//! 回显请求头 / 固定 JSON、HTML / 指定状态码）逐项断言，不经过 registry 与分发。

use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::descriptor::{engine, schema};

const SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/* ============================================================
 * mock 服务器
 * ============================================================ */

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时钟")
        .as_secs()
}

fn header_of(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().trim().to_string())
}

/// 起一个 mock 服务器，返回 `http://127.0.0.1:port`；处理函数返回 (body, content-type)。
fn serve<F>(handler: F) -> String
where
    F: Fn(&tiny_http::Request) -> (String, &'static str) + Send + 'static,
{
    let server = Server::http("127.0.0.1:0").expect("mock 服务器");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let (body, content_type) = handler(&request);
            let header = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
                .expect("Content-Type 头");
            let _ = request.respond(Response::from_string(body).with_header(header));
        }
    });
    base
}

/// 回显服务器：`name` / `id` / `data[].id` 都是本次请求的 path+query；
/// `headers` 回显若干请求头，用于断言描述里的 headers 真的发出去了。
fn echo_server() -> String {
    serve(|request| {
        let url = request.url().to_string();
        let mut headers = serde_json::Map::new();
        for name in ["user-agent", "x-token", "x-hash"] {
            if let Some(value) = header_of(request, name) {
                headers.insert(name.to_string(), Value::String(value));
            }
        }
        let body = json!({
            "name": url,
            "id": url,
            "data": [{
                "id": url,
                "name": url,
                "type_id": format!("{url}#id"),
                "type_name": format!("{url}#name")
            }],
            "headers": Value::Object(headers),
            "vod": {"url": "vod-url"}
        });
        (body.to_string(), "application/json")
    })
}

fn fixed_server(body: &'static str, content_type: &'static str) -> String {
    serve(move |_| (body.to_string(), content_type))
}

/* ============================================================
 * 描述构造
 * ============================================================ */

fn site() -> Site {
    Site {
        api: "csp_Test".to_string(),
        ..Site::default()
    }
}

fn site_ext(ext: &str) -> Site {
    Site {
        api: "csp_Test".to_string(),
        ext: Some(Value::String(ext.to_string())),
        ..Site::default()
    }
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn parse(value: Value) -> schema::Descriptor {
    match schema::parse(&value.to_string()) {
        Ok(desc) => desc,
        Err(err) => panic!("描述应能解析: {err}\n{value}"),
    }
}

fn descriptor_value(kind: &str, base: &str) -> Value {
    json!({
        "schema": 1,
        "api": "csp_Test",
        "kind": kind,
        "class": "com.github.catvod.spider.Test",
        "jar": "test.jar",
        "jar_sha256": SHA,
        "evidence": [{
            "method": "homeContent",
            "file": "_rea/decompiled/test/deobf/com/github/catvod/spider/Test.java",
            "lines": "1-9"
        }],
        "base": {"default": base},
        "endpoints": {
            "categories": {"path": "/cat", "rows": ["data[]"], "fields": {"id": "id", "name": "name"}},
            "list": {"path": "/list", "rows": ["data[]"], "fields": {"id": "id", "name": "name"}},
            "detail": {"path": "/detail", "fields": {"name": "name"}},
            "play": {"parse": 1, "url": "{id}"}
        }
    })
}

/* ============================================================
 * 1. base / vars / params 编码
 * ============================================================ */

#[test]
fn base_ext_token_vars_and_param_encoding() {
    let echo = echo_server();
    let mut value = descriptor_value("json", "https://default.example");
    value["base"]["ext"] = json!("token");
    value["vars"] = json!([
        {"name": "version", "literal": "V123"},
        {"name": "hash", "apply": "md5_hex", "template": "null{version}", "slice": [8, 12]},
        {"name": "ts", "apply": "unix_ts"},
        {"name": "px", "from_ext": "proxy=", "default": "https://fallback.example"},
        {"name": "fb", "literal": "", "default": "FB"}
    ]);
    value["endpoints"]["categories"]["params"] = json!([
        {"key": "a", "value": "{version}"},
        {"key": "h", "value": "{hash}"},
        {"key": "t", "value": "{ts}"},
        {"key": "p", "value": "{px}", "encode": "url"},
        {"key": "f", "value": "{fb}"},
        {"key": "b", "value": "{base}", "encode": "url"},
        {"key": "u", "value": "a b/c", "encode": "url"},
        {"key": "n", "value": "a b/c", "encode": "alnum"}
    ]);
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "validate: {:?}", desc.validate());

    // 第一段 http token 覆盖基址；`proxy=` 段被 vars.from_ext 取到。
    let ext = format!("junk|{echo}|proxy=https://px.example");
    let categories = engine::categories(&desc, &cfg(), &site_ext(&ext)).expect("分类请求");
    let url = categories[0].type_id.clone();

    assert!(
        url.starts_with("/cat?a=V123&h=90d8&t="),
        "md5(\"nullV123\")[8..12] 应为 90d8；实际 {url}"
    );
    let rest = url.trim_start_matches("/cat?a=V123&h=90d8&t=");
    let (ts, rest) = rest.split_once("&p=").expect("时间戳参数后应有 p");
    assert!(
        rest.starts_with("https%3A%2F%2Fpx.example&f=FB&b=http%3A%2F%2F127"),
        "url 编码与默认值；实际 {url}"
    );
    assert!(url.ends_with("&u=a%20b%2Fc&n=a%20b%2Fc"), "编码口径；实际 {url}");
    let secs: u64 = ts.parse().expect("unix_ts 应为秒级整数");
    assert!(
        now_secs().abs_diff(secs) < 300,
        "unix_ts 偏移过大: {secs} vs {}",
        now_secs()
    );
}

/* ============================================================
 * 2. segments（if_var / if_gt）与 page_min
 * ============================================================ */

#[test]
fn segments_if_var_if_gt_and_page_min() {
    let echo = echo_server();
    let mut value = descriptor_value("json", &echo);
    value["endpoints"]["list"] = json!({
        "path": "/list",
        "page_min": 1,
        "segments": [
            {"template": "/c/{cat}", "if_var": "cat"},
            {"template": "/tail"},
            {"template": "/p{page}", "if_gt": {"var": "page", "value": 1}}
        ],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let desc = parse(value);
    let s = site();

    let items = engine::videos(&desc, &cfg(), &s, "", 0).expect("列表");
    assert_eq!(items[0].id, "/list/tail", "空分类 + page=0 走 page_min");

    let items = engine::videos(&desc, &cfg(), &s, "电影", 2).expect("列表");
    assert_eq!(
        items[0].id, "/list/c/%E7%94%B5%E5%BD%B1/tail/p2",
        "if_var 与 if_gt 同时生效"
    );

    let items = engine::videos(&desc, &cfg(), &s, "x", 1).expect("列表");
    assert_eq!(items[0].id, "/list/c/x/tail", "page=1 不追加分页段");
}

/* ============================================================
 * 3. detail 的 URL 规则优先级
 * ============================================================ */

#[test]
fn detail_url_rule_precedence() {
    let echo = echo_server();
    let mut value = descriptor_value("json", &echo);
    value["endpoints"]["detail"] = json!({
        "path": "/d",
        "segments": [{"template": "/{id}"}],
        "rules": [
            {"suffixes": [".html"], "url": "{base}/s/{id}"},
            {"contains": "cid/", "url": "{base}/c/{id}"},
            {"contains_ignore_case": "HTTP", "url": "{base}/u/{id}"},
            {"prefixes": ["pre-"], "url": "{base}/p/{id}"}
        ],
        "fields": {"name": "name"}
    });
    let desc = parse(value);
    let s = site();
    let name = |id: &str| engine::detail(&desc, &cfg(), &s, id).expect("详情").name;

    assert_eq!(name("a.html"), "/s/a.html");
    assert_eq!(name("x/cid/y"), "/c/x/cid/y");
    assert_eq!(name("HTTP-1"), "/u/HTTP-1");
    assert_eq!(name("pre-9"), "/p/pre-9");
    assert_eq!(name("plain"), "/d/plain", "未命中规则回退 path+segments");
    // 规则按声明顺序短路：后缀规则排在 contains 之前。
    assert_eq!(name("x/cid/y.html"), "/s/x/cid/y.html");
}

/* ============================================================
 * 4. JSON 字段后处理链
 * ============================================================ */

#[test]
fn list_field_pipeline() {
    let base = fixed_server(
        r#"{"data":[
            {"id":"pre-abc","name":"  a   b  ","pic":"u/x.jpg","seg":"a/b/c"},
            {"id":"pre-dup","name":"dup","pic":"","seg":"a//"},
            {"id":"pre-empty","name":"","pic":"p.jpg","seg":""}
        ]}"#,
        "application/json",
    );
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["list"] = json!({
        "path": "/list",
        "rows": ["data[]"],
        "fields": {
            "id": {"path": "id", "strip_prefix": "pre-", "suffix": "/s"},
            "name": {"path": "name", "collapse": true, "keep_empty": true},
            "cover": {"path": "pic", "join_base": true, "default": "{base}/fallback.jpg"},
            "remark": {"path": "seg", "segment": 1, "tail_non_empty": true, "non_empty": true}
        }
    });
    let desc = parse(value);
    let items = engine::videos(&desc, &cfg(), &site(), "", 1).expect("列表");
    assert_eq!(items.len(), 3, "keep_empty 的行不丢");
    assert_eq!(items[0].id, "abc/s");
    assert_eq!(items[0].name, "a b", "collapse 折叠空白");
    assert_eq!(
        items[0].cover.as_deref(),
        Some(format!("{base}/u/x.jpg").as_str())
    );
    assert_eq!(items[0].remark.as_deref(), Some("b"), "segment=1");
    assert_eq!(
        items[1].cover.as_deref(),
        Some(format!("{base}/fallback.jpg").as_str()),
        "空值走 default"
    );
    assert_eq!(
        items[1].remark.as_deref(),
        Some("a"),
        "tail_non_empty 回退最后一个非空段"
    );
    assert_eq!(items[2].name, "", "keep_empty 保留空串");
    assert_eq!(items[2].remark, None, "non_empty 空值返回 None");
}

/* ============================================================
 * 5. 详情 JSON：desc_parts / percent_decode / replace / prefix
 * ============================================================ */

#[test]
fn detail_field_pipeline_json() {
    let base = fixed_server(
        r#"{"data":{
            "name":"N_END",
            "y":"",
            "ar":"%E6%97%A5%E6%9C%AC",
            "d":"a@b",
            "actors":"x/y/z",
            "content":"  第一行\n\n第二行  ",
            "note":"补",
            "pic":"/p.jpg"
        }}"#,
        "application/json",
    );
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["detail"] = json!({
        "path": "/detail",
        "root": "data",
        "fields": {
            "name": {"path": "name", "strip_suffix": "_END", "prefix": "P:", "suffix": ":S"},
            "cover": {"path": "pic", "join_base": true},
            "year": {"path": "y", "trim": true, "default": "未知"},
            "area": {"path": "ar", "percent_decode": true},
            "director": {"path": "d", "replace": {"@": "-"}},
            "actor": {"path": "actors", "segment": 0}
        },
        "desc_parts": [
            {"label": "简介：", "from": "content", "collapse": true},
            {"from": "note"}
        ],
        "require": [{"target": "name", "error": "缺名称 {id}"}]
    });
    let desc = parse(value);
    let detail = engine::detail(&desc, &cfg(), &site(), "77").expect("详情");
    assert_eq!(detail.name, "P:N:S");
    assert_eq!(
        detail.cover.as_deref(),
        Some(format!("{base}/p.jpg").as_str())
    );
    assert_eq!(detail.year.as_deref(), Some("未知"), "空值走 default");
    assert_eq!(detail.area.as_deref(), Some("日本"), "percent_decode");
    assert_eq!(detail.director.as_deref(), Some("a-b"), "replace");
    assert_eq!(detail.actor.as_deref(), Some("x"), "segment=0");
    assert_eq!(detail.desc.as_deref(), Some("简介：第一行 第二行\n补"));
}

/* ============================================================
 * 6. HTML：字段 / meta / 分组（MacCMS 形态）
 * ============================================================ */

const YSJ_LIKE_HTML: &str = r#"<html><body>
<div id="nav">
  <a href="/vod/type/1.html"><span>电影</span>BD</a>
  <a href="/vod/type/2.html">剧集</a>
  <a href="">空</a>
</div>
<ul class="list">
  <li class="item"><a class="thumb" href="/vod/detail/id/88.html" title="标题A" data-original="/img/a.jpg"><span class="pic_text">更新&#xE001;至12集</span></a></li>
  <li class="item"><a class="thumb" href="/vod/detail/id/99.html" data-original="/img/b.jpg"><span class="pic_text">完结</span></a></li>
  <li class="item"><span>没有链接</span></li>
</ul>
<h2 class="title">标题A&#xE000;</h2>
<a class="thumb" href="/vod/detail/id/88.html" data-original="/img/a.jpg"></a>
<div class="content_desc"><span>简介文本</span></div>
<ul class="meta">
  <li class="data"><span>年份：</span><a href="/vod/search/year/2024.html">2024</a><span>地区：</span><a href="/vod/search/area/dl.html">大陆</a></li>
  <li class="data"><a href="/vod/search/actor/zs.html">张三</a></li>
  <li class="data">主演：王五</li>
  <li class="data">导演：李四</li>
</ul>
<div class="play_source_tab"><a>线路一</a><a>线路二</a></div>
<ul class="content_playlist"><li><a href="/vod/play/id/88-1.html">第1集</a></li></ul>
<ul class="content_playlist"><li><a href="/vod/play/id/88-2.html">第2集</a></li></ul>
</body></html>"#;

#[test]
fn html_categories_list_and_detail() {
    let base = fixed_server(YSJ_LIKE_HTML, "text/html; charset=utf-8");
    let mut value = descriptor_value("html", &base);
    value["endpoints"]["categories"] = json!({
        "path": "/vod/show/id/20.html",
        "rows": ["div#nav a"],
        "fields": {
            "id": {"self": true, "attr": "href", "segment": 3, "strip_suffix": ".html", "non_empty": true},
            "name": {"self": true, "clean_private_use": true, "replace": {"BD": ""}, "trim": true, "non_empty": true}
        }
    });
    value["endpoints"]["list"] = json!({
        "path": "/vod/show",
        "rows": ["li.item"],
        "fields": {
            "id": {"sel": ["a.thumb"], "attr": "href", "segment": 4, "strip_suffix": ".html", "non_empty": true},
            "name": {"sel": ["a.thumb"], "attr": "title", "non_empty": true,
                     "fallback": {"sel": ["a.thumb"], "non_empty": true}},
            "cover": {"sel": ["a.thumb"], "attr": "data-original", "join_base": true},
            "remark": {"sel": ["span.pic_text"], "clean_private_use": true}
        }
    });
    value["endpoints"]["detail"] = json!({
        "path": "/vod/detail/id/",
        "segments": [{"template": "{id}"}, {"template": ".html"}],
        "fields": {
            "name": {"sel": ["h2.title"], "clean_private_use": true},
            "cover": {"sel": ["a.thumb"], "attr": "data-original", "join_base": true},
            "desc": {"sel": ["div.content_desc > span"]}
        },
        "meta_scope": "li.data",
        "meta_anchor": "a",
        "meta": [
            {"field": "year", "href_contains": "/vod/search/year/"},
            {"field": "area", "href_contains": "/vod/search/area/"},
            {"field": "actor", "href_contains": "/vod/search/actor/", "first_only": true},
            {"field": "actor", "label": "主演", "cut": "/\n"},
            {"field": "director", "label": "导演", "cut": "/\n"}
        ],
        "groups": {
            "tabs": "div.play_source_tab a",
            "lists": "ul.content_playlist",
            "item": "a",
            "url_attr": "href",
            "url_strip_prefix": "/vod/play/id/",
            "name_fallback": "线路{index}"
        }
    });
    let desc = parse(value);
    let s = site();

    let categories = engine::categories(&desc, &cfg(), &s).expect("分类");
    let pairs: Vec<(String, String)> = categories
        .iter()
        .map(|c| (c.type_id.clone(), c.type_name.clone()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("1".to_string(), "电影".to_string()),
            ("2".to_string(), "剧集".to_string())
        ],
        "self 字段 + segment + strip_suffix + clean_private_use；空 href 行丢弃"
    );

    let items = engine::videos(&desc, &cfg(), &s, "", 1).expect("列表");
    assert_eq!(items.len(), 2, "无链接行被丢弃");
    assert_eq!(items[0].id, "88");
    assert_eq!(items[0].name, "标题A", "title 属性优先");
    assert_eq!(
        items[0].cover.as_deref(),
        Some(format!("{base}/img/a.jpg").as_str())
    );
    assert_eq!(items[0].remark.as_deref(), Some("更新至12集"), "私用区字符被过滤");
    assert_eq!(items[1].id, "99");
    assert_eq!(items[1].name, "完结", "标题缺失走 fallback 文本");

    let detail = engine::detail(&desc, &cfg(), &s, "88").expect("详情");
    assert_eq!(detail.name, "标题A");
    assert_eq!(
        detail.cover.as_deref(),
        Some(format!("{base}/img/a.jpg").as_str())
    );
    assert_eq!(detail.desc.as_deref(), Some("简介文本"));
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.actor.as_deref(), Some("王五"), "标签覆盖锚点");
    assert_eq!(detail.director.as_deref(), Some("李四"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items[0].title, "第1集");
    assert_eq!(detail.episodes[0].items[0].url, "88-1.html", "url_strip_prefix");
    assert_eq!(detail.episodes[1].group, "线路二");
}

/* ============================================================
 * 7. 行过滤与去重
 * ============================================================ */

#[test]
fn row_filters_and_dedupe() {
    let base = fixed_server(
        r#"{"data":[
            {"id":"a","name":"电影"},
            {"id":"b","name":"推荐"},
            {"id":"","name":"空id"},
            {"id":"a","name":"电影-重复"},
            {"id":"c","name":"剧集"},
            {"id":"d","name":"剧集-别名"}
        ]}"#,
        "application/json",
    );
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["list"]["filters"] = json!([
        {"field": "name", "equals": "推荐"},
        {"field": "id", "if_empty": true},
        {"field": "name", "contains": "剧集"}
    ]);
    let desc = parse(value.clone());
    let items = engine::videos(&desc, &cfg(), &site(), "", 1).expect("列表");
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["剧集", "剧集-别名"],
        "equals/if_empty 排除，contains 保留"
    );

    value["endpoints"]["categories"]["dedupe"] = json!(true);
    let desc = parse(value);
    let categories = engine::categories(&desc, &cfg(), &site()).expect("分类");
    let ids: Vec<&str> = categories.iter().map(|c| c.type_id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c", "d"], "categories.dedupe 按 id 去重（空 id 丢弃）");
    assert_eq!(
        categories[0].type_name, "电影",
        "重复项保留首次出现的名称"
    );
}

/* ============================================================
 * 8. 搜索：{kw} 编码与 empty_marker
 * ============================================================ */

#[test]
fn search_keyword_params_and_empty_marker() {
    let echo = echo_server();
    let mut value = descriptor_value("json", &echo);
    value["endpoints"]["search"] = json!({
        "path": "/s",
        "params": [{"key": "wd", "value": "{kw}", "encode": "alnum"}],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let desc = parse(value);
    let items = engine::search(&desc, &cfg(), &site(), "a b/c").expect("搜索");
    assert_eq!(items[0].name, "/s?wd=a%20b%2Fc", "search 变量编码");

    let html = r#"<html><body><div id="msg">很抱歉，暂无相关视频</div>
        <ul><li class="row"><a href="/vod/detail/id/1.html">不该出现</a></li></ul></body></html>"#;
    let empty_base = fixed_server(html, "text/html; charset=utf-8");
    let mut value = descriptor_value("html", &empty_base);
    value["endpoints"]["search"] = json!({
        "path": "/vod/search.html",
        "params": [{"key": "wd", "value": "{kw}", "encode": "alnum"}, {"key": "submit", "value": ""}],
        "empty_marker": "很抱歉，暂无相关视频",
        "rows": ["li.row"],
        "fields": {"id": {"sel": ["a"], "attr": "href"}, "name": {"sel": ["a"]}}
    });
    let desc = parse(value);
    let items = engine::search(&desc, &cfg(), &site(), "x").expect("搜索");
    assert!(items.is_empty(), "命中 empty_marker 返回空");
}

/* ============================================================
 * 9. on_error_empty 与 json_error
 * ============================================================ */

#[test]
fn on_error_empty_and_json_error_message() {
    // 404 不可重试，直接走 fetch 失败分支。
    let server = Server::http("127.0.0.1:0").expect("mock 服务器");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let _ = request.respond(Response::from_string("gone").with_status_code(404));
        }
    });

    let mut value = descriptor_value("json", &base);
    value["endpoints"]["categories"]["on_error_empty"] = json!(true);
    let desc = parse(value);
    let categories = engine::categories(&desc, &cfg(), &site()).expect("on_error_empty 应吞掉失败");
    assert!(categories.is_empty());

    let mut value = descriptor_value("json", &base);
    value["endpoints"]["categories"]["on_error_empty"] = json!(false);
    let desc = parse(value);
    let err = engine::categories(&desc, &cfg(), &site()).expect_err("未开启时应报错");
    assert!(err.to_string().contains("404"), "错误应含状态码: {err}");

    let bad = fixed_server("not-json", "application/json");
    let mut value = descriptor_value("json", &bad);
    value["json_error"] = json!("自定义 JSON 错误: {url}");
    let desc = parse(value);
    let err = engine::categories(&desc, &cfg(), &site()).expect_err("非 JSON 应报错");
    assert!(
        err.to_string().contains("自定义 JSON 错误: "),
        "json_error 模板未生效: {err}"
    );
}

/* ============================================================
 * 10. JSON 分组：name_fallback / items / title / url_prefix_if_group
 * ============================================================ */

#[test]
fn groups_json_shapes() {
    let base = fixed_server(
        r#"{"data":{
            "name":"详情标题",
            "play":[
                {"from":"线路A","list":[{"title":"第1集","url":"u1"},{"title":"","url":"u2"}]},
                {"from":"","list":[{"title":"第2集","url":"u3"}]}
            ]
        }}"#,
        "application/json",
    );
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["detail"] = json!({
        "path": "/detail",
        "root": "data",
        "fields": {"name": "name"},
        "groups": {
            "rows": ["play[]"],
            "name": "from",
            "name_fallback": "线路{index}",
            "items": "list[]",
            "title": "title",
            "url": "url",
            "title_fallback_url": true,
            "url_prefix_if_group": {"when": "线路A", "prefix": "A:"}
        }
    });
    let desc = parse(value);
    let detail = engine::detail(&desc, &cfg(), &site(), "1").expect("详情");
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路A");
    assert_eq!(detail.episodes[0].items[0].title, "第1集");
    assert_eq!(detail.episodes[0].items[0].url, "A:u1", "url_prefix_if_group");
    assert_eq!(
        detail.episodes[0].items[1].title, "A:u2",
        "标题为空回退 url（回退的是加前缀后的最终 url）"
    );
    assert_eq!(detail.episodes[1].group, "线路2", "name_fallback");
}

/* ============================================================
 * 11. single_group 与 require / empty_id_error
 * ============================================================ */

#[test]
fn single_group_detail_and_requirements() {
    let base = fixed_server(
        r#"{"data":{"name":"X","urls":[{"key":"k1","url":"v1"},{"key":"","url":"v2"}]}}"#,
        "application/json",
    );
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["detail"] = json!({
        "path": "/detail",
        "root": "data",
        "fields": {"name": {"path": "name", "non_empty": true}},
        "groups": {
            "rows": ["urls[]"],
            "single_group": "LiteApple",
            "title": "key",
            "url": "url",
            "title_fallback_url": true
        },
        "require": [
            {"target": "name", "error": "缺名称 {id}"},
            {"target": "episodes", "error": "缺播放地址 {id}"}
        ]
    });
    let desc = parse(value);
    let detail = engine::detail(&desc, &cfg(), &site(), "5").expect("详情");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "LiteApple");
    assert_eq!(detail.episodes[0].items[1].title, "v2", "标题为空回退 url");

    let empty = fixed_server(r#"{"data":{}}"#, "application/json");
    let mut value = descriptor_value("json", &empty);
    value["endpoints"]["detail"] = json!({
        "path": "/detail",
        "root": "data",
        "fields": {"name": {"path": "name", "non_empty": true}},
        "require": [{"target": "name", "error": "缺名称 {id}"}]
    });
    let desc = parse(value);
    let err = engine::detail(&desc, &cfg(), &site(), "abc").expect_err("应报 require");
    assert!(err.to_string().contains("缺名称 abc"), "{err}");

    // empty_id_error：trim_id 后为空直接报错，不发请求。
    let mut value = descriptor_value("json", "http://127.0.0.1:1");
    value["endpoints"]["detail"]["trim_id"] = json!(true);
    value["endpoints"]["detail"]["empty_id_error"] = json!("详情 id 为空");
    let desc = parse(value);
    let err = engine::detail(&desc, &cfg(), &site(), "   ").expect_err("空 id");
    assert!(err.to_string().contains("详情 id 为空"), "{err}");
}

/* ============================================================
 * 12. 播放规则：prefixes / contains / contains_ignore_case /
 *     suffixes / platforms / proxy
 * ============================================================ */

#[test]
fn play_rules_platforms_and_proxy() {
    let base = fixed_server(
        r#"{"data":{"url":"https://real.example/v.m3u8"}}"#,
        "application/json",
    );
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["play"] = json!({
        "parse": 1,
        "jx": true,
        "url": "{id}",
        "header": {"User-Agent": "UA0"},
        "rules": [
            {"prefixes": ["pre:"], "url": "{id}", "parse": 0, "header": {"User-Agent": "PRE"}},
            {"contains": "mark", "url": "{base}/m/{id}", "jx": false},
            {"contains_ignore_case": "UPPER", "url": "{base}/u/{id}"},
            {"suffixes": [".m3u8"], "url": "https://cdn.example/{id}", "parse": 0},
            {"platforms": [
                {"contains": "youku", "referer": "https://youku.example"},
                {"contains": "qq.com", "referer": "https://qq.example"}
             ],
             "url": "{id}", "parse": 0, "jx": false,
             "header": {"Referer": "{platform.referer}", "User-Agent": "CHROME"}}
        ],
        "proxy": {
            "url": "{base}/proxy?u={id}",
            "extract": "data.url",
            "error": "代理未返回 url: {id}",
            "header": {"User-Agent": "PROXY"}
        }
    });
    let desc = parse(value);
    let s = site();
    let play = |id: &str| engine::player_content(&desc, &cfg(), &s, id).expect("播放");

    let value = play("pre:abc");
    assert_eq!(value["parse"], json!(0));
    assert_eq!(value["url"], json!("pre:abc"));
    assert_eq!(value["header"]["User-Agent"], json!("PRE"));
    assert_eq!(value["jx"], json!(0), "未声明 jx 的规则继承端点 jx");

    let value = play("xmark");
    assert_eq!(value["url"], json!(format!("{base}/m/xmark")));
    assert_eq!(value.get("jx"), None, "规则显式 jx=false 覆盖端点 jx");

    let value = play("UPPER-1");
    assert_eq!(value["url"], json!(format!("{base}/u/UPPER-1")));

    let value = play("https://x.example/a.m3u8");
    assert_eq!(
        value["url"],
        json!("https://cdn.example/https://x.example/a.m3u8")
    );
    assert_eq!(value["parse"], json!(0));

    let value = play("https://v.youku.com/v_show/1");
    assert_eq!(value["header"]["Referer"], json!("https://youku.example"));
    assert_eq!(value["header"]["User-Agent"], json!("CHROME"));
    assert_eq!(value["url"], json!("https://v.youku.com/v_show/1"));
    assert_eq!(value.get("jx"), None);

    let value = play("https://v.qq.com/x");
    assert_eq!(value["header"]["Referer"], json!("https://qq.example"));

    let value = play("plain-id");
    assert_eq!(value["url"], json!("https://real.example/v.m3u8"), "proxy 直链");
    assert_eq!(value["parse"], json!(1), "proxy 使用端点 parse");
    assert_eq!(value["header"]["User-Agent"], json!("PROXY"));
}

/* ============================================================
 * 13. 播放缺省值 / header_mode / header_as_json
 * ============================================================ */

#[test]
fn play_defaults_header_mode_and_json_header() {
    let base = echo_server();
    let mut value = descriptor_value("json", &base);
    value["headers"] = json!({"User-Agent": "DESC-UA", "X-Token": "TK"});
    value["endpoints"]["play"] = json!({
        "parse": 1,
        "url": "{base}/p/{id}",
        "header": {"User-Agent": "EP-UA"},
        "header_mode": "descriptor"
    });
    let desc = parse(value);
    let s = site();
    let played = engine::player_content(&desc, &cfg(), &s, "9").expect("播放");
    assert_eq!(played["url"], json!(format!("{base}/p/9")));
    assert_eq!(
        played["header"]["User-Agent"],
        json!("DESC-UA"),
        "header_mode=descriptor"
    );
    assert_eq!(played["header"]["X-Token"], json!("TK"));

    let mut value = descriptor_value("json", &base);
    value["endpoints"]["play"] = json!({
        "parse": 0,
        "url": "{id}",
        "header_as_json": true,
        "header": {"User-Agent": "EP-UA", "X-Tok": "T"},
        "empty_id_error": "播放 id 为空"
    });
    let desc = parse(value);
    let played = engine::player_content(&desc, &cfg(), &s, "abc").expect("播放");
    let header = played["header"].as_str().expect("header 应为 JSON 字符串");
    let parsed: Value = serde_json::from_str(header).expect("header 字符串可解析");
    assert_eq!(parsed["User-Agent"], json!("EP-UA"));
    assert_eq!(parsed["X-Tok"], json!("T"));

    let err = engine::player_content(&desc, &cfg(), &s, "  ").expect_err("空 id");
    assert!(err.to_string().contains("播放 id 为空"), "{err}");
}

/* ============================================================
 * 14. static_categories：不联网固定分类
 * ============================================================ */

#[test]
fn static_categories_skip_network_and_validate() {
    // 基址指向必然连不上的端口：一旦发请求就会报错，能证明静态分类不联网。
    let mut value = descriptor_value("json", "http://127.0.0.1:1");
    value["endpoints"]["categories"] = json!({
        "static_categories": [
            {"id": "0", "name": "全部"},
            {"id": "1", "name": "电影"}
        ]
    });
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "{:?}", desc.validate());
    let cats = engine::categories(&desc, &cfg(), &site()).expect("静态分类不联网也成功");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "0");
    assert_eq!(cats[1].type_name, "电影");

    // 空数组视为配置错误。
    let mut value = descriptor_value("json", "https://base.example");
    value["endpoints"]["categories"] = json!({"static_categories": []});
    let errs = parse(value).validate();
    assert!(
        errs.iter().any(|e| e.contains("static_categories 不能为空数组")),
        "{errs:?}"
    );

    // 分类 id 不能为空。
    let mut value = descriptor_value("json", "https://base.example");
    value["endpoints"]["categories"] = json!({"static_categories": [{"id": " ", "name": "电影"}]});
    let errs = parse(value).validate();
    assert!(
        errs.iter().any(|e| e.contains("static_categories[0].id 不能为空")),
        "{errs:?}"
    );
}

/* ============================================================
 * 16. 迁移期降级：family_fallback
 * ============================================================ */

#[test]
fn family_fallback_delegates_per_entry() {
    // 缺省：声明了的入口都由描述接管；没声明 search 时 search 回落手写。
    let desc = parse(descriptor_value("json", "https://base.example"));
    for entry in [
        engine::Entry::Categories,
        engine::Entry::List,
        engine::Entry::Detail,
        engine::Entry::Play,
    ] {
        assert!(!engine::delegates(&desc, entry), "{entry:?} 缺省应由描述接管");
    }
    assert!(
        engine::delegates(&desc, engine::Entry::Search),
        "没有 search 端点 → 回落手写"
    );

    // 只让 detail 回落手写；其余入口仍由描述接管。
    let mut value = descriptor_value("json", "https://base.example");
    value["endpoints"]["detail"]["family_fallback"] = json!(true);
    value["endpoints"]["search"] = json!({
        "path": "/s",
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "{:?}", desc.validate());
    assert!(engine::delegates(&desc, engine::Entry::Detail));
    assert!(!engine::delegates(&desc, engine::Entry::List));
    assert!(
        !engine::delegates(&desc, engine::Entry::Search),
        "声明了 search 且未标 fallback"
    );
}

/* ============================================================
 * 17. HTML 分集 URL 绝对化 + 搜索关键词 trim
 * ============================================================ */

#[test]
fn groups_html_join_base_and_search_trim_keyword() {
    let base = fixed_server(JOIN_BASE_HTML, "text/html; charset=utf-8");
    let mut value = descriptor_value("html", &base);
    value["endpoints"]["detail"] = json!({
        "path": "/detail",
        "fields": {"name": {"sel": ["h2.title"]}},
        "groups": {
            "tabs": "div.tabs a",
            "lists": "ul.list",
            "item": "a",
            "url_join_base": true
        }
    });
    let desc = parse(value);
    let detail = engine::detail(&desc, &cfg(), &site(), "9").expect("详情");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(
        detail.episodes[0].items[0].url,
        format!("{base}/v_play/abc.html"),
        "相对 href 按基址绝对化"
    );

    // trim_keyword：空白关键词不发请求直接返回空（基址连不上也能通过）。
    let echo = echo_server();
    let mut value = descriptor_value("json", "http://127.0.0.1:1");
    value["endpoints"]["search"] = json!({
        "path": "/s",
        "params": [{"key": "wd", "value": "{kw}"}],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"},
        "trim_keyword": true
    });
    let desc = parse(value);
    let items = engine::search(&desc, &cfg(), &site(), "   ").expect("空白关键词");
    assert!(items.is_empty(), "trim 后为空 → 空结果且不联网");

    // 非空关键词仍会 trim 后发请求。
    let mut value = descriptor_value("json", &echo);
    value["endpoints"]["search"] = json!({
        "path": "/s",
        "params": [{"key": "wd", "value": "{kw}", "encode": "alnum"}],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"},
        "trim_keyword": true
    });
    let desc = parse(value);
    let items = engine::search(&desc, &cfg(), &site(), "  ab  ").expect("搜索");
    assert_eq!(items[0].name, "/s?wd=ab", "关键词先 trim");
}

const JOIN_BASE_HTML: &str = r#"<html><body>
<h2 class="title">测试标题</h2>
<div class="tabs"><a>线路一</a></div>
<ul class="list">
  <li><a href="/v_play/abc.html">第1集</a></li>
  <li><a href="/v_play/def.html">第2集</a></li>
</ul>
</body></html>"#;

/* ============================================================
 * 18. 迁移期原语：数组 join / 分隔符切段 / 集号回退 / 毫秒时间戳 / 代理相对地址
 * ============================================================ */

const GROUPS_JSON_BODY: &str = r#"{"data":{
  "name":"剧名",
  "actor":["吴京","刘德华"],
  "groups":[
    {"n":"线路一","urls":[{"t":"第1集$http://cdn/1.m3u8"},{"t":"第2集$http://cdn/2.m3u8"}]},
    {"n":"线路二","urls":[{"t":"$http://cdn/3.m3u8"},{"t":"$http://cdn/4.m3u8"}]}
  ]}}"#;

#[test]
fn groups_fields_join_split_and_episode_fallback() {
    let base = fixed_server(GROUPS_JSON_BODY, "application/json");
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["detail"] = json!({
        "path": "/detail",
        "root": "data",
        "fields": {
            "name": "name",
            "actor": {"path": "actor[]", "join": " "}
        },
        "groups": {
            "rows": ["groups[]"],
            "name": "n",
            "items": "urls[]",
            "title": {"path": "t", "split_on": "$", "segment": 0},
            "url": {"path": "t", "split_on": "$", "segment": 1},
            "title_fallback_episode": true
        }
    });
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "{:?}", desc.validate());
    let detail = engine::detail(&desc, &cfg(), &site(), "1").expect("详情");
    assert_eq!(detail.name, "剧名");
    assert_eq!(detail.actor.as_deref(), Some("吴京 刘德华"), "数组按分隔符 join");
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].items[0].title, "第1集");
    assert_eq!(detail.episodes[0].items[0].url, "http://cdn/1.m3u8");
    assert_eq!(detail.episodes[0].items[1].url, "http://cdn/2.m3u8");
    assert_eq!(detail.episodes[1].items[0].title, "第1集", "空标题回退集号");
    assert_eq!(detail.episodes[1].items[1].title, "第2集");
}

#[test]
fn vars_unix_ts_ms_and_proxy_join_base() {
    // 毫秒时间戳：13 位十进制。
    let echo = echo_server();
    let mut value = descriptor_value("json", &echo);
    value["vars"] = json!([{"name": "ms", "apply": "unix_ts_ms"}]);
    value["endpoints"]["categories"] = json!({
        "path": "/cat",
        "params": [{"key": "t", "value": "{ms}"}],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "{:?}", desc.validate());
    let cats = engine::categories(&desc, &cfg(), &site()).expect("分类");
    let query = cats[0].type_id.trim_start_matches("/cat?t=");
    assert_eq!(query.len(), 13, "unix_ts_ms 应为 13 位: {query}");
    assert!(query.chars().all(|c| c.is_ascii_digit()), "{query}");

    // 代理取直链时把相对地址按基址绝对化。
    let proxy = fixed_server(r#"{"data":{"url":"/v/9.m3u8"}}"#, "application/json");
    let mut value = descriptor_value("json", &proxy);
    value["endpoints"]["play"] = json!({
        "parse": 1,
        "proxy": {
            "url": "{base}/play/{id}",
            "extract": "data.url",
            "join_base": true,
            "error": "代理未返回直链"
        }
    });
    let desc = parse(value);
    let play = engine::player_content(&desc, &cfg(), &site(), "9").expect("播放");
    assert_eq!(
        play["url"].as_str(),
        Some(format!("{proxy}/v/9.m3u8").as_str()),
        "proxy.join_base 绝对化"
    );
}

/* ============================================================
 * 19. POST 端点（json / form body）
 * ============================================================ */

/// POST 回显服务器：把方法、body 原样回填到响应 JSON。
fn post_echo_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock 服务器");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            use std::io::Read;
            let method = request.method().as_str().to_string();
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let content_type = request
                .headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("content-type"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            // `name` 回填「方法|Content-Type|body」，供断言拆解。
            let echo = format!("{method}|{content_type}|{body}");
            let resp = json!({
                "method": method,
                "body": body,
                "content_type": content_type,
                "data": [{"id": "1", "name": echo, "type_id": "1", "type_name": method}]
            })
            .to_string();
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("Content-Type 头");
            let _ = request.respond(Response::from_string(resp).with_header(header));
        }
    });
    base
}

#[test]
fn post_json_and_form_bodies() {
    let base = post_echo_server();

    // POST JSON：字符串/数值/布尔三种 body 字段类型。
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["list"] = json!({
        "path": "/api/list",
        "method": "post_json",
        "body": [
            {"key": "page", "value": "{page}", "kind": "number"},
            {"key": "type", "value": "{cat}"},
            {"key": "first", "value": "true", "kind": "bool"}
        ],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "{:?}", desc.validate());
    let items = engine::videos(&desc, &cfg(), &site(), "电影", 3).expect("POST 列表");
    let parts: Vec<&str> = items[0].name.splitn(3, '|').collect();
    assert_eq!(parts[0], "POST");
    assert!(parts[1].contains("application/json"), "Content-Type: {}", parts[1]);
    let body: Value = serde_json::from_str(parts[2]).expect("body 应为 JSON");
    assert_eq!(body["page"], 3, "number 类型按数字发送");
    assert_eq!(body["type"], "电影");
    assert_eq!(body["first"], true, "bool 类型按布尔发送");

    // POST 表单：encode 生效。
    let mut value = descriptor_value("json", &base);
    value["endpoints"]["search"] = json!({
        "path": "/api/search",
        "method": "post_form",
        "body": [{"key": "wd", "value": "{kw}"}],
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let desc = parse(value);
    assert!(desc.validate().is_empty(), "{:?}", desc.validate());
    let items = engine::search(&desc, &cfg(), &site(), "a b").expect("POST 搜索");
    let parts: Vec<&str> = items[0].name.splitn(3, '|').collect();
    assert_eq!(parts[0], "POST");
    assert!(
        parts[1].contains("x-www-form-urlencoded"),
        "表单 Content-Type: {}",
        parts[1]
    );
    // form-urlencoded：空格按 `+` 编码（或 `%20`），不出现二次编码的 `%2520`。
    assert!(
        parts[2].contains("wd=a+b") || parts[2].contains("wd=a%20b"),
        "表单字段编码: {}",
        parts[2]
    );
    assert!(!parts[2].contains("%25"), "不应二次编码: {}", parts[2]);

    // POST 端点缺 body → validate 报错。
    let mut value = descriptor_value("json", "https://base.example");
    value["endpoints"]["list"] = json!({
        "path": "/api/list",
        "method": "post_json",
        "rows": ["data[]"],
        "fields": {"id": "id", "name": "name"}
    });
    let errs = parse(value).validate();
    assert!(
        errs.iter().any(|e| e.contains("body 在 POST 方法下不能为空")),
        "{errs:?}"
    );
}
