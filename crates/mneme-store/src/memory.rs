use crate::op::Op;
use mneme_core::{
    Card, DomainSchema, Entity, Group, InvertedIndex, ReviewEvent, TermAlias, generate_cards,
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// 全内存工作集。读多写少 -> 外层一个 RwLock；索引与卡片重建在锁外算好再换入。
#[derive(Debug, Default)]
pub struct Memory {
    pub schema_toml: String,
    pub schema: Option<DomainSchema>,
    pub entities: BTreeMap<String, Entity>,
    pub groups: BTreeMap<String, Group>,
    pub aliases: Vec<TermAlias>,
    pub events: Vec<ReviewEvent>,
    pub cards: Vec<Card>,
    pub index: InvertedIndex,
    pub rev: u64,
    pub device: String,
    /// 本次写入受影响的主数据 id。`"*"` 表示 schema 变了，必须全量重建。
    /// 增量派生靠它避免「改一条药，重生成全部 77×N 张卡」。
    pub dirty: HashSet<String>,
    seen_events: HashSet<String>,
    seen_ops: HashSet<String>,
}

impl Memory {
    /// 幂等应用。返回是否真的改变了状态。
    pub fn apply(&mut self, op: &Op) -> bool {
        let key = op.key();
        if !self.seen_ops.insert(key) {
            return false;
        }
        match op {
            Op::SetSchema { toml } => {
                let parsed = DomainSchema::from_toml(toml).ok();
                if parsed.is_none() {
                    return false; // 坏 schema 不覆盖已验证的
                }
                self.schema_toml = toml.clone();
                self.schema = parsed;
                self.dirty.insert("*".into());
                for id in self.entities.keys() {
                    self.dirty.insert(id.clone());
                }
                true
            }
            Op::UpsertEntity { entity } => {
                if let Some(old) = self.entities.get(&entity.id) {
                    if old.rev > entity.rev
                        || (old.rev == entity.rev && old.fingerprint() == entity.fingerprint())
                    {
                        return false;
                    }
                }
                self.rev = self.rev.max(entity.rev);
                self.entities.insert(entity.id.clone(), entity.clone());
                self.dirty.insert(entity.id.clone());
                true
            }
            Op::UpsertGroup { group } => {
                if let Some(old) = self.groups.get(&group.id) {
                    if old.rev > group.rev {
                        return false;
                    }
                }
                self.rev = self.rev.max(group.rev);
                self.groups.insert(group.id.clone(), group.clone());
                // 共性基座变化会影响引用它的所有实体的卡片
                for e in self.entities.values() {
                    if e.group_id.as_deref() == Some(group.id.as_str()) {
                        self.dirty.insert(e.id.clone());
                    }
                }
                true
            }
            Op::PutAlias { alias } => {
                self.aliases.retain(|a| a.concept != alias.concept);
                self.aliases.push(alias.clone());
                true
            }
            Op::Review { event } => {
                if !self.seen_events.insert(event.key()) {
                    return false;
                }
                self.events.push(event.clone());
                true // 复习事件不改主数据，无需派生
            }
            Op::Tombstone {
                id,
                rev,
                updated_at,
                device,
            } => match self.entities.get_mut(id) {
                Some(e) if e.rev <= *rev => {
                    e.deleted = true;
                    e.rev = *rev + 1;
                    e.updated_at = *updated_at;
                    e.device = device.clone();
                    self.rev = self.rev.max(*rev + 1);
                    self.dirty.insert(id.clone());
                    true
                }
                _ => false,
            },
        }
    }

    pub fn load_snapshot(&mut self, s: &crate::snapshot::Snapshot) {
        self.schema_toml = s.schema_toml.clone();
        self.schema = DomainSchema::from_toml(&s.schema_toml).ok();
        self.entities = s
            .entities
            .iter()
            .map(|e| (e.id.clone(), e.clone()))
            .collect();
        self.groups = s.groups.iter().map(|g| (g.id.clone(), g.clone())).collect();
        self.aliases = s.aliases.clone();
        self.events = s.events.clone();
        self.seen_events = self.events.iter().map(|e| e.key()).collect();
        self.rev = s.rev;
        self.cards.clear();
        self.dirty.clear();
    }

    pub fn entities_vec(&self) -> Vec<Entity> {
        self.entities.values().cloned().collect()
    }

    pub fn groups_map(&self) -> HashMap<String, Group> {
        self.groups.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// 全量派生（open / refresh 时用）。
    pub fn derive(&self) -> (InvertedIndex, Vec<Card>) {
        let entities = self.entities_vec();
        let mut idx = InvertedIndex::default();
        let mut cards = Vec::new();
        if let Some(schema) = &self.schema {
            idx.rebuild(&entities, schema);
            cards = self.cards_for(schema, &entities, &idx, None);
        }
        (idx, cards)
    }

    /// 增量派生：只重生成脏实体的卡片，其余原样保留。
    /// 索引仍全量重建（10k 条仅 ~26 ms，且避免维护 posting 删除的复杂度）。
    pub fn derive_incremental(
        &self,
        old_cards: &[Card],
        dirty: &HashSet<String>,
    ) -> (InvertedIndex, Vec<Card>) {
        if dirty.contains("*") {
            return self.derive();
        }
        let entities = self.entities_vec();
        let mut idx = InvertedIndex::default();
        let Some(schema) = &self.schema else {
            return (idx, Vec::new());
        };
        idx.rebuild(&entities, schema);
        let mut cards: Vec<Card> = old_cards
            .iter()
            .filter(|c| !dirty.contains(&c.entity_id))
            .cloned()
            .collect();
        cards.append(&mut self.cards_for(schema, &entities, &idx, Some(dirty)));
        cards.sort_by(|a, b| a.id.cmp(&b.id));
        cards.dedup_by(|a, b| a.id == b.id);
        (idx, cards)
    }

    fn cards_for(
        &self,
        schema: &DomainSchema,
        entities: &[Entity],
        idx: &InvertedIndex,
        only: Option<&HashSet<String>>,
    ) -> Vec<Card> {
        let groups = self.groups_map();
        let mut cards = Vec::new();
        for e in entities.iter().filter(|e| !e.deleted) {
            if only.is_some_and(|set| !set.contains(&e.id)) {
                continue;
            }
            if let Ok(mut cs) = generate_cards(schema, e, &groups, idx) {
                cards.append(&mut cs);
            }
        }
        cards.sort_by(|a, b| a.id.cmp(&b.id));
        cards.dedup_by(|a, b| a.id == b.id);
        cards
    }

    pub fn stats(&self) -> super::Stats {
        super::Stats {
            entities: self.entities.values().filter(|e| !e.deleted).count(),
            groups: self.groups.len(),
            cards: self.cards.len(),
            events: self.events.len(),
            rev: self.rev,
            device: self.device.clone(),
        }
    }
}
