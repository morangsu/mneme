use crate::entity::{Entity, Group, Value};
use crate::error::{Error, Result};
use crate::index::InvertedIndex;
use crate::schema::DomainSchema;
use crate::stable_id;
use crate::text::{TermTable, similarity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 反向卡是**派生数据**：由实体 + 出题方向自动生成，不手工维护。
/// 改了主数据，卡片重新生成即可，不存在「两处不一致」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub entity_id: String,
    pub direction: String,
    pub clue_field: String,
    pub clue_term: String,
    pub prompt: String,
    /// 展示用完整答案
    pub answer: String,
    /// 判分用：每个答案字段单独一条，避免长文本拼接拉低相似度
    pub accept: Vec<String>,
    pub answer_fields: Vec<String>,
    pub scope: Option<String>,
    pub candidates: Vec<String>,
    pub enumerate: bool,
    /// 危险区提示：性味归经/毒性/用量/配伍禁忌等，不允许同义替换
    pub immutable_boundary: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Judge {
    pub score: f64,
    pub correct: bool,
    pub matched: String,
}

fn field_value<'a>(
    e: &'a Entity,
    groups: &'a HashMap<String, Group>,
    key: &str,
) -> Option<&'a Value> {
    e.get(key).or_else(|| {
        e.group_id
            .as_ref()
            .and_then(|g| groups.get(g))
            .and_then(|g| g.common.get(key))
    })
}

fn render(tpl: &str, vars: &[(&str, &str)]) -> String {
    let mut s = tpl.to_string();
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// 按 schema 的 directions 为一条实体生成全部反向卡。
///
/// 唯一性收敛（治「多药同功效」）：
/// 1. 用线索跑反查，看除自己外还有几个候选；
/// 2. 有歧义且方向配了 scope -> 自动把限定词（药群/篇章/科属）加进题面并重算；
/// 3. 仍不唯一且允许枚举 -> 转成「列出所有 + 说差异」题，绝不硬给单答案。
pub fn generate_cards(
    schema: &DomainSchema,
    entity: &Entity,
    groups: &HashMap<String, Group>,
    index: &InvertedIndex,
) -> Result<Vec<Card>> {
    let def = schema
        .entity_def(&entity.kind)
        .ok_or_else(|| Error::NotFound(format!("实体类型 {}", entity.kind)))?;
    let primary = def.primary.as_deref().unwrap_or("name");
    let mut cards = Vec::with_capacity(def.directions.len() * 3);

    for d in &def.directions {
        let mut clue_terms: Vec<(String, String)> = Vec::new();
        for f in &d.clue_fields {
            if let Some(v) = field_value(entity, groups, f) {
                for t in v.terms() {
                    clue_terms.push((format!("{}.{}", entity.kind, f), t.to_string()));
                }
            }
        }
        if clue_terms.is_empty() {
            continue;
        }

        let answer_parts: Vec<String> = d
            .answer_fields
            .iter()
            .filter_map(|f| field_value(entity, groups, f))
            .map(Value::display)
            .filter(|s| !s.trim().is_empty())
            .collect();
        if answer_parts.is_empty() {
            continue;
        }
        let answer = answer_parts.join("；");

        let immutable_boundary: Vec<String> = d
            .answer_fields
            .iter()
            .filter(|f| schema.is_immutable(&entity.kind, f))
            .map(|f| {
                let label = schema
                    .field(&entity.kind, f)
                    .map(|x| x.label.as_str())
                    .unwrap_or(f.as_str());
                format!("{label} 须按教材原文，不可用同义说法替换")
            })
            .collect();

        let scope_value = d
            .scope
            .as_ref()
            .and_then(|s| field_value(entity, groups, s))
            .map(Value::display)
            .filter(|s| !s.is_empty());

        for (clue_field, clue_term) in clue_terms {
            let mut clues = vec![(clue_field.clone(), clue_term.clone())];
            let mut candidates = other_candidates(index, &clues, &entity.id);

            if !candidates.is_empty() {
                if let (Some(scope_field), Some(sv)) = (&d.scope, &scope_value) {
                    clues.push((format!("{}.{}", entity.kind, scope_field), sv.clone()));
                    candidates = other_candidates(index, &clues, &entity.id);
                }
            }
            let enumerate = !candidates.is_empty() && d.enumerate_if_ambiguous;

            // 只有真的出现歧义、且已追加 scope 线索时才把限定词写进题面
            let scope_prefix = if clues.len() > 1 {
                scope_value
                    .as_ref()
                    .map(|s| format!("【{s}】"))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let answer_label = d
                .answer_fields
                .iter()
                .map(|f| {
                    schema
                        .field(&entity.kind, f)
                        .map(|x| x.label.as_str())
                        .unwrap_or(f.as_str())
                })
                .collect::<Vec<_>>()
                .join("、");

            let default_tpl = if clue_field.ends_with(&format!(".{primary}")) {
                "{clue}：请说出{answer_label}"
            } else if enumerate {
                "{scope}具有「{clue}」的有哪些？请列出并说出各自差异"
            } else {
                "{scope}具有「{clue}」的是？"
            };
            let tpl = d.template.as_deref().unwrap_or(default_tpl);
            let mut prompt = render(
                tpl,
                &[
                    ("clue", &clue_term),
                    ("scope", &scope_prefix),
                    ("answer_label", &answer_label),
                    ("name", &entity.display_of(primary)),
                ],
            );
            if !immutable_boundary.is_empty() {
                prompt.push_str(&format!("\n（注意：{}）", immutable_boundary.join("；")));
            }

            let mut final_answer = answer.clone();
            let mut accept = answer_parts.clone();
            if enumerate {
                let mut names = vec![entity.display_of(primary)];
                names.extend(candidates.iter().take(8).cloned());
                final_answer = format!("{}；共同点见对比视图", names.join("、"));
                accept = names;
            }

            cards.push(Card {
                id: stable_id(&[&entity.id, &d.id, &clue_field, &clue_term]),
                entity_id: entity.id.clone(),
                direction: d.id.clone(),
                clue_field: clue_field.clone(),
                clue_term: clue_term.clone(),
                prompt,
                answer: final_answer,
                accept,
                answer_fields: d.answer_fields.clone(),
                scope: scope_value.clone(),
                candidates,
                enumerate,
                immutable_boundary: immutable_boundary.clone(),
            });
        }
    }
    Ok(cards)
}

fn other_candidates(
    index: &InvertedIndex,
    clues: &[(String, String)],
    self_id: &str,
) -> Vec<String> {
    index
        .reverse(clues)
        .into_iter()
        .filter_map(|s| index.id_of(s.idx).map(String::from))
        .filter(|id| id != self_id)
        .collect()
}

/// 判分：同义归一 -> 与每条可接受答案取最大相似度。
/// 复用 tiku 的 norm + dice，OCR 错字、全角、标点都能容错。
pub fn judge(input: &str, card: &Card, aliases: &TermTable, threshold: f64) -> Judge {
    let inp = aliases.canonical(input.trim());
    let mut best = 0.0f64;
    let mut matched = String::new();
    for a in &card.accept {
        let s = similarity(&inp, &aliases.canonical(a));
        if s > best {
            best = s;
            matched = a.clone();
        }
    }
    Judge {
        score: best,
        correct: best >= threshold,
        matched,
    }
}

/// 从同一药群/篇章/科属里挑相似实体做**干扰项**，把自由回忆题转成选择题。
/// 顺序由 `stable_id` 决定（确定性、可复现），不引入随机数。
pub fn generate_choices(
    schema: &DomainSchema,
    entity: &Entity,
    entities: &[Entity],
    distractors: usize,
) -> Vec<String> {
    let primary = schema
        .entity_def(&entity.kind)
        .and_then(|d| d.primary.as_deref())
        .unwrap_or("name");
    let answer = entity.display_of(primary);
    let mut candidates: Vec<(String, String)> = entities
        .iter()
        .filter(|e| !e.deleted && e.id != entity.id && e.kind == entity.kind)
        .map(|e| {
            let same_group = entity
                .group_id
                .as_ref()
                .is_some_and(|g| e.group_id.as_ref() == Some(g));
            // 同组优先，其次按确定性哈希排序
            let rank = if same_group { "0" } else { "1" };
            (
                format!("{}-{}", rank, stable_id(&[&entity.id, &e.id])),
                e.display_of(primary),
            )
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = vec![answer];
    for (_, name) in candidates {
        if out.len() > distractors {
            break;
        }
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DomainSchema, Vec<Entity>, InvertedIndex, HashMap<String, Group>) {
        let schema = DomainSchema::from_toml(
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
key = "group"
label = "药群"
kind = "tag"
indexable = true
[[entities.fields]]
key = "effects"
label = "功效"
kind = "tags"
indexable = true
reversible = true
[[entities.fields]]
key = "features"
label = "特点"
kind = "tags"
indexable = true
reversible = true
[[entities.directions]]
id = "effect_to_name"
clue_fields = ["effects"]
answer_fields = ["name", "features"]
scope = "group"
enumerate_if_ambiguous = true
"#,
        )
        .unwrap();

        let mut a = Entity::new("麻黄", "herb", "win", 1);
        a.set("name", Value::Text("麻黄".into()), 1);
        a.set("group", Value::Text("发散风寒药".into()), 1);
        a.set("effects", Value::Tags(vec!["发汗解表".into(), "宣肺平喘".into()]), 1);
        a.set("features", Value::Tags(vec!["表实无汗".into()]), 1);
        let mut b = Entity::new("桂枝", "herb", "win", 1);
        b.set("name", Value::Text("桂枝".into()), 1);
        b.set("group", Value::Text("发散风寒药".into()), 1);
        b.set("effects", Value::Tags(vec!["发汗解表".into(), "温通经脉".into()]), 1);
        b.set("features", Value::Tags(vec!["表虚有汗".into()]), 1);
        let list = vec![a, b];
        let mut idx = InvertedIndex::default();
        idx.rebuild(&list, &schema);
        (schema, list, idx, HashMap::new())
    }

    #[test]
    fn unique_clue_needs_no_scope() {
        let (s, list, idx, g) = fixture();
        let cards = generate_cards(&s, &list[0], &g, &idx).unwrap();
        let c = cards.iter().find(|c| c.clue_term == "宣肺平喘").unwrap();
        assert!(c.candidates.is_empty());
        assert!(!c.enumerate);
    }

    #[test]
    fn ambiguous_clue_gets_scope_then_enumerates() {
        let (s, list, idx, g) = fixture();
        let cards = generate_cards(&s, &list[0], &g, &idx).unwrap();
        let c = cards.iter().find(|c| c.clue_term == "发汗解表").unwrap();
        assert_eq!(c.scope.as_deref(), Some("发散风寒药"));
        // 两味药同药群，加限定仍不唯一 -> 枚举题
        assert!(c.enumerate);
        assert!(c.prompt.contains("列出"));
    }

    #[test]
    fn judge_tolerates_punctuation_and_alias() {
        let (s, list, idx, g) = fixture();
        let card = generate_cards(&s, &list[0], &g, &idx)
            .unwrap()
            .into_iter()
            .find(|c| c.clue_term == "宣肺平喘")
            .unwrap();
        let aliases = TermTable::from_defs(&[]);
        assert!(judge("麻黄！", &card, &aliases, 0.85).correct);
        assert!(!judge("桂枝", &card, &aliases, 0.85).correct);
    }
}
