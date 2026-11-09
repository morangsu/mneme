use crate::{error::Result, memory::Memory};
use mneme_core::{
    Entity, Group, ReviewEvent, TermAlias, content_hash,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}};

pub const FORMAT: &str = "mneme-snapshot/1";

/// **不可变**快照。同步搬的是这个文件，绝不是数据库文件
/// （按文件同步一个活的 SQLite 会撕裂索引，这是思源等工具明确禁止的做法）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub format: String,
    pub version: u32,
    pub rev: u64,
    pub created_at: i64,
    pub device: String,
    pub schema_toml: String,
    pub entities: Vec<Entity>,
    pub groups: Vec<Group>,
    pub aliases: Vec<TermAlias>,
    pub events: Vec<ReviewEvent>,
    /// 对除本字段外的规范化 JSON 求 sha256
    pub checksum: String,
}

impl Snapshot {
    pub fn from_memory(mem: &Memory, device: &str, now: i64) -> Self {
        let mut s = Self {
            format: FORMAT.into(),
            version: 1,
            rev: mem.rev,
            created_at: now,
            device: device.to_string(),
            schema_toml: mem.schema_toml.clone(),
            entities: mem.entities_vec(),
            groups: mem.groups.values().cloned().collect(),
            aliases: mem.aliases.clone(),
            events: mem.events.clone(),
            checksum: String::new(),
        };
        s.entities.sort_by(|a, b| a.id.cmp(&b.id));
        s.groups.sort_by(|a, b| a.id.cmp(&b.id));
        s.aliases.sort_by(|a, b| a.concept.cmp(&b.concept));
        s.events.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.card_id.cmp(&b.card_id)));
        s.checksum = s.compute_hash();
        s
    }

    fn compute_hash(&self) -> String {
        let mut c = self.clone();
        c.checksum.clear();
        content_hash(&serde_json::to_vec(&c).unwrap_or_default())
    }

    /// 引擎在做完三方合并、重新构造快照后重算校验和。
    pub fn recheck(&mut self) {
        self.checksum = self.compute_hash();
    }

    pub fn verify(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(crate::error::Error::Snapshot(format!(
                "未知格式 {}",
                self.format
            )));
        }
        let actual = self.compute_hash();
        if actual != self.checksum {
            return Err(crate::error::Error::Checksum {
                expected: self.checksum.clone(),
                actual,
            });
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let s: Self = serde_json::from_slice(b)?;
        s.verify()?;
        Ok(s)
    }
}

pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn open(root: &Path) -> Result<Self> {
        let dir = root.join("snapshots");
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn write(&self, s: &Snapshot) -> Result<(String, String)> {
        s.verify()?;
        let bytes = s.to_bytes()?;
        let hash = content_hash(&bytes);
        let name = format!("snap-{:08}-{}.json", s.rev, &hash[..8]);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        fs::write(&tmp, &bytes)?;
        fs::rename(tmp, self.dir.join(&name))?;
        Ok((name, hash))
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for e in fs::read_dir(&self.dir)? {
            let n = e?.file_name().to_string_lossy().to_string();
            if n.starts_with("snap-") && n.ends_with(".json") {
                out.push(n);
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn latest(&self) -> Result<Option<(String, Snapshot)>> {
        let name = match self.list()?.last() {
            Some(n) => n.clone(),
            None => return Ok(None),
        };
        let bytes = fs::read(self.dir.join(&name))?;
        Ok(Some((name, Snapshot::from_bytes(&bytes)?)))
    }

    pub fn read(&self, name: &str) -> Result<Snapshot> {
        let bytes = fs::read(self.dir.join(name))?;
        Snapshot::from_bytes(&bytes)
    }

    pub fn write_bytes_as(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let tmp = self.dir.join(format!(".{name}.tmp"));
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, self.dir.join(name))?;
        Ok(())
    }
}
