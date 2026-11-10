//! 实体生命周期：新建 / 删除 / 恢复。
//!
//! 这份逻辑之前长在 FFI 里，桌面端（HTTP server）要做同样的功能时只能抄一遍——
//! 「单条能建、批量建出另一套规则」「手机能删、桌面删出另一种语义」都是
//! 从复制粘贴开始的。现在 FFI 与 server 共用这一份。

use crate::{Op, Store};
use mneme_core::{
    DomainSchema, Entity, Value, coerce, kind_name, now_ms,
};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// 新建成功的回执。
#[derive(Debug, Clone)]
pub struct Created {
    pub id: String,
    pub rev: u64,
    pub name: String,
    /// 替代掉的旧条目 id（`supersedes` 指向它时会把旧条目盖上 `superseded_by`）。
    pub superseded: String,
}

/// 新建失败的分类。
///
/// 重名单独一类：UI 要据此给「去编辑」或「换名字」的出口，不能混进普通错误。
#[derive(Debug, Clone)]
pub enum CreateErr {
    Duplicate {
        message: String,
        id: String,
        deleted: bool,
    },
    Other(String),
}

impl std::fmt::Display for CreateErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate { message, .. } | Self::Other(message) => f.write_str(message),
        }
    }
}

/// 新建一条实体。
///
/// - 只按 schema 收敛字段（`coerce`），未知字段 / 类型不对直接报错，绝不猜。
/// - 主键值不能为空（报错点名字段 label）。
/// - id 默认取主键值；`id_hint` 非空时用它。
/// - 同 id 已存在 → `Duplicate`（回收站里的也要报，提示先恢复）。
/// - `supersedes` 非空时把旧条目盖上 `superseded_by`，两条 op 一起写（原子）。
pub fn create_one(
    store: &Store,
    schema: &DomainSchema,
    kind: &str,
    raw: &BTreeMap<String, Json>,
    id_hint: &str,
) -> Result<Created, CreateErr> {
    let Some(def) = schema.entity_def(kind).cloned() else {
        return Err(CreateErr::Other(format!("未知实体类型：{kind}")));
    };
    let now = now_ms();
    let device = store.device().to_string();
    let mut entity = Entity::new("", kind.to_string(), &device, now);
    let mut bad: Vec<String> = Vec::new();
    for (key, v) in raw {
        let Some(f) = def.fields.iter().find(|f| f.key == *key) else {
            return Err(CreateErr::Other(format!("未知字段：{key}")));
        };
        let Some(val) = coerce(f.kind, v) else {
            bad.push(format!("{}（需要 {}）", f.key, kind_name(f.kind)));
            continue;
        };
        entity.fields.insert(key.clone(), val);
    }
    if !bad.is_empty() {
        return Err(CreateErr::Other(format!(
            "以下字段的值类型不对：{}",
            bad.join("；")
        )));
    }

    let primary = def.primary.clone().unwrap_or_else(|| "name".into());
    let name = entity.display_of(&primary).trim().to_string();
    if name.is_empty() {
        let label = schema
            .field(kind, &primary)
            .map(|f| if f.label.is_empty() { f.key.clone() } else { f.label.clone() })
            .unwrap_or_else(|| primary.clone());
        return Err(CreateErr::Other(format!("「{label}」不能为空")));
    }

    let id = if id_hint.trim().is_empty() {
        name.clone()
    } else {
        id_hint.trim().to_string()
    };
    if let Some(existing) = store.read(|m| m.entities.get(&id).cloned()) {
        return Err(CreateErr::Duplicate {
            message: if existing.deleted {
                format!("《{name}》在回收站里，先恢复或换个名字")
            } else {
                format!("已存在《{name}》")
            },
            id,
            deleted: existing.deleted,
        });
    }

    let supersedes = entity.display_of("supersedes").trim().to_string();
    let mut ops: Vec<Op> = Vec::new();
    let base_rev = store.stats().rev;
    if !supersedes.is_empty() {
        match store.read(|m| m.entities.get(&supersedes).cloned()) {
            Some(mut old) => {
                old.set("superseded_by", Value::Text(id.clone()), now);
                old.rev = base_rev + 1;
                old.updated_at = now;
                old.device = device.clone();
                ops.push(Op::UpsertEntity { entity: old });
            }
            None => {
                return Err(CreateErr::Other(format!("要替代的条目不存在：{supersedes}")));
            }
        }
    }

    entity.id = id.clone();
    entity.rev = base_rev + ops.len() as u64 + 1;
    ops.push(Op::UpsertEntity { entity: entity.clone() });
    store.mutate(&ops).map_err(|e| CreateErr::Other(e.to_string()))?;
    Ok(Created {
        id: entity.id,
        rev: entity.rev,
        name,
        superseded: supersedes,
    })
}

/// 删除（显式墓碑）。绝不靠「远端没有」推断——同步语义依赖这一点。
pub fn delete_entity(store: &Store, id: &str) -> Result<i64, String> {
    let e = store
        .read(|m| m.entities.get(id).cloned())
        .ok_or_else(|| format!("实体不存在：{id}"))?;
    if e.deleted {
        return Err("这条已经在回收站里了".into());
    }
    let now = now_ms();
    let op = Op::Tombstone {
        id: id.to_string(),
        rev: store.stats().rev + 1,
        updated_at: now,
        device: store.device().to_string(),
    };
    store.mutate(&[op]).map_err(|e| e.to_string())?;
    Ok(now)
}

/// 从回收站恢复（写一条 `deleted=false` 的新快照，历史里多一条 restore）。
pub fn restore_entity(store: &Store, id: &str) -> Result<u64, String> {
    let mut e = store
        .read(|m| m.entities.get(id).cloned())
        .ok_or_else(|| format!("实体不存在：{id}"))?;
    if !e.deleted {
        return Err("这条没有被删除".into());
    }
    let now = now_ms();
    e.deleted = false;
    e.rev = store.stats().rev + 1;
    e.updated_at = now;
    e.device = store.device().to_string();
    let rev = e.rev;
    store
        .mutate(&[Op::UpsertEntity { entity: e }])
        .map_err(|err| err.to_string())?;
    Ok(rev)
}

#[cfg(test)]
mod tests {
    use super::*;
        use std::path::PathBuf;

    const SCHEMA: &str = r#"
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
key = "supersedes"
label = "替代自"
kind = "text"
[[entities.fields]]
key = "superseded_by"
label = "已被替代为"
kind = "text"
"#;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mneme-entities-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn seeded(name: &str) -> (std::sync::Arc<Store>, DomainSchema) {
        let dir = tmp(name);
        let store = Store::open(&dir, "t").unwrap();
        let schema = DomainSchema::from_toml(SCHEMA).unwrap();
        store
            .mutate(&[Op::SetSchema { toml: SCHEMA.into() }])
            .unwrap();
        (store, schema)
    }

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, Json> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Json::String(v.to_string())))
            .collect()
    }

    #[test]
    fn create_then_duplicate_rejected() {
        let (store, schema) = seeded("dup");
        let c = create_one(&store, &schema, "herb", &fields(&[("name", "麻黄")]), "").unwrap();
        assert_eq!(c.id, "麻黄");
        assert!(c.superseded.is_empty());

        let err = create_one(&store, &schema, "herb", &fields(&[("name", "麻黄")]), "").unwrap_err();
        match err {
            CreateErr::Duplicate { message, deleted, .. } => {
                assert!(message.contains("已存在"), "{message}");
                assert!(!deleted);
            }
            _ => panic!("应是重名错误"),
        }
    }

    #[test]
    fn create_requires_primary_and_valid_types() {
        let (store, schema) = seeded("valid");
        let err = create_one(&store, &schema, "herb", &fields(&[("effects", "发汗")]), "").unwrap_err();
        assert!(err.to_string().contains("不能为空"), "{err}");

        let err = create_one(
            &store,
            &schema,
            "herb",
            &fields(&[("name", "某药"), ("不存在的", "x")]),
            "",
        )
        .unwrap_err();
        assert!(err.to_string().contains("未知字段"), "{err}");
    }

    #[test]
    fn delete_restore_roundtrip_and_supersede() {
        let dir;
        let (store, schema) = {
            let (s, sc) = seeded("roundtrip");
            dir = s.root().to_path_buf();
            (s, sc)
        };
        create_one(&store, &schema, "herb", &fields(&[("name", "旧药")]), "").unwrap();
        // 新建并替代旧药
        let c = create_one(
            &store,
            &schema,
            "herb",
            &fields(&[("name", "新药"), ("supersedes", "旧药")]),
            "",
        )
        .unwrap();
        assert_eq!(c.superseded, "旧药");
        let old = store.read(|m| m.entities.get("旧药").cloned().unwrap());
        assert_eq!(
            old.get("superseded_by").map(|v| v.display()),
            Some("新药".into())
        );

        delete_entity(&store, "新药").unwrap();
        assert!(store.read(|m| m.entities["新药"].deleted));
        // 再删一次要报错
        assert!(delete_entity(&store, "新药").is_err());
        // 恢复
        restore_entity(&store, "新药").unwrap();
        assert!(!store.read(|m| m.entities["新药"].deleted));
        // 重开后依然成立（持久化）
        drop(store);
        let store = Store::open(&dir, "t").unwrap();
        assert!(store.read(|m| m.entities.contains_key("新药")));
        assert!(!store.read(|m| m.entities["新药"].deleted));
    }
}
