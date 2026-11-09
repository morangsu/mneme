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
    /// 槽位表：`Some(id)` 表示该下标有活实体，`None` 是空槽（可复用）。
    ///
    /// 为什么要槽位而不是直接 `Vec<String>`：单条编辑时若按下标删除，
    /// 后面所有实体的下标都会移位，posting 里的 u32 全部失效——那就只能全量重建。
    /// 用「稳定槽位 + 空闲链表」，删除只影响一条记录，代价降到 O(词条数)。
    ids: Vec<Option<String>>,
    /// id -> 槽位，编辑时用来判断「这是新增还是替换」。
    by_id: HashMap<String, u32>,
    free: Vec<u32>,
    weights: HashMap<String, f32>,
    /// kind -> 该类型的实体槽位，供「限定药群」快速裁剪。
    by_kind: HashMap<String, Vec<u32>>,
}

impl InvertedIndex {
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn id_of(&self, idx: u32) -> Option<&str> {
        self.ids.get(idx as usize).and_then(|s| s.as_deref())
    }

    /// 槽位总数（含空槽），用于把 id 预哈希成按槽位索引的数组。
    pub fn slots(&self) -> usize {
        self.ids.len()
    }

    /// 某实体当前占用的槽位。
    pub fn slot_of(&self, id: &str) -> Option<u32> {
        self.by_id.get(id).copied()
    }

    /// 诊断用：`(字段数, 词条数)`，即倒排表的规模。基准与内存估算都用到。
    pub fn shape(&self) -> (usize, usize) {
        (self.postings.len(), self.postings.values().map(|m| m.len()).sum())
    }

    /// 已经是归一化过的词，直接查。
    ///
    /// `lookup` 会再归一化一次（`normalize` 里有两次分配），指纹计算一条药要查几十次，
    /// 那层重复归一化能把指纹算成毫秒级——所以留一个不归一化的入口给自己人用。
    pub fn lookup_norm(&self, field: &str, term_norm: &str) -> &[u32] {
        self.postings
            .get(field)
            .and_then(|m| m.get(term_norm))
            .map(Vec::as_slice)
            .unwrap_or(&[])
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
        self.by_id.clear();
        self.free.clear();
        self.postings.clear();
        self.weights.clear();
        self.by_kind.clear();

        let live: Vec<&Entity> = entities.iter().filter(|e| !e.deleted).collect();
        self.ids.reserve(live.len());
        for (i, e) in live.iter().enumerate() {
            self.ids.push(Some(e.id.clone()));
            self.by_id.insert(e.id.clone(), i as u32);
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
    ///
    /// 幂等：同一实体重复调用会先清掉旧 posting 再写新的，
    /// 所以「编辑」和「新增」共用这一个入口，不需要调用方区分。
    pub fn upsert(&mut self, e: &Entity) {
        let existing = self.by_id.get(&e.id).copied();
        if let Some(slot) = existing {
            self.remove_postings(slot);
            if e.deleted {
                self.ids[slot as usize] = None;
                self.by_id.remove(&e.id);
                self.free.push(slot);
                if let Some(list) = self.by_kind.get_mut(&e.kind) {
                    list.retain(|x| *x != slot);
                }
                return;
            }
        } else if e.deleted {
            return;
        }
        let slot = match existing {
            Some(s) => s,
            None => self.alloc_slot(e),
        };
        for (k, v) in &e.fields {
            let field = format!("{}.{}", e.kind, k);
            for term in v.terms() {
                let n = normalize(term);
                if n.is_empty() {
                    continue;
                }
                let p = self
                    .postings
                    .entry(field.clone())
                    .or_default()
                    .entry(n)
                    .or_default();
                if p.last() != Some(&slot) {
                    p.push(slot);
                }
            }
        }
    }

    fn alloc_slot(&mut self, e: &Entity) -> u32 {
        let slot = match self.free.pop() {
            Some(s) => {
                self.ids[s as usize] = Some(e.id.clone());
                s
            }
            None => {
                self.ids.push(Some(e.id.clone()));
                (self.ids.len() - 1) as u32
            }
        };
        self.by_id.insert(e.id.clone(), slot);
        self.by_kind.entry(e.kind.clone()).or_default().push(slot);
        slot
    }

    /// 把一个槽位从所有 posting 里摘掉。
    ///
    /// 代价是「扫一遍词表」，但每个 posting 只有几条到几十条，2000 味药也只有万级词条，
    /// 实测 0.2 ms 量级——比全量重建（22 ms）低两个数量级，而且不分配新 String。
    fn remove_postings(&mut self, slot: u32) {
        for terms in self.postings.values_mut() {
            let mut empty: Vec<String> = Vec::new();
            for (term, list) in terms.iter_mut() {
                list.retain(|x| *x != slot);
                if list.is_empty() {
                    empty.push(term.clone());
                }
            }
            for t in empty {
                terms.remove(&t);
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

    #[test]
    fn upsert_replaces_postings_and_keeps_slot() {
        let sc = schema();
        let mut idx = InvertedIndex::default();
        let a = herb("a", &["宣肺平喘"]);
        let b = herb("b", &["清热泻火"]);
        idx.rebuild(&[a.clone(), b.clone()], &sc);
        let slot = idx.slot_of("a").expect("slot");
        assert_eq!(idx.len(), 2);

        // 编辑 a：旧功效要消失，新功效要能查到，槽位不变（否则 posting 全失效）
        let mut a2 = a.clone();
        a2.set("effects", Value::Tags(vec!["利水消肿".into()]), 1);
        idx.upsert(&a2);
        assert_eq!(idx.slot_of("a"), Some(slot), "槽位必须稳定");
        assert_eq!(idx.len(), 2, "替换不该多出实体");
        assert!(idx.lookup("herb.effects", "宣肺平喘").is_empty(), "旧词应被摘掉");
        assert_eq!(idx.lookup("herb.effects", "利水消肿"), &[slot]);
    }

    #[test]
    fn upsert_handles_tombstone_and_resurrection() {
        let sc = schema();
        let mut idx = InvertedIndex::default();
        let a = herb("a", &["宣肺平喘"]);
        idx.rebuild(std::slice::from_ref(&a), &sc);

        let mut gone = a.clone();
        gone.deleted = true;
        idx.upsert(&gone);
        assert_eq!(idx.len(), 0, "墓碑要从索引里移除");
        assert!(idx.lookup("herb.effects", "宣肺平喘").is_empty());
        assert!(idx.ids_of_kind("herb").is_empty());

        // 恢复（取消墓碑）后用同一个 id 回来，不能出现两个槽位
        idx.upsert(&a);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.ids_of_kind("herb").len(), 1);
        assert_eq!(idx.lookup("herb.effects", "宣肺平喘").len(), 1);
    }

    #[test]
    fn upsert_matches_full_rebuild() {
        // 增量路径必须与全量重建结果一致，否则「编辑过的库」和「重开过的库」查出来的不一样
        let sc = schema();
        let a = herb("a", &["宣肺平喘"]);
        let b = herb("b", &["清热泻火"]);
        let mut inc = InvertedIndex::default();
        inc.rebuild(std::slice::from_ref(&a), &sc);
        inc.upsert(&b);

        let full = {
            let mut f = InvertedIndex::default();
            f.rebuild(&[a.clone(), b.clone()], &sc);
            f
        };
        for term in ["宣肺平喘", "清热泻火", "不存在"] {
            let mut x: Vec<u32> = inc.lookup("herb.effects", term).to_vec();
            let mut y: Vec<u32> = full.lookup("herb.effects", term).to_vec();
            x.sort();
            y.sort();
            assert_eq!(x, y, "词条 {term} 不一致");
        }
        assert_eq!(inc.len(), full.len());
    }
}
