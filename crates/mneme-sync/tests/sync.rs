use async_trait::async_trait;
use mneme_core::{Entity, ReviewEvent, Value, now_ms};
use mneme_store::{Op, Snapshot, Store};
use mneme_sync::{
    Error, RemoteMeta, Result, SyncConfig, SyncEngine, SyncTarget, TargetConfig,
    targets::LocalDir,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const SCHEMA: &str = r#"
id = "test"
name = "测试"
[[entities]]
kind = "herb"
primary = "name"
[[entities.fields]]
key = "name"
kind = "text"
[[entities.fields]]
key = "effects"
kind = "tags"
indexable = true
reversible = true
[[entities.directions]]
id = "effect_to_name"
clue_fields = ["effects"]
answer_fields = ["name"]
"#;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mneme-sync-{}-{}-{}", name, std::process::id(), now_ms()))
}

fn make_store(name: &str, device: &str, herbs: &[&str]) -> Arc<Store> {
    let s = Store::open(tmp(name), device).unwrap();
    let mut ops = vec![Op::SetSchema { toml: SCHEMA.into() }];
    for (i, h) in herbs.iter().enumerate() {
        let mut e = Entity::new(*h, "herb", device, 1 + i as i64);
        e.set("name", Value::Text((*h).into()), 1);
        e.set("effects", Value::Tags(vec![format!("功效{}", i)]), 1);
        ops.push(Op::UpsertEntity { entity: e });
    }
    ops.push(Op::Review {
        event: ReviewEvent {
            card_id: format!("c-{device}"),
            ts: 1000 + device.len() as i64,
            grade: 3,
            device: device.into(),
            seq: 0,
        },
    });
    s.mutate(&ops).unwrap();
    s
}

fn cfg(remote: &Path, device: &str, exclude: Vec<String>) -> SyncConfig {
    SyncConfig {
        device_id: device.into(),
        targets: vec![TargetConfig::Local {
            id: "local".into(),
            dir: remote.to_string_lossy().to_string(),
        }],
        exclude,
        max_retries: 3,
        upload_snapshots: true,
        keep_snapshots: 30,
    }
}

/// 第一次条件写故意失败，验证引擎的 412 自愈路径
struct Flaky {
    inner: LocalDir,
    fail: AtomicUsize,
}

#[async_trait]
impl SyncTarget for Flaky {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn kind(&self) -> &str {
        "flaky"
    }
    async fn head(&self, p: &str) -> Result<Option<RemoteMeta>> {
        self.inner.head(p).await
    }
    async fn get(&self, p: &str) -> Result<Vec<u8>> {
        self.inner.get(p).await
    }
    async fn put(&self, p: &str, d: &[u8], m: Option<&str>) -> Result<RemoteMeta> {
        if p == "latest.json" && self.fail.swap(0, Ordering::SeqCst) > 0 {
            return Err(Error::PreconditionFailed("模拟 412".into()));
        }
        self.inner.put(p, d, m).await
    }
    async fn list(&self, p: &str) -> Result<Vec<String>> {
        self.inner.list(p).await
    }
    async fn delete(&self, p: &str) -> Result<()> {
        self.inner.delete(p).await
    }
}

#[tokio::test]
async fn two_devices_converge() {
    let remote = tmp("converge-remote");
    let win = make_store("converge-win", "win", &["麻黄", "桂枝"]);
    let phone = make_store("converge-phone", "phone", &["薄荷"]);

    let e1 = SyncEngine::new(win.clone(), cfg(&remote, "win", vec![]));
    let r1 = e1.sync("local", false).await.unwrap();
    assert!(r1.remote_snapshot.is_none(), "首次同步远端应为空");
    assert_eq!(r1.sha256.len(), 64);

    let e2 = SyncEngine::new(phone.clone(), cfg(&remote, "phone", vec![]));
    let r2 = e2.sync("local", false).await.unwrap();
    assert!(r2.remote_snapshot.is_some());
    assert_eq!(phone.stats().entities, 3);

    let r3 = e1.sync("local", false).await.unwrap();
    assert!(r3.remote_snapshot.is_some());
    assert_eq!(win.stats().entities, 3, "win 应拿到 phone 的新增药");
    assert_eq!(win.stats().entities, phone.stats().entities);
    assert_eq!(win.stats().events, 2, "两台设备的复习事件应各保留一条");
    assert_eq!(phone.stats().events, 2);
}

#[tokio::test]
async fn repeated_sync_is_idempotent() {
    let remote = tmp("idempotent-remote");
    let s = make_store("idempotent", "win", &["麻黄"]);
    let e = SyncEngine::new(s.clone(), cfg(&remote, "win", vec![]));
    e.sync("local", false).await.unwrap();
    let before = s.stats();
    e.sync("local", false).await.unwrap();
    e.sync("local", false).await.unwrap();
    assert_eq!(s.stats().events, before.events, "重复同步不能重复计入事件");
    assert_eq!(s.stats().entities, before.entities);
}

#[tokio::test]
async fn precondition_failure_self_heals() {
    let remote = tmp("heal-remote");
    let s = make_store("heal", "win", &["麻黄"]);
    let e = SyncEngine::new(s.clone(), cfg(&remote, "win", vec![]));
    let flaky = Arc::new(Flaky {
        inner: LocalDir::new("local", &remote).unwrap(),
        fail: AtomicUsize::new(1),
    });
    e.inject_target("local", flaky);
    let r = e.sync("local", false).await.unwrap();
    assert_eq!(r.retries, 1, "412 应触发一次自愈重试并最终成功");
}

#[tokio::test]
async fn excluded_components_stay_local() {
    let remote = tmp("exclude-remote");
    let s = make_store("exclude", "win", &["麻黄"]);
    let mut secret = Entity::new("私密条目", "secret", "win", 9);
    secret.set("name", Value::Text("私密条目".into()), 9);
    s.mutate(&[Op::UpsertEntity { entity: secret }]).unwrap();
    assert_eq!(s.stats().entities, 2);

    let e = SyncEngine::new(
        s.clone(),
        cfg(&remote, "win", vec!["kind:secret".into()]),
    );
    let r = e.sync("local", false).await.unwrap();
    assert_eq!(r.excluded, vec!["kind:secret".to_string()]);
    assert_eq!(s.stats().entities, 2, "本地数据必须完整");

    let t = LocalDir::new("local", &remote).unwrap();
    let latest: serde_json::Value = serde_json::from_slice(&t.get("latest.json").await.unwrap()).unwrap();
    let snap = Snapshot::from_bytes(&t.get(latest["snapshot"].as_str().unwrap()).await.unwrap()).unwrap();
    assert!(
        !snap.entities.iter().any(|x| x.kind == "secret"),
        "被排除的组件不应上传"
    );
    assert_eq!(latest["excluded"][0], "kind:secret", "排除项要显式写进 latest");
}

#[tokio::test]
async fn dry_run_does_not_upload() {
    let remote = tmp("dry-remote");
    let s = make_store("dry", "win", &["麻黄"]);
    let e = SyncEngine::new(s.clone(), cfg(&remote, "win", vec![]));
    let r = e.sync("local", true).await.unwrap();
    assert!(r.dry_run);
    assert_eq!(r.bytes_up, 0);
    let t = LocalDir::new("local", &remote).unwrap();
    assert!(t.head("latest.json").await.unwrap().is_none());
}

#[tokio::test]
async fn events_can_be_excluded_entirely() {
    let remote = tmp("noevents-remote");
    let s = make_store("noevents", "win", &["麻黄"]);
    let e = SyncEngine::new(s.clone(), cfg(&remote, "win", vec!["events".into()]));
    e.sync("local", false).await.unwrap();
    let t = LocalDir::new("local", &remote).unwrap();
    assert!(t.list("events/").await.unwrap().is_empty());
}
