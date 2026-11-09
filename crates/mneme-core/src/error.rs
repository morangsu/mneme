use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("schema 校验失败: {0}")]
    Schema(String),

    #[error("找不到实体/字段: {0}")]
    NotFound(String),

    #[error("字段 `{field}` 标记为 immutable，禁止合并或同义替换")]
    Immutable { field: String },

    #[error("数据格式错误: {0}")]
    Format(String),

    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("时间戳倒退或事件序号冲突: {0}")]
    Clock(String),
}

pub type Result<T> = std::result::Result<T, Error>;
