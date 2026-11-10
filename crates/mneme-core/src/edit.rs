//! 编辑校验：FFI（手机端）与 HTTP server（桌面端）共用的一份实现。
//!
//! 之前这份逻辑长在 FFI 里，server 想给桌面端补编辑路由时只能再抄一遍——
//! 两端口径分叉往往就是从「复制粘贴」开始的，所以下沉到 core。

use crate::entity::{Entity, Value};
use crate::schema::{DomainSchema, FieldKind};
use serde_json::Value as Json;
use serde_json::json;
use std::collections::BTreeMap;

pub fn kind_name(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Text => "文本",
        FieldKind::Tag => "单值",
        FieldKind::Tags => "列表（a|b|c）",
        FieldKind::Number => "数字",
        FieldKind::Bool => "是/否",
    }
}

/// 把 JSON 值按字段类型收敛成 `Value`。
///
/// 宽容面刻意收窄：Tags 接受数组或 `|`/`、` 分隔的字符串（手机端粘贴习惯），
/// Number 接受字符串数字，Bool 接受 `true/1/是`——其余一律 `None`，
/// 让调用方报「类型不对」，而不是猜出一个值写进库里。
pub fn coerce(kind: FieldKind, raw: &Json) -> Option<Value> {
    match kind {
        FieldKind::Tags => match raw {
            Json::Array(_) => Value::from_raw(kind, raw),
            Json::String(s) => Some(Value::Tags(
                s.split(['|', '、'])
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(String::from)
                    .collect(),
            )),
            _ => None,
        },
        FieldKind::Number => match raw {
            Json::String(s) => s.trim().parse::<f64>().ok().map(Value::Number),
            _ => raw.as_f64().map(Value::Number),
        },
        FieldKind::Bool => match raw {
            Json::String(s) => match s.trim() {
                "true" | "1" | "是" => Some(Value::Bool(true)),
                "false" | "0" | "否" => Some(Value::Bool(false)),
                _ => None,
            },
            _ => raw.as_bool().map(Value::Bool),
        },
        _ => Value::from_raw(kind, raw),
    }
}

/// 编辑核心：无 IO，只按 schema 校验并改内存里的实体。
///
/// 返回 `(changed, unchanged)`；任何一条不合法都整体失败，一个字段也不改。
/// 先在副本上改，全部合法才回写，避免「改了一半报错」。
pub fn apply_edit(
    schema: &DomainSchema,
    entity: &mut Entity,
    patch: &BTreeMap<String, Json>,
    now: i64,
) -> Result<(Vec<Json>, Vec<Json>), String> {
    let Some(def) = schema.entity_def(&entity.kind) else {
        return Err(format!("领域包缺少 {} 的定义", entity.kind));
    };
    let mut changed: Vec<Json> = Vec::new();
    let mut unchanged: Vec<Json> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut bad: Vec<String> = Vec::new();

    let mut staged = entity.clone();
    for (key, raw) in patch {
        let Some(f) = def.fields.iter().find(|f| f.key == *key) else {
            return Err(format!("未知字段：{key}"));
        };
        if f.immutable {
            blocked.push(if f.label.is_empty() { f.key.clone() } else { f.label.clone() });
            continue;
        }
        let Some(val) = coerce(f.kind, raw) else {
            bad.push(format!("{}（需要 {}）", f.key, kind_name(f.kind)));
            continue;
        };
        if entity.get(key) == Some(&val) {
            unchanged.push(json!(key));
            continue;
        }
        let before = entity.get(key).map(|v| v.display()).unwrap_or_default();
        let after = val.display();
        staged.set(key.clone(), val, now);
        changed.push(json!({
            "key": key,
            "label": if f.label.is_empty() { f.key.clone() } else { f.label.clone() },
            "before": before,
            "after": after,
        }));
    }

    if !blocked.is_empty() {
        return Err(format!("原样字段不可修改（需新建条目）：{}", blocked.join("、")));
    }
    if !bad.is_empty() {
        return Err(format!("以下字段的值类型不对：{}", bad.join("；")));
    }
    if changed.is_empty() {
        return Ok((changed, unchanged));
    }
    *entity = staged;
    Ok((changed, unchanged))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> DomainSchema {
        DomainSchema::from_toml(
            r#"
id = "test"
name = "测试"
[[entities]]
kind = "herb"
primary = "name"
[[entities.fields]]
key = "name"
label = "药名"
kind = "text"
immutable = true
[[entities.fields]]
key = "effects"
label = "功效"
kind = "tags"
[[entities.fields]]
key = "dosage"
label = "用量"
kind = "text"
[[entities.fields]]
key = "toxic"
label = "毒性"
kind = "bool"
"#,
        )
        .expect("schema")
    }

    fn herb() -> Entity {
        let mut e = Entity::new("麻黄", "herb", "t", 1);
        e.set("name", Value::Text("麻黄".into()), 1);
        e.set(
            "effects",
            Value::Tags(vec!["发汗解表".into(), "宣肺平喘".into()]),
            1,
        );
        e.set("dosage", Value::Text("3-10g".into()), 1);
        e.set("toxic", Value::Bool(false), 1);
        e
    }

    #[test]
    fn apply_edit_all_or_nothing() {
        let s = schema();
        let mut e = herb();
        let mut patch = BTreeMap::new();
        patch.insert("dosage".to_string(), Json::String("2-5g".into()));
        patch.insert("不存在".to_string(), Json::String("x".into()));
        let err = apply_edit(&s, &mut e, &patch, 100).unwrap_err();
        assert!(err.contains("未知字段"), "{err}");
        // 整体失败：一个字段也不能被改掉
        assert_eq!(e.get("dosage").unwrap().display(), "3-10g");
    }

    #[test]
    fn apply_edit_blocks_immutable_and_reports_changed() {
        let s = schema();
        let mut e = herb();
        let mut patch = BTreeMap::new();
        patch.insert("name".to_string(), Json::String("改名".into()));
        let err = apply_edit(&s, &mut e, &patch, 100).unwrap_err();
        assert!(err.contains("原样字段不可修改"), "{err}");

        let mut patch = BTreeMap::new();
        patch.insert("effects".to_string(), Json::String("利水消肿|平喘".into()));
        let (changed, unchanged) = apply_edit(&s, &mut e, &patch, 100).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0]["after"], "利水消肿、平喘");
        assert!(unchanged.is_empty());
        assert_eq!(e.get("effects").unwrap().display(), "利水消肿、平喘");
    }

    #[test]
    fn apply_edit_unchanged_fields_produce_no_change() {
        let s = schema();
        let mut e = herb();
        let mut patch = BTreeMap::new();
        // 同值 + 数组形式 + bool 中文
        patch.insert(
            "effects".to_string(),
            Json::Array(vec![Json::String("发汗解表".into()), Json::String("宣肺平喘".into())]),
        );
        patch.insert("toxic".to_string(), Json::String("否".into()));
        let (_, unchanged) = apply_edit(&s, &mut e, &patch, 100).unwrap();
        assert_eq!(unchanged.len(), 2, "同值字段不该产生事件");
    }

    #[test]
    fn apply_edit_rejects_bad_types_with_kind_names() {
        let s = schema();
        let mut e = herb();
        let mut patch = BTreeMap::new();
        patch.insert("toxic".to_string(), Json::String("也许".into()));
        let err = apply_edit(&s, &mut e, &patch, 100).unwrap_err();
        assert!(err.contains("需要 是/否"), "{err}");
    }
}
