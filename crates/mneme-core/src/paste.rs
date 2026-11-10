//! 「把教材一段话粘进来」的解析：文本 → 字段。
//!
//! 目标不是 NLP，而是**确定性地**把中文教材/笔记里的
//! 「标签：值」行对应到领域包的字段上，规则可预测、结果可解释：
//! 认出来的给用户看，认不出来的原样留下来，绝不猜。
//!
//! 之所以放 core 而不是 UI：桌面端与服务端要共用同一套判定，
//! 否则同一个粘贴在两端填出不同字段，用户没法信任。

use crate::entity::Entity;
use crate::schema::DomainSchema;
use crate::text::{normalize, similarity};
use std::collections::BTreeMap;

/// 一条成功识别的行。
#[derive(Debug, Clone, PartialEq)]
pub struct PasteHit {
    /// 领域包字段 key
    pub key: String,
    /// 字段中文名（用于回显「已识别：功效」）
    pub label: String,
    pub value: String,
    /// 原始行号（从 1 开始），方便用户对着原文核对；0 表示标题行提升（见 promote_primary_from_unknown）
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PasteParse {
    /// key → 值。**同名标签取第一次出现的值**，后续重复记进 `conflicts`。
    pub fields: BTreeMap<String, String>,
    pub hits: Vec<PasteHit>,
    /// 认不出来的行（原样保留，交给用户手动填）
    pub unknown: Vec<String>,
    /// 「功效」出现了 2 次之类的提示
    pub conflicts: Vec<String>,
}

impl PasteParse {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// 把 `标签：值` 拆开。返回 (左侧标签, 右侧值)。
///
/// 支持全角/半角冒号、等号、制表符 —— 从微信、Word、Excel 粘过来的格式各不相同。
fn split_pair(line: &str) -> Option<(&str, &str)> {
    for sep in ['：', ':', '=', '\t'] {
        if let Some(i) = line.find(sep) {
            let (left, right) = line.split_at(i);
            let right = &right[sep.len_utf8()..];
            if !left.trim().is_empty() {
                return Some((left.trim(), right.trim()));
            }
        }
    }
    None
}

/// 标签与字段的匹配：先精确，再「互相包含」。
///
/// 允许包含是为了吃掉「性味」↔「性味归经」、「用量」↔「用法用量」这类
/// 教材写法差异；限制两边长度 ≥2，避免「名」这种单字标签乱命中。
fn match_field<'a>(schema: &'a DomainSchema, kind: &str, raw_label: &str) -> Option<(String, String)> {
    let def = schema.entity_def(kind)?;
    let want = normalize(raw_label);
    if want.is_empty() {
        return None;
    }
    // 1) 与 label 精确相同
    for f in &def.fields {
        if !f.label.is_empty() && normalize(&f.label) == want {
            return Some((f.key.clone(), f.label.clone()));
        }
    }
    // 2) 与 key 精确相同（贴的是英文 key，或导出后再贴回来）
    for f in &def.fields {
        if normalize(&f.key) == want {
            return Some((f.key.clone(), label_of(f)));
        }
    }
    // 3) 互相包含（长度都 ≥2）
    if want.chars().count() >= 2 {
        for f in &def.fields {
            if f.label.is_empty() {
                continue;
            }
            let have = normalize(&f.label);
            if have.chars().count() < 2 {
                continue;
            }
            if have.contains(&want) || want.contains(&have) {
                return Some((f.key.clone(), f.label.clone()));
            }
        }
    }
    None
}

fn label_of(f: &crate::schema::FieldDef) -> String {
    if f.label.is_empty() {
        f.key.clone()
    } else {
        f.label.clone()
    }
}

/// 解析粘贴文本。`kind` 是实体类型（如 `herb`）。
pub fn parse_paste(text: &str, schema: &DomainSchema, kind: &str) -> PasteParse {
    let mut out = PasteParse::default();
    if schema.entity_def(kind).is_none() {
        return out;
    }
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim().trim_start_matches(['-', '*', '•', '·']).trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        let matched = split_pair(line).and_then(|(label, value)| {
            if value.is_empty() {
                return None;
            }
            match_field(schema, kind, label).map(|(k, l)| (k, l, value.to_string()))
        });

        // 没有冒号时再试「标签 + 空格 + 值」：教材里常见「药名 麻黄」
        let matched = matched.or_else(|| {
            let def = schema.entity_def(kind)?;
            for f in &def.fields {
                if f.label.is_empty() {
                    continue;
                }
                let label = f.label.trim();
                if label.chars().count() < 2 {
                    continue;
                }
                if let Some(rest) = line.strip_prefix(label) {
                    let rest = rest.trim_start();
                    if rest.len() != line.len() && !rest.is_empty() {
                        return Some((f.key.clone(), f.label.clone(), rest.to_string()));
                    }
                }
            }
            None
        });

        match matched {
            Some((key, label, value)) => {
                if out.fields.contains_key(&key) {
                    out.conflicts
                        .push(format!("「{label}」出现多次，已取第一次的值"));
                    continue;
                }
                out.fields.insert(key.clone(), value.clone());
                out.hits.push(PasteHit {
                    key,
                    label,
                    value,
                    line: line_no,
                });
            }
            None => out.unknown.push(line.to_string()),
        }
    }
    promote_primary_from_unknown(&mut out, schema, kind);
    out
}

/// 缺主键时，把第一条「纯文本短行」提升为主键值。
///
/// 词条截图/笔记的标题行就是「麻黄」这种光秃秃的名字——它解析不出任何
/// `标签：值`，以前只能躺在 unknown 里让人手动补主键，整条就废了。
/// 在「块里没有主键字段」的前提下，无分隔符的短行作为主键是唯一合理解释，
/// 不是猜：解释写在 hits 里（label = 主键字段名），用户看得见、可撤销。
/// 上限 8 字：中药名最长不过五六字（如「九香虫」「广藿香」），更长的多半是
/// 没写标签的正文句子（「完全没有标签的一段话」这种），留在 unknown 里。
fn promote_primary_from_unknown(out: &mut PasteParse, schema: &DomainSchema, kind: &str) {
    let Some(def) = schema.entity_def(kind) else { return };
    let primary = def.primary.clone().unwrap_or_else(|| "name".into());
    if out.fields.contains_key(&primary) {
        return;
    }
    let pos = out
        .unknown
        .iter()
        .position(|line| split_pair(line).is_none() && line.chars().count() <= 8);
    if let Some(i) = pos {
        let value = out.unknown.remove(i);
        let label = def
            .fields
            .iter()
            .find(|f| f.key == primary)
            .map(|f| label_of(f))
            .unwrap_or_else(|| primary.clone());
        out.fields.insert(primary.clone(), value.clone());
        out.hits.push(PasteHit { key: primary, label, value, line: 0 });
    }
}

/// 一批粘贴结果：**空行分隔**多条。
///
/// 为什么选空行而不是 `---` 之类的分隔符：
/// 从笔记/Word 复制过来时，两条之间天然就有空行；而 `---`、`===`
/// 可能出现在正文里（比如原文自带的分隔线），误切比漏切更难发现。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PasteBatch {
    pub blocks: Vec<PasteParse>,
    /// 空块数量（连续空行会被合并，这里只统计真正的空段）
    pub skipped_blocks: usize,
}

impl PasteBatch {
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|b| b.fields.is_empty())
    }
}

pub fn parse_paste_batch(text: &str, schema: &DomainSchema, kind: &str) -> PasteBatch {
    let mut out = PasteBatch::default();
    let mut buf: Vec<&str> = Vec::new();
    let mut skipped = 0usize;

    let mut flush = |buf: &mut Vec<&str>, out: &mut PasteBatch| {
        if buf.is_empty() {
            return;
        }
        let block_text = buf.join("\n");
        buf.clear();
        let parsed = parse_paste(&block_text, schema, kind);
        if parsed.fields.is_empty() && parsed.unknown.is_empty() {
            skipped += 1;
        } else {
            out.blocks.push(parsed);
        }
    };

    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut buf, &mut out);
        } else {
            buf.push(line);
        }
    }
    flush(&mut buf, &mut out);
    out.skipped_blocks = skipped;
    out
}

/// 可能是同一条实体的候选项。
#[derive(Debug, Clone, PartialEq)]
pub struct Duplicate {
    pub id: String,
    pub name: String,
    /// 人类可读的原因：同名 / 同义名 / 近似名
    pub reason: String,
    /// 0~1，越大越像
    pub score: f64,
}

/// 查重：准备新建/改名时，先看库里有没有「其实是同一味药」的条目。
///
/// 三条判据按可信度排序：
/// 1. **同名**（归一化后完全相同）—— 一定是重复；
/// 2. **同义名**（撞上对方的 `synonyms`）—— 教材别名，几乎一定是同一味；
/// 3. **近似名**（相似度 ≥ 0.8）—— 只提示，让人自己判断。
///
/// 不算已删除的（在回收站里的），但也不算「被替代」的旧版 —— 那正是用户想新建修订版的场景。
pub fn find_duplicates(
    schema: &DomainSchema,
    entities: &[Entity],
    kind: &str,
    fields: &BTreeMap<String, String>,
    exclude_id: Option<&str>,
) -> Vec<Duplicate> {
    let Some(def) = schema.entity_def(kind) else {
        return Vec::new();
    };
    let primary = def.primary.clone().unwrap_or_else(|| "name".into());
    let raw_name = fields.get(&primary).map(|s| s.trim()).unwrap_or("");
    if raw_name.is_empty() {
        return Vec::new();
    }
    let want = normalize(raw_name);

    let mut out: Vec<Duplicate> = Vec::new();
    for e in entities {
        if e.deleted || e.kind != kind {
            continue;
        }
        if Some(e.id.as_str()) == exclude_id {
            continue;
        }
        let name = e.display_of(&primary);
        let tag = |reason: &str, score: f64| Duplicate {
            id: e.id.clone(),
            name: name.clone(),
            reason: reason.to_string(),
            score,
        };

        if normalize(&e.id) == want || normalize(&name) == want {
            out.push(tag("同名", 1.0));
            continue;
        }
        // 对方登记的别名里有我要建的名字
        if let Some(crate::entity::Value::Tags(tags)) = e.get("synonyms") {
            if tags.iter().any(|t| normalize(t) == want) {
                out.push(tag("同义名", 0.95));
                continue;
            }
        }
        // 我填的别名里有对方的名字（反方向）
        if let Some(crate::entity::Value::Tags(tags)) = fields.get("synonyms").and_then(|s| {
            Some(crate::entity::Value::Tags(
                s.split(['|', '、', ',', '，'])
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect(),
            ))
        }) {
            if tags.iter().any(|t| normalize(t) == normalize(&name)) {
                out.push(tag("同义名", 0.95));
                continue;
            }
        }
        let score = similarity(raw_name, &name);
        if score >= 0.8 {
            out.push(tag("近似名", score));
        }
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    out.truncate(5);
    out
}
