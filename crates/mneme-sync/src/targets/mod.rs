pub mod git;
pub mod github;
pub mod local;
pub mod s3;
pub mod webdav;

pub use git::GitTarget;
pub use github::GitHub;
pub use local::LocalDir;
pub use s3::S3Target;
pub use webdav::WebDav;

use crate::{
    config::{TargetConfig, resolve_secret},
    error::{Error, Result},
    traits::SyncTarget,
};
use std::{path::PathBuf, sync::Arc};

/// 配置 -> 后端。加第六种同步服务只需在这里加一个分支 + 一个实现文件。
pub fn build_target(c: &TargetConfig) -> Result<Arc<dyn SyncTarget>> {
    let t: Arc<dyn SyncTarget> = match c {
        TargetConfig::Local { id, dir } => Arc::new(LocalDir::new(id, PathBuf::from(dir))?),
        TargetConfig::WebDav {
            id,
            url,
            username,
            password,
            root,
        } => Arc::new(WebDav::new(
            id,
            url,
            username,
            resolve_secret(password)?,
            root,
        )),
        TargetConfig::GitHub {
            id,
            token,
            owner,
            repo,
            branch,
            path,
        } => Arc::new(GitHub::new(
            id,
            resolve_secret(token)?,
            owner,
            repo,
            branch,
            path,
        )),
        TargetConfig::Git {
            id,
            repo_dir,
            remote,
            branch,
        } => Arc::new(GitTarget::new(id, PathBuf::from(repo_dir), remote, branch)?),
        TargetConfig::S3 {
            id,
            endpoint,
            bucket,
            region,
            access_key,
            secret_key,
            prefix,
        } => Arc::new(S3Target::new(
            id,
            endpoint,
            bucket,
            region,
            resolve_secret(access_key)?,
            resolve_secret(secret_key)?,
            prefix,
        )),
    };
    Ok(t)
}

pub fn unknown_target(id: &str) -> Error {
    Error::Config(format!("未知的同步目标 `{id}`"))
}
