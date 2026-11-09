#![forbid(unsafe_code)]
//! # mneme-sync
//!
//! 可插拔网络同步层。五个真实后端 + 一个 trait：
//! `LocalDir`（也覆盖 Syncthing/坚果云挂载目录）、`WebDAV`、`GitHub Contents`、
//! `Git`（任意 remote）、`S3 兼容`（AWS/R2/MinIO，AWS SigV4）。
//!
//! 设计要点：
//! - 同步单元 = **不可变快照 + 每设备一份 append-only 日志分片**，永不搬数据库文件；
//! - 复习事件按幂等键合并，**只传事件不传 SRS 状态**，两端不会算法分叉；
//! - `If-Match` / `sha` 条件写 -> 412/409 触发「重拉 + 重新三方合并 + 重试」自愈；
//! - 覆盖前自动备份、immutable 字段冲突不自动合并、组件排除用显式列表。

pub mod config;
pub mod engine;
pub mod error;
pub mod incremental;
pub mod secret;
pub mod targets;
pub mod traits;

pub use config::{SyncConfig, TargetConfig};
pub use engine::{SyncEngine, SyncReport};
pub use error::{Error, Result};
pub use secret::{clear as clear_secrets, names as secret_names, put as put_secret};
pub use targets::build_target;
pub use traits::{RemoteMeta, SyncTarget};
