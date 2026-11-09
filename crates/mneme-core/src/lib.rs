#![forbid(unsafe_code)]
//! # mneme-core
//!
//! 领域无关的记忆引擎内核。这里**不认识**中药学、内经、植物学或食疗学，
//! 只认识「schema 描述的实体 / 可反查字段 / 出题方向 / 复习事件」。
//!
//! 加一门新学科 = 加一个 `domains/*.toml`，不改本 crate 一行代码。
//!
//! 依赖方向（严格单向，不允许反向）：
//! `core` <- `store` <- `sync` <- `server` / `cli`
//!
//! 设计约束：
//! - 零 IO、零网络、零全局可变状态，全部纯函数或显式传参 -> 可离线单测。
//! - 同步单元是「不可变快照 + append-only 事件」，不是数据库文件。
//! - `unsafe` 一律禁止；性能靠数据布局、预分配、并行与索引，不靠绕过检查。

pub mod card;
pub mod entity;
pub mod error;
pub mod index;
pub mod merge;
pub mod schema;
pub mod srs;
pub mod text;

pub use card::{Card, Judge, generate_cards, generate_choices, judge};
pub use entity::{Entity, Group, ReviewEvent, SourceAnchor, TermAlias, Value};
pub use error::{Error, Result};
pub use index::{InvertedIndex, Scored};
pub use merge::{Diff, EntityChange, FieldChange, MergePolicy, diff_entities, merge_events, three_way};
pub use schema::{DirectionDef, DomainSchema, EntityDef, FieldDef, FieldKind, FilterOp, TermAliasDef, ViewDef, ViewFilter};
pub use srs::{
    DailyQueue, FsrsScheduler, Scheduler, Sm2Scheduler, SrsState, WeakCard, daily_queue,
    due_cards, fold_events, mastery_label, suggest_grade, weak_cards,
};
pub use text::{TermTable, dice, normalize, remove_redundant, similarity};

/// 内容寻址：任意字节 -> sha256 hex。快照、Bundle、事件幂等键共用。
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// 稳定短 id（不依赖 DefaultHasher，跨版本/跨平台可复现）。
pub fn stable_id(parts: &[&str]) -> String {
    content_hash(parts.join("\u{1f}").as_bytes())[..20].to_string()
}

/// Unix 毫秒。core 不依赖 chrono，时间统一由调用方传入，便于确定性测试。
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
