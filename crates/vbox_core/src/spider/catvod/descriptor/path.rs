//! JSON 路径子集（描述里 `path` 字段的语法）。
//!
//! 只支持定位，不支持表达式/过滤/函数：
//! * `data.list[]` —— 逐段下降，`[]` 表示该段是数组、展开所有元素；
//! * `data.video` —— 普通对象键；
//! * 顶层相对行对象（如 `vod_id`）—— 从当前行开始。
//!
//! 与 JSONPath 的差异是刻意的：描述只允许「取哪里」，任何判断都交给显式的
//! `filters` / `require` / `play.rules`，避免路径本身变成表达式语言。

use serde_json::Value;

/// 一段路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// 对象键。
    Key(String),
    /// 展开数组。
    Iter,
}

/// 解析路径；空串或非法路径返回错误。
pub fn parse(path: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for raw in path.split('.') {
        let mut seg = raw.trim();
        if seg.is_empty() {
            return Err(format!("路径 `{path}` 含空段"));
        }
        let mut iters = 0usize;
        while let Some(rest) = seg.strip_suffix("[]") {
            iters += 1;
            seg = rest.trim_end();
        }
        if seg.is_empty() {
            return Err(format!("路径 `{path}` 含空段"));
        }
        steps.push(Step::Key(seg.to_string()));
        for _ in 0..iters {
            steps.push(Step::Iter);
        }
    }
    if steps.is_empty() {
        return Err(format!("路径 `{path}` 为空"));
    }
    Ok(steps)
}

/// 沿路径展开，返回所有命中值（保持文档顺序）。
pub fn walk<'a>(root: &'a Value, steps: &[Step]) -> Vec<&'a Value> {
    let mut cur: Vec<&Value> = vec![root];
    for step in steps {
        let mut next = Vec::new();
        for v in cur {
            match step {
                Step::Key(k) => {
                    if let Some(child) = v.get(k) {
                        next.push(child);
                    }
                }
                Step::Iter => {
                    if let Some(arr) = v.as_array() {
                        next.extend(arr.iter());
                    }
                }
            }
        }
        if next.is_empty() {
            return next;
        }
        cur = next;
    }
    cur
}

/// 单个取值：路径需不含 `[]`（或命中恰好一个数组元素时取首个）。
pub fn get<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let steps = parse(path).ok()?;
    walk(root, &steps).into_iter().next()
}

/// 标量转字符串；`null` 与复合值返回 `None`（与手写实现的 `as_str` 差异见描述说明）。
pub fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 数组元素或单值统一读成 `Vec<&Value>`：`[]` 已展开；非数组单值视为一项。
pub fn rows<'a>(root: &'a Value, paths: &[String]) -> Vec<&'a Value> {
    for path in paths {
        let Ok(steps) = parse(path) else { continue };
        let found = walk(root, &steps);
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_nested_and_iter() {
        let steps = parse("data.list[]").unwrap();
        assert_eq!(
            steps,
            vec![Step::Key("data".into()), Step::Key("list".into()), Step::Iter]
        );
        assert!(parse("").is_err());
        assert!(parse("a..b").is_err());
    }

    #[test]
    fn walks_list_and_single() {
        let v = json!({"data":{"list":[{"id":"1"},{"id":"2"}]}});
        assert_eq!(walk(&v, &parse("data.list[]").unwrap()).len(), 2);
        assert_eq!(get(&v, "data.list").unwrap().as_array().unwrap().len(), 2);
        assert!(get(&v, "data.nope").is_none());
    }

    #[test]
    fn rows_falls_back_to_second_path() {
        let v = json!({"list":[1,2,3]});
        let paths = vec!["data.list[]".to_string(), "list[]".to_string()];
        assert_eq!(rows(&v, &paths).len(), 3);
    }

    #[test]
    fn scalar_only_leaves_objects_out() {
        assert_eq!(scalar(&json!("x")), Some("x".into()));
        assert_eq!(scalar(&json!(3)), Some("3".into()));
        assert_eq!(scalar(&json!(null)), None);
        assert_eq!(scalar(&json!({"a":1})), None);
    }
}
