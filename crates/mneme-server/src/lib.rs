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
    routing::{get, post, put},
};
use mneme_codec::{self as codec, Format, ImportOptions, ImportStrategy};
use mneme_core::{
    ReviewConfig, ReviewEvent, Sm2Scheduler, TermTable, due_cards, fold_events, judge, now_ms,
};
use mneme_core::{
    Entity as CoreEntity, Group as CoreGroup, Value as CoreValue, MAX_DEPTH,
    daily_queue, find_duplicates, generate_choices, group_tree,
    mastery_label_ext, parse_paste_batch as core_parse_paste_batch,
    path_names, weak_cards as core_weak_cards, would_cycle,
};
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
const DESKTOP: &str = include_str!("../../../web/desktop.html");
const DESKTOP_CSS: &str = include_str!("../../../web/desktop.css");
const DESKTOP_JS: &str = include_str!("../../../web/desktop.js");
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
    /// 同步引擎可热替换：桌面端「保存同步配置」后立即生效，不用重启进程。
    /// RwLock 而不是裸 Option，是因为 upstream 裁决后台派发也要读它。
    pub engine: Arc<std::sync::RwLock<Option<Arc<SyncEngine>>>>,
    pub token: Option<String>,
    pub upstream: Arc<StdMutex<UpstreamState>>,
    /// 已登录的浏览器会话（session_id -> 过期毫秒）。只在内存里，重启即全体失效；
    /// 手机浏览器存 HttpOnly cookie，不再碰 localStorage。
    pub sessions: Arc<StdMutex<std::collections::HashMap<String, i64>>>,
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
            engine: Arc::new(std::sync::RwLock::new(engine)),
            token,
            sessions: Arc::new(StdMutex::new(std::collections::HashMap::new())),
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
        .route("/quiz/plan", get(api_quiz_plan))
        .route("/quiz/answer/{id}", get(api_answer))
        .route("/review", post(api_review))
        .route("/judge", post(api_judge))
        .route("/weak-cards", get(api_weak_cards))
        .route("/review-config", get(api_review_config_get))
        .route("/review-config", put(api_review_config_put))
        .route("/import", post(api_import))
        .route("/export", get(api_export))
        .route("/sync/status", get(api_sync_status))
        .route("/sync/{id}", post(api_sync))
        .route("/sync/config", post(api_sync_config))
        .route("/deleted", get(api_deleted))
        .route("/history/{id}", get(api_history))
        .route("/groups", get(api_groups))
        .route("/groups/upsert", post(api_group_upsert))
        .route("/groups/delete", post(api_group_delete))
        .route("/check-duplicate", post(api_check_duplicate))
        .route("/parse-paste", post(api_parse_paste))
        .route("/create-batch", post(api_create_batch))
        .route("/import/preview", post(api_import_preview))
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
        .route("/desktop", get(|| async { Html(DESKTOP) }))
        .route("/desktop.css", get(|| async { css_body(DESKTOP_CSS) }))
        .route("/desktop.js", get(|| async { js_body(DESKTOP_JS) }))
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/entity/{id}", get(api_entity))
        .route("/api/template", get(api_template))
        .route("/api/search", get(api_search))
        .route("/api/edit", post(api_edit))
        .route("/api/group", post(api_set_group))
        .route("/api/create", post(api_create))
        .route("/api/delete", post(api_delete))
        .route("/api/restore", post(api_restore))
        .route("/api/undo", post(api_undo))
        .route("/api/login", post(api_login))
        .route("/api/logout", post(api_logout))
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
/// 静态页不含任何数据，公开；真正要保护的是 /api/*。
/// 公开它们还有一个原因：未登录时浏览器要能加载 JS，由 JS 弹登录框。
const PUBLIC_PATHS: [&str; 6] = ["/", "/app.css", "/app.js", "/desktop", "/desktop.css", "/desktop.js"];

async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(t) = &state.token else {
        return next.run(req).await;
    };
    if req.method() == axum::http::Method::GET && PUBLIC_PATHS.contains(&req.uri().path()) {
        return next.run(req).await;
    }
    let header_ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.strip_prefix("Bearer ") == Some(t.as_str()));
    let query_ok = req
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == format!("token={t}")));
    if header_ok || query_ok || session_ok(&state, &req) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "需要登录").into_response()
    }
}

/// 会话 cookie 名。HttpOnly：JS 读不到，XSS 偷不走。
pub const SESSION_COOKIE: &str = "mneme_session";
/// 12 小时；过期后浏览器带上的会话一律无效，要重新输 token。
const SESSION_TTL_MS: i64 = 12 * 3600 * 1000;

fn session_ok(state: &AppState, req: &Request) -> bool {
    let Some(sid) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| {
            c.split(';').find_map(|p| {
                let p = p.trim();
                p.strip_prefix(SESSION_COOKIE).and_then(|rest| {
                    rest.strip_prefix('=').map(|v| v.trim().to_string())
                })
            })
        })
    else {
        return false;
    };
    let now = now_ms();
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    // 顺手清过期：会话表最多几百条（每台设备一个），不值得定时器
    sessions.retain(|_, exp| *exp > now);
    match sessions.get(&sid) {
        Some(exp) if *exp > now => true,
        _ => {
            sessions.remove(&sid);
            false
        }
    }
}

/// 登录：token 对上就发 HttpOnly 会话 cookie。
/// 每次登录都换新 session id——同一个人在两台浏览器登录互不影响，登出一台不害另一台。
async fn api_login(State(s): State<AppState>, Json(b): Json<LoginBody>) -> Response {
    let Some(t) = &s.token else {
        return Json(json!({ "error": "这个实例没有设置口令，无需登录" })).into_response();
    };
    if b.token.trim() != *t {
        return Json(json!({ "error": "口令不对" })).into_response();
    }
    let now = now_ms();
    let sid = {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for v in [
            now as u64,
            std::process::id() as u64,
            (&h as *const u64) as u64,
        ] {
            for byte in v.to_le_bytes() {
                h ^= byte as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
        }
        format!("{h:032x}")
    };
    {
        let mut sessions = s.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, exp| *exp > now);
        sessions.insert(sid.clone(), now + SESSION_TTL_MS);
    }
    let cookie = format!(
        "{SESSION_COOKIE}={sid}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL_MS / 1000
    );
    (
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true, "expires_hours": SESSION_TTL_MS / 3600_000 })),
    )
        .into_response()
}

async fn api_logout(State(s): State<AppState>, req: Request) -> Response {
    if let Some(sid) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| {
            c.split(';').find_map(|p| {
                let p = p.trim();
                p.strip_prefix(SESSION_COOKIE)
                    .and_then(|rest| rest.strip_prefix('=').map(|v| v.trim().to_string()))
            })
        })
    {
        s.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
    }
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    ([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response()
}

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

fn css_body(s: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], s)
}

fn js_body(s: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], s)
}

/// 实体详情：与 FFI 的 entityDetail 共用 core 的 detail_view。
async fn api_entity(State(s): State<AppState>, Path(id): Path<String>) -> Json<JsonValue> {
    let (schema, entities, groups) = s
        .store
        .read(|m| (m.schema.clone(), m.entities_vec(), m.groups.values().cloned().collect::<Vec<_>>()));
    let Some(schema) = schema else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    let Some(e) = entities.into_iter().find(|x| x.id == id && !x.deleted) else {
        return Json(json!({ "error": format!("实体不存在或已删除：{id}") }));
    };
    Json(mneme_core::entity::detail_view(&schema, &e, &groups))
}

/// 新建表单的字段形状（kind 省略 = 第一个实体类型）。
/// 与 FFI 的 entityTemplate 同源：都用 schema 定义，不硬编码。
async fn api_template(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    let Some(schema) = s.store.read(|m| m.schema.clone()) else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    let kinds: Vec<String> = schema.entities.iter().map(|e| e.kind.clone()).collect();
    let kind = match q.get("kind").filter(|k| !k.is_empty()) {
        Some(k) if kinds.iter().any(|x| x == k) => k.clone(),
        _ => kinds.first().cloned().unwrap_or_default(),
    };
    let Some(def) = schema.entity_def(&kind) else {
        return Json(json!({ "error": format!("未知实体类型：{kind}") }));
    };
    let fields: Vec<JsonValue> = def
        .fields
        .iter()
        .map(|f| {
            json!({
                "key": f.key,
                "label": if f.label.is_empty() { f.key.clone() } else { f.label.clone() },
                "kind": match f.kind {
                    mneme_core::FieldKind::Text => "text",
                    mneme_core::FieldKind::Tag => "tag",
                    mneme_core::FieldKind::Tags => "tags",
                    mneme_core::FieldKind::Number => "number",
                    mneme_core::FieldKind::Bool => "bool",
                },
                "immutable": f.immutable,
                "required": f.required,
                "value": "",
            })
        })
        .collect();
    Json(json!({
        "kind": kind,
        "kinds": kinds,
        "primary": def.primary.clone().unwrap_or_else(|| "name".into()),
        "fields": fields,
    }))
}

/// 编辑实体：与 FFI 的 `editEntity` 共用 core 的 `apply_edit`（两端口径不分叉）。
/// 体：`{"id":"麻黄","fields":{"effects":"发汗解表|利水"}}`。
/// 语义：immutable 拒改、同值不产生事件、全部合法才写。
async fn api_edit(State(s): State<AppState>, Json(b): Json<EditBody>) -> Json<JsonValue> {
    if b.fields.is_empty() {
        return Json(json!({ "error": "没有要修改的字段" }));
    }
    let (schema, found) = s.store.read(|m| {
        (
            m.schema.clone(),
            m.entities_vec()
                .into_iter()
                .find(|x| x.id == b.id && !x.deleted),
        )
    });
    let Some(schema) = schema else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    let Some(mut entity) = found else {
        return Json(json!({ "error": format!("实体不存在或已删除：{}", b.id) }));
    };
    let now = now_ms();
    let (changed, unchanged) = match mneme_core::apply_edit(&schema, &mut entity, &b.fields, now) {
        Ok(v) => v,
        Err(e) => return Json(json!({ "error": e })),
    };
    if changed.is_empty() {
        return Json(json!({
            "ok": true, "id": entity.id, "noop": true,
            "rev": entity.rev, "changed": [], "unchanged": unchanged,
        }));
    }
    let max_rev = s.store.stats().rev;
    entity.rev = max_rev + 1;
    entity.updated_at = now;
    entity.device = s.store.device().to_string();
    match s.store.mutate(&[Op::UpsertEntity { entity: entity.clone() }]) {
        Ok(_) => Json(json!({
            "ok": true, "id": entity.id, "rev": entity.rev,
            "changed": changed, "unchanged": unchanged,
        })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// 把实体挂到某个分组（`group_id` 传空 = 移出分组）。
/// 与 FFI 的 `setEntityGroup` 同一份 store 实现：校验分组存在、
/// 有 `group_name` 字段时同步写显示名（视图按它回退）。
async fn api_set_group(State(s): State<AppState>, Json(b): Json<SetGroupBody>) -> Json<JsonValue> {
    let gid = b.group_id.trim();
    let Some(mut e) = s.store.read(|m| m.entities.get(&b.id).cloned()) else {
        return Json(json!({ "error": format!("实体不存在：{}", b.id) }));
    };
    if e.deleted {
        return Json(json!({ "error": "这条在回收站里，先恢复再分组" }));
    }
    let now = now_ms();
    let label = if gid.is_empty() {
        e.group_id = None;
        String::new()
    } else {
        let g = s.store.read(|m| {
            m.groups
                .values()
                .find(|g| g.id == gid && !g.deleted)
                .map(|g| (g.id.clone(), g.name.clone()))
        });
        let Some((id, name)) = g else {
            return Json(json!({ "error": format!("分组不存在：{gid}") }));
        };
        e.group_id = Some(id);
        name
    };
    if e.get("group_name").is_some() {
        e.set("group_name", mneme_core::Value::Text(label.clone()), now);
    }
    e.rev = s.store.stats().rev + 1;
    e.updated_at = now;
    e.device = s.store.device().to_string();
    match s.store.mutate(&[Op::UpsertEntity { entity: e }]) {
        Ok(_) => Json(json!({ "ok": true, "id": b.id, "group_id": gid, "group_name": label })),
        Err(err) => Json(json!({ "error": err.to_string() })),
    }
}

/// 新建实体：与 FFI `createEntity` 同一份 store 实现（重名分类、替代链、原子写）。
async fn api_create(State(s): State<AppState>, Json(b): Json<CreateBody>) -> Json<JsonValue> {
    if b.fields.is_empty() {
        return Json(json!({ "error": "没有要写入的字段" }));
    }
    let Some(schema) = s.store.read(|m| m.schema.clone()) else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    match mneme_store::entities::create_one(&s.store, &schema, &b.kind, &b.fields, &b.id_hint) {
        Ok(c) => Json(json!({
            "ok": true, "id": c.id, "rev": c.rev, "name": c.name,
            "superseded": c.superseded,
        })),
        Err(mneme_store::entities::CreateErr::Duplicate { message, id, deleted }) => {
            Json(json!({ "error": message, "existing_id": id, "deleted": deleted }))
        }
        Err(mneme_store::entities::CreateErr::Other(e)) => Json(json!({ "error": e })),
    }
}

/// 删除（显式墓碑，进回收站）。
async fn api_delete(State(s): State<AppState>, Json(b): Json<IdBody>) -> Json<JsonValue> {
    match mneme_store::entities::delete_entity(&s.store, &b.id) {
        Ok(deleted_at) => Json(json!({ "ok": true, "id": b.id, "deleted_at": deleted_at })),
        Err(e) => Json(json!({ "error": e })),
    }
}

/// 从回收站恢复。
async fn api_restore(State(s): State<AppState>, Json(b): Json<IdBody>) -> Json<JsonValue> {
    match mneme_store::entities::restore_entity(&s.store, &b.id) {
        Ok(rev) => Json(json!({ "ok": true, "id": b.id, "rev": rev })),
        Err(e) => Json(json!({ "error": e })),
    }
}

/// 撤销本会话对这条实体的最近一次编辑（字段级回滚，与 FFI undoEdit 同源）。
async fn api_undo(State(s): State<AppState>, Json(b): Json<IdBody>) -> Json<JsonValue> {
    match s.store.undo_entity(&b.id, now_ms()) {
        Ok((rev, restored)) => Json(json!({ "ok": true, "id": b.id, "rev": rev, "restored": restored })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    kind: String,
    fields: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    id_hint: String,
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

#[derive(Deserialize)]
struct EditBody {
    id: String,
    fields: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct SetGroupBody {
    id: String,
    group_id: String,
}

/// 跨字段检索（与 FFI search 同一份 core 实现）。
async fn api_search(State(s): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<JsonValue> {
    let query = q.get("query").cloned().unwrap_or_default();
    let limit: usize = q
        .get("limit")
        .and_then(|x| x.parse().ok())
        .filter(|n| *n > 0 && *n <= 200)
        .unwrap_or(50);
    let (schema, entities, groups) = s
        .store
        .read(|m| {
            (
                m.schema.clone(),
                m.entities_vec(),
                m.groups.values().cloned().collect::<Vec<_>>(),
            )
        });
    let Some(schema) = schema else {
        return Json(json!({ "error": "还没有加载领域 schema" }));
    };
    let out = mneme_core::search(&schema, &entities, &groups, &query, None, limit);
    Json(json!(out))
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
    if let Err(e) = s.store.ensure_cards() {
        return Json(json!({ "error": e.to_string() }));
    }
    let now = now_ms();
    let device = s.store.device().to_string();
    let ev = ReviewEvent {
        card_id: b.card_id.clone(),
        ts: now,
        grade: b.grade.clamp(1, 4),
        device: device.clone(),
        seq: 0,
    };
    let mut ops: Vec<Op> = vec![Op::Review { event: ev.clone() }];
    let mut mastery = JsonValue::Null;

    // 复习后顺带派生掌握度：同实体所有已复习卡的 SRS 状态聚合。
    // 手工覆盖 30 天内有效（mastery_override_at），期间派生值不覆盖用户判断。
    // 与 mneme-ffi 的 review() 同源；调度参数也取同一份 review.json，避免「队列说到期」与「掌握度说不用复习」打架。
    let (mut events, cards, entities) =
        s.store
            .read(|m| (m.events.clone(), m.cards_flat(), m.entities_vec()));
    events.push(ev.clone());
    let sched = review_cfg(s.store.root()).scheduler();
    let states = fold_events(&events, &sched);
    let entity_id = cards
        .iter()
        .find(|c| c.id == b.card_id)
        .map(|c| c.entity_id.clone());
    if let (Some(entity_id), Some(entity)) = (
        entity_id.as_deref(),
        entities.iter().find(|e| Some(e.id.as_str()) == entity_id.as_deref()),
    ) {
        let e_cards: Vec<_> = cards.iter().filter(|c| c.entity_id == entity_id).collect();
        let reviewed: Vec<_> = e_cards
            .iter()
            .filter_map(|c| states.get(&c.id))
            .filter(|x| x.reps > 0)
            .collect();
        if !reviewed.is_empty() {
            let stability =
                reviewed.iter().map(|x| x.stability).sum::<f64>() / reviewed.len() as f64;
            let lapses: u32 = reviewed.iter().map(|x| x.lapses).sum();
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
                CoreValue::Number(n) => Some(*n as i64),
                _ => None,
            });
            let overridden = override_at.map(|t| now - t < 30 * 86_400_000).unwrap_or(false);
            let cur = entity.get("mastery").map(|v| v.display()).unwrap_or_default();
            if cur != label && !overridden {
                let mut e2 = entity.clone();
                e2.set("mastery", CoreValue::Text(label.into()), now);
                e2.rev = s.store.stats().rev + 1;
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

    match s.store.mutate(&ops) {
        Ok(n) => Json(json!({ "ok": true, "applied": n, "event": ev.key(), "mastery": mastery })),
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

// ---------------------------------------------------------------- 复习计划 / 错题本 / 复习参数

/// 复习参数：与 mneme-ffi 同源，读 `review.json`（没有就用默认值）。
/// server 是长驻进程，但 review.json 改动会经 PUT 即时写盘，读侧每次现读，
/// 所以桌面与手机端改动互相可见，不用重启。
fn review_cfg(root: &FsPath) -> ReviewConfig {
    fs::read(root.join("review.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<ReviewConfig>(&b).ok())
        .map(|c| c.normalized())
        .unwrap_or_default()
}

/// 按用户配置出题（/api/quiz/plan）：到期优先、新卡限量、会话截断，
/// 卡片带记忆状态（S/D/R/lapses/leech）与选择题干扰项，与 mneme-ffi 的 quizPlan 同源。
async fn api_quiz_plan(State(s): State<AppState>) -> Json<JsonValue> {
    if let Err(e) = s.store.ensure_cards() {
        return Json(json!({ "error": e.to_string() }));
    }
    let cfg = review_cfg(s.store.root());
    let now = now_ms();
    let (cards, events, entities, schema) = s.store.read(|m| {
        (
            m.cards_flat(),
            m.events.clone(),
            m.entities_vec(),
            m.schema.clone(),
        )
    });
    let sched = cfg.scheduler();
    let states = fold_events(&events, &sched);
    let q = daily_queue(
        &cards, &states, now,
        cfg.new_per_day as usize, cfg.reviews_per_day as usize,
    );
    let weak_n = core_weak_cards(&events, now, usize::MAX).len();
    let leech_n = states.values().filter(|x| x.leech).count();

    let mut ids = q.items.clone();
    if cfg.session_limit > 0 && ids.len() > cfg.session_limit as usize {
        ids.truncate(cfg.session_limit as usize);
    }

    let entity_name = |eid: &str| {
        entities
            .iter()
            .find(|e| e.id == eid)
            .map(|e| e.display_of("name"))
            .unwrap_or_else(|| eid.to_string())
    };
    // 答案是不是「主字段名」（如药名）：是的话 UI 绝不能把实体名露出来
    let ask_name = |c: &mneme_core::Card| {
        let primary = entities
            .iter()
            .find(|e| e.id == c.entity_id)
            .and_then(|e| {
                schema
                    .as_ref()
                    .and_then(|sc| sc.entity_def(&e.kind))
                    .and_then(|d| d.primary.clone())
            })
            .unwrap_or_else(|| "name".into());
        c.answer_fields.iter().any(|f| *f == primary)
    };
    let items: Vec<JsonValue> = ids
        .iter()
        .filter_map(|id| cards.iter().find(|c| &c.id == id))
        .map(|c| {
            let choices = schema.as_ref().and_then(|sc| {
                entities
                    .iter()
                    .find(|e| e.id == c.entity_id)
                    .map(|e| generate_choices(sc, e, &entities, 3))
            }).unwrap_or_default();
            let st = states.get(&c.id);
            let elapsed_days = st
                .map(|x| (now - x.last).max(0) as f64 / 86_400_000.0)
                .unwrap_or(0.0);
            json!({
                "id": c.id, "prompt": c.prompt, "direction": c.direction,
                "entity_id": c.entity_id,
                "entity_name": entity_name(&c.entity_id),
                "ask_name": ask_name(c),
                "answer_fields": c.answer_fields,
                "choices": choices, "boundary": c.immutable_boundary,
                "stability": st.map(|x| (x.stability * 100.0).round() / 100.0),
                "difficulty": st.map(|x| (x.difficulty * 100.0).round() / 100.0),
                "lapses": st.map(|x| x.lapses).unwrap_or(0),
                "leech": st.map(|x| x.leech).unwrap_or(false),
                "retrievability": st.map(|x| {
                    (sched.retrievability(elapsed_days, x.stability) * 100.0).round() / 100.0
                }),
            })
        })
        .collect();
    Json(json!({
        "due": q.due.len(), "new": q.new.len(), "total": q.items.len(),
        "queued": items.len(),
        "weak": weak_n, "leech": leech_n,
        "session_limit": cfg.session_limit,
        "retention": cfg.desired_retention,
        "cards": items,
    }))
}

/// 错题本：最近 30 天末尾连续答错 >=2 次的卡，连对两次自然移出。
async fn api_weak_cards(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    if let Err(e) = s.store.ensure_cards() {
        return Json(json!({ "error": e.to_string() }));
    }
    let limit: usize = q.get("limit").and_then(|x| x.parse().ok()).unwrap_or(50);
    let now = now_ms();
    let (events, cards, entities) = s.store.read(|m| {
        (
            m.events.clone(),
            m.cards_flat(),
            m.entities_vec(),
        )
    });
    let weak = core_weak_cards(&events, now, limit);
    let items: Vec<JsonValue> = weak
        .iter()
        .filter_map(|w| cards.iter().find(|c| c.id == w.card_id).map(|c| (c, w)))
        .map(|(c, w)| {
            let name = entities
                .iter()
                .find(|e| e.id == c.entity_id)
                .map(|e| e.display_of("name"))
                .unwrap_or_else(|| c.entity_id.clone());
            json!({
                "card_id": c.id, "prompt": c.prompt, "direction": c.direction,
                "entity_name": name,
                "answer_fields": c.answer_fields,
                "misses": w.misses, "last_ts": w.last_ts,
            })
        })
        .collect();
    Json(json!({ "items": items }))
}

async fn api_review_config_get(State(s): State<AppState>) -> Json<JsonValue> {
    Json(json!(review_cfg(s.store.root())))
}

async fn api_review_config_put(
    State(s): State<AppState>,
    Json(patch): Json<JsonValue>,
) -> Json<JsonValue> {
    // 补丁语义：只传要改的字段，其余沿用当前配置（与 mneme-ffi 的 setReviewConfig 口径一致）
    let mut cur = serde_json::to_value(review_cfg(s.store.root())).unwrap_or_else(|_| json!({}));
    if let (Some(obj), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            obj.insert(k.clone(), v.clone());
        }
    }
    let cfg: ReviewConfig = match serde_json::from_value::<ReviewConfig>(cur) {
        Ok(c) => c.normalized(),
        Err(e) => return Json(json!({ "error": format!("复习配置 JSON 有误: {e}") })),
    };
    match serde_json::to_vec_pretty(&cfg) {
        Ok(bytes) => match fs::write(s.store.root().join("review.json"), bytes) {
            Ok(()) => Json(json!({ "ok": true, "config": cfg })),
            Err(e) => Json(json!({ "error": format!("保存 review.json 失败: {e}") })),
        },
        Err(e) => Json(json!({ "error": format!("复习配置序列化失败: {e}") })),
    }
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
    let target = s
        .engine
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .and_then(|e| e.target("github-private").ok());
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
    let engine = s.engine.read().unwrap_or_else(|e| e.into_inner()).clone();
    let targets = engine.as_ref().map(|e| e.target_ids()).unwrap_or_default();
    Json(json!({ "targets": targets, "configured": engine.is_some() }))
}

async fn api_sync(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    let engine = s.engine.read().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(engine) = engine else {
        return Json(json!({ "error": "未配置 data/sync.json" }));
    };
    let dry = q.get("dry").is_some_and(|x| x == "true" || x == "1");
    match engine.sync(&id, dry).await {
        Ok(r) => Json(json!({ "ok": true, "report": r.to_string(), "sha256": r.sha256, "retries": r.retries })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// 桌面端保存同步配置：写 `sync.json` 并热替换引擎。
///
/// token 允许明文（桌面单机、文件就在用户自己的数据目录里）或 `${ENV:VAR}` 引用；
/// 与手机端不同，桌面没有 Keystore，这份文件不要提交或外传。
#[derive(Deserialize)]
struct SyncConfigBody {
    owner: String,
    repo: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    path: String,
    token: String,
}

async fn api_sync_config(State(s): State<AppState>, Json(b): Json<SyncConfigBody>) -> Json<JsonValue> {
    if b.owner.trim().is_empty() || b.repo.trim().is_empty() {
        return Json(json!({ "error": "owner / repo 不能为空" }));
    }
    if b.token.trim().is_empty() {
        // 没有 token 却覆盖配置 = 把引擎换成必然 401 的状态，宁可拒绝
        let engine = s.engine.read().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(e) = engine else {
            return Json(json!({ "error": "首次配置必须提供 token" }));
        };
        // 已有引擎时沿用其 token（UI 留空 = 不改凭据）
        let old = e.config().targets.iter().find_map(|t| match t {
            mneme_sync::TargetConfig::GitHub { token, .. } => Some(token.clone()),
            _ => None,
        });
        let Some(old) = old else {
            return Json(json!({ "error": "首次配置必须提供 token" }));
        };
        return apply_sync_config(&s, b.owner, b.repo, b.branch, b.path, old).await;
    }
    apply_sync_config(&s, b.owner, b.repo, b.branch, b.path, b.token.trim().to_string()).await
}

async fn apply_sync_config(
    s: &AppState,
    owner: String,
    repo: String,
    branch: String,
    path: String,
    token: String,
) -> Json<JsonValue> {
    use mneme_sync::TargetConfig;
    let cfg = SyncConfig {
        device_id: s.store.device().to_string(),
        targets: vec![TargetConfig::GitHub {
            id: "github-private".into(),
            token,
            owner,
            repo,
            branch: if branch.trim().is_empty() { "main".into() } else { branch.trim().into() },
            path: path.trim().to_string(),
        }],
        exclude: vec![],
        max_retries: 3,
        upload_snapshots: true,
        keep_snapshots: 30,
    };
    if let Err(e) = cfg.to_file(&s.store.root().join("sync.json")) {
        return Json(json!({ "error": e.to_string() }));
    }
    let ids = cfg.targets.iter().map(|t| t.id().to_string()).collect::<Vec<_>>();
    *s.engine.write().unwrap_or_else(|e| e.into_inner()) =
        Some(Arc::new(SyncEngine::new(s.store.clone(), cfg)));
    Json(json!({ "ok": true, "targets": ids, "configured": true }))
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


// ---------------------------------------------------------------------------
// 手机端能力补齐：与 mneme-ffi 的对应函数同源（store / core 层），
// 这里只做 HTTP 形态适配，不重写任何业务规则。
// ---------------------------------------------------------------------------
fn errj(msg: impl Into<String>) -> Json<JsonValue> {
    Json(json!({ "error": msg.into() }))
}

fn schema_of(s: &AppState) -> Option<mneme_core::DomainSchema> {
    s.store.read(|m| m.schema.clone())
}

/// 回收站：墓碑条目（与 FFI listDeleted 同源）。
async fn api_deleted(State(s): State<AppState>) -> Json<JsonValue> {
    let (kind, primary, items) = s.store.read(|m| {
        let schema = m.schema.as_ref();
        let kind = schema
            .and_then(|x| x.entities.first().map(|e| e.kind.clone()))
            .unwrap_or_else(|| "herb".into());
        let primary = schema
            .and_then(|x| x.entity_def(&kind).and_then(|d| d.primary.clone()))
            .unwrap_or_else(|| "name".into());
        let items: Vec<JsonValue> = m
            .entities
            .values()
            .filter(|e| e.deleted)
            .map(|e| json!({
                "id": e.id, "kind": e.kind, "name": e.display_of(&primary),
                "deleted_at": e.updated_at, "rev": e.rev, "device": e.device,
            }))
            .collect();
        (kind, primary, items)
    });
    let _ = (kind, primary);
    Json(json!({ "items": items }))
}

/// 字段级变更历史（与 FFI entityHistory 同源）。
async fn api_history(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<JsonValue> {
    let limit: usize = q.get("limit").and_then(|x| x.parse().ok()).unwrap_or(50);
    match s.store.entity_history(&id, limit) {
        Ok(items) => Json(json!({ "items": items })),
        Err(e) => errj(e.to_string()),
    }
}

/// 分组树 + 未分组计数。
async fn api_groups(State(s): State<AppState>) -> Json<JsonValue> {
    let (groups, entities) = s.store.read(|m| (m.groups.values().cloned().collect::<Vec<_>>(), m.entities_vec()));
    let tree = group_tree(&groups);
    let items: Vec<JsonValue> = tree.iter().map(|n| {
        let count = entities.iter().filter(|e| !e.deleted && e.group_id.as_deref() == Some(n.id.as_str())).count();
        json!({
            "id": n.id, "name": n.name, "parent": n.parent, "depth": n.depth,
            "path": n.display_path(), "path_names": n.path_names, "path_ids": n.path_ids,
            "entities": count,
        })
    }).collect();
    let unassigned = entities.iter().filter(|e| {
        !e.deleted && e.group_id.as_deref().map(|g| !tree.iter().any(|n| n.id == g)).unwrap_or(true)
    }).count();
    Json(json!({ "items": items, "unassigned": unassigned, "total": tree.len() }))
}

#[derive(Deserialize)]
struct GroupBody {
    #[serde(default)]
    id: String,
    name: String,
    #[serde(default)]
    parent: String,
}

/// 新建 / 重命名 / 改上级：名称非空、禁止成环、限制深度。
async fn api_group_upsert(State(s): State<AppState>, Json(b): Json<GroupBody>) -> Json<JsonValue> {
    let name = b.name.trim().to_string();
    if name.is_empty() { return errj("分组名不能为空"); }
    let groups: Vec<CoreGroup> = s.store.read(|m| m.groups.values().cloned().collect());
    let id = if b.id.trim().is_empty() { name.clone() } else { b.id.trim().to_string() };
    let parent = if b.parent.trim().is_empty() { None } else { Some(b.parent.trim().to_string()) };
    let existing = groups.iter().find(|g| g.id == id).cloned();
    if let Some(p) = parent.as_deref() {
        if !groups.iter().any(|g| g.id == p && !g.deleted) {
            return errj(format!("上级分组不存在：{p}"));
        }
        if would_cycle(&groups, &id, Some(p)) { return errj("不能把分组挂到自己的下级里"); }
        if path_names(&groups, p).len() >= MAX_DEPTH {
            return errj(format!("最多 {MAX_DEPTH} 层，不能再往下了"));
        }
    }
    let now = now_ms();
    let device = s.store.device().to_string();
    let mut rev = s.store.stats().rev + 1;
    let renamed = existing.as_ref().map(|g| g.name != name).unwrap_or(false);
    let mut ops: Vec<Op> = vec![Op::UpsertGroup {
        group: CoreGroup {
            id: id.clone(), kind: "group".into(), name: name.clone(), parent: parent.clone(),
            common: existing.as_ref().map(|g| g.common.clone()).unwrap_or_default(),
            rev, updated_at: now, device: device.clone(), deleted: false,
        },
    }];
    if renamed {
        let members: Vec<CoreEntity> = s.store.read(|m| {
            m.entities_vec().into_iter().filter(|e| e.group_id.as_deref() == Some(id.as_str())).collect()
        });
        for mut e in members {
            e.set("group_name", CoreValue::Text(name.clone()), now);
            rev += 1;
            e.rev = rev;
            e.updated_at = now;
            e.device = device.clone();
            ops.push(Op::UpsertEntity { entity: e });
        }
    }
    match s.store.mutate(&ops) {
        Ok(_) => Json(json!({ "ok": true, "id": id, "name": name, "parent": parent })),
        Err(e) => errj(e.to_string()),
    }
}

/// 删除分组：子组上移、成员变未分组，写墓碑而非物理删。
async fn api_group_delete(State(s): State<AppState>, Json(b): Json<IdBody>) -> Json<JsonValue> {
    let (groups, entities) = s.store.read(|m| (m.groups.values().cloned().collect::<Vec<_>>(), m.entities_vec()));
    let Some(target) = groups.iter().find(|g| g.id == b.id && !g.deleted).cloned() else {
        return errj(format!("分组不存在：{}", b.id));
    };
    let now = now_ms();
    let device = s.store.device().to_string();
    let mut rev = s.store.stats().rev;
    let mut ops: Vec<Op> = Vec::new();
    let children: Vec<CoreGroup> = groups.iter()
        .filter(|g| !g.deleted && g.parent.as_deref() == Some(b.id.as_str())).cloned().collect();
    for mut c in children.clone() {
        c.parent = target.parent.clone();
        rev += 1; c.rev = rev; c.updated_at = now; c.device = device.clone();
        ops.push(Op::UpsertGroup { group: c });
    }
    let members: Vec<CoreEntity> = entities.into_iter()
        .filter(|e| e.group_id.as_deref() == Some(b.id.as_str())).collect();
    for mut e in members.clone() {
        e.group_id = None;
        if e.get("group_name").is_some() { e.set("group_name", CoreValue::Text(String::new()), now); }
        rev += 1; e.rev = rev; e.updated_at = now; e.device = device.clone();
        ops.push(Op::UpsertEntity { entity: e });
    }
    rev += 1;
    let mut tomb = target.clone();
    tomb.deleted = true; tomb.rev = rev; tomb.updated_at = now; tomb.device = device;
    ops.push(Op::UpsertGroup { group: tomb });
    match s.store.mutate(&ops) {
        Ok(_) => Json(json!({ "ok": true, "id": b.id, "reparented": children.len(), "unassigned": members.len() })),
        Err(e) => errj(e.to_string()),
    }
}

#[derive(Deserialize)]
struct DuplicateBody {
    kind: String,
    fields: BTreeMap<String, String>,
    #[serde(default)]
    exclude_id: String,
}

async fn api_check_duplicate(State(s): State<AppState>, Json(b): Json<DuplicateBody>) -> Json<JsonValue> {
    let Some(schema) = schema_of(&s) else { return errj("还没有加载领域 schema") };
    let entities = s.store.read(|m| m.entities_vec());
    let exclude = if b.exclude_id.trim().is_empty() { None } else { Some(b.exclude_id.as_str()) };
    let items = find_duplicates(&schema, &entities, &b.kind, &b.fields, exclude);
    Json(json!({ "items": items.iter().map(|d| json!({
        "id": d.id, "name": d.name, "reason": d.reason, "score": d.score,
    })).collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
struct PasteBody {
    text: String,
    #[serde(default)]
    kind: String,
}

/// 粘贴解析：单条与批量统一返回 blocks（与 FFI parsePasteMany 同源）。
async fn api_parse_paste(State(s): State<AppState>, Json(b): Json<PasteBody>) -> Json<JsonValue> {
    let Some(schema) = schema_of(&s) else { return errj("还没有加载领域 schema") };
    let kind = if b.kind.trim().is_empty() {
        schema.entities.first().map(|e| e.kind.clone()).unwrap_or_else(|| "herb".into())
    } else { b.kind.trim().to_string() };
    if b.text.trim().is_empty() { return Json(json!({ "blocks": [], "count": 0, "primary": "name" })); }
    let batch = core_parse_paste_batch(&b.text, &schema, &kind);
    let blocks: Vec<JsonValue> = batch.blocks.iter().enumerate().map(|(i, p)| json!({
        "index": i, "fields": p.fields,
        "hits": p.hits.iter().map(|h| json!({
            "key": h.key, "label": h.label, "value": h.value, "line": h.line,
        })).collect::<Vec<_>>(),
        "unknown": p.unknown, "conflicts": p.conflicts, "matched": p.fields.len(),
    })).collect();
    let primary = schema.entity_def(&kind).and_then(|d| d.primary.clone()).unwrap_or_else(|| "name".into());
    Json(json!({
        "blocks": blocks, "count": batch.blocks.len(),
        "skipped_blocks": batch.skipped_blocks, "primary": primary,
    }))
}

#[derive(Deserialize)]
struct BatchItem {
    #[serde(default)]
    fields: BTreeMap<String, JsonValue>,
    #[serde(default)]
    id_hint: String,
}
#[derive(Deserialize)]
struct BatchBody {
    kind: String,
    items: Vec<BatchItem>,
}

/// 批量新建：逐条走 create_one（查重 / superseded / 原子写与手机端一致）。
async fn api_create_batch(State(s): State<AppState>, Json(b): Json<BatchBody>) -> Json<JsonValue> {
    let Some(schema) = schema_of(&s) else { return errj("还没有加载领域 schema") };
    if b.items.is_empty() { return errj("没有要新建的条目"); }
    let (mut created, mut skipped, mut failed): (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) = (vec![], vec![], vec![]);
    for (index, item) in b.items.iter().enumerate() {
        match mneme_store::entities::create_one(&s.store, &schema, &b.kind, &item.fields, item.id_hint.trim()) {
            Ok(c) => created.push(json!({ "index": index, "id": c.id, "rev": c.rev, "name": c.name })),
            Err(mneme_store::entities::CreateErr::Duplicate { message, id, deleted }) =>
                skipped.push(json!({ "index": index, "name": id, "reason": message, "existing_id": id, "deleted": deleted })),
            Err(mneme_store::entities::CreateErr::Other(e)) => failed.push(json!({ "index": index, "error": e })),
        }
    }
    Json(json!({ "ok": true, "created": created, "skipped": skipped, "failed": failed }))
}

#[derive(Deserialize)]
struct PreviewBody {
    content: String,
    filename: String,
}

/// 导入差异预览：逐条 create / update / same + immutable 警告，不写库。
async fn api_import_preview(State(s): State<AppState>, Json(b): Json<PreviewBody>) -> Json<JsonValue> {
    let bytes = b.content.as_bytes();
    let fmt = codec::detect(Some(FsPath::new(&b.filename)), bytes);
    if !fmt.importable() { return errj(format!("{fmt} 不支持导入")); }
    match codec::preview_import(&s.store, bytes, fmt) {
        Ok(p) => Json(json!(p)),
        Err(e) => errj(e.to_string()),
    }
}
