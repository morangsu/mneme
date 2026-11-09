use crate::{
    error::{Error, Result},
    traits::SyncTarget,
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use mneme_core::{Entity, content_hash};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

pub const MANIFEST_PATH: &str = "manifest.json";
pub const LOG_PATH: &str = "sync/log.jsonl";
pub const DECISION_LOG_PATH: &str = "sync/upstream-decisions.jsonl";
pub const MANIFEST_FORMAT: &str = "mneme-manifest/1";

/// 每个设备分片的指纹与长度。长度就是增量拉取的 offset。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShardMeta {
    pub sha256: String,
    pub len: u64,
    pub rev: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeltaMeta {
    pub path: String,
    pub from_rev: u64,
    pub to_rev: u64,
    pub sha256: String,
}

/// 远端目录索引。设备先看它，再决定要不要下载分片/快照，
/// 这样在 GitHub 匿名 60/h、认证 5000/h 的配额下不会反复拉同样的东西。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub rev: u64,
    pub updated_at: i64,
    pub device: String,
    pub snapshot: Option<String>,
    pub snapshot_sha256: String,
    pub events: BTreeMap<String, ShardMeta>,
    pub deltas: Vec<DeltaMeta>,
    pub excluded: Vec<String>,
    /// 私有库当前采用的公共基础库版本（拉取决策后推进）
    pub public_upstream_rev: u64,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format: MANIFEST_FORMAT.into(),
            version: 1,
            rev: 0,
            updated_at: 0,
            device: String::new(),
            snapshot: None,
            snapshot_sha256: String::new(),
            events: BTreeMap::new(),
            deltas: Vec::new(),
            excluded: Vec::new(),
            public_upstream_rev: 0,
        }
    }
}

impl Manifest {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let m: Self = serde_json::from_slice(b)?;
        if m.format != MANIFEST_FORMAT {
            return Err(Error::Sync(format!("未知 manifest 格式 {}", m.format)));
        }
        Ok(m)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncLogEntry {
    pub ts: i64,
    pub device: String,
    pub target: String,
    pub action: String,
    pub rev: u64,
    pub sha256: String,
    pub bytes_up: usize,
    pub bytes_down: usize,
    pub conflicts: usize,
    pub retries: u32,
    pub note: String,
}

impl SyncLogEntry {
    pub fn from_report(r: &crate::engine::SyncReport, now: i64) -> Self {
        Self {
            ts: now,
            device: String::new(),
            target: r.target.clone(),
            action: if r.dry_run { "dry-run" } else { "sync" }.into(),
            rev: 0,
            sha256: r.sha256.clone(),
            bytes_up: r.bytes_up,
            bytes_down: r.bytes_down,
            conflicts: r.conflicts,
            retries: r.retries,
            note: format!("backend={} pulled={} pushed={}", r.backend, r.pulled_events, r.pushed_event_bytes),
        }
    }
}

/// 纯 Rust gzip（miniz_oxide），不引 C 依赖。
pub fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut e = GzEncoder::new(Vec::with_capacity(bytes.len() / 2), Compression::fast());
    e.write_all(bytes)?;
    Ok(e.finish()?)
}

pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut d = GzDecoder::new(bytes);
    let mut out = Vec::new();
    d.read_to_end(&mut out)?;
    Ok(out)
}

/// 只打包自 base 以来变化的实体，避免每次同步都传全量快照（10k ≈ 2.7 MB）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaSnapshot {
    pub from_rev: u64,
    pub to_rev: u64,
    pub entities: Vec<Entity>,
    pub tombstones: Vec<String>,
    pub checksum: String,
}

impl DeltaSnapshot {
    pub fn between(base: &[Entity], current: &[Entity], from_rev: u64, to_rev: u64) -> Self {
        use std::collections::HashMap;
        let before: HashMap<&str, String> = base
            .iter()
            .map(|e| (e.id.as_str(), e.fingerprint()))
            .collect();
        let mut entities = Vec::new();
        let mut tombstones = Vec::new();
        for e in current {
            if e.deleted {
                tombstones.push(e.id.clone());
            } else if before.get(e.id.as_str()) != Some(&e.fingerprint()) {
                entities.push(e.clone());
            }
        }
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        tombstones.sort();
        let mut d = Self {
            from_rev,
            to_rev,
            entities,
            tombstones,
            checksum: String::new(),
        };
        d.checksum = d.compute();
        d
    }
    fn compute(&self) -> String {
        let mut c = self.clone();
        c.checksum.clear();
        content_hash(&serde_json::to_vec(&c).unwrap_or_default())
    }
    pub fn verify(&self) -> Result<()> {
        if self.compute() != self.checksum {
            return Err(Error::Sync("delta 校验和不匹配".into()));
        }
        Ok(())
    }
}

pub async fn read_manifest(t: &dyn SyncTarget) -> Result<Manifest> {
    match t.get(MANIFEST_PATH).await {
        Ok(b) => Manifest::from_bytes(&b),
        Err(Error::NotFound(_)) => Ok(Manifest::default()),
        Err(e) => Err(e),
    }
}

/// manifest 用条件写：并发同步时后写者收到 412，由引擎自愈重跑。
pub async fn write_manifest(t: &dyn SyncTarget, m: &Manifest) -> Result<()> {
    let head = t.head(MANIFEST_PATH).await?;
    t.put(
        MANIFEST_PATH,
        &m.to_bytes()?,
        head.as_ref().map(|x| x.etag.as_str()),
    )
    .await?;
    Ok(())
}

pub async fn append_log(t: &dyn SyncTarget, e: &SyncLogEntry) -> Result<()> {
    let head = t.head(LOG_PATH).await?;
    let mut bytes = t.get(LOG_PATH).await.unwrap_or_default();
    bytes.extend_from_slice(serde_json::to_string(e)?.as_bytes());
    bytes.push(b'\n');
    t.put(LOG_PATH, &bytes, head.as_ref().map(|x| x.etag.as_str()))
        .await?;
    Ok(())
}

/// 把公共库拉取的逐项决策写进私有库，GitHub 上每次写入都是一个 commit -> 完全可溯源。
pub async fn append_decisions(t: &dyn SyncTarget, entries: &[serde_json::Value]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let head = t.head(DECISION_LOG_PATH).await?;
    let mut bytes = t.get(DECISION_LOG_PATH).await.unwrap_or_default();
    for e in entries {
        bytes.extend_from_slice(serde_json::to_string(e)?.as_bytes());
        bytes.push(b'\n');
    }
    t.put(
        DECISION_LOG_PATH,
        &bytes,
        head.as_ref().map(|x| x.etag.as_str()),
    )
    .await?;
    Ok(())
}

/// 增量拉事件：`sha256` 相同直接跳过；否则只取本地已消费长度之后的字节。
/// append-only 分片保证前缀不变，因此不需要 CRDT，也不需要断点续传协议。
pub async fn incremental_events(
    t: &dyn SyncTarget,
    m: &Manifest,
    local: &BTreeMap<String, ShardMeta>,
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for (dev, meta) in &m.events {
        if local.get(dev).is_some_and(|x| x.sha256 == meta.sha256) {
            continue;
        }
        let path = format!("events/{dev}.jsonl");
        let bytes = match t.get(&path).await {
            Ok(b) => b,
            Err(Error::NotFound(_)) => continue,
            Err(e) => return Err(e),
        };
        let offset = local.get(dev).map(|x| x.len as usize).unwrap_or(0);
        let slice = if offset <= bytes.len() {
            bytes[offset..].to_vec()
        } else {
            // 远端分片被重建过（比本地短）：回退到整片，靠行级去重保证幂等
            bytes.clone()
        };
        if !slice.is_empty() {
            out.push((dev.clone(), slice));
        }
    }
    Ok(out)
}

/// 从本地日志目录算出各分片的当前指纹与长度。
pub fn local_shards(bytes_of: impl Fn(&str) -> Option<Vec<u8>>, names: &[String]) -> BTreeMap<String, ShardMeta> {
    let mut out = BTreeMap::new();
    for dev in names {
        if let Some(b) = bytes_of(dev) {
            out.insert(
                dev.clone(),
                ShardMeta {
                    sha256: content_hash(&b),
                    len: b.len() as u64,
                    rev: 0,
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::LocalDir;
    use mneme_core::{Entity, Value, now_ms};
    use std::path::PathBuf;

    fn tmp(n: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mneme-inc-{}-{}-{}", n, std::process::id(), now_ms()))
    }

    #[test]
    fn gzip_roundtrip() {
        let raw = b"hello mneme \xe4\xb8\xad\xe6\x96\x87".repeat(100);
        let packed = gzip(&raw).unwrap();
        assert!(packed.len() < raw.len());
        assert_eq!(gunzip(&packed).unwrap(), raw);
    }

    #[test]
    fn delta_only_contains_changed_entities() {
        let mut a = Entity::new("a", "herb", "w", 1);
        a.set("name", Value::Text("a".into()), 1);
        let mut b = Entity::new("b", "herb", "w", 1);
        b.set("name", Value::Text("b".into()), 1);
        let base = vec![a.clone(), b.clone()];
        b.set("name", Value::Text("改名了".into()), 2);
        let mut c = Entity::new("c", "herb", "w", 1);
        c.set("name", Value::Text("c".into()), 1);
        let d = DeltaSnapshot::between(&base, &vec![a, b, c], 1, 2);
        assert_eq!(d.entities.len(), 2);
        d.verify().unwrap();
    }

    #[tokio::test]
    async fn manifest_and_incremental_events() {
        let root = tmp("events");
        let t = LocalDir::new("local", &root).unwrap();
        t.put("events/phone.jsonl", b"line1\nline2\n", None).await.unwrap();
        let bytes = t.get("events/phone.jsonl").await.unwrap();
        let mut m = Manifest::default();
        m.events.insert(
            "phone".into(),
            ShardMeta {
                sha256: content_hash(&bytes),
                len: bytes.len() as u64,
                rev: 1,
            },
        );
        write_manifest(&t, &m).await.unwrap();

        // 已消费到同样长度 -> 不产生任何下载
        let local = local_shards(|d| if d == "phone" { Some(bytes.clone()) } else { None }, &["phone".into()]);
        let inc = incremental_events(&t, &read_manifest(&t).await.unwrap(), &local)
            .await
            .unwrap();
        assert!(inc.is_empty());

        // 远端追加一行 -> 只拿新增那一行
        let mut more = bytes.clone();
        more.extend_from_slice(b"line3\n");
        t.put("events/phone.jsonl", &more, None).await.unwrap();
        let mut m2 = Manifest::default();
        m2.events.insert(
            "phone".into(),
            ShardMeta { sha256: content_hash(&more), len: more.len() as u64, rev: 2 },
        );
        write_manifest(&t, &m2).await.unwrap();
        let inc2 = incremental_events(&t, &read_manifest(&t).await.unwrap(), &local)
            .await
            .unwrap();
        assert_eq!(inc2.len(), 1);
        assert_eq!(inc2[0].1, b"line3\n");
    }

    #[tokio::test]
    async fn log_is_append_only_and_readable() {
        let root = tmp("log");
        let t = LocalDir::new("local", &root).unwrap();
        for i in 0..3 {
            append_log(&t, &SyncLogEntry {
                ts: i, device: "w".into(), target: "github".into(), action: "sync".into(),
                rev: i as u64, sha256: format!("h{i}"), bytes_up: 1, bytes_down: 2,
                conflicts: 0, retries: 0, note: String::new(),
            }).await.unwrap();
        }
        let bytes = t.get(LOG_PATH).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&bytes).lines().count(), 3);
        append_decisions(&t, &[serde_json::json!({"conflict_id":"c1","decision":"keep_mine"})])
            .await
            .unwrap();
        assert!(!t.get(DECISION_LOG_PATH).await.unwrap().is_empty());
    }
}
