use crate::error::Result;
use mneme_core::{Card, Entity, ReviewEvent};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

/// 本地 SQLite **物化镜像**。
///
/// 它是 APP 侧真正可查、可离线、带事务的数据库；但**不是同步单元**：
/// - 真相源仍是 append-only 事件日志 + 不可变快照；
/// - 镜像随时可以从日志/快照 `rebuild`，损坏了直接重建即可；
/// - 同步绝不搬 `.sqlite` 文件（WAL 与索引会被撕裂，且无法字段级合并）。
#[derive(Debug)]
pub struct SqliteMirror {
    conn: Mutex<Connection>,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorStats {
    pub entities: usize,
    pub cards: usize,
    pub events: usize,
}

impl SqliteMirror {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS entities (
                 id TEXT PRIMARY KEY, kind TEXT NOT NULL, group_id TEXT,
                 fingerprint TEXT, deleted INTEGER NOT NULL DEFAULT 0,
                 rev INTEGER NOT NULL, json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_entities_kind ON entities(kind);
             CREATE INDEX IF NOT EXISTS idx_entities_group ON entities(group_id);
             CREATE TABLE IF NOT EXISTS cards (
                 id TEXT PRIMARY KEY, entity_id TEXT NOT NULL, direction TEXT,
                 prompt TEXT, answer TEXT, json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_cards_entity ON cards(entity_id);
             CREATE TABLE IF NOT EXISTS events (
                 k TEXT PRIMARY KEY, card_id TEXT NOT NULL, ts INTEGER NOT NULL,
                 grade INTEGER NOT NULL, device TEXT NOT NULL, json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
             CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn encode_entity(e: &Entity) -> String {
        serde_json::to_string(e).unwrap_or_default()
    }

    /// 只写脏实体的记录与卡片（rebuild / apply_changes / sync 的共同底座）。
    fn write_dirty(
        tx: &rusqlite::Transaction<'_>,
        dirty: &std::collections::HashSet<String>,
        entities: &[Entity],
        cards: &std::collections::BTreeMap<String, Vec<Card>>,
        events: &[ReviewEvent],
        events_from: usize,
        write_cards: bool,
    ) -> Result<()> {
        {
            let mut del_cards = tx.prepare("DELETE FROM cards WHERE entity_id = ?1")?;
            let mut up = tx.prepare(
                "INSERT OR REPLACE INTO entities(id,kind,group_id,fingerprint,deleted,rev,json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for e in entities.iter().filter(|e| dirty.contains(&e.id)) {
                // 卡片没装载时不能删也不能写：删了就等于把镜像里的卡片清空
                if write_cards {
                    del_cards.execute(params![e.id])?;
                }
                up.execute(params![
                    e.id,
                    e.kind,
                    e.group_id,
                    format!("{:016x}", e.fast_fp()),
                    e.deleted as i64,
                    e.rev as i64,
                    Self::encode_entity(e)
                ])?;
            }
        }
        if write_cards {
            let mut st = tx.prepare(
                "INSERT OR REPLACE INTO cards(id,entity_id,direction,prompt,answer,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for id in dirty {
                let Some(block) = cards.get(id) else { continue };
                for c in block {
                    st.execute(params![
                        c.id,
                        c.entity_id,
                        c.direction,
                        c.prompt,
                        c.answer,
                        serde_json::to_string(c).unwrap_or_default()
                    ])?;
                }
            }
        }
        if events_from < events.len() {
            let mut st = tx.prepare(
                "INSERT OR IGNORE INTO events(k,card_id,ts,grade,device,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for e in &events[events_from..] {
                st.execute(params![
                    e.key(),
                    e.card_id,
                    e.ts,
                    e.grade as i64,
                    e.device,
                    serde_json::to_string(e).unwrap_or_default()
                ])?;
            }
        }
        Ok(())
    }

    /// 读 meta（给「卡片总数」这类启动期就要用的轻量水信用）。
    pub fn meta_value(&self, k: &str) -> Option<String> {
        let conn = self.conn.lock();
        Self::meta_get(&conn, k)
    }

    fn meta_get(conn: &rusqlite::Connection, k: &str) -> Option<String> {
        conn.query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| r.get(0))
            .ok()
    }

    fn meta_set(
        tx: &rusqlite::Transaction<'_>,
        k: &str,
        v: &str,
    ) -> Result<()> {
        tx.execute(
            "INSERT OR REPLACE INTO meta(k,v) VALUES(?1,?2)",
            params![k, v],
        )?;
        Ok(())
    }

    /// 全量重刷（镜像损坏、schema 变更、首次建库时）。
    pub fn rebuild(
        &self,
        entities: &[Entity],
        cards: &std::collections::BTreeMap<String, Vec<Card>>,
        events: &[ReviewEvent],
    ) -> Result<()> {
        self.invalidate()?;
        self.sync_inner(entities, cards, events, true, true)
    }

    /// 清空镜像（下一次 `sync` 会整份写回）。比「逐行 DELETE」快，且不受残留影响。
    pub fn invalidate(&self) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute_batch("DELETE FROM entities; DELETE FROM cards; DELETE FROM events; DELETE FROM meta;")?;
        tx.commit()?;
        Ok(())
    }

    /// 把镜像对齐到内存态：**只写差异**。
    ///
    /// 之前每次 open 都整表 DELETE+INSERT（2000 味药 577 ms）。镜像本来就是
    /// 可重建的物化视图，没落后就不该写；落后也只补那几条。
    /// 判定依据是 `(rev, fingerprint, deleted)`——rev 单调递增，fingerprint 兜住
    /// 「不同设备各自加过 rev 但内容不同」的情况。
    pub fn sync(
        &self,
        entities: &[Entity],
        cards: &std::collections::BTreeMap<String, Vec<Card>>,
        events: &[ReviewEvent],
        write_cards: bool,
    ) -> Result<()> {
        self.sync_inner(entities, cards, events, false, write_cards)
    }

    fn sync_inner(
        &self,
        entities: &[Entity],
        cards: &std::collections::BTreeMap<String, Vec<Card>>,
        events: &[ReviewEvent],
        force: bool,
        write_cards: bool,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        // 镜像现有水位
        let mut existing: std::collections::HashMap<String, (i64, u64, i64)> =
            std::collections::HashMap::new();
        if !force {
            let mut st = tx.prepare("SELECT id, rev, fingerprint, deleted FROM entities")?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    u64::from_str_radix(&r.get::<_, Option<String>>(2)?.unwrap_or_default(), 16)
                        .unwrap_or(0),
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (id, rev, fp, del) = row?;
                existing.insert(id, (rev, fp, del));
            }
        }
        let events_from = if force {
            0
        } else {
            Self::meta_get(&tx, "events_n")
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| *n <= events.len())
                .unwrap_or(0)
        };

        let mut dirty: std::collections::HashSet<String> = std::collections::HashSet::new();
        for e in entities {
            match existing.get(&e.id) {
                Some((rev, fp, del))
                    if *rev == e.rev as i64
                        && *del == e.deleted as i64
                        && *fp == e.fast_fp() => {}
                _ => {
                    dirty.insert(e.id.clone());
                }
            }
        }
        // 内存里已经没有的实体（硬删除）要从镜像摘掉
        let mut removed: Vec<String> = Vec::new();
        if !force {
            let live: std::collections::HashSet<&str> =
                entities.iter().map(|e| e.id.as_str()).collect();
            for id in existing.keys() {
                if !live.contains(id.as_str()) {
                    removed.push(id.clone());
                }
            }
        }
        for id in &removed {
            tx.execute("DELETE FROM cards WHERE entity_id = ?1", params![id])?;
            tx.execute("DELETE FROM entities WHERE id = ?1", params![id])?;
        }

        if !dirty.is_empty() || !removed.is_empty() || events_from < events.len() {
            Self::write_dirty(&tx, &dirty, entities, cards, events, events_from, write_cards)?;
        }
        Self::meta_set(&tx, "events_n", &events.len().to_string())?;
        let _ = Self::meta_set(&tx, "entities_n", &entities.len().to_string());

        tx.commit()?;
        Ok(())
    }

    /// 增量派生后的镜像更新：只替换脏实体的记录与卡片。
    pub fn apply_changes(
        &self,
        dirty: &std::collections::HashSet<String>,
        entities: &[Entity],
        cards: &std::collections::BTreeMap<String, Vec<Card>>,
        events: &[ReviewEvent],
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let events_from = Self::meta_get(&conn, "events_n")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n <= events.len())
            .unwrap_or(0);
        let tx = conn.transaction()?;
        Self::write_dirty(&tx, dirty, entities, cards, events, events_from, true)?;
        Self::meta_set(&tx, "events_n", &events.len().to_string())?;
        tx.commit()?;
        Ok(())
    }

    /// WAL 收敛：把日志合并回主库并把文件截断。
    ///
    /// 一次大批量导入会让 WAL 涨到和主库一样大（实测 2000 味药：主库 34 MB / WAL 34 MB）。
    /// 它不会无限涨（SQLite 会自动 checkpoint），但「目录里凭空多一倍的体积」很容易被
    /// 当成数据膨胀。所以只在 WAL 已经很大时才主动收一次，平时不付这个代价。
    pub fn checkpoint_if_large(&self, threshold_bytes: u64) -> Result<bool> {
        let wal = self.path.with_file_name(format!(
            "{}-wal",
            self.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        ));
        let size = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        if size < threshold_bytes {
            return Ok(false);
        }
        let conn = self.conn.lock();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(true)
    }

    pub fn insert_event(&self, e: &ReviewEvent) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO events(k,card_id,ts,grade,device,json) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                e.key(),
                e.card_id,
                e.ts,
                e.grade as i64,
                e.device,
                serde_json::to_string(e).unwrap_or_default()
            ],
        )?;
        Ok(())
    }

    /// 镜像里现有多少张卡（判断「卡片表是否落后」用）。
    pub fn card_count(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// 整表重写卡片（懒加载补齐镜像时用一次；卡片数据量固定，不需要增量）。
    pub fn sync_cards(
        &self,
        cards: &std::collections::BTreeMap<String, Vec<Card>>,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM cards", [])?;
        {
            let mut st = tx.prepare(
                "INSERT OR REPLACE INTO cards(id,entity_id,direction,prompt,answer,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for block in cards.values() {
                for c in block {
                    st.execute(params![
                        c.id,
                        c.entity_id,
                        c.direction,
                        c.prompt,
                        c.answer,
                        serde_json::to_string(c).unwrap_or_default()
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_meta(&self, k: &str, v: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        Self::meta_set(&tx, k, v)?;
        tx.commit()?;
        Ok(())
    }

    pub fn counts(&self) -> Result<MirrorStats> {
        let conn = self.conn.lock();
        let q = |sql: &str| -> Result<usize> {
            Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
        };
        Ok(MirrorStats {
            entities: q("SELECT COUNT(*) FROM entities WHERE deleted = 0")?,
            cards: q("SELECT COUNT(*) FROM cards")?,
            events: q("SELECT COUNT(*) FROM events")?,
        })
    }

    /// 术语/全文检索（JSON 上的 LIKE；需要分词检索时可换 FTS5 虚拟表）。
    pub fn search(&self, term: &str, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare(
            "SELECT id FROM entities WHERE deleted = 0 AND json LIKE ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = st.query_map(params![format!("%{term}%"), limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.filter_map(|x| x.ok()).collect())
    }

    pub fn entity_json(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT json FROM entities WHERE id = ?1")?;
        let mut rows = st.query(params![id])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get(0)?)),
            None => Ok(None),
        }
    }

    pub fn integrity_check(&self) -> Result<bool> {
        let conn = self.conn.lock();
        let v: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        Ok(v == "ok")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mneme_core::{Entity, Value, now_ms};

    fn tmp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "mneme-sqlite-{}-{}-{}-{}.db",
            tag,
            std::process::id(),
            now_ms(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn herb(id: &str) -> Entity {
        let mut e = Entity::new(id, "herb", "w", 1);
        e.set("name", Value::Text(id.into()), 1);
        e.set("effects", Value::Tags(vec!["宣肺平喘".into()]), 1);
        e
    }

    #[test]
    fn rebuild_is_idempotent_and_searchable() {
        let p = tmp("rebuild");
        let m = SqliteMirror::open(&p).unwrap();
        let mut b = herb("桂枝");
        b.set("effects", Value::Tags(vec!["温通经脉".into()]), 2);
        let list = vec![herb("麻黄"), b];
        m.rebuild(&list, &std::collections::BTreeMap::new(), &[]).unwrap();
        m.rebuild(&list, &std::collections::BTreeMap::new(), &[]).unwrap();
        assert_eq!(m.counts().unwrap().entities, 2);
        assert!(m.integrity_check().unwrap());
        assert_eq!(m.search("宣肺平喘", 10).unwrap(), vec!["麻黄".to_string()]);
        assert!(m.entity_json("麻黄").unwrap().unwrap().contains("宣肺平喘"));
        drop(m);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(format!("{}-wal", p.display()));
        let _ = std::fs::remove_file(format!("{}-shm", p.display()));
    }

    #[test]
    fn events_are_idempotent_by_key() {
        let p = tmp("events");
        let m = SqliteMirror::open(&p).unwrap();
        let e = ReviewEvent {
            card_id: "c".into(), ts: 1, grade: 3, device: "w".into(), seq: 0,
        };
        m.insert_event(&e).unwrap();
        m.insert_event(&e).unwrap();
        assert_eq!(m.counts().unwrap().events, 1);
    }

    #[test]
    fn checkpoint_truncates_wal_when_it_grows() {
        let dir = std::env::temp_dir().join(format!("mneme-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let m = SqliteMirror::open(dir.join("index.sqlite")).unwrap();

        // 造一批实体，让 WAL 长起来（真实场景是一次批量导入）
        let entities: Vec<mneme_core::Entity> = (0..2000)
            .map(|i| {
                let mut e = mneme_core::Entity::new(format!("e{i}"), "herb", "t", 1);
                e.set(
                    "name",
                    mneme_core::Value::Text(format!("药{i}")),
                    1,
                );
                e
            })
            .collect();
        m.rebuild(&entities, &std::collections::BTreeMap::new(), &[])
            .unwrap();
        let wal_path = dir.join("index.sqlite-wal");
        let before = std::fs::metadata(&wal_path).map(|x| x.len()).unwrap_or(0);
        assert!(before > 1024, "WAL 应该已经长大：{before} B");

        // 阈值取 1 KB，必然触发
        assert!(m.checkpoint_if_large(1024).unwrap(), "应该执行了 checkpoint");
        let after = std::fs::metadata(&wal_path).map(|x| x.len()).unwrap_or(0);
        assert!(after < before, "WAL 应收敛：{before} B -> {after} B");
        drop(m);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
