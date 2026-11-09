//! 跨字段全文检索：子串 + 同义名 + 拼音首字母，带命中片段与排序。
//!
//! 设计取舍：
//! - **线性扫描，不另建索引。** 一个领域库是几百到几千条实体，每条几十个字段，
//!   一次全表扫描在微秒级；建二级索引反而要维护一致性（增删改/同步/回滚都要跟）。
//!   等真到十万条再谈索引，现在优化是负收益。
//! - **多词默认 AND**：`麻黄 解表` 要求两个词都命中（可在不同字段）。AND 一条都
//!   没有时才退化成 OR，并让 UI 知道（`mode = "or"`），避免用户以为搜错了。
//! - **归一化再用**：复用 `text::normalize`，与索引/反查口径完全一致，
//!   否则「辛、微苦」和「辛微苦」会一种能搜到一种搜不到。
//! - **拼音只做首字母**：`mh` → 麻黄。全拼数据量大一个数量级，当前不做。

use crate::entity::{Entity, Group, Value};
use crate::groups::{GroupNode, tree as group_tree};
use crate::pinyin::{initials, initials_match};
use crate::schema::DomainSchema;
use crate::text::{normalize, normalize_mapped};

/// 命中方式。UI 用它决定高亮样式与说明文案（「拼音首字母」要显式告诉用户）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Name,
    Synonym,
    Pinyin,
    Field,
    Group,
}

/// 一条命中。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// 分组显示路径（`发散风寒药 / 麻黄类`），没挂分组则为空串。
    pub group: String,
    pub score: f64,
    pub via: Via,
    /// 命中字段的 key（主键/同义名/拼音/分组命中时为空）。
    pub field: String,
    /// 命中字段的中文标签。
    pub label: String,
    /// 命中位置附近的原文片段（主键命中时就是名字本身）。
    pub snippet: String,
}

/// 检索结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    /// 截断前的命中总数。
    pub total: usize,
    /// `and`（每个词都命中）/ `or`（AND 无结果时的回退）。
    pub mode: &'static str,
    /// 实际参与匹配的词（归一化后），供 UI 提示「没搜到 X」。
    pub terms: Vec<String>,
}

/// 检索入口。
///
/// - `kind`：只搜某类实体（`None` = 不限）。
/// - `limit`：命中条数上限（`0` 表示不截断）。
pub fn search(
    schema: &DomainSchema,
    entities: &[Entity],
    groups: &[Group],
    query: &str,
    kind: Option<&str>,
    limit: usize,
) -> Outcome {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(normalize)
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return Outcome { hits: Vec::new(), total: 0, mode: "and", terms };
    }

    // 用分组**树**而不是平铺名字表：搜「解表药」应当命中它下面所有药，
    // 而不是只命中「解表药」这一层的成员。多级分组做了，检索就得跟上。
    let tree = group_tree(groups);
    let nodes: std::collections::HashMap<&str, &GroupNode> =
        tree.iter().map(|n| (n.id.as_str(), n)).collect();

    // AND 优先：每个词都要在实体里找到位置
    let mut scored: Vec<(f64, Hit)> = Vec::new();
    let mut or_hits: Vec<(f64, Hit)> = Vec::new();

    for e in entities.iter().filter(|e| !e.deleted) {
        if let Some(k) = kind {
            if e.kind != k {
                continue;
            }
        }
        let Some(def) = schema.entity_def(&e.kind) else { continue };
        let primary = def.primary.clone().unwrap_or_else(|| "name".into());
        let name = e.display_of(&primary);
        let name_norm = normalize(&name);
        let name_ini = initials(&name);

        let mut best: Option<Match> = None;
        let mut sum = 0.0;
        let mut hits_found = 0usize;
        let mut all = true;
        for t in &terms {
            match match_term(schema, e, &primary, &name, &name_norm, &name_ini, t, &nodes) {
                Some(m) => {
                    hits_found += 1;
                    sum += m.score;
                    if best.as_ref().map(|b| m.score > b.score).unwrap_or(true) {
                        best = Some(m);
                    }
                }
                None => all = false,
            }
        }
        let Some(top) = best else { continue };
        let group = group_path(e, &nodes);
        let mut hit = Hit {
            id: e.id.clone(),
            name,
            kind: e.kind.clone(),
            group,
            score: sum,
            via: top.via,
            field: top.field,
            label: top.label,
            snippet: top.snippet,
        };
        if all {
            scored.push((sum, hit));
        } else if hits_found > 0 {
            // OR 回退时按命中词数打折，命中越少越靠后
            hit.score = sum * (hits_found as f64 / terms.len() as f64);
            or_hits.push((hit.score, hit));
        }
    }

    let mode = if scored.is_empty() && !or_hits.is_empty() {
        scored = or_hits;
        "or"
    } else {
        "and"
    };

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            // 同分时按名字长度：短名更像用户要找的那条
            .then(a.1.name.chars().count().cmp(&b.1.name.chars().count()))
            .then(a.1.id.cmp(&b.1.id))
    });
    let total = scored.len();
    let hits: Vec<Hit> = scored
        .into_iter()
        .map(|(_, h)| h)
        .take(if limit == 0 { usize::MAX } else { limit })
        .collect();
    Outcome { hits, total, mode, terms }
}

struct Match {
    score: f64,
    via: Via,
    field: String,
    label: String,
    snippet: String,
}

fn label_of(schema: &DomainSchema, kind: &str, key: &str) -> String {
    schema
        .field(kind, key)
        .map(|f| f.label.clone())
        .unwrap_or_else(|| key.to_string())
}

fn match_term(
    schema: &DomainSchema,
    e: &Entity,
    primary: &str,
    name: &str,
    name_norm: &str,
    name_ini: &str,
    term: &str,
    nodes: &std::collections::HashMap<&str, &GroupNode>,
) -> Option<Match> {
    // 1. 主键（药名）——权重最高，完全相等 > 前缀 > 包含
    let score = if name_norm == term {
        Some(1000.0)
    } else if name_norm.starts_with(term) {
        Some(700.0)
    } else if name_norm.contains(term) {
        Some(500.0)
    } else {
        None
    };
    if let Some(score) = score {
        return Some(Match {
            score,
            via: Via::Name,
            field: String::new(),
            label: label_of(schema, &e.kind, primary),
            snippet: name.to_string(),
        });
    }

    // 2. 同义名（别称）——用户常常只记得别名
    if let Some(Value::Tags(tags)) = e.get("synonyms") {
        for t in tags {
            if normalize(t).contains(term) {
                return Some(Match {
                    score: 450.0,
                    via: Via::Synonym,
                    field: "synonyms".into(),
                    label: label_of(schema, &e.kind, "synonyms"),
                    snippet: t.clone(),
                });
            }
        }
    }

    // 3. 拼音首字母——只对纯 ASCII 字母的查询生效，避免「辛」这种单字被误判
    if let Some(is_prefix) = initials_match(name_ini, term) {
        return Some(Match {
            score: if is_prefix { 400.0 } else { 300.0 },
            via: Via::Pinyin,
            field: String::new(),
            label: label_of(schema, &e.kind, primary),
            snippet: name.to_string(),
        });
    }
    if let Some(Value::Tags(tags)) = e.get("synonyms") {
        for t in tags {
            let ini = initials(t);
            if let Some(is_prefix) = initials_match(&ini, term) {
                return Some(Match {
                    score: if is_prefix { 280.0 } else { 240.0 },
                    via: Via::Pinyin,
                    field: "synonyms".into(),
                    label: label_of(schema, &e.kind, "synonyms"),
                    snippet: t.clone(),
                });
            }
        }
    }

    // 4. 其余字段（含 immutable：性味归经也该能搜），按 schema 顺序轻微降权
    let mut best: Option<Match> = None;
    for (idx, (key, value)) in e.fields.iter().enumerate() {
        if key == primary || key == "synonyms" {
            continue;
        }
        let text = value.display();
        if text.is_empty() {
            continue;
        }
        let (norm, map) = normalize_mapped(&text);
        if !norm.contains(term) {
            continue;
        }
        let score = (200.0 - idx as f64 * 2.0).max(120.0);
        if best.as_ref().map(|b| score > b.score).unwrap_or(true) {
            best = Some(Match {
                score,
                via: Via::Field,
                field: key.clone(),
                label: label_of(schema, &e.kind, key),
                snippet: snippet(&text, &norm, &map, term),
            });
        }
    }
    if best.is_some() {
        return best;
    }

    // 5. 分组名（含祖先路径）——「解表药」这类章节/药群检索
    let path = group_path(e, nodes);
    if !path.is_empty() && normalize(&path).contains(term) {
        return Some(Match {
            score: 150.0,
            via: Via::Group,
            field: "group_name".into(),
            label: "药群".into(),
            snippet: path,
        });
    }
    None
}

/// 分组显示路径：`解表药 / 发散风寒药`。
///
/// 没挂 Group 记录时退回 `group_name` 文本 —— 老数据不会因为引入分组树就丢分组。
fn group_path(e: &Entity, nodes: &std::collections::HashMap<&str, &GroupNode>) -> String {
    match e.group_id.as_deref().and_then(|g| nodes.get(g)) {
        Some(n) => n.display_path(),
        None => e.get("group_name").map(|v| v.display()).unwrap_or_default(),
    }
}

/// 从原文里截一段带上下文的片段。
///
/// 归一化会丢掉标点，所以要靠 `map` 把「归一化串里的位置」反推回「原文的字符下标」，
/// 否则「发汗解表、宣肺平喘」命中「宣肺」时截出来的窗口会整体偏移。
fn snippet(text: &str, norm: &str, map: &[usize], term: &str) -> String {
    let Some(pos) = norm.find(term) else {
        return head(text, 40);
    };
    let norm_char = norm[..pos].chars().count();
    let Some(&orig) = map.get(norm_char) else {
        return head(text, 40);
    };
    let chars: Vec<char> = text.chars().collect();
    let term_len = term.chars().count().max(1);
    let start = orig.saturating_sub(10);
    let end = (orig + term_len + 14).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

fn head(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= n {
        return text.to_string();
    }
    format!("{}…", chars[..n].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Value;

    fn schema() -> DomainSchema {
        DomainSchema::from_toml(
            r#"
id = "test"
name = "测试"
[[entities]]
kind = "herb"
primary = "name"
[[entities.fields]]
key = "name"
label = "药名"
kind = "text"
[[entities.fields]]
key = "synonyms"
label = "同义名"
kind = "tags"
[[entities.fields]]
key = "effects"
label = "功效"
kind = "tags"
[[entities.fields]]
key = "nature"
label = "性味归经"
kind = "text"
immutable = true
"#,
        )
        .expect("schema")
    }

    fn ent(id: &str, name: &str, effects: &[&str], nature: &str) -> Entity {
        let mut e = Entity::new(id, "herb", "test", 1);
        e.set("name", Value::Text(name.into()), 1);
        e.set(
            "effects",
            Value::Tags(effects.iter().map(|s| s.to_string()).collect()),
            1,
        );
        e.set("nature", Value::Text(nature.into()), 1);
        e
    }

    fn run(q: &str) -> Outcome {
        let s = schema();
        let es = vec![
            ent("麻黄", "麻黄", &["发汗解表", "宣肺平喘"], "辛、微苦，温。归肺、膀胱经"),
            ent("桂枝", "桂枝", &["发汗解肌", "温通经脉"], "辛、甘，温"),
            ent("苦杏仁", "苦杏仁", &["降气止咳平喘"], "苦，微温"),
            ent("石膏", "石膏", &["清热泻火"], "甘、辛，大寒"),
        ];
        search(&s, &es, &[], q, None, 0)
    }

    #[test]
    fn name_exact_beats_field() {
        let r = run("麻黄");
        assert_eq!(r.hits[0].id, "麻黄");
        assert_eq!(r.hits[0].via, Via::Name);
        assert!(r.hits[0].score >= 1000.0);
    }

    #[test]
    fn field_search_returns_snippet_with_label() {
        let r = run("宣肺");
        assert_eq!(r.hits[0].id, "麻黄");
        assert_eq!(r.hits[0].via, Via::Field);
        assert_eq!(r.hits[0].field, "effects");
        assert_eq!(r.hits[0].label, "功效");
        assert!(r.hits[0].snippet.contains("宣肺"));
    }

    #[test]
    fn immutable_fields_are_searchable() {
        // 性味归经是原样字段，禁止改写，但不代表不能搜
        let r = run("膀胱经");
        assert_eq!(r.hits[0].id, "麻黄");
        assert_eq!(r.hits[0].field, "nature");
    }

    #[test]
    fn pinyin_initials_hit_name() {
        let r = run("mh");
        assert_eq!(r.hits[0].id, "麻黄");
        assert_eq!(r.hits[0].via, Via::Pinyin);
        let r = run("kxr");
        assert_eq!(r.hits[0].id, "苦杏仁");
    }

    #[test]
    fn multi_term_is_and_then_falls_back_to_or() {
        // 两个词分别在药名与功效里，AND 成立
        let r = run("麻黄 宣肺");
        assert_eq!(r.mode, "and");
        assert_eq!(r.hits.len(), 1);
        // 一个词谁都命中不了 -> 退化成 OR
        let r = run("平喘 不存在的词");
        assert_eq!(r.mode, "or");
        assert!(r.hits.iter().any(|h| h.id == "麻黄"));
    }

    #[test]
    fn empty_query_returns_nothing() {
        let r = run("   ");
        assert!(r.hits.is_empty());
        assert_eq!(r.total, 0);
    }

    #[test]
    fn deleted_entities_are_skipped() {
        let s = schema();
        let mut e = ent("麻黄", "麻黄", &["发汗解表"], "");
        e.deleted = true;
        let r = search(&s, &[e], &[], "麻黄", None, 0);
        assert!(r.hits.is_empty());
    }

    #[test]
    fn truncation_reports_total() {
        let s = schema();
        let es: Vec<Entity> = (0..5)
            .map(|i| ent(&format!("e{i}"), &format!("测试{i}"), &["发汗"], ""))
            .collect();
        let r = search(&s, &es, &[], "测试", None, 2);
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.total, 5);
    }

    #[test]
    fn snippet_keeps_context_around_punctuation() {
        // 归一化丢了顿号，但片段要按原文截，不能整体偏移
        let s = schema();
        let e = ent("麻黄", "麻黄", &["发汗解表", "宣肺平喘"], "辛、微苦，温。归肺、膀胱经");
        let r = search(&s, &[e], &[], "膀胱经", None, 0);
        assert_eq!(r.hits[0].snippet, "辛、微苦，温。归肺、膀胱经");
    }
}
