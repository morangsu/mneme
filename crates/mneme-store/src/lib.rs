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

pub mod cardcache;
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

/// 脏实体超过这个数就走全量重建：逐条增量在这些场景（批量导入、改分组、改 schema）
/// 反而不如一次并行重建快，而且逐条生成卡片时彼此看不到对方，候选判定会不一致。
const FULL_DERIVE_THRESHOLD: usize = 64;
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
    /// 读/写卡片缓存（命中时省掉整段卡片生成）
    pub cards_cache: std::time::Duration,
    /// 同步 SQLite 物化镜像（只有脏实体才写）
    pub sqlite_rebuild: std::time::Duration,
}

impl OpenTimings {
    pub fn total(&self) -> std::time::Duration {
        self.snapshot_load + self.replay + self.derive + self.cards_cache + self.sqlite_rebuild
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
    /// 卡片装载闸门。懒加载必须只装一次：FFI 与 server 可能并发进来，
    /// 没有这道锁就会把 34k 张卡装两遍（白花 80 ms，还浪费一份内存）。
    cards_gate: parking_lot::Mutex<()>,
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

        // 索引是卡片的前置：候选判定要查 posting
        let started = std::time::Instant::now();
        mem.rebuild_index();
        t.derive = started.elapsed();

        #[cfg(feature = "sqlite")]
        let sqlite = Some(Arc::new(sqlite_mirror::SqliteMirror::open(
            root.join("index.sqlite"),
        )?));
        let store = Arc::new(Self {
            root,
            cards_gate: parking_lot::Mutex::new(()),
            device: sanitize(device),
            log,
            snaps,
            mem: RwLock::new(mem),
            #[cfg(feature = "sqlite")]
            sqlite,
        });

        // 卡片**不在这里装**：矩阵页只需要一个数字，真正要卡片的是复习/导出。
        // 用上次落盘时的总数占位，等 `ensure_cards()` 再真装。
        let started = std::time::Instant::now();
        #[cfg(feature = "sqlite")]
        if let Some(s) = &store.sqlite {
            // 老库升级（meta 里还没有 cards_n）时退回一次 COUNT(*) 并写回水位，
            // 否则升级后的第一次启动会显示「卡片 0」（真机踩过）。
            let n = match s.meta_value("cards_n").and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => n,
                None => {
                    let n = s.card_count().unwrap_or(0);
                    let _ = s.set_meta("cards_n", &n.to_string());
                    n
                }
            };
            store.mem.write().set_cards_total(n);
        }
        t.cards_cache = started.elapsed();

        #[cfg(feature = "sqlite")]
        {
            let started = std::time::Instant::now();
            store.sync_sqlite()?;
            // WAL 超过 16 MB 才收敛（一次大导入会留下与主库等大的 WAL）
            if let Some(s) = &store.sqlite {
                let _ = s.checkpoint_if_large(16 * 1024 * 1024);
            }
            t.sqlite_rebuild = started.elapsed();
        }
        Ok((store, t))
    }

    /// 确保卡片已装入内存（幂等）。
    ///
    /// 所有「要卡片内容」的路径都必须先调它：复习、判分、错题本、导出。
    /// 矩阵/检索/编辑详情不需要——它们只用实体与索引，这也是懒加载能省下 80 ms 的原因。
    pub fn ensure_cards(&self) -> Result<()> {
        if self.mem.read().cards_loaded {
            return Ok(());
        }
        let _g = self.cards_gate.lock();
        if self.mem.read().cards_loaded {
            return Ok(());
        }
        self.load_or_build_cards()?;
        self.mem.write().mark_cards_loaded();
        // 镜像的 cards 表可能落后（懒加载的 open 不写卡片），这里补齐一次
        #[cfg(feature = "sqlite")]
        if let Some(s) = &self.sqlite {
            let (total, have) = (self.mem.read().cards_len(), s.card_count().unwrap_or(0));
            if total != have {
                let m = self.mem.read();
                s.sync_cards(&m.cards)?;
                drop(m);
            }
            // 水位无条件写：镜像卡片数可能本来就对（编辑路径已经写过），
            // 但 meta 里的总数还是空的——不写就会出现「卡片 0」（实测踩过）。
            s.set_meta("cards_n", &total.to_string())?;
        }
        Ok(())
    }

    /// 优先用 `cards.bin` 里的块，只重算指纹不匹配的实体。
    ///
    /// 命中率高的情况：换设备打开同一份数据、重启后没改过东西、只改了少数几条。
    /// 每次只要有任何一块被重算，就把整份缓存重写一遍（写 10 MB 约 20 ms，
    /// 远低于重算 2.4 s；不写的话下次还是全量算）。
    fn load_or_build_cards(&self) -> Result<()> {
        let t0 = std::time::Instant::now();
        // 直接拿走所有权（`remove` 移出块），避免把 34k 张卡整份 clone 一遍
        let mut cache = cardcache::load(&self.root);
        let _t_load = t0.elapsed();

        let (schema_fp, mut missing, mut adopted, any_cache) = {
            let m = self.mem.read();
            let schema_fp = m.schema_fp();
            let fps = m.block_fps();
            let mut missing: Vec<String> = Vec::new();
            let mut adopted: Vec<(String, Vec<mneme_core::Card>)> = Vec::new();
            let mut usable = cache.take().filter(|c| c.schema_fp == schema_fp);
            for id in m.entities.keys() {
                if m.entities[id].deleted {
                    continue;
                }
                let fp = fps.get(id).copied();
                let hit = match (&mut usable, fp) {
                    (Some(c), Some(fp)) => c.blocks.get(id).map(|b| b.fp == fp).unwrap_or(false),
                    _ => false,
                };
                if hit {
                    let block = usable
                        .as_mut()
                        .and_then(|c| c.blocks.remove(id))
                        .expect("checked");
                    adopted.push((id.clone(), block.cards));
                } else {
                    missing.push(id.clone());
                }
            }
            (schema_fp, missing, adopted, usable.is_some())
        };
        let _t_plan = t0.elapsed();

        if !adopted.is_empty() {
            self.mem.write().adopt_blocks(adopted);
        }
        if !missing.is_empty() {
            let dirty: std::collections::HashSet<String> = missing.iter().cloned().collect();
            if missing.len() > FULL_DERIVE_THRESHOLD {
                self.mem.write().rebuild_cards();
            } else {
                self.mem.write().apply_derived(&dirty);
            }
        }
        let _t_build = t0.elapsed();

        // 缓存缺失/不匹配/有块重算过 -> 重写一份
        if !any_cache || !missing.is_empty() {
            let _ = schema_fp;
            self.save_card_cache();
        }
        Ok(())
    }

    /// 把当前内存里的卡片块整份落盘（失败只影响下次启动速度，不返回错误）。
    fn save_card_cache(&self) {
        let (schema_fp, fps, blocks) = {
            let m = self.mem.read();
            let mut blocks = std::collections::HashMap::with_capacity(m.cards.len());
            for (id, cards) in &m.cards {
                blocks.insert(
                    id.clone(),
                    cardcache::Block {
                        fp: 0, // 下面统一用批量指纹填
                        cards: cards.clone(),
                    },
                );
            }
            (m.schema_fp(), m.block_fps(), blocks)
        };
        let mut out = std::collections::HashMap::with_capacity(blocks.len());
        for (id, mut block) in blocks {
            block.fp = fps.get(&id).copied().unwrap_or(0);
            out.insert(id, block);
        }
        let cache = cardcache::CardCache {
            schema_fp,
            blocks: out,
        };
        let _ = cardcache::save(&self.root, &cache);
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
        // 写路径要替换卡片块、也要让统计准确，所以先把卡装进来。
        // 两个例外：只追加复习事件（不改卡片）、以及**空库**首次导入
        // ——那时装卡等于对空表做一次全量重建再把它作废，纯浪费。
        let had_entities = !self.mem.read().entities.is_empty();
        let touches_entities = ops.iter().any(|op| !matches!(op, Op::Review { .. }));
        if had_entities && touches_entities {
            self.ensure_cards()?;
        }
        let (n, dirty) = {
            let mut m = self.mem.write();
            let mut n = 0;
            for op in ops {
                if m.apply(op) {
                    n += 1;
                }
            }
            let dirty = std::mem::take(&mut m.dirty);
            if !dirty.is_empty() {
                if dirty.contains("*") || dirty.len() > FULL_DERIVE_THRESHOLD {
                    m.rebuild_derived();
                } else {
                    m.apply_derived(&dirty);
                }
            }
            (n, dirty)
        };
        // 只有复习事件时索引/卡片都不变，连重建都跳过
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
        #[cfg(feature = "sqlite")]
        {
            let m = self.mem.read();
            if let Some(s) = &self.sqlite {
                s.apply_changes(&dirty, &m.entities_vec(), &m.cards, &m.events)?;
            }
        }
        // 全量重建（批量导入 / 改 schema）之后必须把卡片缓存写回去，
        // 否则下次启动进复习还得再重建 2.2 秒。单条编辑不写（10 MB 不值得）。
        let full = dirty.contains("*") || dirty.len() > FULL_DERIVE_THRESHOLD;
        if full {
            self.mem.write().mark_cards_loaded();
            self.save_card_cache();
        } else if !had_entities && !dirty.is_empty() {
            // 空库导入：卡片刚全量建好，同样要落盘
            self.mem.write().mark_cards_loaded();
            self.save_card_cache();
        }
        Ok(n)
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite(&self) -> Option<&Arc<sqlite_mirror::SqliteMirror>> {
        self.sqlite.as_ref()
    }

    /// 把 SQLite 镜像对齐到内存态：只写差异（新增/修改/删除 + 新事件）。
    ///
    /// 以前是每次 open 都整表 DELETE+INSERT，2000 味药要 577 ms，纯浪费；
    /// 镜像本来就是「可重建的物化视图」，只有真的落后才需要补。
    #[cfg(feature = "sqlite")]
    pub fn sync_sqlite(&self) -> Result<()> {
        if let Some(s) = &self.sqlite {
            let m = self.mem.read();
            // 卡片没装载时绝不能写 cards 表：那等于把镜像里的卡片全删了。
            // 同理也不能写 cards_n —— 那一刻内存里是 0，写进去就把水位抹平了，
            // 下次启动矩阵页会显示「卡片 0」（实测踩过）。
            s.sync(&m.entities_vec(), &m.cards, &m.events, m.cards_loaded)?;
            if m.cards_loaded {
                s.set_meta("cards_n", &m.cards_len().to_string())?;
            }
        }
        Ok(())
    }

    /// 保留旧入口：强制整表重刷（镜像怀疑损坏 / 迁移后用）。
    #[cfg(feature = "sqlite")]
    pub fn rebuild_sqlite(&self) -> Result<()> {
        if let Some(s) = &self.sqlite {
            s.invalidate()?;
            self.sync_sqlite()?;
        }
        Ok(())
    }

    /// 索引与卡片是派生数据，锁外算完整体换入。
    pub fn refresh(&self) -> Result<()> {
        self.mem.write().rebuild_derived();
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
