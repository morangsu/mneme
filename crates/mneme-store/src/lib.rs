#![forbid(unsafe_code)]
//! # mneme-store
//!
//! 存储层：**append-only 事件日志 + 不可变快照 + 内存倒排索引**。
//!
//! 为什么不直接同步一个 SQLite 文件：按文件同步一个「活的」数据库会撕裂索引
//! （快照/版本号机制与文件拷贝的语义不同，这是同类工具明确禁止的做法）。
//! 这里同步的最小单元是「不可变 JSON 文件 + 每设备一份只追加日志」，
//! 因此 WebDAV / GitHub / Git / S3 / 网盘挂载目录都能直接搬，且不会损坏数据。
//!
//! 并发：读多写少，外层一个 `parking_lot::RwLock`；索引与卡片重建在**锁外**
//! 算好再整体换入，写临界区只做「追加日志 + 更新 BTreeMap」。

mod error;
mod eventlog;
mod history;
mod memory;
mod op;
mod snapshot;

#[cfg(feature = "sqlite")]
pub mod sqlite_mirror;

pub use error::{Error, Result};
pub use eventlog::{EventLog, sanitize};
pub use history::{EntityRevision, FieldChange};
pub use memory::Memory;
pub use op::Op;
pub use snapshot::{Snapshot, SnapshotStore, FORMAT};

use mneme_core::now_ms;
use parking_lot::RwLock;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

/// 冷启动分解计时。
///
/// 为什么要暴露：桌面上「开窗到出数据」的预算只有几百毫秒，
/// 而 `open()` 里混着四件性质完全不同的事（读快照 / 重放日志 / 派生索引 / 刷镜像），
/// 不拆开就只能看到总数、猜不出该优化谁。
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenTimings {
    /// 读最新快照并校验
    pub snapshot_load: std::time::Duration,
    /// 重放各设备事件分片到内存
    pub replay: std::time::Duration,
    /// 派生倒排索引与卡片（`refresh`）
    pub derive: std::time::Duration,
    /// 全量重刷 SQLite 物化镜像
    pub sqlite_rebuild: std::time::Duration,
}

impl OpenTimings {
    pub fn total(&self) -> std::time::Duration {
        self.snapshot_load + self.replay + self.derive + self.sqlite_rebuild
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub entities: usize,
    pub groups: usize,
    pub cards: usize,
    pub events: usize,
    pub rev: u64,
    pub device: String,
}

pub struct Store {
    root: PathBuf,
    device: String,
    log: EventLog,
    snaps: SnapshotStore,
    mem: RwLock<Memory>,
    #[cfg(feature = "sqlite")]
    sqlite: Option<Arc<sqlite_mirror::SqliteMirror>>,
}

impl Store {
    /// 加载顺序：最新快照作基线 -> 重放所有设备分片日志 -> 派生索引与卡片。
    pub fn open(root: impl AsRef<Path>, device: &str) -> Result<Arc<Self>> {
        Ok(Self::open_timed(root, device)?.0)
    }

    /// 同 `open`，额外返回各阶段耗时（桌面端启动预算用）。
    ///
    /// 行为与 `open` 完全一致——计时用的是 `Instant`，不改变任何加载顺序。
    pub fn open_timed(
        root: impl AsRef<Path>,
        device: &str,
    ) -> Result<(Arc<Self>, OpenTimings)> {
        let mut t = OpenTimings::default();
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let log = EventLog::open(&root, device)?;
        let snaps = SnapshotStore::open(&root)?;

        let mut mem = Memory::default();
        mem.device = sanitize(device);
        let started = std::time::Instant::now();
        if let Some((_, snap)) = snaps.latest()? {
            snap.verify()?;
            mem.load_snapshot(&snap);
        }
        t.snapshot_load = started.elapsed();

        let started = std::time::Instant::now();
        log.replay(&mut mem)?;
        t.replay = started.elapsed();

        #[cfg(feature = "sqlite")]
        let sqlite = Some(Arc::new(sqlite_mirror::SqliteMirror::open(
            root.join("index.sqlite"),
        )?));
        let store = Arc::new(Self {
            root,
            device: sanitize(device),
            log,
            snaps,
            mem: RwLock::new(mem),
            #[cfg(feature = "sqlite")]
            sqlite,
        });
        let started = std::time::Instant::now();
        store.refresh()?;
        t.derive = started.elapsed();

        #[cfg(feature = "sqlite")]
        {
            let started = std::time::Instant::now();
            store.rebuild_sqlite()?;
            t.sqlite_rebuild = started.elapsed();
        }
        Ok((store, t))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn log(&self) -> &EventLog {
        &self.log
    }

    pub fn snapshots(&self) -> &SnapshotStore {
        &self.snaps
    }

    /// 只读访问。回调里不要做重活，避免占着读锁。
    pub fn read<R>(&self, f: impl FnOnce(&Memory) -> R) -> R {
        let m = self.mem.read();
        f(&m)
    }

    /// 唯一的写入口：先落盘（append-only），再应用内存，最后在锁外重建派生数据。
    pub fn mutate(&self, ops: &[Op]) -> Result<usize> {
        if ops.is_empty() {
            return Ok(0);
        }
        self.log.append(ops)?;
        let (n, dirty, old_cards) = {
            let mut m = self.mem.write();
            let mut n = 0;
            for op in ops {
                if m.apply(op) {
                    n += 1;
                }
            }
            let dirty = std::mem::take(&mut m.dirty);
            (n, dirty, m.cards.clone())
        };
        // P0：只有复习事件时索引/卡片都不变，连重建都跳过
        if dirty.is_empty() {
            #[cfg(feature = "sqlite")]
            if let Some(s) = &self.sqlite {
                for op in ops {
                    if let Op::Review { event } = op {
                        s.insert_event(event)?;
                    }
                }
            }
            return Ok(n);
        }
        // 派生在锁外算好，写临界区只做整体换入
        let (idx, cards) = {
            let m = self.mem.read();
            m.derive_incremental(&old_cards, &dirty)
        };
        {
            let mut m = self.mem.write();
            m.index = idx;
            m.cards = cards;
        }
        #[cfg(feature = "sqlite")]
        if let Some(s) = &self.sqlite {
            let (entities, all_cards, events) = {
                let m = self.mem.read();
                (m.entities_vec(), m.cards.clone(), m.events.clone())
            };
            s.apply_changes(&dirty, &entities, &all_cards, &events)?;
        }
        Ok(n)
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite(&self) -> Option<&Arc<sqlite_mirror::SqliteMirror>> {
        self.sqlite.as_ref()
    }

    /// SQLite 是可重建的物化镜像：从当前内存态整表重刷。
    /// 日志/快照才是真相源，所以镜像坏了直接调它即可。
    #[cfg(feature = "sqlite")]
    pub fn rebuild_sqlite(&self) -> Result<()> {
        if let Some(s) = &self.sqlite {
            let (entities, cards, events) = {
                let m = self.mem.read();
                (m.entities_vec(), m.cards.clone(), m.events.clone())
            };
            s.rebuild(&entities, &cards, &events)?;
        }
        Ok(())
    }

    /// 索引与卡片是派生数据，锁外算完整体换入。
    pub fn refresh(&self) -> Result<()> {
        let (idx, cards) = {
            let m = self.mem.read();
            m.derive()
        };
        {
            let mut m = self.mem.write();
            m.index = idx;
            m.cards = cards;
        }
        Ok(())
    }

    /// 同步拉回新的设备分片后，把日志重放进当前内存（apply 幂等，不会重复计数）。
    pub fn replay(&self) -> Result<usize> {
        let n = {
            let mut m = self.mem.write();
            self.log.replay(&mut *m)?
        };
        self.refresh()?;
        Ok(n)
    }

    pub fn stats(&self) -> Stats {
        self.mem.read().stats()
    }

    pub fn current_snapshot(&self, now: i64) -> Result<Snapshot> {
        let m = self.mem.read();
        Ok(Snapshot::from_memory(&m, &self.device, now))
    }

    /// 生成一份新的不可变快照，返回 (文件名, 文件 sha256)。
    pub fn make_snapshot(&self) -> Result<(String, String)> {
        let s = self.current_snapshot(now_ms())?;
        self.snaps.write(&s)
    }

    /// 覆盖前先备份本地状态，再装入远端快照。
    pub fn apply_remote_snapshot(&self, s: &Snapshot) -> Result<()> {
        s.verify()?;
        self.backup("before-remote")?;
        {
            let mut m = self.mem.write();
            m.load_snapshot(s);
        }
        self.snaps.write(s)?;
        self.refresh()?;
        #[cfg(feature = "sqlite")]
        self.rebuild_sqlite()?;
        Ok(())
    }

    pub fn backup(&self, tag: &str) -> Result<PathBuf> {
        let dir = self.root.join("backups");
        fs::create_dir_all(&dir)?;
        let s = self.current_snapshot(now_ms())?;
        let bytes = s.to_bytes()?;
        let hash = mneme_core::content_hash(&bytes);
        let p = dir.join(format!("{}-{}-{}.json", now_ms(), sanitize(tag), &hash[..8]));
        fs::write(&p, &bytes)?;
        Ok(p)
    }

    pub fn write_snapshot_bytes(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.snaps.write_bytes_as(name, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mneme_core::{Entity, ReviewEvent, Value};

    const SCHEMA: &str = r#"
id = "test"
name = "测试"
[[entities]]
kind = "herb"
primary = "name"
[[entities.fields]]
key = "name"
[[entities.fields]]
key = "effects"
kind = "tags"
indexable = true
reversible = true
[[entities.directions]]
id = "effect_to_name"
clue_fields = ["effects"]
answer_fields = ["name"]
enumerate_if_ambiguous = true
"#;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mneme-store-{}-{}-{}", name, std::process::id(), now_ms()))
    }

    fn make_herb(id: &str, effect: &str) -> Entity {
        let mut e = Entity::new(id, "herb", "win", 1);
        e.set("name", Value::Text(id.into()), 1);
        e.set("effects", Value::Tags(vec![effect.into()]), 1);
        e
    }

    fn seed(root: &Path, device: &str) -> Arc<Store> {
        let s = Store::open(root, device).unwrap();
        s.mutate(&[
            Op::SetSchema { toml: SCHEMA.into() },
            Op::UpsertEntity { entity: make_herb("麻黄", "宣肺平喘") },
            Op::UpsertEntity { entity: make_herb("桂枝", "温通经脉") },
            Op::Review {
                event: ReviewEvent {
                    card_id: "c1".into(),
                    ts: 1000,
                    grade: 3,
                    device: device.into(),
                    seq: 0,
                },
            },
        ])
        .unwrap();
        s
    }

    #[test]
    fn replay_and_snapshot_roundtrip() {
        let root = tmp("roundtrip");
        let s = seed(&root, "win");
        assert_eq!(s.stats().entities, 2);
        assert!(s.stats().cards > 0, "卡片应从主数据派生");
        let (name, hash) = s.make_snapshot().unwrap();
        assert_eq!(hash.len(), 64);
        drop(s);

        let s2 = Store::open(&root, "win").unwrap();
        assert_eq!(s2.stats().entities, 2);
        assert_eq!(s2.stats().events, 1);
        assert!(s2.snapshots().read(&name).is_ok());
    }

    #[test]
    fn truncated_last_line_does_not_break_open() {
        let root = tmp("truncated");
        let s = seed(&root, "win");
        let p = s.log().file_for("win");
        drop(s);
        let mut bytes = fs::read(&p).unwrap();
        bytes.extend_from_slice(b"{\"op\":\"upsert_ent");
        fs::write(&p, bytes).unwrap();
        let s2 = Store::open(&root, "win").unwrap();
        assert_eq!(s2.stats().entities, 2);
    }

    #[test]
    fn checksum_detects_tampering() {
        let root = tmp("checksum");
        let s = seed(&root, "win");
        let mut snap = s.current_snapshot(1).unwrap();
        snap.entities[0].set("effects", Value::Text("被改了".into()), 2);
        assert!(snap.verify().is_err());
    }

    #[test]
    fn device_shards_are_independent() {
        let root = tmp("shards");
        let a = seed(&root, "win");
        drop(a);
        let b = Store::open(&root, "phone").unwrap();
        b.mutate(&[Op::UpsertEntity { entity: make_herb("薄荷", "清利头目") }])
            .unwrap();
        assert_eq!(b.stats().entities, 3);
        let files = b.log().list_files().unwrap();
        assert_eq!(files.len(), 2, "每台设备只写自己的分片");
        drop(b);
        let c = Store::open(&root, "win").unwrap();
        assert_eq!(c.stats().entities, 3, "重放应读到两台设备的分片");
    }

    /// 与 make_herb 的区别：`|` 拆成多个标签（更接近真实编辑后的值），并指定 rev/设备/时间。
    fn edited(id: &str, effects: &str, rev: u64, device: &str, ts: i64) -> Entity {
        let mut e = Entity::new(id, "herb", device, ts);
        e.set("name", Value::Text(id.into()), ts);
        e.set(
            "effects",
            Value::Tags(effects.split('|').map(|s| s.to_string()).collect()),
            ts,
        );
        e.rev = rev;
        e
    }

    #[test]
    fn history_reports_field_level_changes() {
        let root = tmp("hist");
        let s = seed(&root, "win");
        s.mutate(&[Op::UpsertEntity {
            entity: edited("麻黄", "发汗解表|宣肺平喘", 2, "win", 2000),
        }])
        .unwrap();

        let h = s.entity_history("麻黄", 10).unwrap();
        assert_eq!(h.len(), 2, "创建 + 一次修改：{h:?}");
        assert_eq!(h[0].action, "create");
        assert_eq!(h[0].changes.len(), 2, "name 与 effects 都是新字段");
        assert_eq!(h[1].action, "update");
        assert_eq!(h[1].changes.len(), 1, "只有 effects 变了");
        assert_eq!(h[1].changes[0].key, "effects");
        assert_eq!(h[1].changes[0].before, "宣肺平喘");
        assert_eq!(h[1].changes[0].after, "发汗解表、宣肺平喘");
        assert_eq!(h[1].ts, 2000);
    }

    #[test]
    fn undo_restores_previous_values() {
        let root = tmp("undo");
        let s = seed(&root, "win");
        s.mutate(&[Op::UpsertEntity {
            entity: edited("麻黄", "发汗解表|宣肺平喘", 2, "win", 2000),
        }])
        .unwrap();

        let (rev, restored) = s.undo_entity("麻黄", 3000).unwrap();
        assert_eq!(restored, vec!["effects".to_string()]);
        assert!(rev > 2, "撤销要产生更大的 rev，而不是回退");
        assert_eq!(
            s.read(|m| m.entities["麻黄"].get("effects").unwrap().display()),
            "宣肺平喘"
        );
        // 撤销本身也是一条历史
        let h = s.entity_history("麻黄", 10).unwrap();
        assert_eq!(h.len(), 3);
        assert_eq!(h[2].action, "update");
        assert_eq!(h[2].changes[0].after, "宣肺平喘");
    }

    #[test]
    fn undo_rejects_change_from_another_device() {
        let root = tmp("undo-dev");
        let s = seed(&root, "win");
        s.mutate(&[Op::UpsertEntity {
            entity: edited("麻黄", "上游改的", 2, "upstream", 2000),
        }])
        .unwrap();
        let err = s.undo_entity("麻黄", 3000).unwrap_err().to_string();
        assert!(err.contains("不能在本机撤销"), "{err}");
    }

    #[test]
    fn undo_rejects_freshly_created_entity() {
        let root = tmp("undo-new");
        let s = seed(&root, "win");
        let err = s.undo_entity("桂枝", 3000).unwrap_err().to_string();
        assert!(err.contains("新建"), "{err}");
    }

    #[test]
    fn tombstone_shows_up_in_history() {
        let root = tmp("hist-del");
        let s = seed(&root, "win");
        let rev = s.stats().rev;
        s.mutate(&[Op::Tombstone {
            id: "麻黄".into(),
            rev,
            updated_at: 4000,
            device: "win".into(),
        }])
        .unwrap();
        let h = s.entity_history("麻黄", 10).unwrap();
        assert_eq!(h.last().unwrap().action, "delete");
        assert_eq!(h.last().unwrap().ts, 4000);
    }

    #[test]
    fn backup_written_before_remote_apply() {
        let root = tmp("backup");
        let s = seed(&root, "win");
        let remote = s.current_snapshot(999).unwrap();
        s.apply_remote_snapshot(&remote).unwrap();
        let backups: Vec<_> = fs::read_dir(root.join("backups"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(backups.len(), 1);
    }
}
