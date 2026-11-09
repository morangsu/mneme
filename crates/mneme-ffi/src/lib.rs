//! # mneme-ffi
//!
//! uniffi 绑定：把 Rust 内核原样暴露给 Android Compose。
//! **APP 不重写任何业务逻辑** —— 归一化、反查、出题、判分、三方合并、
//! 上游冲突判定、同步协议全部在 Rust 侧，Kotlin 只负责 UI 与系统能力。
//!
//! 契约：所有方法返回 JSON 字符串，错误也用 `{"error":"..."}` 表达，
//! 不跨 FFI 抛异常。方法名由 uniffi 自动转成 camelCase。

use mneme_codec::{self as codec, Format, ImportOptions, ImportStrategy};
use mneme_core::{
    DomainSchema, Entity, ReviewEvent, Sm2Scheduler, TermTable, due_cards, fold_events,
    generate_choices, judge as judge_answer, now_ms,
};
use mneme_store::{Op, Store};
use mneme_sync::{SyncConfig, SyncEngine, incremental};
use mneme_upstream::{
    Decision, FilePublicSource, GitHubPublicSource, ProvenanceStore, PublicBundle, PublicSource,
    PullPlan, Resolution, Topology, adopt, apply_decisions, plan_pull,
};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

uniffi::setup_scaffolding!();

struct App {
    root: PathBuf,
    store: Arc<Store>,
    engine: Option<Arc<SyncEngine>>,
    topology: Topology,
    provenance: ProvenanceStore,
    plans: BTreeMap<String, PullPlan>,
    public: Option<Arc<dyn PublicSource>>,
}

static STATE: OnceLock<Mutex<Option<App>>> = OnceLock::new();
static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn lock() -> std::result::Result<MutexGuard<'static, Option<App>>, String> {
    let l = STATE.get().ok_or_else(|| "未调用 openDatabase".to_string())?;
    let g = l.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        return Err("数据库未打开".into());
    }
    Ok(g)
}

fn call<F>(f: F) -> String
where
    F: FnOnce(&mut App) -> Json,
{
    match lock() {
        Ok(mut g) => f(g.as_mut().expect("checked")).to_string(),
        Err(e) => json!({ "error": e }).to_string(),
    }
}

fn err(e: impl std::fmt::Display) -> Json {
    json!({ "error": e.to_string() })
}

fn load_topology(root: &Path) -> Topology {
    let p = root.join("topology.json");
    fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Topology>(&b).ok())
        .unwrap_or_else(|| Topology::example("morangsu"))
}

fn load_provenance(root: &Path) -> ProvenanceStore {
    let p = root.join("upstream/provenance.json");
    fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<ProvenanceStore>(&b).ok())
        .unwrap_or_default()
}

fn save_json(root: &Path, rel: &str, v: &impl serde::Serialize) -> std::io::Result<()> {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default())?;
    fs::rename(tmp, p)
}

fn schema_of(a: &App) -> Option<DomainSchema> {
    a.store.read(|m| m.schema.clone())
}

fn resolve_field(schema: &DomainSchema, raw: &str) -> Option<String> {
    if raw.contains('.') {
        return Some(raw.to_string());
    }
    for e in &schema.entities {
        for f in &e.fields {
            if f.key == raw || f.label == raw {
                return Some(format!("{}.{}", e.kind, f.key));
            }
        }
    }
    None
}

fn parse_clues(schema: &DomainSchema, raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|part| {
            let (lhs, rhs) = part.split_once(':').unwrap_or(("", part));
            let field = if lhs.is_empty() {
                schema.entities.first().and_then(|e| {
                    e.fields
                        .iter()
                        .find(|f| f.reversible)
                        .map(|f| format!("{}.{}", e.kind, f.key))
                })
            } else {
                resolve_field(schema, lhs)
            };
            field
                .map(|f| (f, rhs.trim().to_string()))
                .filter(|(_, v)| !v.is_empty())
        })
        .collect()
}

// ---------------------------------------------------------------- 生命周期

#[uniffi::export]
pub fn open_database(path: String, device: String) -> String {
    let root = PathBuf::from(&path);
    let result = (|| -> std::result::Result<Json, String> {
        let store = Store::open(&root, &device).map_err(|e| e.to_string())?;
        let engine = SyncConfig::from_file(&root.join("sync.json"))
            .ok()
            .map(|c| Arc::new(SyncEngine::new(store.clone(), c)));
        let topology = load_topology(&root);
        let provenance = load_provenance(&root);
        let public: Option<Arc<dyn PublicSource>> = {
            let local = root.join("upstream/bundle.json");
            if local.exists() {
                Some(Arc::new(FilePublicSource::new(local)))
            } else {
                GitHubPublicSource::new(topology.public_data.clone(), "bundle.json")
                    .ok()
                    .map(|s| Arc::new(s) as Arc<dyn PublicSource>)
            }
        };
        let app = App {
            root,
            store,
            engine,
            topology,
            provenance,
            plans: BTreeMap::new(),
            public,
        };
        let _ = STATE.set(Mutex::new(Some(app)));
        Ok(json!({ "ok": true, "path": path, "device": device }))
    })();
    match result {
        Ok(v) => v.to_string(),
        Err(e) => json!({ "error": e }).to_string(),
    }
}

#[uniffi::export]
pub fn state() -> String {
    call(|a| {
        let st = a.store.stats();
        let (schema, aliases) = a.store.read(|m| {
            (
                m.schema.as_ref().map(|x| x.name.clone()).unwrap_or_default(),
                m.aliases.len(),
            )
        });
        let sqlite = a.store.sqlite().map(|s| {
            let c = s.counts().ok();
            json!({
                "path": s.path().to_string_lossy(),
                "entities": c.map(|x| x.entities).unwrap_or(0),
                "cards": c.map(|x| x.cards).unwrap_or(0),
                "events": c.map(|x| x.events).unwrap_or(0),
            })
        });
        json!({
            "schema": schema, "entities": st.entities, "groups": st.groups,
            "cards": st.cards, "events": st.events, "rev": st.rev,
            "aliases": aliases, "device": st.device,
            "upstream_rev": a.provenance.upstream_rev,
            "topology": a.topology.schema_id,
            "sqlite": sqlite,
        })
    })
}

// ---------------------------------------------------------------- 视图与反查

#[uniffi::export]
pub fn views() -> String {
    call(|a| {
        let v = a.store.read(|m| m.schema.as_ref().map(|x| x.views.clone()).unwrap_or_default());
        json!({ "views": v })
    })
}

#[uniffi::export]
pub fn view(id: String) -> String {
    call(|a| {
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let Some(v) = schema.views.iter().find(|x| x.id == id).cloned() else {
            return err(format!("未知视图 {id}"));
        };
        let (entities, index) = a.store.read(|m| (m.entities_vec(), m.entities_vec()));
        let _ = index;
        let kind = schema.entities.first().map(|e| e.kind.as_str()).unwrap_or("");
        let primary = schema
            .entity_def(kind)
            .and_then(|d| d.primary.as_deref())
            .unwrap_or("name");
        let columns: Vec<Json> = v
            .columns
            .iter()
            .map(|k| {
                json!({
                    "key": k,
                    "label": schema.field(kind, k).map(|f| f.label.as_str()).unwrap_or(k.as_str()),
                    "immutable": schema.is_immutable(kind, k),
                })
            })
            .collect();
        let mut list: Vec<Entity> = entities
            .into_iter()
            .filter(|e| !e.deleted)
            .filter(|e| match &v.filter {
                None => true,
                Some(f) => {
                    let cur = e.get(&f.field).map(|x| x.display()).unwrap_or_default();
                    match f.op {
                        mneme_core::FilterOp::Eq => cur == f.value,
                        mneme_core::FilterOp::Ne => cur != f.value,
                        mneme_core::FilterOp::Contains => cur.contains(&f.value),
                        mneme_core::FilterOp::NonEmpty => !cur.is_empty(),
                    }
                }
            })
            .collect();
        if let Some(g) = &v.group_by {
            list.sort_by(|x, y| {
                x.get(g).map(|z| z.display()).cmp(&y.get(g).map(|z| z.display()))
                    .then(x.id.cmp(&y.id))
            });
        }
        let group_key = v.group_by.clone().unwrap_or_default();
        let rows: Vec<Json> = list
            .iter()
            .map(|e| {
                let mut row = serde_json::Map::new();
                for c in &v.columns {
                    row.insert(c.clone(), json!(e.get(c).map(|x| x.display()).unwrap_or_default()));
                }
                if !group_key.is_empty() {
                    row.insert("_group".into(), json!(e.get(&group_key).map(|x| x.display()).unwrap_or_default()));
                }
                Json::Object(row)
            })
            .collect();
        json!({ "columns": columns, "primary": primary, "hidden": v.hidden, "group_by": v.group_by, "rows": rows })
    })
}

#[uniffi::export]
pub fn reverse(clues_json: String) -> String {
    call(|a| {
        let clues: Vec<String> = match serde_json::from_str(&clues_json) {
            Ok(v) => v,
            Err(e) => return err(format!("cluesJson 解析失败: {e}")),
        };
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let parsed = parse_clues(&schema, &clues);
        let (hits, ids, entities) = a.store.read(|m| {
            let hits = m.index.reverse(&parsed);
            let ids: Vec<String> = hits.iter().filter_map(|h| m.index.id_of(h.idx).map(String::from)).collect();
            (hits, ids, m.entities_vec())
        });
        let kind = schema.entities.first().map(|e| e.kind.as_str()).unwrap_or("");
        let primary = schema.entity_def(kind).and_then(|d| d.primary.as_deref()).unwrap_or("name");
        let items: Vec<Json> = hits
            .iter()
            .zip(ids.iter())
            .filter_map(|(h, id)| {
                let e = entities.iter().find(|x| &x.id == id)?;
                Some(json!({
                    "id": id, "name": e.display_of(primary), "group": e.group_id,
                    "score": h.score,
                    "summary": e.fields.iter().filter(|(k,_)| !schema.is_immutable(&e.kind, k))
                        .take(3).map(|(k,v)| format!("{k}:{}", v.display())).collect::<Vec<_>>().join(" / "),
                }))
            })
            .collect();
        json!({ "clues": parsed, "items": items })
    })
}

// ---------------------------------------------------------------- 复习

#[uniffi::export]
pub fn quiz(limit: u32) -> String {
    call(|a| {
        let (cards, states, entities, schema) = a.store.read(|m| {
            (
                m.cards.clone(),
                fold_events(&m.events, &Sm2Scheduler::default()),
                m.entities_vec(),
                m.schema.clone(),
            )
        });
        let due = due_cards(&states, now_ms(), cards.len());
        let mut picked: Vec<_> = due.iter().filter_map(|id| cards.iter().find(|c| &c.id == id)).collect();
        for c in &cards {
            if picked.len() >= limit as usize { break; }
            if !picked.iter().any(|p| p.id == c.id) { picked.push(c); }
        }
        picked.truncate(limit as usize);
        let out: Vec<Json> = picked
            .iter()
            .map(|c| {
                let choices = schema.as_ref().and_then(|s| {
                    entities.iter().find(|e| e.id == c.entity_id).map(|e| generate_choices(s, e, &entities, 3))
                }).unwrap_or_default();
                json!({
                    "id": c.id, "prompt": c.prompt, "direction": c.direction,
                    "choices": choices, "boundary": c.immutable_boundary,
                })
            })
            .collect();
        json!({ "due": due.len(), "cards": out })
    })
}

#[uniffi::export]
pub fn answer(card_id: String) -> String {
    call(|a| {
        let found = a.store.read(|m| m.cards.iter().find(|c| c.id == card_id).cloned());
        match found {
            Some(c) => json!({ "answer": c.answer, "accept": c.accept, "boundary": c.immutable_boundary }),
            None => err("卡片不存在"),
        }
    })
}

#[uniffi::export]
pub fn review(card_id: String, grade: u8) -> String {
    call(|a| {
        let ev = ReviewEvent {
            card_id: card_id.clone(),
            ts: now_ms(),
            grade: grade.clamp(1, 4),
            device: a.store.device().to_string(),
            seq: 0,
        };
        match a.store.mutate(&[Op::Review { event: ev.clone() }]) {
            Ok(n) => json!({ "ok": true, "applied": n, "event": ev.key() }),
            Err(e) => err(e),
        }
    })
}

#[uniffi::export]
pub fn judge(card_id: String, input: String) -> String {
    call(|a| {
        let (card, defs) = a.store.read(|m| {
            (
                m.cards.iter().find(|c| c.id == card_id).cloned(),
                m.schema.as_ref().map(|x| x.aliases.clone()).unwrap_or_default(),
            )
        });
        let Some(card) = card else { return err("卡片不存在") };
        let table = TermTable::from_defs(&defs);
        let j = judge_answer(&input, &card, &table, 0.85);
        json!({ "score": j.score, "correct": j.correct, "matched": j.matched })
    })
}

// ---------------------------------------------------------------- 同步与导入导出

#[uniffi::export]
pub fn sync(target_id: String, dry: bool) -> String {
    call(|a| {
        let Some(engine) = a.engine.clone() else {
            return err("未配置 sync.json（私有仓库同步配置）");
        };
        match runtime().block_on(engine.sync(&target_id, dry)) {
            Ok(r) => json!({
                "ok": true, "report": r.to_string(), "sha256": r.sha256,
                "retries": r.retries, "conflicts": r.conflicts,
                "bytes_up": r.bytes_up, "bytes_down": r.bytes_down,
                "target": r.target, "backend": r.backend,
            }),
            Err(e) => err(e),
        }
    })
}

#[uniffi::export]
pub fn import_data(content: String, filename: String, dry: bool) -> String {
    call(|a| {
        let bytes = content.as_bytes();
        let fmt = codec::detect(Some(Path::new(&filename)), bytes);
        if !fmt.importable() { return err(format!("{fmt} 不支持导入")); }
        match codec::import(&a.store, bytes, fmt, &ImportOptions {
            dry_run: dry,
            strategy: ImportStrategy::Merge,
            verify_hash: true,
        }) {
            Ok(r) => json!(r),
            Err(e) => err(e),
        }
    })
}

#[uniffi::export]
pub fn export_data(format: String) -> String {
    call(|a| {
        let fmt = match Format::from_str(&format) {
            Ok(f) => f,
            Err(e) => return err(e),
        };
        match codec::export_bytes(&a.store, fmt, now_ms()) {
            Ok((bytes, r)) => json!({
                "format": r.format, "content": String::from_utf8_lossy(&bytes).to_string(),
                "bytes": r.bytes, "sha256": r.checksum, "filename": format!("mneme-export.{}", fmt.extension()),
            }),
            Err(e) => err(e),
        }
    })
}

// ---------------------------------------------------------------- 上游拉取与冲突裁决

#[uniffi::export]
pub fn upstream_status() -> String {
    call(|a| {
        json!({
            "upstream_rev": a.provenance.upstream_rev,
            "upstream_checksum": a.provenance.upstream_checksum,
            "tracked_entities": a.provenance.entities.len(),
            "pending_plans": a.plans.len(),
            "topology": {
                "app_repo": a.topology.app_repo.slug(),
                "public_data": a.topology.public_data.slug(),
                "private_data": a.topology.private_data.slug(),
            },
            "public_source": a.public.as_ref().map(|s| s.id().to_string()),
        })
    })
}

#[uniffi::export]
pub fn upstream_pull() -> String {
    call(|a| {
        let Some(source) = a.public.clone() else { return err("未配置公共基础库") };
        let bytes = match runtime().block_on(source.fetch_if_changed()) {
            Ok(Some(b)) => b,
            Ok(None) => return json!({ "unchanged": true, "upstream_rev": a.provenance.upstream_rev }),
            Err(e) => return err(e),
        };
        let bundle = match PublicBundle::from_bytes(&bytes) {
            Ok(b) => b,
            Err(e) => return err(e),
        };
        if bundle.rev <= a.provenance.upstream_rev {
            return json!({ "unchanged": true, "upstream_rev": a.provenance.upstream_rev });
        }
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let local = a.store.read(|m| m.entities_vec());
        let device = a.store.device().to_string();
        let plan = match plan_pull(&local, &bundle, &a.provenance, &schema, &a.topology, &device, now_ms()) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
        let _ = save_json(&a.root, &format!("upstream/staging-{}.json", plan.id), &plan);
        let value = serde_json::to_value(&plan).unwrap_or_else(|_| json!({}));
        a.plans.insert(plan.id.clone(), plan);
        value
    })
}

#[uniffi::export]
pub fn upstream_resolve(plan_id: String, decisions_json: String) -> String {
    call(|a| {
        let Some(plan) = a.plans.get(&plan_id).cloned() else {
            return err("拉取计划不存在或已提交，请重新 upstreamPull");
        };
        let raw: Vec<Json> = match serde_json::from_str(&decisions_json) {
            Ok(v) => v,
            Err(e) => return err(format!("decisionsJson 解析失败: {e}")),
        };
        let mut resolutions: Vec<Resolution> = Vec::new();
        for r in raw {
            let id = r["conflict_id"].as_str().unwrap_or_default().to_string();
            let d = Decision::from_str(r["decision"].as_str().unwrap_or("keep_mine"))
                .unwrap_or(Decision::KeepMine);
            resolutions.push(Resolution {
                conflict_id: id,
                decision: d,
                custom_value: r["custom_value"].as_str().map(String::from),
            });
        }
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let local = a.store.read(|m| m.entities_vec());
        let device = a.store.device().to_string();
        let now = now_ms();
        let (entities, new_prov, report) = match apply_decisions(
            &plan, &resolutions, &local, &schema, &a.provenance, &a.topology, &device, now,
        ) {
            Ok(x) => x,
            Err(e) => return err(e), // 未决冲突：一项都不写
        };
        let max_rev = a.store.stats().rev;
        let ops: Vec<Op> = entities
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut x = e.clone();
                x.rev = max_rev + i as u64 + 1;
                x.updated_at = now;
                x.device = device.clone();
                Op::UpsertEntity { entity: x }
            })
            .collect();
        if let Err(e) = a.store.mutate(&ops) {
            return err(e);
        }
        a.provenance = new_prov.clone();
        let _ = save_json(&a.root, "upstream/provenance.json", &new_prov);
        let _ = save_json(&a.root, "upstream/last-report.json", &report);

        // 决策日志本地留痕 + 尽力推到私有库（GitHub 每次写入都是一个 commit）
        let entries: Vec<Json> = report.log.iter().map(|e| json!(e)).collect();
        let log_path = a.root.join("upstream/decisions.jsonl");
        if let Ok(mut old) = fs::read_to_string(&log_path) {
            for e in &entries {
                old.push_str(&e.to_string());
                old.push('\n');
            }
            let _ = fs::write(&log_path, old);
        } else {
            let mut s = String::new();
            for e in &entries { s.push_str(&e.to_string()); s.push('\n'); }
            let _ = fs::write(&log_path, s);
        }
        let mut pushed = false;
        if let Some(engine) = &a.engine {
            if let Ok(t) = engine.target("github-private") {
                if runtime().block_on(incremental::append_decisions(&*t, &entries)).is_ok() {
                    pushed = true;
                }
            }
        }
        a.plans.remove(&plan_id);
        json!({
            "ok": true, "plan_id": plan_id,
            "auto_applied": report.auto_applied,
            "conflicts_resolved": report.conflicts_resolved,
            "added": report.added, "removed": report.removed,
            "keep_mine": report.keep_mine, "take_upstream": report.take_upstream,
            "keep_both": report.keep_both,
            "new_rev": report.new_rev,
            "decision_log_pushed_to_private": pushed,
            "log": report.log,
        })
    })
}

/// 首次接入公共库：把当前公共快照写成 base，并标记出用户已有的个性化调整。
#[uniffi::export]
pub fn upstream_adopt() -> String {
    call(|a| {
        let Some(source) = a.public.clone() else { return err("未配置公共基础库") };
        let bytes = match runtime().block_on(source.fetch_if_changed()) {
            Ok(Some(b)) => b,
            Ok(None) => return err("公共库内容未变化，若确需重建请删除本地缓存后重试"),
            Err(e) => return err(e),
        };
        let bundle = match PublicBundle::from_bytes(&bytes) { Ok(b) => b, Err(e) => return err(e) };
        let local = a.store.read(|m| m.entities_vec());
        a.provenance = adopt(&local, &bundle, &a.topology, now_ms());
        let _ = save_json(&a.root, "upstream/provenance.json", &a.provenance);
        json!({
            "ok": true, "upstream_rev": a.provenance.upstream_rev,
            "tracked_entities": a.provenance.entities.len(),
            "note": "本地与公共库不同的字段已标记为个性化调整，后续拉取会逐项交用户决定",
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mneme_core::{Value, normalize};

    #[test]
    fn ffi_smoke_without_native_lib() {
        let root = std::env::temp_dir().join(format!("mneme-ffi-{}", now_ms()));
        let s = Store::open(&root, "test").unwrap();
        let toml = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../domains/materia-medica.toml"),
        ).unwrap();
        s.mutate(&[Op::SetSchema { toml }]).unwrap();
        let app = App {
            root: root.clone(),
            store: s,
            engine: None,
            topology: Topology::example("tester"),
            provenance: ProvenanceStore::default(),
            plans: BTreeMap::new(),
            public: None,
        };
        let _ = STATE.set(Mutex::new(Some(app)));
        assert!(state().contains("中药学"));
        assert!(views().contains("matrix"));
        assert!(quiz(1).contains("cards"));
        assert!(upstream_status().contains("topology"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_clues_maps_labels() {
        let s = DomainSchema::from_toml(
            "id=\"t\"\nname=\"t\"\n[[entities]]\nkind=\"herb\"\n[[entities.fields]]\nkey=\"effects\"\nlabel=\"功效\"\nindexable=true\nreversible=true\n",
        ).unwrap();
        let c = parse_clues(&s, &["功效:宣肺平喘".into(), "herb.effects:利水消肿".into()]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, "herb.effects");
        assert_eq!(normalize("ＡＢＣ"), "abc");
        let _ = Value::Text(String::new());
    }
}
