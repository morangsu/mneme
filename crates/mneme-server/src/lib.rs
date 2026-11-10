#![forbid(unsafe_code)]
//! axum HTTP API + 内嵌静态前端。
//! 单二进制分发：HTML/CSS/JS 用 `include_str!` 编进去，运行时不需要额外的 web 目录。

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
};
use mneme_codec::{self as codec, Format, ImportOptions, ImportStrategy};
use mneme_core::{ReviewEvent, Sm2Scheduler, TermTable, due_cards, fold_events, judge, now_ms};
use mneme_store::{Op, Store};
use mneme_sync::{SyncConfig, SyncEngine, incremental};
use mneme_upstream::{
    Decision, FilePublicSource, GitHubPublicSource, ProvenanceStore, PublicBundle, PublicSource,
    PullPlan, Resolution, Topology, adopt, apply_decisions, build_public_bundle, plan_pull,
};
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex as StdMutex},
};

const INDEX: &str = include_str!("../../../web/index.html");
const CSS: &str = include_str!("../../../web/app.css");
const JS: &str = include_str!("../../../web/app.js");

/// 三仓库拓扑在进程内的状态。公共库 B 只读，私有库 C 是同步落点。
pub struct UpstreamState {
    pub root: PathBuf,
    pub topology: Topology,
    pub provenance: ProvenanceStore,
    pub plans: BTreeMap<String, PullPlan>,
    pub source: Option<Arc<dyn PublicSource>>,
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub engine: Option<Arc<SyncEngine>>,
    pub token: Option<String>,
    pub upstream: Arc<StdMutex<UpstreamState>>,
}

fn load_topology(root: &FsPath) -> Topology {
    fs::read(root.join("topology.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Topology>(&b).ok())
        .unwrap_or_else(|| Topology::example("morangsu"))
}

fn load_provenance(root: &FsPath) -> ProvenanceStore {
    fs::read(root.join("upstream/provenance.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<ProvenanceStore>(&b).ok())
        .unwrap_or_default()
}

fn save_json(root: &FsPath, rel: &str, v: &impl serde::Serialize) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("json.tmp");
    if fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default()).is_ok() {
        let _ = fs::rename(tmp, p);
    }
}

impl AppState {
    pub fn new(store: Arc<Store>, engine: Option<Arc<SyncEngine>>, token: Option<String>) -> Self {
        let root = store.root().to_path_buf();
        let topology = load_topology(&root);
        let provenance = load_provenance(&root);
        let local_bundle = root.join("upstream/bundle.json");
        let source: Option<Arc<dyn PublicSource>> = if local_bundle.exists() {
            Some(Arc::new(FilePublicSource::new(local_bundle)))
        } else {
            GitHubPublicSource::new(topology.public_data.clone(), "bundle.json")
                .ok()
                .map(|s| Arc::new(s) as Arc<dyn PublicSource>)
        };
        Self {
            store,
            engine,
            token,
            upstream: Arc::new(StdMutex::new(UpstreamState {
                root,
                topology,
                provenance,
                plans: BTreeMap::new(),
                source,
            })),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/state", get(api_state))
        .route("/views", get(api_views))
        .route("/view/{id}", get(api_view))
        .route("/reverse", get(api_reverse))
        .route("/quiz", get(api_quiz))
        .route("/quiz/answer/{id}", get(api_answer))
        .route("/review", post(api_review))
        .route("/judge", post(api_judge))
        .route("/import", post(api_import))
        .route("/export", get(api_export))
        .route("/sync/status", get(api_sync_status))
        .route("/sync/{id}", post(api_sync))
        .route("/upstream/status", get(api_upstream_status))
        .route("/upstream/adopt", post(api_upstream_adopt))
        .route("/upstream/pull", post(api_upstream_pull))
        .route("/upstream/plan/{id}", get(api_upstream_plan))
        .route("/upstream/resolve", post(api_upstream_resolve))
        .route("/upstream/publish", get(api_upstream_publish))
        .layer(middleware::from_fn_with_state(state.clone(), auth));

    Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/api/health", get(|| async { "ok" }))
        .nest("/api", api)
        .with_state(state)
}

/// 已有 listener 的版本：桌面壳要先用 `:0` 拿随机端口再开窗，
/// `serve` 里自己 bind 会和这个 listener 打架（实测 10048 端口占用）。
pub async fn serve_on(
    state: AppState,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    axum::serve(listener, router(state)).await?;
    Ok(())
}

pub async fn serve(state: AppState, port: u16, lan: bool) -> anyhow::Result<()> {
    let addr = if lan {
        SocketAddr::from(([0, 0, 0, 0], port))
    } else {
        SocketAddr::from(([127, 0, 0, 1], port))
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("mneme server → http://{}", listener.local_addr()?);
    if lan {
        println!("提示：局域网 http://<ip>:{port} 不是安全上下文，Service Worker 不可用；离线请用 --format html 导出单文件包");
    }
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// 只在 /api 上校验口令；静态页本身可读，但拿不到任何数据。
async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(t) = &state.token else {
        return next.run(req).await;
    };
    let header_ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.strip_prefix("Bearer ") == Some(t.as_str()));
    let query_ok = req
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == format!("token={t}")));
    if header_ok || query_ok {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "需要口令：在 URL 加 ?token=xxx").into_response()
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX)
}
async fn css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], CSS)
}
async fn js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], JS)
}

async fn api_state(State(s): State<AppState>) -> Json<JsonValue> {
    let st = s.store.stats();
    let (schema, aliases) = s.store.read(|m| {
        (
            m.schema.as_ref().map(|x| x.name.clone()).unwrap_or_default(),
            m.aliases.len(),
        )
    });
    Json(json!({
        "schema": schema,
        "entities": st.entities,
        "groups": st.groups,
        "cards": st.cards,
        "events": st.events,
        "rev": st.rev,
        "aliases": aliases,
        "device": st.device,
    }))
}

async fn api_views(State(s): State<AppState>) -> Json<JsonValue> {
    let views = s.store.read(|m| m.schema.as_ref().map(|x| x.views.clone()).unwrap_or_default());
    Json(json!({ "views": views }))
}

async fn api_view(State(s): State<AppState>, Path(id): Path<String>) -> Json<JsonValue> {
    let (schema, entities) = s.store.read(|m| (m.schema.clone(), m.entities_vec()));
    let Some(schema) = schema else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    let Some(v) = schema.views.iter().find(|x| x.id == id) else {
        return Json(json!({ "error": format!("未知视图 {id}") }));
    };
    let kind = schema.entities.first().map(|e| e.kind.as_str()).unwrap_or("");
    let primary = schema
        .entity_def(kind)
        .and_then(|d| d.primary.as_deref())
        .unwrap_or("name");

    let columns: Vec<JsonValue> = v
        .columns
        .iter()
        .map(|k| {
            let f = schema.field(kind, k);
            json!({
                "key": k,
                "label": f.map(|x| x.label.as_str()).unwrap_or(k.as_str()),
                "immutable": schema.is_immutable(kind, k),
            })
        })
        .collect();

    let mut list: Vec<_> = entities
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
        list.sort_by(|a, b| {
            a.get(g)
                .map(|x| x.display())
                .cmp(&b.get(g).map(|x| x.display()))
                .then(a.id.cmp(&b.id))
        });
    } else {
        list.sort_by(|a, b| a.id.cmp(&b.id));
    }

    let rows: Vec<JsonValue> = list
        .iter()
        .map(|e| {
            let mut row = serde_json::Map::new();
            for c in &v.columns {
                row.insert(
                    c.clone(),
                    json!(e.get(c).map(|x| x.display()).unwrap_or_default()),
                );
            }
            if let Some(g) = &v.group_by {
                row.insert(
                    "_group".into(),
                    json!(e.get(g).map(|x| x.display()).unwrap_or_default()),
                );
            }
            JsonValue::Object(row)
        })
        .collect();
    Json(json!({
        "columns": columns,
        "primary": primary,
        "hidden": v.hidden,
        "group_by": v.group_by,
        "rows": rows,
    }))
}

/// 线索字段既接受「功效」这样的 label，也接受 `effects` / `herb.effects`。
fn resolve_field(schema: &mneme_core::DomainSchema, raw: &str) -> Option<String> {
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

async fn api_reverse(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    let raw = q.get("clue").cloned().unwrap_or_default();
    if raw.is_empty() {
        return Json(json!({ "items": [], "error": "缺少 clue" }));
    }
    let (schema, entities) = s.store.read(|m| (m.schema.clone(), m.entities_vec()));
    let Some(schema) = schema else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    let mut clues: Vec<(String, String)> = Vec::new();
    for part in raw.split('|') {
        let (lhs, rhs) = match part.split_once(':') {
            Some(x) => x,
            None => ("", part),
        };
        let field = if lhs.is_empty() {
            schema
                .entities
                .first()
                .and_then(|e| e.fields.iter().find(|f| f.reversible))
                .map(|f| format!("{}.{}", schema.entities[0].kind, f.key))
        } else {
            resolve_field(&schema, lhs)
        };
        if let (Some(f), true) = (field, !rhs.trim().is_empty()) {
            clues.push((f, rhs.trim().to_string()));
        }
    }
    let (hits, ids) = s.store.read(|m| {
        let hits = m.index.reverse(&clues);
        let ids: Vec<Option<String>> = hits
            .iter()
            .map(|h| m.index.id_of(h.idx).map(String::from))
            .collect();
        (hits, ids)
    });
    let by_id: HashMap<&str, &mneme_core::Entity> =
        entities.iter().map(|e| (e.id.as_str(), e)).collect();
    let kind = schema.entities.first().map(|e| e.kind.as_str()).unwrap_or("");
    let primary = schema
        .entity_def(kind)
        .and_then(|d| d.primary.as_deref())
        .unwrap_or("name");
    let items: Vec<JsonValue> = hits
        .iter()
        .zip(ids.iter())
        .filter_map(|(h, id_opt)| {
            let id = id_opt.as_ref()?;
            let e = *by_id.get(id.as_str())?;
            let summary = e
                .fields
                .iter()
                .filter(|(k, _)| !schema.is_immutable(&e.kind, k))
                .take(3)
                .map(|(k, v)| format!("{}:{}", k, v.display()))
                .collect::<Vec<_>>()
                .join(" / ");
            Some(json!({
                "id": id,
                "name": e.display_of(primary),
                "group": e.group_id,
                "score": h.score,
                "summary": summary,
            }))
        })
        .collect();
    Json(json!({ "clues": clues, "items": items }))
}

async fn api_quiz(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    let limit: usize = q.get("limit").and_then(|x| x.parse().ok()).unwrap_or(1);
    if let Err(e) = s.store.ensure_cards() {
        return Json(json!({ "error": e.to_string() }));
    }
    let (cards, states) = s.store.read(|m| (m.cards_flat(), fold_events(&m.events, &Sm2Scheduler::default())));
    let due = due_cards(&states, now_ms(), cards.len());
    let mut picked: Vec<_> = due
        .iter()
        .filter_map(|id| cards.iter().find(|c| &c.id == id))
        .collect();
    for c in &cards {
        if picked.len() >= limit {
            break;
        }
        if !picked.iter().any(|p| p.id == c.id) {
            picked.push(c);
        }
    }
    picked.truncate(limit);
    Json(json!({
        "due": due.len(),
        "cards": picked.iter().map(|c| json!({"id": c.id, "prompt": c.prompt, "direction": c.direction})).collect::<Vec<_>>(),
    }))
}

async fn api_answer(State(s): State<AppState>, Path(id): Path<String>) -> Json<JsonValue> {
    if let Err(e) = s.store.ensure_cards() {
        return Json(json!({ "error": e.to_string() }));
    }
    let found = s.store.read(|m| m.find_card(&id).cloned());
    match found {
        Some(c) => Json(json!({"answer": c.answer, "accept": c.accept, "boundary": c.immutable_boundary})),
        None => Json(json!({ "error": "卡片不存在" })),
    }
}

#[derive(Deserialize)]
struct ReviewBody {
    card_id: String,
    grade: u8,
}

async fn api_review(State(s): State<AppState>, Json(b): Json<ReviewBody>) -> Json<JsonValue> {
    let ev = ReviewEvent {
        card_id: b.card_id,
        ts: now_ms(),
        grade: b.grade.clamp(1, 4),
        device: s.store.device().to_string(),
        seq: 0,
    };
    match s.store.mutate(&[Op::Review { event: ev.clone() }]) {
        Ok(n) => Json(json!({ "ok": true, "applied": n, "event": ev.key() })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

#[derive(Deserialize)]
struct JudgeBody {
    card_id: String,
    input: String,
}

/// 自由输入判分：同义表述归一 + 中文归一化 + Dice 相似度，
/// 全角、标点、OCR 错字都能容错；危险区字段不做同义替换。
async fn api_judge(State(s): State<AppState>, Json(b): Json<JudgeBody>) -> Json<JsonValue> {
    if let Err(e) = s.store.ensure_cards() {
        return Json(json!({ "error": e.to_string() }));
    }
    let (card, defs) = s.store.read(|m| {
        (
            m.find_card(&b.card_id).cloned(),
            m.schema.as_ref().map(|x| x.aliases.clone()).unwrap_or_default(),
        )
    });
    let Some(card) = card else {
        return Json(json!({ "error": "卡片不存在" }));
    };
    let table = TermTable::from_defs(&defs);
    let j = judge(&b.input, &card, &table, 0.85);
    Json(json!({
        "score": j.score,
        "correct": j.correct,
        "matched": j.matched,
    }))
}

// ---------------------------------------------------------------- 三仓库上游

async fn public_bundle(s: &AppState, allow_unchanged: bool) -> std::result::Result<Option<PublicBundle>, String> {
    let source = {
        let u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
        u.source.clone()
    };
    let Some(src) = source else {
        return Err("未配置公共基础库（topology.json）".into());
    };
    match src.fetch_if_changed().await {
        Ok(Some(b)) => PublicBundle::from_bytes(&b)
            .map(Some)
            .map_err(|e| e.to_string()),
        Ok(None) if allow_unchanged => {
            let p = {
                let u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
                u.root.join("upstream/bundle.json")
            };
            let b = fs::read(p).map_err(|e| e.to_string())?;
            PublicBundle::from_bytes(&b).map(Some).map_err(|e| e.to_string())
        }
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

async fn api_upstream_status(State(s): State<AppState>) -> Json<JsonValue> {
    let u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
    Json(json!({
        "upstream_rev": u.provenance.upstream_rev,
        "upstream_checksum": u.provenance.upstream_checksum,
        "tracked_entities": u.provenance.entities.len(),
        "pending_plans": u.plans.len(),
        "app_repo": u.topology.app_repo.slug(),
        "public_data": u.topology.public_data.slug(),
        "private_data": u.topology.private_data.slug(),
        "public_source": u.source.as_ref().map(|x| x.id().to_string()),
    }))
}

/// 首次接入公共库：把当前公共快照写成 base，并标出用户已有的个性化调整。
async fn api_upstream_adopt(State(s): State<AppState>) -> Json<JsonValue> {
    let bundle = match public_bundle(&s, true).await {
        Ok(Some(b)) => b,
        Ok(None) => return Json(json!({ "error": "公共库无内容" })),
        Err(e) => return Json(json!({ "error": e })),
    };
    let local = s.store.read(|m| m.entities_vec());
    let mut u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
    u.provenance = adopt(&local, &bundle, &u.topology, now_ms());
    save_json(&u.root, "upstream/provenance.json", &u.provenance);
    save_json(&u.root, "upstream/bundle.json", &bundle);
    Json(json!({
        "ok": true,
        "upstream_rev": u.provenance.upstream_rev,
        "tracked_entities": u.provenance.entities.len(),
        "note": "本地与公共库不同的字段已标记为个性化调整",
    }))
}

/// 拉取公共基础库 -> 生成 staging 计划。**不写库**，冲突逐项等用户裁决。
async fn api_upstream_pull(State(s): State<AppState>) -> Json<JsonValue> {
    let bundle = match public_bundle(&s, false).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            let rev = s.upstream.lock().unwrap_or_else(|e| e.into_inner()).provenance.upstream_rev;
            return Json(json!({ "unchanged": true, "upstream_rev": rev }));
        }
        Err(e) => return Json(json!({ "error": e })),
    };
    let schema = match s.store.read(|m| m.schema.clone()) {
        Some(x) => x,
        None => return Json(json!({ "error": "还没有加载领域 schema" })),
    };
    let local = s.store.read(|m| m.entities_vec());
    let device = s.store.device().to_string();
    let mut u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
    if bundle.rev <= u.provenance.upstream_rev {
        return Json(json!({ "unchanged": true, "upstream_rev": u.provenance.upstream_rev }));
    }
    let plan = match plan_pull(&local, &bundle, &u.provenance, &schema, &u.topology, &device, now_ms()) {
        Ok(p) => p,
        Err(e) => return Json(json!({ "error": e.to_string() })),
    };
    save_json(&u.root, &format!("upstream/staging-{}.json", plan.id), &plan);
    save_json(&u.root, "upstream/bundle.json", &bundle);
    let value = serde_json::to_value(&plan).unwrap_or_else(|_| json!({}));
    u.plans.insert(plan.id.clone(), plan);
    Json(value)
}

async fn api_upstream_plan(State(s): State<AppState>, Path(id): Path<String>) -> Json<JsonValue> {
    let mut u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = u.plans.get(&id) {
        return Json(serde_json::to_value(p).unwrap_or_else(|_| json!({})));
    }
    let p = u.root.join(format!("upstream/staging-{id}.json"));
    match fs::read(&p).ok().and_then(|b| serde_json::from_slice::<PullPlan>(&b).ok()) {
        Some(plan) => {
            let v = serde_json::to_value(&plan).unwrap_or_else(|_| json!({}));
            u.plans.insert(id, plan);
            Json(v)
        }
        None => Json(json!({ "error": "拉取计划不存在" })),
    }
}

#[derive(Deserialize)]
struct ResolveBody {
    plan_id: String,
    decisions: Vec<ResolveItem>,
}
#[derive(Deserialize)]
struct ResolveItem {
    conflict_id: String,
    decision: String,
    #[serde(default)]
    custom_value: Option<String>,
}

/// 提交裁决：所有冲突都有决定才落库，并把决策日志推到私有库（可溯源）。
async fn api_upstream_resolve(State(s): State<AppState>, Json(b): Json<ResolveBody>) -> Json<JsonValue> {
    let plan = {
        let u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
        u.plans.get(&b.plan_id).cloned()
    };
    let Some(plan) = plan else { return Json(json!({ "error": "拉取计划不存在，请重新 pull" })) };
    let resolutions: Vec<Resolution> = b
        .decisions
        .iter()
        .map(|r| Resolution {
            conflict_id: r.conflict_id.clone(),
            decision: Decision::from_str(&r.decision).unwrap_or(Decision::KeepMine),
            custom_value: r.custom_value.clone(),
        })
        .collect();
    let schema = match s.store.read(|m| m.schema.clone()) {
        Some(x) => x,
        None => return Json(json!({ "error": "还没有加载领域 schema" })),
    };
    let local = s.store.read(|m| m.entities_vec());
    let device = s.store.device().to_string();
    let now = now_ms();
    let mut u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
    let (entities, new_prov, report) = match apply_decisions(
        &plan, &resolutions, &local, &schema, &u.provenance, &u.topology, &device, now,
    ) {
        Ok(x) => x,
        Err(e) => return Json(json!({ "error": e.to_string() })),
    };
    let max_rev = s.store.stats().rev;
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
    if let Err(e) = s.store.mutate(&ops) {
        return Json(json!({ "error": e.to_string() }));
    }
    u.provenance = new_prov;
    save_json(&u.root, "upstream/provenance.json", &u.provenance);
    save_json(&u.root, "upstream/last-report.json", &report);
    let entries: Vec<JsonValue> = report.log.iter().map(|e| json!(e)).collect();
    u.plans.remove(&b.plan_id);
    let target = s.engine.as_ref().and_then(|e| e.target("github-private").ok());
    // std::sync::MutexGuard 不能跨 await（Future 会失去 Send），先用完再释放。
    // 决策日志推私有库改为后台派发，不阻塞裁决结果。
    drop(u);
    let dispatched = if let Some(t) = target {
        tokio::spawn(async move {
            let _ = incremental::append_decisions(&*t, &entries).await;
        });
        true
    } else {
        false
    };
    Json(json!({
        "ok": true, "plan_id": b.plan_id,
        "auto_applied": report.auto_applied, "conflicts_resolved": report.conflicts_resolved,
        "added": report.added, "removed": report.removed,
        "keep_mine": report.keep_mine, "take_upstream": report.take_upstream, "keep_both": report.keep_both,
        "new_rev": report.new_rev, "decision_log_dispatched_to_private": dispatched,
        "log": report.log,
    }))
}

/// 维护者侧：生成只含基础字段的公共库内容包（个人字段绝不外发）。
async fn api_upstream_publish(State(s): State<AppState>) -> Json<JsonValue> {
    let (entities, groups) = s.store.read(|m| {
        (
            m.entities_vec(),
            m.groups.values().cloned().collect::<Vec<_>>(),
        )
    });
    let u = s.upstream.lock().unwrap_or_else(|e| e.into_inner());
    let b = build_public_bundle(&entities, &groups, u.provenance.upstream_rev + 1, &u.topology, now_ms());
    Json(json!({
        "bundle": b,
        "bytes": b.to_bytes().map(|x| x.len()).unwrap_or(0),
        "note": "只包含 base_fields；mastery/synonyms/boundary 等个人字段已被剔除",
    }))
}

#[derive(Deserialize)]
struct ImportBody {
    content: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    dry: bool,
    #[serde(default)]
    strategy: Option<String>,
}

async fn api_import(State(s): State<AppState>, Json(b): Json<ImportBody>) -> Json<JsonValue> {
    let bytes = b.content.as_bytes();
    let path = if b.filename.is_empty() {
        None
    } else {
        Some(std::path::Path::new(&b.filename))
    };
    let fmt = codec::detect(path, bytes);
    if !fmt.importable() {
        return Json(json!({ "error": format!("{fmt} 不支持导入") }));
    }
    let opts = ImportOptions {
        dry_run: b.dry,
        strategy: b
            .strategy
            .as_deref()
            .and_then(|x| ImportStrategy::from_str(x).ok())
            .unwrap_or(ImportStrategy::Merge),
        verify_hash: true,
    };
    match codec::import(&s.store, bytes, fmt, &opts) {
        Ok(r) => Json(json!(r)),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn api_export(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let fmt: Format = q
        .get("format")
        .and_then(|x| Format::from_str(x).ok())
        .unwrap_or(Format::Bundle);
    match codec::export_bytes(&s.store, fmt, now_ms()) {
        Ok((bytes, _)) => {
            let mime = match fmt {
                Format::OfflineHtml => "text/html; charset=utf-8",
                Format::Markdown => "text/markdown; charset=utf-8",
                Format::Csv | Format::Anki => "text/csv; charset=utf-8",
                Format::Tsv => "text/tab-separated-values; charset=utf-8",
                _ => "application/json; charset=utf-8",
            };
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"mneme.{}\"", fmt.extension()),
                )
                .body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn api_sync_status(State(s): State<AppState>) -> Json<JsonValue> {
    let targets = s
        .engine
        .as_ref()
        .map(|e| e.target_ids())
        .unwrap_or_default();
    Json(json!({ "targets": targets, "synced": false }))
}

async fn api_sync(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    let Some(engine) = &s.engine else {
        return Json(json!({ "error": "未配置 data/sync.json" }));
    };
    let dry = q.get("dry").is_some_and(|x| x == "true" || x == "1");
    match engine.sync(&id, dry).await {
        Ok(r) => Json(json!({ "ok": true, "report": r.to_string(), "sha256": r.sha256, "retries": r.retries })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// 供 CLI 的 `serve` 子命令直接构造默认配置。
pub fn load_sync_config(store: &Arc<Store>, path: &std::path::Path) -> Option<Arc<SyncEngine>> {
    if !path.exists() {
        return None;
    }
    SyncConfig::from_file(path)
        .ok()
        .map(|c| Arc::new(SyncEngine::new(store.clone(), c)))
}
