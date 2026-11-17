//! 脏 JSON 容错修复
//!
//! TVBox 订阅 JSON 常常不规范：带 `//` 行注释、`/* */` 块注释、尾随逗号，
//! 偶尔出现未转义内容。这里做一个"字符串感知"的扫描器，只在字符串外做删除/修正，
//! 避免误伤 JSON 字符串内部的 `//` 等字符（例如视频地址里的 `//`）。

/// 对原始文本做容错处理，返回可交给 serde 解析的文本
///
/// 会先剥离 UTF-8 BOM（`\u{FEFF}`）：部分源站返回带 BOM 的 JSON，
/// serde_json 会以 `expected value at line 1 column 1` 拒绝解析。
pub fn normalize(input: &str) -> String {
    let without_bom = input.trim_start_matches('\u{FEFF}');
    let stripped = strip_comments(without_bom);
    let no_ctrl = strip_control_chars(&stripped);
    remove_trailing_commas(&no_ctrl)
}

/// 删除 JSON 规范禁止的“裸控制字符”（0x00–0x1F）。
///
/// TVBox 订阅里偶尔会把 JS 代码等塞进 `ext` 字段并夹带未转义的换行/制表符，
/// 这些裸控制字符会让 serde 直接报 `control character found while parsing a string`。
/// 合法 JSON 里的换行/制表符一定是转义形式（`\n` / `\t` = 反斜杠+字母），
/// 不会被这里误删——我们只删“裸”控制字符字节。
fn strip_control_chars(s: &str) -> String {
    s.chars().filter(|c| (*c as u32) > 0x1F).collect()
}

/// 删除字符串外的 // 与 /* */ 注释
///
/// 注意：必须按 **char** 而不是 byte 遍历。若按字节 `bytes[i] as char` 再拼回 String，
/// 会把 UTF-8 多字节字符（中文/emoji）拆成多个 Latin-1 码点，产生 `Ã¥Â°Â` 型乱码。
fn strip_comments(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let n = chars.len();
    let mut in_string = false;
    let mut escape = false;

    while i < n {
        let c = chars[i];

        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        // 不在字符串内
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        // 行注释 //
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // 块注释 /* */
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2; // 跳过 */
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 删除对象/数组里成员之间的尾随逗号，例如 `{ "a":1, }` -> `{ "a":1 }`
///
/// 同样必须按 char 遍历，理由见 `strip_comments`。
fn remove_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == ',' {
            // 查看后续第一个非空白字符
            let mut j = i + 1;
            while j < n && chars[j].is_whitespace() {
                j += 1;
            }
            if j < n && (chars[j] == '}' || chars[j] == ']') {
                // 是尾随逗号：跳过逗号本身，仅保留逗号后的空白
                i += 1;
                while i < j {
                    out.push(chars[i]);
                    i += 1;
                }
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_line_comment() {
        let j = r#"{ "a": 1, /* 块 */ "b": 2 } // 行尾注释"#;
        let n = normalize(j);
        assert!(n.contains("\"a\": 1"));
        assert!(!n.contains("行尾注释"));
        assert!(!n.contains("块"));
    }

    #[test]
    fn test_trailing_comma() {
        let j = r#"{ "a": 1, "b": 2, }"#;
        let n = normalize(j);
        assert!(n.ends_with('}'));
        assert!(!n.contains(", }"));
    }

    #[test]
    fn test_comment_inside_string_kept() {
        let j = r#"{ "url": "http://x.com/a//b" }"#;
        let n = normalize(j);
        assert!(n.contains("http://x.com/a//b"));
    }

    /// 回归：normalize 不得破坏 UTF-8 多字节字符（中文 / emoji）
    #[test]
    fn test_utf8_preserved() {
        let j = r#"{
  "sites": [
    { "key": "drpy_js_豆瓣", "name": "🔥聚玩盒子4K", "type": 3, "api": "https://x/api" },
  ],
  "desc": "中文 // 不是注释",
}"#;
        let n = normalize(j);
        assert!(n.contains("🔥聚玩盒子4K"), "emoji/中文被破坏: {n}");
        assert!(n.contains("drpy_js_豆瓣"), "中文 key 被破坏: {n}");
        assert!(n.contains("中文 // 不是注释"), "字符串内注释被误删: {n}");
        assert!(!n.contains(",}"), "尾随逗号未清理: {n}");
    }

    /// 回归：带 UTF-8 BOM 的 JSON 必须能解析（部分源站会返回 BOM）
    #[test]
    fn test_bom_stripped() {
        let j = "\u{FEFF}{ \"sites\": [], \"urls\": [] }";
        let n = normalize(j);
        assert!(
            serde_json::from_str::<serde_json::Value>(&n).is_ok(),
            "BOM 未被剥离: {:?}",
            n
        );
    }
}
