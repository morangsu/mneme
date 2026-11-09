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
    DomainSchema, Entity, FieldKind, ReviewConfig, ReviewEvent, TermTable, Value, daily_queue,
    fold_events, generate_choices, judge as judge_answer, mastery_label_ext, now_ms,
    suggest_grade as core_suggest_grade, weak_cards as core_weak_cards,
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
    /// 复习参数（可从 review.json 覆盖）
    review: ReviewConfig,
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

fn load_review_config(root: &Path) -> ReviewConfig {
    fs::read(root.join("review.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<ReviewConfig>(&b).ok())
        .map(ReviewConfig::normalized)
        .unwrap_or_default()
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
        let cfg = SyncConfig::from_file(&root.join("sync.json")).ok();
        // 公开库是只读的；把私有库的 token 复用过去，
        // 免掉匿名 60/h 配额（手机上没有环境变量可放凭据）。
        let gh_token = cfg.as_ref().and_then(|c| {
            c.targets.iter().find_map(|t| match t {
                mneme_sync::TargetConfig::GitHub { token, .. }
                    if !token.starts_with("${ENV:") =>
                {
                    Some(token.clone())
                }
                _ => None,
            })
        });
        let engine = cfg.map(|c| Arc::new(SyncEngine::new(store.clone(), c)));
        let topology = load_topology(&root);
        let provenance = load_provenance(&root);
        let public: Option<Arc<dyn PublicSource>> = {
            let local = root.join("upstream/bundle.json");
            if local.exists() {
                Some(Arc::new(FilePublicSource::new(local)))
            } else {
                Some(Arc::new(GitHubPublicSource::with_token(
                    topology.public_data.clone(),
                    "bundle.json",
                    gh_token,
                )) as Arc<dyn PublicSource>)
            }
        };
        let review = load_review_config(&root);
        let app = App {
            root,
            store,
            engine,
            topology,
            provenance,
            plans: BTreeMap::new(),
            public,
            review,
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
        let (schema, aliases, kind, recycle) = a.store.read(|m| {
            (
                m.schema.as_ref().map(|x| x.name.clone()).unwrap_or_default(),
                m.aliases.len(),
                m.schema
                    .as_ref()
                    .and_then(|x| x.entities.first().map(|e| e.kind.clone()))
                    .unwrap_or_default(),
                m.entities.values().filter(|e| e.deleted).count(),
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
            "kind": kind, "recycle": recycle,
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
                // 行里必须带实体 id：否则 UI 只能靠主键文本反查，改名后就对不上了。
                row.insert("_id".into(), json!(e.id));
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
    // 老入口：到期与新卡合计不超过 limit。新代码请用 quizPlan。
    quiz_queue_inner(limit, limit, limit)
}

/// 每日队列：到期优先（越逾期越靠前）、新卡限量（默认 10 / 50）。
/// `weak` 是错题本张数，UI 据此显示「薄弱 N 张」入口。
#[uniffi::export]
pub fn quiz_queue(new_limit: u32, due_limit: u32) -> String {
    quiz_queue_inner(new_limit, due_limit, 0)
}

/// 按用户配置出题（新卡/天、到期/天、本轮张数、期望记忆率都在 `review.json`）。
#[uniffi::export]
pub fn quiz_plan() -> String {
    let (new_limit, due_limit, session) = lock()
        .map(|g| {
            let a = g.as_ref().expect("checked");
            (
                a.review.new_per_day,
                a.review.reviews_per_day,
                a.review.session_limit,
            )
        })
        .unwrap_or((10, 100, 20));
    quiz_queue_inner(new_limit, due_limit, session)
}

/// 读取复习参数。
#[uniffi::export]
pub fn review_config() -> String {
    call(|a| json!(a.review))
}

/// 写入复习参数：越界值自动夹回合理区间，并落盘到 `review.json`。
#[uniffi::export]
pub fn set_review_config(config_json: String) -> String {
    call(|a| {
        let cfg: ReviewConfig = match serde_json::from_str::<ReviewConfig>(&config_json) {
            Ok(c) => c.normalized(),
            Err(e) => return err(format!("复习配置 JSON 有误: {e}")),
        };
        if let Err(e) = save_json(&a.root, "review.json", &cfg) {
            return err(format!("保存 review.json 失败: {e}"));
        }
        a.review = cfg.clone();
        json!({ "ok": true, "config": cfg })
    })
}

fn quiz_queue_inner(new_limit: u32, due_limit: u32, session_limit: u32) -> String {
    call(|a| {
        let now = now_ms();
        let (cards, events, entities, schema) = a.store.read(|m| {
            (
                m.cards.clone(),
                m.events.clone(),
                m.entities_vec(),
                m.schema.clone(),
            )
        });
        let sched = a.review.scheduler();
        let states = fold_events(&events, &sched);
        let q = daily_queue(&cards, &states, now, new_limit as usize, due_limit as usize);
        let weak_n = core_weak_cards(&events, now, usize::MAX).len();
        let leech_n = states.values().filter(|s| s.leech).count();

        let mut ids = q.items.clone();
        if session_limit > 0 && ids.len() > session_limit as usize {
            ids.truncate(session_limit as usize);
        }

        let entity_name = |eid: &str| {
            entities
                .iter()
                .find(|e| e.id == eid)
                .map(|e| e.display_of("name"))
                .unwrap_or_else(|| eid.to_string())
        };
        // 答案是不是「药名」：是的话 UI 绝不能把实体名露出来（等于直接给答案）。
        // 判定依据是卡片自己的 answer_fields，而不是方向字符串 —— 领域包换了方向名也不会失效。
        let ask_name = |c: &mneme_core::Card| {
            let primary = entities
                .iter()
                .find(|e| e.id == c.entity_id)
                .and_then(|e| {
                    schema
                        .as_ref()
                        .and_then(|s| s.entity_def(&e.kind))
                        .and_then(|d| d.primary.clone())
                })
                .unwrap_or_else(|| "name".into());
            c.answer_fields.iter().any(|f| *f == primary)
        };
        let items: Vec<Json> = ids
            .iter()
            .filter_map(|id| cards.iter().find(|c| &c.id == id))
            .map(|c| {
                let choices = schema.as_ref().and_then(|s| {
                    entities
                        .iter()
                        .find(|e| e.id == c.entity_id)
                        .map(|e| generate_choices(s, e, &entities, 3))
                }).unwrap_or_default();
                let st = states.get(&c.id);
                let elapsed_days = st
                    .map(|s| (now - s.last).max(0) as f64 / 86_400_000.0)
                    .unwrap_or(0.0);
                json!({
                    "id": c.id, "prompt": c.prompt, "direction": c.direction,
                    "entity_id": c.entity_id,
                    "entity_name": entity_name(&c.entity_id),
                    "ask_name": ask_name(c),
                    "answer_fields": c.answer_fields,
                    "choices": choices, "boundary": c.immutable_boundary,
                    // 记忆状态透明化：让用户看见「这张卡多久后该复习、有多难」
                    "stability": st.map(|s| (s.stability * 100.0).round() / 100.0),
                    "difficulty": st.map(|s| (s.difficulty * 100.0).round() / 100.0),
                    "lapses": st.map(|s| s.lapses).unwrap_or(0),
                    "leech": st.map(|s| s.leech).unwrap_or(false),
                    "retrievability": st.map(|s| {
                        (sched.retrievability(elapsed_days, s.stability) * 100.0).round() / 100.0
                    }),
                })
            })
            .collect();
        json!({
            "due": q.due.len(), "new": q.new.len(), "total": q.items.len(),
            "queued": items.len(),
            "weak": weak_n, "leech": leech_n,
            "session_limit": session_limit,
            "retention": a.review.desired_retention,
            "cards": items,
        })
    })
}#[uniffi::export]
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
        let now = now_ms();
        let device = a.store.device().to_string();
        let ev = ReviewEvent {
            card_id: card_id.clone(),
            ts: now,
            grade: grade.clamp(1, 4),
            device: device.clone(),
            seq: 0,
        };
        let mut ops: Vec<Op> = vec![Op::Review { event: ev.clone() }];
        let mut mastery = Json::Null;

        // 复习后顺带派生掌握度：同实体所有已复习卡的 SRS 状态聚合。
        // 手工覆盖 30 天内有效（mastery_override_at），期间派生值不覆盖用户判断。
        let (mut events, cards, entities) =
            a.store.read(|m| (m.events.clone(), m.cards.clone(), m.entities_vec()));
        events.push(ev.clone());
        // 与出题用同一套调度参数，否则「队列说到期」和「掌握度说不用复习」会打架
        let states = fold_events(&events, &a.review.scheduler());
        let entity_id = cards
            .iter()
            .find(|c| c.id == card_id)
            .map(|c| c.entity_id.clone());
        if let (Some(entity_id), Some(entity)) = (
            entity_id.as_deref(),
            entities.iter().find(|e| Some(e.id.as_str()) == entity_id.as_deref()),
        ) {
            let e_cards: Vec<_> = cards.iter().filter(|c| c.entity_id == entity_id).collect();
            let reviewed: Vec<_> = e_cards
                .iter()
                .filter_map(|c| states.get(&c.id))
                .filter(|s| s.reps > 0)
                .collect();
            if !reviewed.is_empty() {
                let stability =
                    reviewed.iter().map(|s| s.stability).sum::<f64>() / reviewed.len() as f64;
                let lapses: u32 = reviewed.iter().map(|s| s.lapses).sum();
                // 覆盖率：这味药有多少张卡、复习过几张。新卡会稀释均值，
                // 所以「熟练」还得要求大部分角度都学过（详见 mastery_label_ext）。
                let reviewed_n = reviewed.len() as u32;
                let total_n = e_cards.len() as u32;
                let ids: Vec<&str> = e_cards.iter().map(|c| c.id.as_str()).collect();
                let last_grade = events
                    .iter()
                    .filter(|e| ids.contains(&e.card_id.as_str()))
                    .map(|e| (e.ts, e.grade))
                    .max_by_key(|x| x.0)
                    .map(|x| x.1);
                let label = mastery_label_ext(stability, lapses, last_grade, reviewed_n, total_n);

                let override_at = entity.get("mastery_override_at").and_then(|v| match v {
                    Value::Number(n) => Some(*n as i64),
                    _ => None,
                });
                let overridden =
                    override_at.map(|t| now - t < 30 * 86_400_000).unwrap_or(false);
                let cur = entity.get("mastery").map(|v| v.display()).unwrap_or_default();
                if cur != label && !overridden {
                    let mut e2 = entity.clone();
                    e2.set("mastery", Value::Text(label.into()), now);
                    e2.rev = a.store.stats().rev + 1;
                    e2.updated_at = now;
                    e2.device = device.clone();
                    mastery = json!({
                        "changed": true, "label": label, "prev": cur,
                        "reviewed": reviewed_n, "total": total_n,
                        "stability": (stability * 100.0).round() / 100.0,
                    });
                    ops.push(Op::UpsertEntity { entity: e2 });
                } else {
                    mastery = json!({
                        "changed": false, "label": cur, "overridden": overridden,
                        "reviewed": reviewed_n, "total": total_n,
                        "stability": (stability * 100.0).round() / 100.0,
                    });
                }
            }
        }

        match a.store.mutate(&ops) {
            Ok(n) => json!({ "ok": true, "applied": n, "event": ev.key(), "mastery": mastery }),
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

#[uniffi::export]
pub fn suggest_grade(correct: bool, elapsed_ms: i64, score: f64) -> String {
    let (grade, reason) = core_suggest_grade(correct, elapsed_ms, score);
    json!({ "grade": grade, "reason": reason }).to_string()
}

/// 错题本：最近 30 天末尾连续答错 >=2 次的卡。连对两次自然移出。
#[uniffi::export]
pub fn weak_cards(limit: u32) -> String {
    call(|a| {
        let now = now_ms();
        let (events, cards) = a.store.read(|m| (m.events.clone(), m.cards.clone()));
        let weak = core_weak_cards(&events, now, limit as usize);
        let items: Vec<Json> = weak
            .iter()
            .filter_map(|w| cards.iter().find(|c| c.id == w.card_id).map(|c| (c, w)))
            .map(|(c, w)| {
                json!({
                    "card_id": c.id, "prompt": c.prompt, "direction": c.direction,
                    "entity_name": entities_name(a, &c.entity_id),
                    // 薄弱模式同样是出题：答药名的卡不能预显药名
                    "ask_name": c.answer_fields.iter().any(|f| f == "name"),
                    "answer_fields": c.answer_fields,
                    "misses": w.misses, "last_ts": w.last_ts,
                })
            })
            .collect();
        json!({ "items": items })
    })
}

fn entities_name(a: &App, entity_id: &str) -> String {
    a.store.read(|m| {
        m.entities
            .get(entity_id)
            .map(|e| e.display_of("name"))
            .unwrap_or_else(|| entity_id.to_string())
    })
}

// ---------------------------------------------------------------- 首次配置

/// 安装 / 切换领域包。APP 首次启动时从内置 assets 读 TOML 调这里，
/// 之后再导入数据或从公共库拉取。schema 落成 `SetSchema` 事件，可重放、可同步。
#[uniffi::export]
pub fn set_schema(toml: String) -> String {
    call(|a| {
        let schema = match DomainSchema::from_toml(&toml) {
            Ok(s) => s,
            Err(e) => return err(format!("领域包解析失败: {e}")),
        };
        let n_entities = schema.entities.len();
        let (id, name) = (schema.id.clone(), schema.name.clone());
        match a.store.mutate(&[Op::SetSchema { toml }]) {
            Ok(_) => json!({ "ok": true, "schema": id, "name": name, "entity_kinds": n_entities }),
            Err(e) => err(e),
        }
    })
}

// ---------------------------------------------------------------- 编辑

fn kind_name(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Text => "text",
        FieldKind::Tag => "tag",
        FieldKind::Tags => "tags",
        FieldKind::Number => "number",
        FieldKind::Bool => "bool",
    }
}

/// 把 UI 提交的裸值按字段类型转成 `Value`。
/// Tags 允许传数组，也允许传 `a|b` / `a、b` 的字符串 —— UI 只有一个输入框。
fn coerce(kind: FieldKind, raw: &Json) -> Option<Value> {
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

/// 读单条实体的全部字段（含 `immutable` 标记与当前值），供编辑面板渲染。
#[uniffi::export]
pub fn entity_detail(entity_id: String) -> String {
    call(|a| {
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let found = a.store.read(|m| {
            m.entities_vec().into_iter().find(|x| x.id == entity_id && !x.deleted)
        });
        let Some(e) = found else { return err(format!("实体不存在或已删除：{entity_id}")) };
        let def = schema.entity_def(&e.kind);
        let primary = def
            .and_then(|d| d.primary.clone())
            .unwrap_or_else(|| "name".into());
        let fields: Vec<Json> = def
            .map(|d| {
                d.fields
                    .iter()
                    .map(|f| {
                        json!({
                            "key": f.key,
                            "label": if f.label.is_empty() { f.key.clone() } else { f.label.clone() },
                            "kind": kind_name(f.kind),
                            "immutable": f.immutable,
                            "required": f.required,
                            "value": e.get(&f.key).map(|v| v.display()).unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "id": e.id,
            "kind": e.kind,
            "primary": primary,
            "title": e.display_of(&primary),
            "rev": e.rev,
            "updated_at": e.updated_at,
            "device": e.device,
            "fields": fields,
        })
    })
}

/// 编辑核心：无 IO，只按 schema 校验并改内存里的实体。
/// 拆出来是为了能单测 —— 全局 STATE 只能初始化一次，走不了多条用例。
///
/// 返回 `(changed, unchanged)`；任何一条不合法都整体失败，一个字段也不改。
fn apply_edit(
    schema: &DomainSchema,
    entity: &mut Entity,
    patch: &BTreeMap<String, Json>,
    now: i64,
) -> std::result::Result<(Vec<Json>, Vec<Json>), String> {
    let Some(def) = schema.entity_def(&entity.kind) else {
        return Err(format!("领域包缺少 {} 的定义", entity.kind));
    };
    let mut changed: Vec<Json> = Vec::new();
    let mut unchanged: Vec<Json> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut bad: Vec<String> = Vec::new();

    // 先在副本上改，全部合法才回写，避免「改了一半报错」
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

/// 批量改一条实体的字段。`fields_json` 形如 `{"effects":"a|b","dosage":"3-10g"}`。
///
/// - `immutable` 字段一律拒改（性味归经、用量、配伍禁忌这类「原样字段」），
///   报错里列出具体字段名，UI 据此提示「需新建条目」。
/// - 值没变化的字段不产生事件（`unchanged`），避免无意义的 rev 增长与同步流量。
/// - 全部是合法改动才写；只写一条 `UpsertEntity`，`rev` 取库内最大值 +1。
#[uniffi::export]
pub fn edit_entity(entity_id: String, fields_json: String) -> String {
    call(|a| {
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let patch: BTreeMap<String, Json> = match serde_json::from_str(&fields_json) {
            Ok(v) => v,
            Err(e) => return err(format!("fieldsJson 解析失败: {e}")),
        };
        if patch.is_empty() {
            return err("没有要修改的字段");
        }
        let found = a.store.read(|m| {
            m.entities_vec().into_iter().find(|x| x.id == entity_id && !x.deleted)
        });
        let Some(mut entity) = found else {
            return err(format!("实体不存在或已删除：{entity_id}"));
        };
        let now = now_ms();
        let (changed, unchanged) = match apply_edit(&schema, &mut entity, &patch, now) {
            Ok(v) => v,
            Err(e) => return err(e),
        };
        if changed.is_empty() {
            return json!({
                "ok": true, "id": entity.id, "noop": true,
                "rev": entity.rev, "changed": [], "unchanged": unchanged,
            });
        }
        let max_rev = a.store.stats().rev;
        entity.rev = max_rev + 1;
        entity.updated_at = now;
        entity.device = a.store.device().to_string();
        let op = Op::UpsertEntity { entity: entity.clone() };
        match a.store.mutate(&[op]) {
            Ok(_) => json!({
                "ok": true,
                "id": entity.id,
                "rev": entity.rev,
                "changed": changed,
                "unchanged": unchanged,
            }),
            Err(e) => err(e),
        }
    })
}

// ---------------------------------------------------------------- 新建 / 删除 / 历史

/// 空白表单的字段定义（新建用）。与 `entityDetail` 同构，只是值全空。
#[uniffi::export]
pub fn entity_template(kind: String) -> String {
    call(|a| {
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let Some(def) = schema.entity_def(&kind) else {
            return err(format!("未知实体类型：{kind}"));
        };
        let fields: Vec<Json> = def
            .fields
            .iter()
            .map(|f| {
                json!({
                    "key": f.key,
                    "label": if f.label.is_empty() { f.key.clone() } else { f.label.clone() },
                    "kind": kind_name(f.kind),
                    "immutable": f.immutable,
                    "required": f.required,
                    "value": "",
                })
            })
            .collect();
        json!({
            "id": "",
            "kind": kind,
            "primary": def.primary.clone().unwrap_or_else(|| "name".into()),
            "title": "新建",
            "rev": 0,
            "fields": fields,
        })
    })
}

/// 新建一条实体。`immutable` 字段**允许**在这里填写 —— 这正是「教材抄错一个字」的合法出路：
/// 不改原条目，另建一条并标 `supersedes`。
///
/// 重名不自动覆盖，返回 `existing_id` 让 UI 决定「去编辑」还是「换个名字」。
#[uniffi::export]
pub fn create_entity(kind: String, fields_json: String, id_hint: String) -> String {
    call(|a| {
        let Some(schema) = schema_of(a) else { return err("还没有加载领域 schema") };
        let Some(def) = schema.entity_def(&kind).cloned() else {
            return err(format!("未知实体类型：{kind}"));
        };
        let raw: BTreeMap<String, Json> = match serde_json::from_str(&fields_json) {
            Ok(v) => v,
            Err(e) => return err(format!("fieldsJson 解析失败: {e}")),
        };
        if raw.is_empty() {
            return err("没有要写入的字段");
        }

        let now = now_ms();
        let device = a.store.device().to_string();
        let mut entity = Entity::new("", kind.clone(), &device, now);
        let mut bad: Vec<String> = Vec::new();
        for (key, v) in &raw {
            let Some(f) = def.fields.iter().find(|f| f.key == *key) else {
                return err(format!("未知字段：{key}"));
            };
            let Some(val) = coerce(f.kind, v) else {
                bad.push(format!("{}（需要 {}）", f.key, kind_name(f.kind)));
                continue;
            };
            entity.fields.insert(key.clone(), val);
        }
        if !bad.is_empty() {
            return err(format!("以下字段的值类型不对：{}", bad.join("；")));
        }

        let primary = def.primary.clone().unwrap_or_else(|| "name".into());
        let name = entity.display_of(&primary).trim().to_string();
        if name.is_empty() {
            let label = schema
                .field(&kind, &primary)
                .map(|f| if f.label.is_empty() { f.key.clone() } else { f.label.clone() })
                .unwrap_or_else(|| primary.clone());
            return err(format!("「{label}」不能为空"));
        }

        let id = if id_hint.trim().is_empty() {
            name.clone()
        } else {
            id_hint.trim().to_string()
        };
        if let Some(existing) = a.store.read(|m| m.entities.get(&id).cloned()) {
            return json!({
                "error": if existing.deleted {
                    format!("《{name}》在回收站里，先恢复或换个名字")
                } else {
                    format!("已存在《{name}》")
                },
                "existing_id": id,
                "deleted": existing.deleted,
            });
        }

        // 「复制为新」：除了在新条目上写 supersedes，还要给旧条目盖 superseded_by，
        // 否则视图无法只靠新条目的字段把旧版隐藏掉（过滤是逐行判定）。
        let supersedes = entity.display_of("supersedes").trim().to_string();
        let mut ops: Vec<Op> = Vec::new();
        let base_rev = a.store.stats().rev;
        if !supersedes.is_empty() {
            match a.store.read(|m| m.entities.get(&supersedes).cloned()) {
                Some(mut old) => {
                    old.set("superseded_by", Value::Text(id.clone()), now);
                    old.rev = base_rev + 1;
                    old.updated_at = now;
                    old.device = device.clone();
                    ops.push(Op::UpsertEntity { entity: old });
                }
                None => {
                    // 指了一条不存在的 id：拒绝，别写出悬空引用
                    return err(format!("要替代的条目不存在：{supersedes}"));
                }
            }
        }

        entity.id = id.clone();
        entity.rev = base_rev + ops.len() as u64 + 1;
        ops.push(Op::UpsertEntity { entity: entity.clone() });
        match a.store.mutate(&ops) {
            Ok(_) => json!({
                "ok": true,
                "id": entity.id,
                "rev": entity.rev,
                "name": name,
                "superseded": supersedes,
            }),
            Err(e) => err(e),
        }
    })
}

/// 删除 = 写墓碑（保留 30 天，可在回收站恢复），**绝不物理删**。
#[uniffi::export]
pub fn delete_entity(entity_id: String) -> String {
    call(|a| {
        let Some(e) = a.store.read(|m| m.entities.get(&entity_id).cloned()) else {
            return err(format!("实体不存在：{entity_id}"));
        };
        if e.deleted {
            return err("这条已经在回收站里了");
        }
        let now = now_ms();
        let rev = a.store.stats().rev + 1;
        let device = a.store.device().to_string();
        let op = Op::Tombstone {
            id: entity_id.clone(),
            rev,
            updated_at: now,
            device,
        };
        match a.store.mutate(&[op]) {
            Ok(_) => json!({ "ok": true, "id": entity_id, "deleted_at": now }),
            Err(e) => err(e),
        }
    })
}

/// 从回收站恢复（写一条 deleted=false 的新快照，历史里会多一条 restore）。
#[uniffi::export]
pub fn restore_entity(entity_id: String) -> String {
    call(|a| {
        let Some(mut e) = a.store.read(|m| m.entities.get(&entity_id).cloned()) else {
            return err(format!("实体不存在：{entity_id}"));
        };
        if !e.deleted {
            return err("这条没有被删除");
        }
        let now = now_ms();
        e.deleted = false;
        e.rev = a.store.stats().rev + 1;
        e.updated_at = now;
        e.device = a.store.device().to_string();
        let rev = e.rev;
        let op = Op::UpsertEntity { entity: e };
        match a.store.mutate(&[op]) {
            Ok(_) => json!({ "ok": true, "id": entity_id, "rev": rev }),
            Err(e) => err(e),
        }
    })
}

/// 回收站列表（只读）。
#[uniffi::export]
pub fn list_deleted() -> String {
    call(|a| {
        let kind = a
            .store
            .read(|m| m.schema.as_ref().and_then(|s| s.entities.first().map(|e| e.kind.clone())))
            .unwrap_or_else(|| "herb".into());
        let primary = a
            .store
            .read(|m| {
                m.schema
                    .as_ref()
                    .and_then(|s| s.entity_def(&kind).and_then(|d| d.primary.clone()))
            })
            .unwrap_or_else(|| "name".into());
        let items: Vec<Json> = a.store.read(|m| {
            m.entities
                .values()
                .filter(|e| e.deleted)
                .map(|e| {
                    json!({
                        "id": e.id,
                        "kind": e.kind,
                        "name": e.display_of(&primary),
                        "deleted_at": e.updated_at,
                        "rev": e.rev,
                        "device": e.device,
                    })
                })
                .collect()
        });
        json!({ "items": items })
    })
}

/// 单条实体的字段级变更时间线（最近 `limit` 条）。
#[uniffi::export]
pub fn entity_history(entity_id: String, limit: u32) -> String {
    call(|a| {
        let limit = if limit == 0 { 50 } else { limit as usize };
        match a.store.entity_history(&entity_id, limit) {
            Ok(items) => json!({ "items": items }),
            Err(e) => err(e),
        }
    })
}

/// 撤销这条实体最近一次**本机**编辑：把上一版快照写回去，产生新事件（rev 递增）。
#[uniffi::export]
pub fn undo_edit(entity_id: String) -> String {
    call(|a| match a.store.undo_entity(&entity_id, now_ms()) {
        Ok((rev, restored)) => json!({ "ok": true, "id": entity_id, "rev": rev, "restored": restored }),
        Err(e) => err(e),
    })
}

/// 写入 `sync.json` 并立即重建同步引擎，不需要重启 APP。
///
/// `config_json` 就是 `SyncConfig` 的 JSON（含 `targets` 数组）；凭据允许直接写明文，
/// 也可以写 `${ENV:VAR}`。APP 侧存在私有目录里，不外出。
#[uniffi::export]
pub fn configure_sync(config_json: String) -> String {
    call(|a| {
        let cfg: SyncConfig = match serde_json::from_str(&config_json) {
            Ok(c) => c,
            Err(e) => return err(format!("同步配置 JSON 有误: {e}")),
        };
        if cfg.targets.is_empty() {
            return err("至少要有一个同步目标");
        }
        if let Err(e) = cfg.to_file(&a.root.join("sync.json")) {
            return err(e);
        }
        let ids: Vec<String> = cfg.targets.iter().map(|t| t.id().to_string()).collect();
        let device = cfg.device_id.clone();
        a.engine = Some(Arc::new(SyncEngine::new(a.store.clone(), cfg)));
        json!({ "ok": true, "device": device, "targets": ids })
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
            // 上游没推进，但本地还有未决计划（用户切到「冲突」页会再调一次这里）：
            // 把那份计划原样还回去。否则每进一次冲突页就多一份 staging + 多一个 pending。
            if let Some(plan) = a.plans.values().next() {
                return serde_json::to_value(plan).unwrap_or_else(|_| json!({}));
            }
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
        let mut value = serde_json::to_value(&plan).unwrap_or_else(|_| json!({}));
        // 没有冲突 = 全部可自动合并：立刻落库，不留一个"等用户点确认"的空步骤。
        // 有冲突则一项都不写，等用户在「冲突」页逐项裁决。
        let no_conflicts = plan.conflicts.is_empty();
        let plan_id = plan.id.clone();
        if no_conflicts {
            match apply_plan(a, &plan, &[]) {
                Ok(r) => {
                    let _ = fs::remove_file(a.root.join(format!("upstream/staging-{}.json", plan.id)));
                    if let Some(o) = value.as_object_mut() {
                        o.insert("applied".into(), r);
                    }
                }
                Err(e) => {
                    if let Some(o) = value.as_object_mut() {
                        o.insert("apply_error".into(), json!(e));
                    }
                }
            }
            a.plans.remove(&plan_id);
        } else {
            // 同一时刻只允许一份未决计划：旧的作废，别把 staging 堆在磁盘上
            for old in std::mem::take(&mut a.plans).into_values() {
                let _ = fs::remove_file(a.root.join(format!("upstream/staging-{}.json", old.id)));
            }
            a.plans.insert(plan_id, plan);
        }
        value
    })
}

/// 把计划真正写进库：无冲突项自动合并，冲突项按用户裁决处理。
/// 只要还有未决冲突就整体失败（一项都不写），避免写一半留下不一致状态。
fn apply_plan(
    a: &mut App,
    plan: &PullPlan,
    resolutions: &[Resolution],
) -> std::result::Result<Json, String> {
    let Some(schema) = schema_of(a) else { return Err("还没有加载领域 schema".into()) };
    let local = a.store.read(|m| m.entities_vec());
    let device = a.store.device().to_string();
    let now = now_ms();
    let (entities, new_prov, report) = apply_decisions(
        plan, resolutions, &local, &schema, &a.provenance, &a.topology, &device, now,
    )
    .map_err(|e| e.to_string())?;

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
    a.store.mutate(&ops).map_err(|e| e.to_string())?;

    a.provenance = new_prov.clone();
    let _ = save_json(&a.root, "upstream/provenance.json", &new_prov);
    let _ = save_json(&a.root, "upstream/last-report.json", &report);

    // 决策日志本地留痕 + 尽力推到私有库（GitHub 每次写入都是一个 commit）
    let entries: Vec<Json> = report.log.iter().map(|e| json!(e)).collect();
    let log_path = a.root.join("upstream/decisions.jsonl");
    let mut old = fs::read_to_string(&log_path).unwrap_or_default();
    for e in &entries {
        old.push_str(&e.to_string());
        old.push('\n');
    }
    let _ = fs::write(&log_path, old);

    let mut pushed = false;
    if let Some(engine) = &a.engine {
        if let Ok(t) = engine.target("github-private") {
            if runtime().block_on(incremental::append_decisions(&*t, &entries)).is_ok() {
                pushed = true;
            }
        }
    }

    Ok(json!({
        "plan_id": plan.id,
        "auto_applied": report.auto_applied,
        "conflicts_resolved": report.conflicts_resolved,
        "added": report.added, "removed": report.removed,
        "keep_mine": report.keep_mine, "take_upstream": report.take_upstream,
        "keep_both": report.keep_both,
        "new_rev": report.new_rev,
        "decision_log_pushed_to_private": pushed,
        "log": report.log,
    }))
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
        match apply_plan(a, &plan, &resolutions) {
            Ok(r) => {
                a.plans.remove(&plan_id);
                let _ = fs::remove_file(a.root.join(format!("upstream/staging-{plan_id}.json")));
                let mut o = r;
                if let Some(m) = o.as_object_mut() {
                    m.insert("ok".into(), json!(true));
                    m.insert("plan_id".into(), json!(plan_id));
                }
                o
            }
            Err(e) => err(e), // 未决冲突：一项都不写
        }
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
            review: ReviewConfig::default(),
        };
        let _ = STATE.set(Mutex::new(Some(app)));
        assert!(state().contains("中药学"));
        assert!(views().contains("matrix"));
        assert!(quiz(1).contains("cards"));
        assert!(upstream_status().contains("topology"));
        // ---- 新建 / 编辑 / 历史 / 撤销 / 删除 / 恢复（同一次全局 STATE 里串起来） ----
        let created = create_entity(
            "herb".into(),
            r#"{"name":"测试药","effects":"解表|清热","nature":"苦，寒"}"#.into(),
            "".into(),
        );
        assert!(created.contains("\"ok\":true"), "{created}");
        assert!(created.contains("测试药"), "{created}");

        // 重名必须拒绝，并把已有 id 带回去让 UI 决定「去编辑」还是「改名」
        let dup = create_entity("herb".into(), r#"{"name":"测试药"}"#.into(), "".into());
        assert!(dup.contains("已存在"), "{dup}");
        assert!(dup.contains("existing_id"), "{dup}");

        let edited = edit_entity("测试药".into(), r#"{"effects":"解表|清热|解毒"}"#.into());
        assert!(edited.contains("\"ok\":true"), "{edited}");

        let hist = entity_history("测试药".into(), 10);
        assert!(hist.contains("\"action\":\"create\""), "{hist}");
        assert!(hist.contains("解毒"), "{hist}");

        let undone = undo_edit("测试药".into());
        assert!(undone.contains("\"ok\":true"), "{undone}");
        assert!(undone.contains("effects"), "{undone}");
        let detail = entity_detail("测试药".into());
        assert!(detail.contains("解表、清热"), "撤销后应回到改前的值：{detail}");
        assert!(!detail.contains("解毒"), "{detail}");

        let del = delete_entity("测试药".into());
        assert!(del.contains("\"ok\":true"), "{del}");
        assert!(list_deleted().contains("测试药"), "{}", list_deleted());
        // 墓碑不参与视图
        assert!(!view("matrix".into()).contains("测试药"));

        let res = restore_entity("测试药".into());
        assert!(res.contains("\"ok\":true"), "{res}");
        assert!(!list_deleted().contains("测试药"));
        assert!(view("matrix".into()).contains("测试药"));

        // ---- A2：每日队列 / 自动评分建议 / 错题本 / 掌握度派生 ----
        let q: Json = serde_json::from_str(&quiz_queue(10, 50)).unwrap();
        assert!(q["total"].as_u64().unwrap_or(0) > 0, "队列应有卡：{q}");
        assert!(q["weak"].is_u64());
        let card_id = q["cards"][0]["id"].as_str().unwrap().to_string();
        let queue_entity = q["cards"][0]["entity_id"].as_str().unwrap().to_string();

        // 自动评分建议：纯函数三档
        assert!(suggest_grade(false, 1_000, 0.9).contains("\"grade\":1"));
        assert!(suggest_grade(true, 2_000, 1.0).contains("\"grade\":4"));
        assert!(suggest_grade(true, 20_000, 1.0).contains("\"grade\":2"));

        // 复习 → 掌握度写回（三档之一，且 rev 前进）
        let rev_before = entity_detail(queue_entity.clone()).split("\"rev\":").nth(1).unwrap_or("0")
            .chars().take_while(|c| c.is_ascii_digit()).collect::<String>()
            .parse::<u64>().unwrap_or(0);
        let r = review(card_id.clone(), 4);
        assert!(r.contains("mastery"), "{r}");
        let detail = entity_detail(queue_entity.clone());
        assert!(
            detail.contains("生疏") || detail.contains("熟悉") || detail.contains("熟练"),
            "mastery 应派生出三档之一：{detail}"
        );

        // ---- 复习参数可配置：本轮张数 / 新卡上限 / 期望记忆率 ----
        let cfg = review_config();
        assert!(cfg.contains("desired_retention"), "{cfg}");
        assert!(cfg.contains("session_limit"), "{cfg}");
        let saved = set_review_config(
            r#"{"new_per_day":3,"reviews_per_day":40,"session_limit":2,"desired_retention":0.85,
                "max_interval_days":3650,"easy_interval_days":4,"leech_threshold":8,
                "learning_steps_minutes":[1,10]}"#
                .into(),
        );
        assert!(saved.contains("\"ok\":true"), "{saved}");
        assert!(saved.contains("\"session_limit\":2"), "越界值应原样接受：{saved}");
        let plan: Json = serde_json::from_str(&quiz_plan()).unwrap();
        assert_eq!(plan["session_limit"].as_u64(), Some(2), "本轮张数应生效：{plan}");
        assert!(plan["cards"].as_array().unwrap().len() <= 2, "{plan}");
        assert!(plan["retention"].as_f64().unwrap() > 0.84);
        // 越界值夹回
        let clamped = set_review_config(
            r#"{"new_per_day":9999,"reviews_per_day":99999,"session_limit":9999,"desired_retention":0.5,
                "max_interval_days":0,"easy_interval_days":0,"leech_threshold":0,
                "learning_steps_minutes":[]}"#
                .into(),
        );
        assert!(clamped.contains("\"new_per_day\":500"), "{clamped}");
        assert!(clamped.contains("\"desired_retention\":0.7"), "{clamped}");

        // 错题本：连错两次（同毫秒事件会被幂等键去重，必须隔开）
        let _ = review(card_id.clone(), 1);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let _ = review(card_id.clone(), 1);
        let w = weak_cards(10);
        assert!(w.contains(&card_id), "{w}");

        // 复制为新：旧条目要被盖上 superseded_by，主视图隐藏旧版、superseded 视图能看到
        let copy = create_entity(
            "herb".into(),
            r#"{"name":"测试药（修订）","supersedes":"测试药"}"#.into(),
            "测试药（修订）".into(),
        );
        assert!(copy.contains("\"ok\":true"), "{copy}");
        let old_detail = entity_detail("测试药".into());
        assert!(old_detail.contains("superseded_by"), "{old_detail}");
        assert!(old_detail.contains("测试药（修订）"), "{old_detail}");

        let matrix: Json = serde_json::from_str(&view("matrix".into())).unwrap();
        let ids: Vec<&str> = matrix["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["_id"].as_str())
            .collect();
        assert!(ids.contains(&"测试药（修订）"), "新版应在主视图：{ids:?}");
        assert!(!ids.contains(&"测试药"), "旧版应被隐藏：{ids:?}");

        let sup: Json = serde_json::from_str(&view("superseded".into())).unwrap();
        let sup_ids: Vec<&str> = sup["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["_id"].as_str())
            .collect();
        assert_eq!(sup_ids, vec!["测试药"], "被替代视图只应有旧版");

        let _ = fs::remove_dir_all(&root);
    }

    fn materia_schema() -> DomainSchema {
        DomainSchema::from_toml(
            &fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../domains/materia-medica.toml"),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn herb() -> Entity {
        let mut e = Entity::new("herb-mahuang", "herb", "test", 0);
        e.set("name", Value::Text("麻黄".into()), 0);
        e.set("features", Value::Tags(vec!["发汗力强".into()]), 0);
        e.set("nature", Value::Text("辛、微苦，温。归肺、膀胱经".into()), 0);
        e
    }

    #[test]
    fn edit_rejects_immutable_fields() {
        let schema = materia_schema();
        let mut e = herb();
        let patch: BTreeMap<String, Json> =
            serde_json::from_str(r#"{"nature":"辛，温","effects":"发汗解表"}"#).unwrap();
        let err = apply_edit(&schema, &mut e, &patch, 1).unwrap_err();
        // 报错必须点名是哪个字段，否则 UI 只能干巴巴说「失败了」
        assert!(err.contains("性味归经"), "{err}");
        assert!(err.contains("需新建条目"), "{err}");
        // 一项都不许改：连合法的 effects 也不能落
        assert_eq!(e.get("effects"), None);
        assert_eq!(e.get("nature").unwrap().display(), "辛、微苦，温。归肺、膀胱经");
    }

    #[test]
    fn edit_tags_accepts_pipe_string() {
        let schema = materia_schema();
        let mut e = herb();
        let patch: BTreeMap<String, Json> =
            serde_json::from_str(r#"{"features":"宣肺平喘| 利水消肿 |"}"#).unwrap();
        let (changed, unchanged) = apply_edit(&schema, &mut e, &patch, 7).unwrap();
        assert_eq!(changed.len(), 1);
        assert!(unchanged.is_empty());
        assert_eq!(
            e.get("features"),
            Some(&Value::Tags(vec!["宣肺平喘".into(), "利水消肿".into()]))
        );
        // 空段被丢掉，不会写成空标签
        assert_eq!(e.updated_at, 7);
    }

    #[test]
    fn edit_same_value_is_noop() {
        let schema = materia_schema();
        let mut e = herb();
        let before = e.clone();
        let patch: BTreeMap<String, Json> =
            serde_json::from_str(r#"{"features":"发汗力强","name":"麻黄"}"#).unwrap();
        let (changed, unchanged) = apply_edit(&schema, &mut e, &patch, 9).unwrap();
        assert!(changed.is_empty());
        assert_eq!(unchanged.len(), 2);
        // 无变化时实体一个字节都不该动（rev 不涨，同步不产生流量）
        assert_eq!(e.rev, before.rev);
        assert_eq!(e.updated_at, before.updated_at);
    }

    #[test]
    fn edit_unknown_field_rejected() {
        let schema = materia_schema();
        let mut e = herb();
        let patch: BTreeMap<String, Json> = serde_json::from_str(r#"{"nope":"x"}"#).unwrap();
        let err = apply_edit(&schema, &mut e, &patch, 1).unwrap_err();
        assert!(err.contains("未知字段"), "{err}");
    }

    #[test]
    fn coerce_covers_number_and_bool() {
        assert_eq!(coerce(FieldKind::Number, &json!("3")), Some(Value::Number(3.0)));
        assert_eq!(coerce(FieldKind::Number, &json!("abc")), None);
        assert_eq!(coerce(FieldKind::Bool, &json!("是")), Some(Value::Bool(true)));
        assert_eq!(coerce(FieldKind::Bool, &json!("0")), Some(Value::Bool(false)));
        assert_eq!(coerce(FieldKind::Tags, &json!("a、b")), Some(Value::Tags(vec!["a".into(), "b".into()])));
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
