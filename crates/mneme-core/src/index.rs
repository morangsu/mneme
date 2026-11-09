use crate::entity::Entity;
use crate::schema::DomainSchema;
use crate::text::normalize;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub idx: u32,
    pub score: f32,
    pub hits: u32,
}

/// 倒排索引。
///
/// 数据布局上的两个性能决定（性能清单 §4「缓存友好 / 减少分配」）：
/// 1. postings 存 `Vec<u32>` 下标而不是 `HashSet<String>`，实体 id 单独放 `ids: Vec<String>`，
///    交集运算全程在紧凑整数数组上做，避免指针追逐和字符串哈希；
/// 2. 构建阶段用 Rayon 并行做归一化/分词，合并进 HashMap 串行执行，
///    避免给热路径加锁（细粒度读写由 store 层的分片 RwLock 负责）。
#[derive(Debug, Default)]
pub struct InvertedIndex {
    postings: HashMap<String, HashMap<String, Vec<u32>>>,
    ids: Vec<String>,
    weights: HashMap<String, f32>,
    /// kind -> 该类型的实体在 ids 中的下标，供「限定药群」快速裁剪。
    by_kind: HashMap<String, Vec<u32>>,
}

impl InvertedIndex {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn id_of(&self, idx: u32) -> Option<&str> {
        self.ids.get(idx as usize).map(String::as_str)
    }

    pub fn lookup(&self, field: &str, term: &str) -> &[u32] {
        self.postings
            .get(field)
            .and_then(|m| m.get(&normalize(term)))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// 全量重建。批量导入后调用一次，比逐条维护更省。
    pub fn rebuild(&mut self, entities: &[Entity], schema: &DomainSchema) {
        self.ids.clear();
        self.postings.clear();
        self.weights.clear();
        self.by_kind.clear();

        let live: Vec<&Entity> = entities.iter().filter(|e| !e.deleted).collect();
        self.ids.reserve(live.len());
        for (i, e) in live.iter().enumerate() {
            self.ids.push(e.id.clone());
            self.by_kind.entry(e.kind.clone()).or_default().push(i as u32);
        }
        for def in &schema.entities {
            for f in def.fields.iter().filter(|f| f.indexable) {
                self.weights.insert(format!("{}.{}", def.kind, f.key), f.weight);
            }
        }

        type Triple = (String, String, u32);
        let triples: Vec<Triple> = live
            .par_iter()
            .enumerate()
            .map(|(i, e)| {
                let mut out: Vec<Triple> = Vec::with_capacity(e.fields.len() * 2);
                for (k, v) in &e.fields {
                    let field = format!("{}.{}", e.kind, k);
                    for term in v.terms() {
                        let n = normalize(term);
                        if !n.is_empty() {
                            out.push((field.clone(), n, i as u32));
                        }
                    }
                }
                out
            })
            .reduce(Vec::new, |mut acc, mut part| {
                acc.append(&mut part);
                acc
            });

        for (field, term, idx) in triples {
            let p = self.postings.entry(field).or_default().entry(term).or_default();
            if p.last() != Some(&idx) {
                p.push(idx);
            }
        }
    }

    /// 单条写入的增量路径（避免每次编辑都全量重建）。
    pub fn add(&mut self, e: &Entity) {
        if e.deleted {
            return;
        }
        let idx = self.ids.len() as u32;
        self.ids.push(e.id.clone());
        self.by_kind.entry(e.kind.clone()).or_default().push(idx);
        for (k, v) in &e.fields {
            let field = format!("{}.{}", e.kind, k);
            for term in v.terms() {
                let n = normalize(term);
                if !n.is_empty() {
                    self.postings
                        .entry(field.clone())
                        .or_default()
                        .entry(n)
                        .or_default()
                        .push(idx);
                }
            }
        }
    }

    /// 多线索 AND 反查：给「功效=宣肺平喘」「禁忌=表虚自汗」-> 候选实体。
    /// 从最短 posting 起步做交集，按字段权重打分。
    pub fn reverse(&self, clues: &[(String, String)]) -> Vec<Scored> {
        if clues.is_empty() {
            return vec![];
        }
        let mut lists: Vec<(&[u32], f32)> = Vec::with_capacity(clues.len());
        for (field, term) in clues {
            let p = self.lookup(field, term);
            if p.is_empty() {
                return vec![]; // 严格 AND：一个线索无命中即无解
            }
            lists.push((p, self.weights.get(field).copied().unwrap_or(1.0)));
        }
        lists.sort_by_key(|(p, _)| p.len());

        let mut acc: HashMap<u32, (f32, u32)> = HashMap::with_capacity(lists[0].0.len());
        for &i in lists[0].0 {
            acc.insert(i, (lists[0].1, 1));
        }
        for (p, w) in lists.iter().skip(1) {
            let set: HashSet<u32> = p.iter().copied().collect();
            acc.retain(|k, _| set.contains(k));
            for (s, h) in acc.values_mut() {
                *s += w;
                *h += 1;
            }
        }
        let mut out: Vec<Scored> = acc
            .into_iter()
            .map(|(idx, (score, hits))| Scored { idx, score, hits })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.idx.cmp(&b.idx)));
        out
    }

    /// 限定在某个 kind（药群/篇章）内反查，用于出题唯一性收敛。
    pub fn ids_of_kind(&self, kind: &str) -> &[u32] {
        self.by_kind.get(kind).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Value;

    fn herb(id: &str, effects: &[&str]) -> Entity {
        let mut e = Entity::new(id, "herb", "win", 1);
        e.set(
            "name",
            Value::Text(id.to_string()),
            1,
        );
        e.set(
            "effects",
            Value::Tags(effects.iter().map(|s| s.to_string()).collect()),
            1,
        );
        e
    }

    fn schema() -> DomainSchema {
        crate::schema::DomainSchema::from_toml(
            r#"
id = "test"
name = "测试"
[[entities]]
kind = "herb"
primary = "name"
[[entities.fields]]
key = "name"
kind = "text"
[[entities.fields]]
key = "effects"
kind = "tags"
indexable = true
reversible = true
"#,
        )
        .unwrap()
    }

    #[test]
    fn reverse_is_order_independent() {
        let list = vec![
            herb("麻黄", &["发汗解表", "宣肺平喘", "利水消肿"]),
            herb("桂枝", &["发汗解表", "温通经脉"]),
        ];
        let mut idx = InvertedIndex::default();
        idx.rebuild(&list, &schema());
        let got = idx.reverse(&[("herb.effects".into(), "宣肺平喘".into())]);
        assert_eq!(got.len(), 1);
        assert_eq!(idx.id_of(got[0].idx), Some("麻黄"));
        let both = idx.reverse(&[("herb.effects".into(), "发汗解表".into())]);
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn multi_clue_intersection() {
        let list = vec![
            herb("麻黄", &["发汗解表", "宣肺平喘"]),
            herb("桂枝", &["发汗解表", "温通经脉"]),
        ];
        let mut idx = InvertedIndex::default();
        idx.rebuild(&list, &schema());
        let got = idx.reverse(&[
            ("herb.effects".into(), "发汗解表".into()),
            ("herb.effects".into(), "宣肺平喘".into()),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(idx.id_of(got[0].idx), Some("麻黄"));
    }

    #[test]
    fn fullwidth_term_hits() {
        let list = vec![herb("麻黄", &["宣肺平喘"])];
        let mut idx = InvertedIndex::default();
        idx.rebuild(&list, &schema());
        assert_eq!(idx.lookup("herb.effects", "宣肺平喘！").len(), 1);
    }
}
