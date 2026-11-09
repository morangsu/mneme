use crate::error::{Error, Result};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMeta {
    pub etag: String,
    pub size: u64,
}

/// 所有后端的**唯一**契约：只搬「不可变文件」和「每设备一份的日志分片」。
/// 同步层从不搬数据库文件，因此 WebDAV / GitHub / Git / S3 / 网盘目录
/// 在实现上完全对等，加第六种后端只需实现这 5 个方法。
#[async_trait]
pub trait SyncTarget: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> &str;

    /// Git 之类需要先 pull 的后端在这里做；默认无操作。
    async fn prepare(&self) -> Result<()> {
        Ok(())
    }

    /// 全部写完之后提交/推送；默认无操作。
    async fn finalize(&self) -> Result<()> {
        Ok(())
    }

    async fn head(&self, path: &str) -> Result<Option<RemoteMeta>>;
    async fn get(&self, path: &str) -> Result<Vec<u8>>;
    /// `if_match` 为 Some 时带 If-Match / sha / ETag 做乐观并发控制，
    /// 不匹配必须返回 `Error::PreconditionFailed`。
    async fn put(&self, path: &str, data: &[u8], if_match: Option<&str>) -> Result<RemoteMeta>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
    async fn delete(&self, path: &str) -> Result<()>;
}

/// 防目录穿越。所有后端构造远端路径前都要过这一关。
pub fn safe_rel(p: &str) -> Result<String> {
    let clean = p.replace('\\', "/");
    if clean.starts_with('/') || clean.contains("..") || clean.contains(':') && clean.find(':') == Some(1) {
        return Err(Error::Sync(format!("非法路径: {p}")));
    }
    let parts: Vec<&str> = clean
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    Ok(parts.join("/"))
}
