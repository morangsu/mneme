use crate::model::*;
use mneme_core::{DomainSchema, Entity, Value, normalize, stable_id};
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn same(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}

fn display(e: &Entity, key: &str) -> String {
    e.get(key).map(Value::display).unwrap_or_default()
}

/// **首次接入公共库**：把当前公共快照写成 base，并逐字段标记溯源。
/// 本地与公共库不同的字段一律视为「用户个性化调整」(Origin::User)，
/// 之后的每次拉取都能精确区分「用户改的」与「上游改的」。
pub fn adopt(
    local: &[Entity],
    public: &PublicBundle,
    topo: &Topology,
    now: i64,
) -> ProvenanceStore {
    let local_map: HashMap<&str, &Entity> = local.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut store = ProvenanceStore {
        version: 1,
        upstream_rev: public.rev,
        upstream_checksum: public.checksum.clone(),
        entities: BTreeMap::new(),
    };
    for ue in &public.entities {
        let mut ep = EntityProvenance::default();
        for (key, v) in &ue.fields {
            if !topo.is_base_field(key) {
                continue;
            }
            let up = v.display();
            let mine = local_map
                .get(ue.id.as_str())
                .map(|e| display(e, key))
                .unwrap_or_default();
            let origin = if same(&mine, &up) {
                Origin::Upstream
            } else {
                Origin::User
            };
            ep.fields.insert(
                key.clone(),
                FieldProvenance {
                    upstream_value: up,
                    origin,
                    edited_at: if origin == Origin::User { now } else { 0 },
                },
            );
        }
        store.entities.insert(ue.id.clone(), ep);
    }
    store
}

/// 生成拉取计划。**纯函数，不写任何库**。
///
/// 判定规则（字段级三方合并）：
/// - `local == base` 且上游变了 -> 自动采用上游（用户没动过）
/// - 两边都变且值不同 -> **冲突，交用户逐项决定**
/// - 只有用户改过 -> 保留用户值
/// - 上游新增实体 -> 自动加入；上游移除且本地改过 -> 冲突；本地自建实体 -> 永不删
pub fn plan_pull(
    local: &[Entity],
    upstream_bundle: &PublicBundle,
    prov: &ProvenanceStore,
    schema: &DomainSchema,
    topo: &Topology,
    device: &str,
    now: i64,
) -> Result<PullPlan> {
    upstream_bundle.verify()?;
    let local_map: HashMap<&str, &Entity> = local.iter().map(|e| (e.id.as_str(), e)).collect();
    let up_map: HashMap<&str, &Entity> = upstream_bundle
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    let primary = schema
        .entities
        .first()
        .and_then(|e| e.primary.as_deref())
        .unwrap_or("name");

    let mut plan = PullPlan {
        id: stable_id(&[&upstream_bundle.checksum, &prov.upstream_rev.to_string(), &now.to_string()]),
        from_rev: prov.upstream_rev,
        to_rev: upstream_bundle.rev,
        public_checksum: upstream_bundle.checksum.clone(),
        created_at: now,
        auto: Vec::new(),
        conflicts: Vec::new(),
        additions: Vec::new(),
        removals: Vec::new(),
        summary_text: String::new(),
        upstream: upstream_bundle.clone(),
    };

    for (id, ue) in &up_map {
        let Some(le) = local_map.get(id) else {
            plan.additions.push((*ue).clone());
            continue;
        };
        let ep = prov.entities.get(*id);
        for (key, uv) in &ue.fields {
            if !topo.is_base_field(key) {
                continue; // 个人字段不参与公共库合并
            }
            let up = uv.display();
            let mine = display(le, key);
            let base = ep
                .and_then(|e| e.fields.get(key))
                .map(|f| f.upstream_value.clone())
                .unwrap_or_else(|| up.clone());

            let local_changed = !same(&mine, &base);
            let upstream_changed = !same(&up, &base);
            let immutable = schema.is_immutable(&ue.kind, key);
            // 保守判定：只要本地值 != 上次公共库值，就认为用户动过，
            // 不依赖 provenance 是否被及时更新（用户在 adopt 之后随手改字段是常态）。
            let personalized = local_changed
                || topo.is_personal_field(key)
                || ep.and_then(|e| e.fields.get(key))
                    .is_some_and(|f| f.origin == Origin::User);

            if !local_changed && upstream_changed {
                plan.auto.push(AutoMerge {
                    entity_id: (*id).to_string(),
                    field: key.clone(),
                    from: mine,
                    to: up.clone(),
                    reason: "本地未改动，自动采用上游".into(),
                });
            } else if local_changed && upstream_changed && !same(&mine, &up) {
                plan.conflicts.push(Conflict {
                    id: stable_id(&[id, key, "conflict"]),
                    entity_id: (*id).to_string(),
                    entity_name: display(le, primary),
                    field: key.clone(),
                    label: schema
                        .field(&ue.kind, key)
                        .map(|f| f.label.as_str())
                        .unwrap_or(key.as_str())
                        .to_string(),
                    base,
                    upstream: up,
                    local: mine,
                    immutable,
                    personalized,
                    suggestion: if personalized || immutable {
                        Decision::KeepMine
                    } else {
                        Decision::TakeUpstream
                    },
                    kind: if immutable {
                        ConflictKind::ImmutableClash
                    } else {
                        ConflictKind::BothChanged
                    },
                    page: le.source.as_ref().and_then(|s| s.page),
                });
            }
        }
    }

    // 上游已移除的实体：本地自建的永不删；用户改过的交用户决定
    for le in local.iter().filter(|e| !e.deleted) {
        if up_map.contains_key(le.id.as_str()) {
            continue;
        }
        let Some(ep) = prov.entities.get(&le.id) else {
            continue; // 无溯源记录 = 用户自建，保留
        };
        let edited = ep.fields.values().any(|f| f.origin == Origin::User);
        if edited {
            plan.conflicts.push(Conflict {
                id: stable_id(&[&le.id, "*", "removed"]),
                entity_id: le.id.clone(),
                entity_name: display(le, primary),
                field: "*".into(),
                label: "整条记录".into(),
                base: "公共库中存在".into(),
                upstream: "公共库已移除".into(),
                local: "本地保留且改过".into(),
                immutable: false,
                personalized: true,
                suggestion: Decision::KeepMine,
                kind: ConflictKind::UpstreamRemovedLocallyEdited,
                page: le.source.as_ref().and_then(|s| s.page),
            });
        } else {
            plan.removals.push(le.id.clone());
        }
    }
    plan.auto.sort_by(|a, b| a.entity_id.cmp(&b.entity_id).then(a.field.cmp(&b.field)));
    plan.conflicts.sort_by(|a, b| a.entity_id.cmp(&b.entity_id).then(a.field.cmp(&b.field)));
    plan.additions.sort_by(|a, b| a.id.cmp(&b.id));
    plan.summary_text = plan.summary();
    let _ = device;
    Ok(plan)
}

/// 提交裁决。**所有冲突都必须有决定，否则一项都不写**（返回 Unresolved）。
pub fn apply_decisions(
    plan: &PullPlan,
    resolutions: &[Resolution],
    local: &[Entity],
    schema: &DomainSchema,
    prov: &ProvenanceStore,
    topo: &Topology,
    device: &str,
    now: i64,
) -> Result<(Vec<Entity>, ProvenanceStore, PullReport)> {
    let mut by_id: BTreeMap<String, Decision> = BTreeMap::new();
    let mut custom: BTreeMap<String, String> = BTreeMap::new();
    let known: BTreeSet<&str> = plan.conflicts.iter().map(|c| c.id.as_str()).collect();
    for r in resolutions {
        if !known.contains(r.conflict_id.as_str()) {
            return Err(Error::UnknownConflict(r.conflict_id.clone()));
        }
        by_id.insert(r.conflict_id.clone(), r.decision);
        if let Some(v) = &r.custom_value {
            custom.insert(r.conflict_id.clone(), v.clone());
        }
    }
    let missing: Vec<String> = plan
        .conflicts
        .iter()
        .filter(|c| !by_id.contains_key(&c.id))
        .map(|c| c.id.clone())
        .collect();
    if !missing.is_empty() {
        return Err(Error::Unresolved(missing));
    }

    let mut map: BTreeMap<String, Entity> = local
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let up_map: HashMap<&str, &Entity> = plan
        .upstream
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    let mut new_prov = prov.clone();
    new_prov.version = 1;
    new_prov.upstream_rev = plan.to_rev;
    new_prov.upstream_checksum = plan.public_checksum.clone();

    let mut log: Vec<LogEntry> = Vec::new();
    let mut report = PullReport {
        plan_id: plan.id.clone(),
        new_rev: plan.to_rev,
        log: Vec::new(),
        ..Default::default()
    };

    // 1) 自动合并：直接用上游的完整 Value（不丢 Tags 类型）
    for a in &plan.auto {
        let Some(ue) = up_map.get(a.entity_id.as_str()) else { continue };
        let Some(v) = ue.fields.get(&a.field) else { continue };
        if let Some(e) = map.get_mut(&a.entity_id) {
            e.fields.insert(a.field.clone(), v.clone());
            e.updated_at = now;
            e.device = "upstream".into();
        }
        let ep = new_prov.entities.entry(a.entity_id.clone()).or_default();
        ep.fields.insert(
            a.field.clone(),
            FieldProvenance {
                upstream_value: v.display(),
                origin: Origin::Upstream,
                edited_at: 0,
            },
        );
        report.auto_applied += 1;
        log.push(LogEntry {
            ts: now, device: device.into(), action: "auto".into(),
            entity_id: a.entity_id.clone(), field: a.field.clone(),
            decision: Some("take_upstream".into()), from_rev: plan.from_rev, to_rev: plan.to_rev,
        });
    }

    // 2) 上游新增
    for e in &plan.additions {
        map.insert(e.id.clone(), e.clone());
        let mut ep = EntityProvenance::default();
        for (k, v) in &e.fields {
            if topo.is_base_field(k) {
                ep.fields.insert(k.clone(), FieldProvenance {
                    upstream_value: v.display(), origin: Origin::Upstream, edited_at: 0,
                });
            }
        }
        new_prov.entities.insert(e.id.clone(), ep);
        report.added += 1;
        log.push(LogEntry {
            ts: now, device: device.into(), action: "add".into(),
            entity_id: e.id.clone(), field: "*".into(), decision: None,
            from_rev: plan.from_rev, to_rev: plan.to_rev,
        });
    }

    // 3) 上游移除（显式墓碑，不物理删除）
    for id in &plan.removals {
        if let Some(e) = map.get_mut(id) {
            e.deleted = true;
            e.updated_at = now;
            e.device = "upstream".into();
        }
        new_prov.entities.remove(id);
        report.removed += 1;
    }

    // 4) 冲突：逐项按用户决定执行
    for c in &plan.conflicts {
        let d = by_id[&c.id];
        if c.kind == ConflictKind::UpstreamRemovedLocallyEdited {
            if d == Decision::TakeUpstream {
                if let Some(e) = map.get_mut(&c.entity_id) {
                    e.deleted = true;
                    e.updated_at = now;
                }
                new_prov.entities.remove(&c.entity_id);
                report.take_upstream += 1;
            } else {
                report.keep_mine += 1;
            }
            log.push(LogEntry {
                ts: now, device: device.into(), action: "conflict".into(),
                entity_id: c.entity_id.clone(), field: c.field.clone(),
                decision: Some(format!("{d:?}").to_ascii_lowercase()),
                from_rev: plan.from_rev, to_rev: plan.to_rev,
            });
            report.conflicts_resolved += 1;
            continue;
        }

        let ue = up_map.get(c.entity_id.as_str());
        let up_value = ue.and_then(|e| e.fields.get(&c.field)).cloned();
        match d {
            Decision::KeepMine => {
                report.keep_mine += 1;
            }
            Decision::TakeUpstream => {
                if let (Some(e), Some(v)) = (map.get_mut(&c.entity_id), &up_value) {
                    e.fields.insert(c.field.clone(), v.clone());
                    e.updated_at = now;
                    e.device = "upstream".into();
                }
                report.take_upstream += 1;
            }
            Decision::KeepBoth => {
                // 主字段保留用户值，上游值追加到 synonyms（个人字段，绝不回传公共库）
                if let Some(e) = map.get_mut(&c.entity_id) {
                    let mut tags = match e.get("synonyms") {
                        Some(Value::Tags(v)) => v.clone(),
                        Some(Value::Text(s)) => vec![s.clone()],
                        _ => Vec::new(),
                    };
                    if !tags.iter().any(|x| same(x, &c.upstream)) {
                        tags.push(c.upstream.clone());
                    }
                    e.fields.insert("synonyms".into(), Value::Tags(tags));
                    e.updated_at = now;
                }
                report.keep_both += 1;
            }
        }
        if let Some(v) = custom.get(&c.id) {
            if let Some(e) = map.get_mut(&c.entity_id) {
                let kind = schema
                    .field(&e.kind, &c.field)
                    .map(|f| f.kind)
                    .unwrap_or(mneme_core::FieldKind::Text);
                let raw = serde_json::Value::String(v.clone());
                if let Some(value) = Value::from_raw(kind, &raw) {
                    e.fields.insert(c.field.clone(), value);
                }
            }
        }

        // 关键：即使 KeepMine，也把 base 推进到本次上游值，
        // 否则同一冲突会在每次拉取时反复出现。
        let ep = new_prov.entities.entry(c.entity_id.clone()).or_default();
        ep.fields.insert(
            c.field.clone(),
            FieldProvenance {
                upstream_value: c.upstream.clone(),
                origin: if d == Decision::TakeUpstream { Origin::Upstream } else { Origin::User },
                edited_at: if d == Decision::TakeUpstream { 0 } else { now },
            },
        );
        report.conflicts_resolved += 1;
        log.push(LogEntry {
            ts: now, device: device.into(), action: "conflict".into(),
            entity_id: c.entity_id.clone(), field: c.field.clone(),
            decision: Some(format!("{d:?}").to_ascii_lowercase()),
            from_rev: plan.from_rev, to_rev: plan.to_rev,
        });
    }

    report.log = log;
    let entities: Vec<Entity> = map.into_values().collect();
    Ok((entities, new_prov, report))
}

/// 维护者侧：从本地数据生成**只含基础内容**的公共包。
/// 个人字段（掌握度、同义、批注…）一律剔除，绝不外发。
pub fn build_public_bundle(
    local: &[Entity],
    groups: &[mneme_core::Group],
    rev: u64,
    topo: &Topology,
    now: i64,
) -> PublicBundle {
    let entities = local
        .iter()
        .filter(|e| !e.deleted)
        .map(|e| {
            let mut x = e.clone();
            x.fields.retain(|k, _| topo.is_base_field(k));
            x.source = e.source.as_ref().map(|s| mneme_core::SourceAnchor {
                page: s.page,
                ..Default::default()
            });
            x.device = "public".into();
            x
        })
        .collect();
    PublicBundle::new(rev, &topo.schema_id, entities, groups.to_vec(), now)
}
