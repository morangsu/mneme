use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("store: {0}")]
    Store(#[from] mneme_store::Error),
    #[error("codec: {0}")]
    Codec(#[from] mneme_codec::Error),
    #[error("core: {0}")]
    Core(#[from] mneme_core::Error),
    /// 远端 ETag/If-Match 不匹配。引擎靠这个错误触发「重新拉取 -> 重新合并 -> 重试」。
    #[error("412 前置条件失败: {0}")]
    PreconditionFailed(String),
    #[error("远端返回 {status}: {body}")]
    Remote { status: u16, body: String },
    #[error("找不到对象: {0}")]
    NotFound(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("同步错误: {0}")]
    Sync(String),
}

pub type Result<T> = std::result::Result<T, Error>;
