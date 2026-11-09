use crate::{
    error::{Error, Result},
    traits::{RemoteMeta, SyncTarget, safe_rel},
};
use async_trait::async_trait;
use mneme_core::content_hash;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// 任意 Git remote（GitHub / Gitee / 自建）。
/// 数据在**本地工作树**里读写，prepare 拉、finalize 提交并推送，
/// 冲突由 `git rebase` 处理（纯文本文件，天然可合并）。
pub struct GitTarget {
    id: String,
    dir: PathBuf,
    remote: String,
    branch: String,
}

impl GitTarget {
    pub fn new(
        id: impl Into<String>,
        dir: impl AsRef<Path>,
        remote: impl Into<String>,
        branch: impl Into<String>,
    ) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        if !dir.join(".git").exists() {
            let out = Command::new("git").arg("init").current_dir(&dir).output();
            if out.is_err() {
                return Err(Error::Config("找不到 git 可执行文件".into()));
            }
        }
        Ok(Self {
            id: id.into(),
            dir,
            remote: remote.into(),
            branch: branch.into(),
        })
    }

    fn full(&self, p: &str) -> Result<PathBuf> {
        Ok(self.dir.join(safe_rel(p)?))
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git").args(args).current_dir(&self.dir).output()?;
        if !out.status.success() {
            return Err(Error::Sync(format!(
                "git {} 失败: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn try_run(&self, args: &[&str]) {
        let _ = Command::new("git").args(args).current_dir(&self.dir).output();
    }
}

fn walk(base: &Path, cur: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in fs::read_dir(cur)? {
        let p = e?.path();
        if p.file_name().and_then(|s| s.to_str()) == Some(".git") {
            continue;
        }
        if p.is_dir() {
            walk(base, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

#[async_trait]
impl SyncTarget for GitTarget {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "git"
    }

    async fn prepare(&self) -> Result<()> {
        if !self.remote.is_empty() {
            // 离线时 pull 失败不阻断：本地工作树仍可读写，下次 finalize 再推
            let _ = self.run(&["pull", "--rebase", &self.remote, &self.branch]);
        }
        Ok(())
    }

    async fn finalize(&self) -> Result<()> {
        self.try_run(&["add", "-A"]);
        self.try_run(&[
            "-c",
            "user.name=mneme",
            "-c",
            "user.email=mneme@local",
            "commit",
            "-m",
            "mneme sync",
        ]);
        if !self.remote.is_empty() {
            self.run(&["push", &self.remote, &self.branch])?;
        }
        Ok(())
    }

    async fn head(&self, path: &str) -> Result<Option<RemoteMeta>> {
        let p = self.full(path)?;
        if !p.exists() {
            return Ok(None);
        }
        let b = fs::read(&p)?;
        Ok(Some(RemoteMeta {
            etag: content_hash(&b),
            size: b.len() as u64,
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
        fs::write(&p, data)?;
        Ok(RemoteMeta {
            etag: content_hash(data),
            size: data.len() as u64,
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        walk(&self.dir, &self.dir, &mut out)?;
        let clean = safe_rel(prefix)?;
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
