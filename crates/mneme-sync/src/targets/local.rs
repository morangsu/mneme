use crate::{
    error::{Error, Result},
    traits::{RemoteMeta, SyncTarget, safe_rel},
};
use async_trait::async_trait;
use mneme_core::content_hash;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 本地目录后端。既是最简单的实现，也是双设备收敛/412 自愈测试的「假远端」。
/// 指向 Syncthing、坚果云、OneDrive 的同步文件夹，就等于接上了这些服务。
pub struct LocalDir {
    id: String,
    root: PathBuf,
}

impl LocalDir {
    pub fn new(id: impl Into<String>, dir: impl AsRef<Path>) -> Result<Self> {
        let root = dir.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self { id: id.into(), root })
    }

    fn full(&self, p: &str) -> Result<PathBuf> {
        Ok(self.root.join(safe_rel(p)?))
    }
}

fn walk(base: &Path, cur: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in fs::read_dir(cur)? {
        let p = e?.path();
        if p.is_dir() {
            walk(base, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

#[async_trait]
impl SyncTarget for LocalDir {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "local"
    }

    async fn head(&self, path: &str) -> Result<Option<RemoteMeta>> {
        let p = self.full(path)?;
        if !p.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&p)?;
        Ok(Some(RemoteMeta {
            etag: content_hash(&bytes),
            size: bytes.len() as u64,
        }))
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>> {
        let p = self.full(path)?;
        if !p.exists() {
            return Err(Error::NotFound(path.into()));
        }
        Ok(fs::read(p)?)
    }

    async fn put(&self, path: &str, data: &[u8], if_match: Option<&str>) -> Result<RemoteMeta> {
        let p = self.full(path)?;
        if let Some(expected) = if_match {
            let cur = if p.exists() {
                content_hash(&fs::read(&p)?)
            } else {
                String::new()
            };
            if cur != expected {
                return Err(Error::PreconditionFailed(format!(
                    "{path}: 期望 {expected}, 实际 {cur}"
                )));
            }
        }
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension(format!(
            "{}.tmp",
            p.extension().and_then(|s| s.to_str()).unwrap_or("bin")
        ));
        fs::write(&tmp, data)?;
        fs::rename(tmp, &p)?;
        Ok(RemoteMeta {
            etag: content_hash(data),
            size: data.len() as u64,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        let clean = safe_rel(prefix)?;
        walk(&self.root, &self.root, &mut out)?;
        out.retain(|s| s.starts_with(&clean));
        out.sort();
        Ok(out)
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let p = self.full(path)?;
        if p.exists() {
            fs::remove_file(p)?;
        }
        Ok(())
    }
}
