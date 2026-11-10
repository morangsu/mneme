use crate::entity::{Entity, ReviewEvent, Value};
use crate::error::Result;
use crate::schema::DomainSchema;
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    pub field: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntityChange {
    Added(String),
    Modified(String, Vec<FieldChange>),
    Removed(String),
    /// immutable 字段被两端同时改动且不一致 -> 不自动合并，交人工裁决
    Conflict(String, String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diff {
    pub changes: Vec<EntityChange>,
    /// 需要人工裁决的冲突（会计入 SyncReport.conflicts）
    pub warnings: Vec<String>,
    /// 已自动合并但值得用户知道的事（双端同字段都改过），不计入冲突数
    pub notices: Vec<String>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.warnings.is_empty()
    }

    /// 同步前的差异预览文本（人读）。
    pub fn summary(&self) -> String {
        let mut lines = Vec::with_capacity(self.changes.len() + self.warnings.len() + 1);
        lines.push(format!("共 {} 项变化", self.changes.len()));
        for c in self.changes.iter().take(50) {
            match c {
                EntityChange::Added(id) => lines.push(format!("  + {id}")),
                EntityChange::Removed(id) => lines.push(format!("  - {id}")),
                EntityChange::Modified(id, fs) => lines.push(format!(
                    "  ~ {id} ({})",
                    fs.iter().map(|f| f.field.as_str()).collect::<Vec<_>>().join(",")
                )),
                EntityChange::Conflict(id, f) => lines.push(format!("  ! {id} 冲突字段 {f}")),
            }
        }
        for w in &self.warnings {
            lines.push(format!("  ! {w}"));
        }
        for n in &self.notices {
            lines.push(format!("  · {n}"));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergePolicy {
    /// 字段级 last-write-wins，按 (rev, updated_at, device) 裁决
    #[default]
    FieldLevelLww,
    PreferLocal,
    PreferRemote,
}

fn to_map(xs: &[Entity]) -> HashMap<&str, &Entity> {
    xs.iter().map(|e| (e.id.as_str(), e)).collect()
}

/// base -> target 的差异。同步覆盖前先展示这个，避免静默改数据。
pub fn diff_entities(base: &[Entity], target: &[Entity], schema: &DomainSchema) -> Diff {
    let b = to_map(base);
    let t = to_map(target);
    let ids: BTreeSet<&str> = b.keys().chain(t.keys()).copied().collect();
    let mut diff = Diff::default();
    for id in ids {
        match (b.get(id), t.get(id)) {
            (None, Some(e)) => diff.changes.push(EntityChange::Added(e.id.clone())),
            (Some(e), None) => diff.changes.push(EntityChange::Removed(e.id.clone())),
            (Some(x), Some(y)) => {
                if x.fingerprint() == y.fingerprint() {
                    continue;
                }
                if x.deleted != y.deleted {
                    diff.changes.push(EntityChange::Modified(
                        y.id.clone(),
                        vec![FieldChange {
                            field: "deleted".into(),
                            before: Some(Value::Bool(x.deleted)),
                            after: Some(Value::Bool(y.deleted)),
                        }],
                    ));
                    continue;
                }
                let keys: BTreeSet<&str> = x
                    .fields
                    .keys()
                    .chain(y.fields.keys())
                    .map(String::as_str)
                    .collect();
                let mut changes = Vec::new();
                for k in keys {
                    let before = x.fields.get(k).cloned();
                    let after = y.fields.get(k).cloned();
                    if before != after {
                        if schema.is_immutable(&y.kind, k) {
                            diff.warnings.push(format!(
                                "{} 的 immutable 字段 `{}` 发生变化，需人工核对教材原文",
                                y.id, k
                            ));
                        }
                        changes.push(FieldChange {
                            field: k.to_string(),
                            before,
                            after,
                        });
                    }
                }
                if !changes.is_empty() {
                    diff.changes.push(EntityChange::Modified(y.id.clone(), changes));
                }
            }
            (None, None) => {}
        }
    }
    diff
}

fn newer(a: &Entity, b: &Entity) -> std::cmp::Ordering {
    a.rev
        .cmp(&b.rev)
        .then(a.updated_at.cmp(&b.updated_at))
        .then(a.device.cmp(&b.device))
}

/// 三方合并。规则 deliberately 简单可测：
/// - 一方没改（fingerprint 等于 base）-> 直接取另一方；
/// - 两端都改 -> 逐字段选较新者；`immutable` 字段冲突则记 Conflict 并保留 local；
/// - 删除只认**显式墓碑**（`deleted = true`），绝不因「远端整份没有」推断删除。
pub fn three_way(
    base: &[Entity],
    local: &[Entity],
    remote: &[Entity],
    schema: &DomainSchema,
    policy: MergePolicy,
) -> Result<(Vec<Entity>, Diff)> {
    let b = to_map(base);
    let l = to_map(local);
    let r = to_map(remote);
    let ids: BTreeSet<&str> = b
        .keys()
        .chain(l.keys())
        .chain(r.keys())
        .copied()
        .collect();
    let mut out = Vec::with_capacity(ids.len());
    let mut warnings = Vec::new();
    let mut notices: Vec<String> = Vec::new();
    let mut conflicts: Vec<EntityChange> = Vec::new();

    for id in ids {
        let bb = b.get(id).copied();
        let ll = l.get(id).copied();
        let rr = r.get(id).copied();

        let chosen: Option<Entity> = match (ll, rr) {
            (None, None) => None,
            (Some(x), None) => Some(x.clone()),
            (None, Some(y)) => Some(y.clone()),
            (Some(x), Some(y)) => {
                let fp_x = x.fingerprint();
                let fp_y = y.fingerprint();
                let fp_b = bb.map(Entity::fingerprint);
                if fp_x == fp_y {
                    Some(x.clone())
                } else if policy == MergePolicy::PreferLocal {
                    Some(x.clone())
                } else if policy == MergePolicy::PreferRemote {
                    Some(y.clone())
                } else if fp_b.as_deref() == Some(fp_x.as_str()) {
                    Some(y.clone())
                } else if fp_b.as_deref() == Some(fp_y.as_str()) {
                    Some(x.clone())
                } else {
                    let (mut merged, winner_device) = if newer(x, y).is_ge() {
                        (x.clone(), x.device.clone())
                    } else {
                        (y.clone(), y.device.clone())
                    };
                    let keys: BTreeSet<String> = x
                        .fields
                        .keys()
                        .chain(y.fields.keys())
                        .cloned()
                        .collect();
                    for k in keys {
                        let a = x.fields.get(&k);
                        let c = y.fields.get(&k);
                        if a == c {
                            continue;
                        }
                        let base_v = bb.and_then(|e| e.fields.get(&k));
                        let pick = if a == base_v {
                            c.cloned()
                        } else if c == base_v {
                            a.cloned()
                        } else if newer(x, y).is_ge() {
                            a.cloned()
                        } else {
                            c.cloned()
                        };
                        if schema.is_immutable(&merged.kind, &k) {
                            warnings.push(format!(
                                "{id} 的 immutable 字段 `{k}` 双端不一致，已保留本地值，请人工裁决"
                            ));
                            conflicts.push(EntityChange::Conflict(id.to_string(), k.clone()));
                            if let Some(v) = a.cloned() {
                                merged.fields.insert(k.clone(), v);
                            }
                        } else {
                            // 两端都偏离基线且值不同 = 有一方的修改会被静默覆盖。
                            // 合并结果不变（仍按 rev/时间取较新者），但要在同步回执里说清楚。
                            if a != base_v && c != base_v {
                                let winner = if newer(x, y).is_ge() { "本地" } else { "远端" };
                                notices.push(format!(
                                    "{id} 的字段 `{k}` 双端都修改过，已按 rev/时间取{winner}值，请确认"
                                ));
                            }
                            match pick {
                                Some(v) => {
                                    merged.fields.insert(k.clone(), v);
                                }
                                None => {
                                    merged.fields.remove(&k);
                                }
                            }
                        }
                    }
                    merged.deleted = x.deleted || y.deleted;
                    merged.rev = x.rev.max(y.rev) + 1;
                    merged.updated_at = x.updated_at.max(y.updated_at);
                    merged.device = winner_device;
                    Some(merged)
                }
            }
        };
        if let Some(e) = chosen {
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    let mut diff = diff_entities(base, &out, schema);
    diff.warnings.extend(warnings);
    diff.notices.extend(notices);
    diff.changes.extend(conflicts);
    Ok((out, diff))
}

/// 事件合并：按幂等键去重 + 全序排序。append-only，天然无需 CRDT。
pub fn merge_events(a: &[ReviewEvent], b: &[ReviewEvent]) -> Vec<ReviewEvent> {
    let mut seen: HashSet<String> = HashSet::with_capacity(a.len() + b.len());
    let mut out: Vec<ReviewEvent> = a
        .iter()
        .chain(b.iter())
        .filter(|e| seen.insert(e.key()))
        .cloned()
        .collect();
    out.sort_by(|x, y| {
        x.ts.cmp(&y.ts)
            .then(x.seq.cmp(&y.seq))
            .then(x.device.cmp(&y.device))
            .then(x.card_id.cmp(&y.card_id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> DomainSchema {
        DomainSchema::from_toml(
            r#"
id="t"
name="t"
[[entities]]
kind="herb"
primary="name"
[[entities.fields]]
key="name"
[[entities.fields]]
key="effects"
indexable=true
reversible=true
[[entities.fields]]
key="dose"
immutable=true
"#,
        )
        .unwrap()
    }

    fn e(id: &str, effect: &str, dose: &str, rev: u64, ts: i64, device: &str) -> Entity {
        let mut x = Entity::new(id, "herb", device, ts);
        x.set("name", Value::Text(id.into()), ts);
        x.set("effects", Value::Text(effect.into()), ts);
        x.set("dose", Value::Text(dose.into()), ts);
        x.rev = rev;
        x
    }

    #[test]
    fn one_side_changed_wins() {
        let s = schema();
        let base = vec![e("a", "旧", "3g", 1, 1, "w")];
        let local = base.clone();
        let remote = vec![e("a", "新", "3g", 2, 2, "p")];
        let (out, diff) = three_way(&base, &local, &remote, &s, MergePolicy::default()).unwrap();
        assert_eq!(out[0].get("effects").unwrap().display(), "新");
        assert_eq!(diff.changes.len(), 1);
    }

    #[test]
    fn immutable_conflict_is_not_auto_merged() {
        let s = schema();
        let base = vec![e("a", "旧", "3g", 1, 1, "w")];
        let local = vec![e("a", "本地功效", "5g", 2, 2, "w")];
        let remote = vec![e("a", "远端功效", "9g", 2, 3, "p")];
        let (out, diff) = three_way(&base, &local, &remote, &s, MergePolicy::default()).unwrap();
        assert!(!diff.warnings.is_empty());
        assert_eq!(out[0].get("dose").unwrap().display(), "5g");
        assert!(diff.warnings.iter().any(|w| w.contains("dose")));
    }

    #[test]
    fn deletion_requires_explicit_tombstone() {
        let s = schema();
        let base = vec![e("a", "x", "3g", 1, 1, "w")];
        let local = base.clone();
        let remote: Vec<Entity> = vec![];
        let (out, _) = three_way(&base, &local, &remote, &s, MergePolicy::default()).unwrap();
        assert_eq!(out.len(), 1, "远端整份没有不能推断删除");
    }

    #[test]
    fn both_sides_changed_same_field_reports_notice() {
        let s = schema();
        let base = vec![e("a", "旧", "3g", 1, 1, "w")];
        let local = vec![e("a", "本地功效", "3g", 2, 2, "w")];
        let remote = vec![e("a", "远端功效", "3g", 3, 3, "p")];
        let (out, diff) = three_way(&base, &local, &remote, &s, MergePolicy::default()).unwrap();
        // 合并结果仍是 LWW（远端 rev 更大）
        assert_eq!(out[0].get("effects").unwrap().display(), "远端功效");
        // 但回执必须提示，且不能算成需要裁决的冲突
        assert!(diff.notices.iter().any(|n| n.contains("effects") && n.contains("双端")));
        assert!(diff.warnings.is_empty(), "普通字段双端改动不应计入 conflicts");
        assert!(diff.summary().contains("双端"), "SyncReport 文本应带提示");
    }

    #[test]
    fn merge_events_is_idempotent() {
        let x = ReviewEvent {
            card_id: "c".into(),
            ts: 10,
            grade: 3,
            device: "w".into(),
            seq: 0,
        };
        let m = merge_events(&[x.clone(), x.clone()], &[x]);
        assert_eq!(m.len(), 1);
    }
}
