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
             CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);",
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

    /// 全量重建（首次打开、载入远端快照、镜像损坏时）。
    pub fn rebuild(
        &self,
        entities: &[Entity],
        cards: &[Card],
        events: &[ReviewEvent],
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute_batch("DELETE FROM entities; DELETE FROM cards; DELETE FROM events;")?;
        {
            let mut st = tx.prepare(
                "INSERT OR REPLACE INTO entities(id,kind,group_id,fingerprint,deleted,rev,json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for e in entities {
                st.execute(params![
                    e.id,
                    e.kind,
                    e.group_id,
                    e.fingerprint(),
                    e.deleted as i64,
                    e.rev as i64,
                    Self::encode_entity(e)
                ])?;
            }
        }
        {
            let mut st = tx.prepare(
                "INSERT OR REPLACE INTO cards(id,entity_id,direction,prompt,answer,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for c in cards {
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
        {
            let mut st = tx.prepare(
                "INSERT OR IGNORE INTO events(k,card_id,ts,grade,device,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for e in events {
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
        tx.commit()?;
        Ok(())
    }

    /// 增量派生后的镜像更新：只替换脏实体的记录与卡片。
    pub fn apply_changes(
        &self,
        dirty: &std::collections::HashSet<String>,
        entities: &[Entity],
        cards: &[Card],
        events: &[ReviewEvent],
    ) -> Result<()> {
        // 必须在加锁前判断：rebuild 会自己拿同一把非重入锁，
        // 持锁调用会直接死锁（SetSchema 的 "*" 必走这条分支）。
        if dirty.contains("*") {
            return self.rebuild(entities, cards, events);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM cards WHERE entity_id = ?1")?;
            let mut up = tx.prepare(
                "INSERT OR REPLACE INTO entities(id,kind,group_id,fingerprint,deleted,rev,json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for e in entities.iter().filter(|e| dirty.contains(&e.id)) {
                del.execute(params![e.id])?;
                up.execute(params![
                    e.id,
                    e.kind,
                    e.group_id,
                    e.fingerprint(),
                    e.deleted as i64,
                    e.rev as i64,
                    Self::encode_entity(e)
                ])?;
            }
        }
        {
            let mut st = tx.prepare(
                "INSERT OR REPLACE INTO cards(id,entity_id,direction,prompt,answer,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for c in cards.iter().filter(|c| dirty.contains(&c.entity_id)) {
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
        {
            let mut st = tx.prepare(
                "INSERT OR IGNORE INTO events(k,card_id,ts,grade,device,json) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for e in events {
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
        tx.commit()?;
        Ok(())
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
        m.rebuild(&list, &[], &[]).unwrap();
        m.rebuild(&list, &[], &[]).unwrap();
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
}
