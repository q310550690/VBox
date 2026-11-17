//! JS spider 的 Jsoup 规则函数（pdfh/pdfa/pd/pdfla）集成测试。
//! 说明：这些是 catvod/cat.js 规则最常用的宿主函数，不访问网络。

use vbox_core::model::Rules;
use vbox_core::spider::sandbox::Sandbox;

fn html() -> &'static str {
    r#"<html><body>
      <ul class="list">
        <li><a class="pic" href="/d/1.html" title="A"><span class="note">HD</span></a></li>
        <li><a class="pic" href="/d/2.html" title="B"><span class="note">10集</span></a></li>
      </ul>
      <div id="playlist"><h3>线路一</h3>
        <ul class="eps"><li><a href="/p/1.html">第1集</a></li><li><a href="/p/2.html">第2集</a></li></ul>
      </div>
    </body></html>"#
}

#[test]
fn js_pdfh_pdfa_pd_pdfla() {
    let page = html();
    let script = format!(
        r#"
        var H = {page:?};
        function home(filter) {{
            var items = pdfa(H, 'ul.list&&li');
            var titles = items.map(function (it) {{ return pdfh(it, 'a.pic&&title'); }});
            var eps = pdfla(H, 'ul.eps&&li', 'a&&Text', 'a&&href', 'https://www.example.com/');
            return JSON.stringify({{
                title: pdfh(H, 'h3&&Text'),
                count: items.length,
                titles: titles,
                joined: pd('<a href="/x/1.html">A</a>', 'a&&href', 'https://www.example.com/list/'),
                eps: eps
            }});
        }}
        "#
    );
    let sandbox = Sandbox::new(script, "html-rule-test", Rules::default());
    let value = sandbox.call("home", &[]).expect("home 应执行成功");
    assert_eq!(value["title"].as_str(), Some("线路一"));
    assert_eq!(value["count"].as_i64(), Some(2));
    assert_eq!(value["titles"][0].as_str(), Some("A"));
    assert_eq!(value["titles"][1].as_str(), Some("B"));
    assert_eq!(
        value["joined"].as_str(),
        Some("https://www.example.com/x/1.html")
    );
    assert_eq!(
        value["eps"][0].as_str(),
        Some("第1集$https://www.example.com/p/1.html")
    );
}

#[test]
fn js_module_syntax_is_sanitized() {
    let script = r#"
    import { request } from './lib/cat.js';
    import * as cheerio from 'cheerio';
    export function home(filter) {
        return JSON.stringify({ class: [{ type_id: '1', type_name: '电影' }] });
    }
    export default home;
    "#;
    let sandbox = Sandbox::new(script, "module-test", Rules::default());
    let value = sandbox.call("home", &[]).expect("import/export 应被降级");
    assert_eq!(value["class"][0]["type_name"].as_str(), Some("电影"));
}

#[test]
fn js_aes_x_roundtrip() {
    let script = r#"
    function home() {
        var key = '1234567890123456';
        var iv = '1234567890123456';
        var enc = aesX('AES/CBC/PKCS7Padding', true, 'hello', false, key, iv, true);
        var dec = aesX('AES/CBC/PKCS7Padding', false, enc, true, key, iv, false);
        return JSON.stringify({ enc: enc, dec: dec });
    }
    "#;
    let sandbox = Sandbox::new(script, "aes-test", Rules::default());
    let value = sandbox.call("home", &[]).expect("aesX 应执行成功");
    assert!(!value["enc"].as_str().unwrap_or("").is_empty());
    assert_eq!(value["dec"].as_str(), Some("hello"));
}
