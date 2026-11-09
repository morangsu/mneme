use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 字段类型。决定了导入时如何把裸值转成 `Value`，以及能否进倒排索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    #[default]
    Text,
    Tag,
    Tags,
    Number,
    Bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldDef {
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: FieldKind,
    /// 建倒排索引。reversible=true 时强制要求为 true。
    #[serde(default)]
    pub indexable: bool,
    /// 可作为「线索」被反查（功效->药名 / 原文->篇名 ...）。
    #[serde(default)]
    pub reversible: bool,
    /// 危险区：性味归经、毒性、用量、配伍禁忌、特殊煎法、鉴别点。
    /// 禁止同义替换，合并冲突时人工裁决，视图默认显示。
    #[serde(default)]
    pub immutable: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

/// 一个出题方向 = 「给哪些字段 -> 答哪些字段」。
/// 这就是老师说的「知道部分回答其余」的 declaractive 描述。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DirectionDef {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub clue_fields: Vec<String>,
    #[serde(default)]
    pub answer_fields: Vec<String>,
    /// 候选不唯一时自动追加的限定字段，例如 group（发散风寒药）。
    #[serde(default)]
    pub scope: Option<String>,
    /// 加了限定仍不唯一 -> 转成「枚举 + 说差异」题。
    #[serde(default)]
    pub enumerate_if_ambiguous: bool,
    #[serde(default)]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityDef {
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub fields: Vec<FieldDef>,
    /// 共性基座的类型名（药群 / 篇章 / 科属 / 食材类）。共性只存一份。
    #[serde(default)]
    pub group_kind: Option<String>,
    /// 作为题面/答案主体的字段（药名、篇名、种名...）。
    #[serde(default)]
    pub primary: Option<String>,
    #[serde(default)]
    pub directions: Vec<DirectionDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    #[default]
    Eq,
    Ne,
    Contains,
    NonEmpty,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewFilter {
    pub field: String,
    #[serde(default)]
    pub op: FilterOp,
    #[serde(default)]
    pub value: String,
}

/// 多视图 = 同一份数据的不同「列配置 + 筛选 + 分组」，不是多份表。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewDef {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub filter: Option<ViewFilter>,
    /// 出题视图隐藏的列（药名/答案），答题视图再展开。
    #[serde(default)]
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TermAliasDef {
    pub concept: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub boundary: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
}

fn default_version() -> u32 {
    1
}

/// 一门学科的完整声明。四个示例见 `domains/*.toml`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainSchema {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub entities: Vec<EntityDef>,
    #[serde(default)]
    pub aliases: Vec<TermAliasDef>,
    #[serde(default)]
    pub views: Vec<ViewDef>,
}

impl DomainSchema {
    pub fn from_toml(s: &str) -> Result<Self> {
        let schema: Self = toml::from_str(s)?;
        schema.validate()?;
        Ok(schema)
    }

    pub fn to_toml(&self) -> Result<String> {
        self.validate()?;
        Ok(toml::to_string_pretty(self).unwrap_or_default())
    }

    pub fn entity_def(&self, kind: &str) -> Option<&EntityDef> {
        self.entities.iter().find(|e| e.kind == kind)
    }

    pub fn field<'a>(&'a self, kind: &str, key: &str) -> Option<&'a FieldDef> {
        self.entity_def(kind)?.fields.iter().find(|f| f.key == key)
    }

    pub fn is_immutable(&self, kind: &str, key: &str) -> bool {
        self.field(kind, key).is_some_and(|f| f.immutable)
    }

    pub fn reversible_fields(&self, kind: &str) -> Vec<&FieldDef> {
        self.entity_def(kind)
            .map(|e| e.fields.iter().filter(|f| f.reversible).collect())
            .unwrap_or_default()
    }

    /// 加载时一次性校验，避免运行期才炸。
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::Schema("id 不能为空".into()));
        }
        if self.entities.is_empty() {
            return Err(Error::Schema("至少需要一个 [[entities]]".into()));
        }
        let mut kinds = HashSet::new();
        for e in &self.entities {
            if !kinds.insert(e.kind.as_str()) {
                return Err(Error::Schema(format!("实体类型重复: {}", e.kind)));
            }
            let mut keys = HashSet::new();
            for f in &e.fields {
                if !keys.insert(f.key.as_str()) {
                    return Err(Error::Schema(format!(
                        "{}.{} 字段重复",
                        e.kind, f.key
                    )));
                }
                if f.reversible && !f.indexable {
                    return Err(Error::Schema(format!(
                        "{}.{} 是 reversible，必须同时 indexable = true",
                        e.kind, f.key
                    )));
                }
            }
            if let Some(p) = &e.primary {
                if !keys.contains(p.as_str()) {
                    return Err(Error::Schema(format!(
                        "{} 的 primary `{p}` 不在 fields 里",
                        e.kind
                    )));
                }
            }
            for d in &e.directions {
                for k in d.clue_fields.iter().chain(d.answer_fields.iter()) {
                    if !keys.contains(k.as_str()) {
                        return Err(Error::Schema(format!(
                            "方向 `{}` 引用了不存在的字段 `{k}`",
                            d.id
                        )));
                    }
                }
                if let Some(s) = &d.scope {
                    if !keys.contains(s.as_str()) {
                        return Err(Error::Schema(format!(
                            "方向 `{}` 的 scope `{s}` 不存在",
                            d.id
                        )));
                    }
                }
            }
        }
        let mut view_ids = HashSet::new();
        for v in &self.views {
            if !view_ids.insert(v.id.as_str()) {
                return Err(Error::Schema(format!("视图 id 重复: {}", v.id)));
            }
        }
        Ok(())
    }

    /// 按 kind 预建字段索引，热路径避免线性查找。
    pub fn field_map(&self) -> HashMap<String, HashMap<String, FieldDef>> {
        self.entities
            .iter()
            .map(|e| {
                (
                    e.kind.clone(),
                    e.fields
                        .iter()
                        .map(|f| (f.key.clone(), f.clone()))
                        .collect(),
                )
            })
            .collect()
    }
}
