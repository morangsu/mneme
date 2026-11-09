use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("core: {0}")]
    Core(#[from] mneme_core::Error),
    #[error("校验和不匹配: 期望 {expected}, 实际 {actual}")]
    Checksum { expected: String, actual: String },
    #[error("快照损坏: {0}")]
    Snapshot(String),
    #[error("设备名非法: {0}")]
    Device(String),
    /// 业务规则拒绝（撤销跨设备、重复主键、对新建条目撤销…）。
    /// 单独一类是为了让上层能原样把中文原因显示给用户，不再包一层"core:"前缀。
    #[error("{0}")]
    Invalid(String),

    #[cfg(feature = "sqlite")]
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
