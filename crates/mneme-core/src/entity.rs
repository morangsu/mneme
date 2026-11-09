use crate::{content_hash, schema::FieldKind, stable_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// untagged：TOML/JSON 里的裸字符串、字符串数组、数字、布尔直接落进来。
/// 引用关系用 `Entity.group_id` 表达，不引入 Ref 变体（避免与字符串歧义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Text(String),
    Tags(Vec<String>),
    Number(f64),
    Bool(bool),
}

impl Value {
    pub fn from_raw(kind: FieldKind, raw: &serde_json::Value) -> Option<Self> {
        match kind {
            FieldKind::Text | FieldKind::Tag => raw.as_str().map(|s| Self::Text(s.to_string())),
            FieldKind::Tags => match raw {
                serde_json::Value::Array(a) => Some(Self::Tags(
                    a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                )),
                serde_json::Value::String(s) => Some(Self::Tags(vec![s.clone()])),
                _ => None,
            },
            FieldKind::Number => raw.as_f64().map(Self::Number),
            FieldKind::Bool => raw.as_bool().map(Self::Bool),
        }
    }

    /// 建索引用的词。返回借用，热路径不分配。
    pub fn terms(&self) -> Vec<&str> {
        match self {
            Self::Text(s) => vec![s.as_str()],
            Self::Tags(v) => v.iter().map(String::as_str).collect(),
            _ => vec![],
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Tags(v) => v.join("、"),
            Self::Number(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(s) => s.trim().is_empty(),
            Self::Tags(v) => v.is_empty(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceAnchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// 一药一行 / 一篇一行 / 一种一行。共性不在这里，靠 group_id 指向 Group。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceAnchor>,
    /// 单调递增版本号，字段级三方合并的裁决依据之一。
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub device: String,
    /// 显式墓碑。绝不靠「远端整份没有」来推断删除（曾踩过的真实 bug）。
    #[serde(default)]
    pub deleted: bool,
}

impl Entity {
    pub fn new(id: impl Into<String>, kind: impl Into<String>, device: &str, now: i64) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            group_id: None,
            fields: BTreeMap::new(),
            source: None,
            rev: 1,
            updated_at: now,
            device: device.to_string(),
            deleted: false,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: Value, now: i64) {
        self.fields.insert(key.into(), value);
        self.rev += 1;
        self.updated_at = now;
    }

    pub fn display_of(&self, key: &str) -> String {
        self.get(key).map(Value::display).unwrap_or_default()
    }

    /// 内容指纹：快照 diff 只比较这个，不逐字段深比。
    /// 廉价内容指纹（FNV-1a），只给**可重建的物化镜像**用。
    ///
    /// `fingerprint()` 走 sha256，2000 条实体一次全表比对要 30 ms；
    /// 镜像只是缓存，判错「有没有变」最多让镜像多刷一行，不需要密码学强度。
    /// 同步/合并/冲突判定仍必须用 `fingerprint()`，那里的「不同」要能上溯。
    pub fn fast_fp(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |bytes: &[u8]| {
            for b in bytes {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
            h = h.wrapping_mul(0x100_0000_01b3) ^ 0x1f;
        };
        mix(self.id.as_bytes());
        mix(self.kind.as_bytes());
        mix(self.group_id.as_deref().unwrap_or("").as_bytes());
        for (k, v) in &self.fields {
            mix(k.as_bytes());
            // 刻意不用 `v.display()`：那是每个字段一次 String 分配，
            // 2000 条实体 × 21 字段就是 4 万次分配，只为算个哈希。
            match v {
                Value::Text(t) => mix(t.as_bytes()),
                Value::Tags(list) => {
                    for t in list {
                        mix(t.as_bytes());
                    }
                }
                Value::Number(n) => mix(&n.to_le_bytes()),
                Value::Bool(b) => mix(&[*b as u8]),
            }
        }
        mix(if self.deleted { b"D" } else { b"A" });
        h
    }

    pub fn fingerprint(&self) -> String {
        let mut s = String::with_capacity(128);
        s.push_str(&self.id);
        s.push('\u{1f}');
        s.push_str(&self.kind);
        s.push('\u{1f}');
        s.push_str(self.group_id.as_deref().unwrap_or(""));
        s.push('\u{1f}');
        for (k, v) in &self.fields {
            s.push_str(k);
            s.push('=');
            s.push_str(&v.display());
            s.push('\u{1e}');
        }
        s.push_str(if self.deleted { "D" } else { "A" });
        content_hash(s.as_bytes())
    }
}

/// 共性基座：药群 / 篇章 / 科属 / 食材类。只维护一份。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// 上级分组 id。`None` = 顶层。
    ///
    /// 放在 `Group` 而不是 `common` 里：层级是核心结构，不是可选元数据 ——
    /// 视图分组、循环检测、深度限制都靠它。旧数据没有这个字段时反序列化成“顶层”，
    /// 不会因为加字段而读不出快照。
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub common: BTreeMap<String, Value>,
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TermAlias {
    pub concept: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}

/// append-only 复习事件。**只传事件、不传 SRS 状态**：
/// 排期在每台设备上由事件重算，避免两端算法状态分叉。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub card_id: String,
    pub ts: i64,
    /// 1=重来 2=困难 3=良好 4=简单
    pub grade: u8,
    pub device: String,
    #[serde(default)]
    pub seq: u64,
}

impl ReviewEvent {
    /// 幂等键。同一事件重复回传 N 次也只算一次。
    pub fn key(&self) -> String {
        stable_id(&[&self.card_id, &self.ts.to_string(), &self.device, &self.seq.to_string()])
    }
}
