use mneme_core::{Entity, Group, ReviewEvent, TermAlias};
use serde::{Deserialize, Serialize};

/// 唯一写入原语。所有变更都先落成 Op 再应用到内存，
/// 因此「磁盘日志」与「内存状态」永远是同一份事实的两种视图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    SetSchema { toml: String },
    UpsertEntity { entity: Entity },
    UpsertGroup { group: Group },
    PutAlias { alias: TermAlias },
    Review { event: ReviewEvent },
    /// 显式墓碑。删除绝不靠「远端整份没有」推断。
    Tombstone {
        id: String,
        rev: u64,
        updated_at: i64,
        device: String,
    },
}

impl Op {
    /// 日志内的幂等键。重放时同键只应用一次。
    pub fn key(&self) -> String {
        match self {
            Self::SetSchema { toml } => format!("schema:{}", mneme_core::content_hash(toml.as_bytes())),
            Self::UpsertEntity { entity } => format!("e:{}:{}", entity.id, entity.rev),
            Self::UpsertGroup { group } => format!("g:{}:{}", group.id, group.rev),
            Self::PutAlias { alias } => format!("a:{}", alias.concept),
            Self::Review { event } => format!("r:{}", event.key()),
            Self::Tombstone { id, rev, device, .. } => format!("t:{id}:{rev}:{device}"),
        }
    }
}
