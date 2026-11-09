use mneme_core::{Entity, Group, content_hash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const PUBLIC_FORMAT: &str = "mneme-public/1";

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("core: {0}")]
    Core(#[from] mneme_core::Error),
    #[error("上游包校验失败: {0}")]
    Bundle(String),
    #[error("还有 {0:?} 项冲突未裁决，未写入任何数据")]
    Unresolved(Vec<String>),
    #[error("决策引用了不存在的冲突: {0}")]
    UnknownConflict(String),
    #[error("GitHub 限流，重置于 {0}")]
    RateLimited(String),
    #[error("配置错误: {0}")]
    Config(String),
}
pub type Result<T> = std::result::Result<T, Error>;

fn default_branch() -> String {
    "main".into()
}

/// 三个仓库各自的角色。`token_env` 为空 = 公共库（匿名只读）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default)]
    pub path: String,
    /// 私有库写 `${ENV:MNEME_GITHUB_TOKEN}`；公共库留空
    #[serde(default)]
    pub token_env: Option<String>,
}

impl RepoRef {
    pub fn is_public(&self) -> bool {
        self.token_env.is_none()
    }
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
    pub fn contents_url(&self, file: &str) -> String {
        let p = if self.path.is_empty() {
            file.to_string()
        } else {
            format!("{}/{}", self.path.trim_matches('/'), file)
        };
        format!(
            "https://api.github.com/repos/{}/{}/contents/{}",
            self.owner, self.repo, p
        )
    }
}

fn default_base_fields() -> BTreeSet<String> {
    [
        "name", "group_name", "effects", "indications", "features",
        "contraindications", "nature", "toxicity", "dosage",
        "compatibility", "decoction", "differential", "canonical",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_personal_fields() -> BTreeSet<String> {
    ["mastery", "synonyms", "boundary", "notes", "processing"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 三仓库拓扑。公共库只放基础内容；个人字段绝不外发。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topology {
    /// 公共：软件服务本体（源码 / 发布）
    pub app_repo: RepoRef,
    /// 公共：对外开放的基础数据库（只读上游）
    pub public_data: RepoRef,
    /// 私有：个性化内容 + 设备同步落点
    pub private_data: RepoRef,
    #[serde(default = "default_base_fields")]
    pub base_fields: BTreeSet<String>,
    /// 这些字段一律视为用户个性化区域，冲突时优先保护
    #[serde(default = "default_personal_fields")]
    pub personal_fields: BTreeSet<String>,
    #[serde(default)]
    pub schema_id: String,
}

impl Topology {
    pub fn example(owner: &str) -> Self {
        Self {
            app_repo: RepoRef {
                owner: owner.into(), repo: "mneme".into(), branch: "main".into(),
                path: String::new(), token_env: None,
            },
            public_data: RepoRef {
                owner: owner.into(), repo: "mneme-data-public".into(), branch: "main".into(),
                path: "data".into(), token_env: None,
            },
            private_data: RepoRef {
                owner: owner.into(), repo: "mneme-data-private".into(), branch: "main".into(),
                path: "data".into(), token_env: Some("MNEME_GITHUB_TOKEN".into()),
            },
            base_fields: default_base_fields(),
            personal_fields: default_personal_fields(),
            schema_id: "materia-medica".into(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !self.app_repo.is_public() || !self.public_data.is_public() {
            return Err(Error::Config("源码库与基础数据库必须是公共库".into()));
        }
        if self.private_data.is_public() {
            return Err(Error::Config("私有同步库必须配置 token_env".into()));
        }
        if self.private_data.slug() == self.public_data.slug() {
            return Err(Error::Config("公共库与私有库不能是同一个仓库".into()));
        }
        Ok(())
    }

    pub fn is_base_field(&self, key: &str) -> bool {
        self.base_fields.contains(key)
    }
    pub fn is_personal_field(&self, key: &str) -> bool {
        self.personal_fields.contains(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Upstream,
    User,
    Local,
}

/// 字段级溯源。判定「用户有没有改过」不靠猜，靠这份记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldProvenance {
    /// 上次从公共库看到的该字段值（三方合并的 base）
    pub upstream_value: String,
    pub origin: Origin,
    pub edited_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityProvenance {
    pub fields: BTreeMap<String, FieldProvenance>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvenanceStore {
    pub version: u32,
    pub upstream_rev: u64,
    pub upstream_checksum: String,
    pub entities: BTreeMap<String, EntityProvenance>,
}

/// 公共库发布的「基础内容包」。不含任何个人字段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublicBundle {
    pub format: String,
    pub rev: u64,
    pub schema_id: String,
    pub published_at: i64,
    pub entities: Vec<Entity>,
    pub groups: Vec<Group>,
    pub checksum: String,
}

impl PublicBundle {
    pub fn new(rev: u64, schema_id: &str, entities: Vec<Entity>, groups: Vec<Group>, now: i64) -> Self {
        let mut b = Self {
            format: PUBLIC_FORMAT.into(),
            rev,
            schema_id: schema_id.into(),
            published_at: now,
            entities,
            groups,
            checksum: String::new(),
        };
        b.entities.sort_by(|a, b| a.id.cmp(&b.id));
        b.recheck();
        b
    }
    pub fn recheck(&mut self) {
        self.checksum = self.compute();
    }
    fn compute(&self) -> String {
        let mut c = self.clone();
        c.checksum.clear();
        content_hash(&serde_json::to_vec(&c).unwrap_or_default())
    }
    pub fn verify(&self) -> Result<()> {
        if self.format != PUBLIC_FORMAT {
            return Err(Error::Bundle(format!("未知格式 {}", self.format)));
        }
        if self.compute() != self.checksum {
            return Err(Error::Bundle("sha256 不匹配".into()));
        }
        Ok(())
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let x: Self = serde_json::from_slice(b)?;
        x.verify()?;
        Ok(x)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    KeepMine,
    TakeUpstream,
    KeepBoth,
}

impl std::str::FromStr for Decision {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            // 同时接受 CLI 的 snake_case 与 serde 变体名（APP 侧用 "KeepMine" 等）
            "keep_mine" | "keepmine" | "mine" | "k" => Ok(Self::KeepMine),
            "take_upstream" | "takeupstream" | "upstream" | "u" => Ok(Self::TakeUpstream),
            "keep_both" | "keepboth" | "both" | "b" => Ok(Self::KeepBoth),
            _ => Err(format!("未知决策 `{s}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    BothChanged,
    ImmutableClash,
    UpstreamRemovedLocallyEdited,
}

/// 一条必须由用户决定的冲突。三值并列展示，不自动裁决。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conflict {
    pub id: String,
    pub entity_id: String,
    pub entity_name: String,
    pub field: String,
    pub label: String,
    pub base: String,
    pub upstream: String,
    pub local: String,
    pub immutable: bool,
    pub personalized: bool,
    pub suggestion: Decision,
    pub kind: ConflictKind,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoMerge {
    pub entity_id: String,
    pub field: String,
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// staging 计划：拉取只生成它，不写库。自包含（内含上游实体），
/// 因此裁决时无需再次联网，也不受公共库后续变化影响。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullPlan {
    pub id: String,
    pub from_rev: u64,
    pub to_rev: u64,
    pub public_checksum: String,
    pub created_at: i64,
    pub auto: Vec<AutoMerge>,
    pub conflicts: Vec<Conflict>,
    pub additions: Vec<Entity>,
    pub removals: Vec<String>,
    /// 直接给 UI 显示的一句话摘要（APP 无需自己拼）
    #[serde(rename = "summary", default)]
    pub summary_text: String,
    pub upstream: PublicBundle,
}

impl PullPlan {
    pub fn needs_decision(&self) -> bool {
        !self.conflicts.is_empty()
    }
    pub fn summary(&self) -> String {
        format!(
            "上游 r{} -> r{}：自动合并 {} 项 / 新增 {} / 移除 {} / **待裁决 {} 项**",
            self.from_rev,
            self.to_rev,
            self.auto.len(),
            self.additions.len(),
            self.removals.len(),
            self.conflicts.len()
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolution {
    pub conflict_id: String,
    pub decision: Decision,
    /// 用户手填的第三方案（可选，优先于 decision 的固定动作）
    #[serde(default)]
    pub custom_value: Option<String>,
}

/// 可溯源日志：每一次拉取的决策都留痕，写进私有库。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: i64,
    pub device: String,
    pub action: String,
    pub entity_id: String,
    pub field: String,
    pub decision: Option<String>,
    pub from_rev: u64,
    pub to_rev: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PullReport {
    pub plan_id: String,
    pub auto_applied: usize,
    pub conflicts_resolved: usize,
    pub added: usize,
    pub removed: usize,
    pub keep_mine: usize,
    pub take_upstream: usize,
    pub keep_both: usize,
    pub new_rev: u64,
    pub log: Vec<LogEntry>,
}
