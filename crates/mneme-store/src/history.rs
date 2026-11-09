//! 变更历史与撤销。
//!
//! 数据源是**事件日志本身**（`events/*.jsonl`），不是折叠后的内存状态：
//! - 每条 `UpsertEntity` 都是全量快照，所以相邻两版一 diff 就是「谁在什么时候把哪个字段从什么改成什么」；
//! - `Tombstone` 也要出现在历史里，否则"删了又恢复"这段时间线会断。
//!
//! 撤销 = 把上一版快照写回去，**产生一条新事件（rev 递增）**，而不是篡改历史。
//! 这样多设备同步、审计、回放全都自洽。

use crate::{Error, Op, Result, Store};
use mneme_core::Entity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldChange {
    pub key: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityRevision {
    pub rev: u64,
    pub ts: i64,
    pub device: String,
    /// create | update | delete | restore
    pub action: String,
    #[serde(default)]
    pub changes: Vec<FieldChange>,
}

impl Store {
    /// 单条实体的字段级时间线，按事件顺序返回最近 `limit` 条。
    ///
    /// 只读：不写日志、不改内存。
    pub fn entity_history(&self, id: &str, limit: usize) -> Result<Vec<EntityRevision>> {
        let ops = self.log().read_ops()?;
        let mut out: Vec<EntityRevision> = Vec::new();
        let mut prev: Option<Entity> = None;

        for op in ops {
            match op {
                Op::UpsertEntity { entity } if entity.id == id => {
                    let mut keys: BTreeSet<String> = BTreeSet::new();
                    if let Some(p) = &prev {
                        keys.extend(p.fields.keys().cloned());
                    }
                    keys.extend(entity.fields.keys().cloned());

                    let mut changes = Vec::new();
                    for k in keys {
                        let before = prev
                            .as_ref()
                            .and_then(|p| p.get(&k))
                            .map(|v| v.display())
                            .unwrap_or_default();
                        let after = entity.get(&k).map(|v| v.display()).unwrap_or_default();
                        if before != after {
                            changes.push(FieldChange {
                                key: k,
                                before,
                                after,
                            });
                        }
                    }

                    let action = match (&prev, entity.deleted) {
                        (None, _) => "create",
                        (Some(p), false) if p.deleted => "restore",
                        (_, true) => "delete",
                        _ => "update",
                    };
                    // 只留真的有内容的记录：空 diff 的 update 说明是幂等重放或纯元数据变更
                    if !changes.is_empty() || action != "update" {
                        out.push(EntityRevision {
                            rev: entity.rev,
                            ts: entity.updated_at,
                            device: entity.device.clone(),
                            action: action.into(),
                            changes,
                        });
                    }
                    prev = Some(entity);
                }
                Op::Tombstone {
                    id: tid,
                    rev,
                    updated_at,
                    device,
                } if tid == id => {
                    out.push(EntityRevision {
                        rev,
                        ts: updated_at,
                        device,
                        action: "delete".into(),
                        changes: Vec::new(),
                    });
                    if let Some(p) = prev.as_mut() {
                        p.deleted = true;
                    }
                }
                _ => {}
            }
        }

        if out.len() > limit {
            out = out.split_off(out.len() - limit);
        }
        Ok(out)
    }

    /// 撤销这条实体最近一次编辑：把上一版快照的字段值写回去。
    ///
    /// 拒绝的三种情况（都返回中文原因，UI 直接显示）：
    /// 1. 没有两次以上快照 —— 说明是新建的，撤销应该用"删除"；
    /// 2. 最近一次变更不是本设备做的 —— 上游合并/另一台设备写的，撤了会打乱别人的意图；
    /// 3. 上一版与当前版没有字段差异 —— 没什么可撤。
    pub fn undo_entity(&self, id: &str, now: i64) -> Result<(u64, Vec<String>)> {
        let ops = self.log().read_ops()?;
        let snapshots: Vec<Entity> = ops
            .into_iter()
            .filter_map(|op| match op {
                Op::UpsertEntity { entity } if entity.id == id => Some(entity),
                _ => None,
            })
            .collect();

        let Some(last) = snapshots.last().cloned() else {
            return Err(Error::Invalid("这条还没有可撤销的编辑记录".into()));
        };
        if snapshots.len() < 2 {
            return Err(Error::Invalid(
                "这是新建的条目，撤销请用「删除」".into(),
            ));
        }
        if last.device != self.device() {
            return Err(Error::Invalid(format!(
                "最近一次变更来自「{}」，不能在本机撤销",
                last.device
            )));
        }

        let prev = &snapshots[snapshots.len() - 2];
        let mut e = last.clone();
        let mut restored: Vec<String> = Vec::new();
        let keys: BTreeSet<String> = prev
            .fields
            .keys()
            .chain(last.fields.keys())
            .cloned()
            .collect();

        for k in keys {
            match prev.get(&k) {
                Some(v) => {
                    if e.get(&k) != Some(v) {
                        e.fields.insert(k.clone(), v.clone());
                        restored.push(k);
                    }
                }
                None => {
                    if e.fields.remove(&k).is_some() {
                        restored.push(k);
                    }
                }
            }
        }

        if restored.is_empty() {
            return Err(Error::Invalid("最近的变更没有字段差异，无法撤销".into()));
        }

        e.deleted = prev.deleted;
        let rev = self.stats().rev.max(last.rev) + 1;
        e.rev = rev;
        e.updated_at = now;
        e.device = self.device().to_string();
        self.mutate(&[Op::UpsertEntity { entity: e }])?;
        Ok((rev, restored))
    }
}
