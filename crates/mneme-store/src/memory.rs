use crate::op::Op;
use mneme_core::{
    Card, DomainSchema, Entity, Group, InvertedIndex, ReviewEvent, TermAlias, generate_cards,
    text::normalize,
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
    /// 实体 id -> 该实体的全部卡（块内按卡 id 升序）。
    ///
    /// 为什么按实体分块而不是一个大 `Vec<Card>`：编辑一条药只影响它自己的卡，
    /// 分块后替换是 O(log n)，而扁平表要么整表克隆、要么全表排序去重——
    /// 那正是「改一个字要 290 ms」的来源。
    pub cards: BTreeMap<String, Vec<Card>>,
    /// card_id -> entity_id，按卡 id 取卡时不用扫全表。
    pub card_owner: HashMap<String, String>,
    /// 卡片总数（分块后 `cards.len()` 变成块数，别让它冒充卡数）。
    ///
    /// 懒加载时这个数可以是「上次落盘时的值」，不必等卡片真的装进来——
    /// 矩阵页顶部只显示数量，没必要为了一个数字去解码 34k 张卡。
    cards_total: usize,
    /// 卡片是否已真正装入内存。`false` 时 `cards` 是空表，
    /// 任何需要卡片内容的路径都必须先 `Store::ensure_cards()`。
    pub cards_loaded: bool,
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
        self.card_owner.clear();
        self.cards_total = 0;
        self.dirty.clear();
    }

    pub fn entities_vec(&self) -> Vec<Entity> {
        self.entities.values().cloned().collect()
    }

    pub fn groups_map(&self) -> HashMap<String, Group> {
        self.groups.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// 只重建倒排索引（并行）。卡片依赖它，所以 open 的第一步就是它。
    pub fn rebuild_index(&mut self) {
        let entities = self.entities_vec();
        self.index = InvertedIndex::default();
        if let Some(schema) = self.schema.clone() {
            self.index.rebuild(&entities, &schema);
        }
    }

    /// 只重建全部卡片块（索引需已就绪；缓存整体失效时用）。
    pub fn rebuild_cards(&mut self) {
        self.cards.clear();
        self.card_owner.clear();
        self.cards_total = 0;
        let Some(schema) = self.schema.clone() else {
            return;
        };
        let groups = self.groups_map();
        let entities: Vec<Entity> = self.entities.values().filter(|e| !e.deleted).cloned().collect();
        let mut blocks: Vec<(String, Vec<Card>)> = Vec::with_capacity(entities.len());
        for e in &entities {
            let mut cs = generate_cards(&schema, e, &groups, &self.index).unwrap_or_default();
            cs.sort_by(|a, b| a.id.cmp(&b.id));
            cs.dedup_by(|a, b| a.id == b.id);
            blocks.push((e.id.clone(), cs));
        }
        for (id, cs) in blocks {
            self.put_block(&id, cs);
        }
    }

    /// 全量重建派生数据（schema 变更、批量导入后用）。
    pub fn rebuild_derived(&mut self) {
        self.rebuild_index();
        self.rebuild_cards();
        self.mark_cards_loaded();
    }

    /// 只重建脏实体的索引与卡片。
    ///
    /// 前提：调用方保证 `dirty` 不含 `"*"` 且规模不大（Store 里超过阈值会退回全量）。
    pub fn apply_derived(&mut self, dirty: &HashSet<String>) {
        let Some(schema) = self.schema.clone() else {
            return;
        };
        let groups = self.groups_map();
        let mut updates: Vec<(String, Vec<Card>)> = Vec::with_capacity(dirty.len());
        for id in dirty {
            match self.entities.get(id).cloned() {
                Some(e) if !e.deleted => {
                    // 先改索引再生成卡片：卡片里的「这条线索是否唯一」要基于最新索引
                    self.index.upsert(&e);
                    let mut cs =
                        generate_cards(&schema, &e, &groups, &self.index).unwrap_or_default();
                    cs.sort_by(|a, b| a.id.cmp(&b.id));
                    cs.dedup_by(|a, b| a.id == b.id);
                    updates.push((id.clone(), cs));
                }
                Some(e) => {
                    self.index.upsert(&e); // 墓碑：从索引里摘掉
                    updates.push((id.clone(), Vec::new()));
                }
                None => updates.push((id.clone(), Vec::new())),
            }
        }
        for (id, cs) in updates {
            self.put_block(&id, cs);
        }
    }

    /// 换掉某个实体的整块卡片，同时维护 owner 映射与总数。
    fn put_block(&mut self, entity_id: &str, cards: Vec<Card>) {
        if let Some(old) = self.cards.remove(entity_id) {
            self.cards_total = self.cards_total.saturating_sub(old.len());
            for c in &old {
                if self.card_owner.get(&c.id).map(String::as_str) == Some(entity_id) {
                    self.card_owner.remove(&c.id);
                }
            }
        }
        if cards.is_empty() {
            return;
        }
        for c in &cards {
            self.card_owner.insert(c.id.clone(), entity_id.to_string());
        }
        self.cards_total += cards.len();
        self.cards.insert(entity_id.to_string(), cards);
    }

    /// 用缓存里的块替换（指纹匹配才调用）。
    pub fn adopt_blocks(&mut self, blocks: Vec<(String, Vec<Card>)>) {
        for (id, cs) in blocks {
            self.put_block(&id, cs);
        }
    }

    pub fn cards_len(&self) -> usize {
        self.cards_total
    }

    /// 启动时用上次落盘的总数占位（见 `cards_total` 的注释）。
    pub fn set_cards_total(&mut self, n: usize) {
        self.cards_total = n;
    }

    /// 装载结束：以实际内容为准修正总数并标记已装载。
    pub fn mark_cards_loaded(&mut self) {
        self.cards_total = self.cards.values().map(|v| v.len()).sum();
        self.cards_loaded = true;
    }

    /// 扁平化（导出、出题队列等读路径用）。克隆一次，调用方只在需要 `&[Card]` 时用。
    pub fn cards_flat(&self) -> Vec<Card> {
        let mut v = Vec::with_capacity(self.cards_total);
        for block in self.cards.values() {
            v.extend(block.iter().cloned());
        }
        v
    }

    pub fn find_card(&self, id: &str) -> Option<&Card> {
        let owner = self.card_owner.get(id)?;
        self.cards.get(owner)?.iter().find(|c| c.id == id)
    }

    /// 全部实体的卡片块指纹（一次批量算，见下）。
    ///
    /// 依赖不只是「我自己变了没」，还包括**同线索词的实体变了没**：
    /// 「具有『宣肺平喘』的是？」在只有麻黄时是一道唯一答案题，多了一味药就该变成
    /// 「列出并说差异」。所以把每个线索词 posting 的规模与成员一起混进指纹——
    /// 依赖变了才重算这一块。
    ///
    /// 批量算是因为单条算要重复做三件事：拼 `kind.field`、归一化同一个术语、
    /// 反查 posting。2000 条实体逐条算要 130 ms，批量带缓存后约 7 ms。
    pub fn block_fps(&self) -> HashMap<String, u64> {
        let mut out = HashMap::with_capacity(self.entities.len());
        let Some(schema) = self.schema.as_ref() else {
            return out;
        };
        // kind.field 只与 (kind, field) 有关，缓存起来
        let mut field_keys: HashMap<(&str, &str), String> = HashMap::new();
        // 同一个术语（「发汗解表」这类）会被几十条药共用，归一化结果缓存下来
        let mut norm_cache: HashMap<&str, String> = HashMap::new();
        // 成员指纹按槽位预哈希：一个常见词条的 posting 可能有上百条，
        // 每条都去 hash 一次 id 字符串，2000 味药就是千万级字符哈希。
        let slot_fp: Vec<u64> = (0..self.index.slots())
            .map(|i| {
                self.index
                    .id_of(i as u32)
                    .map(|id| fnv1a(id.as_bytes()).rotate_left(17))
                    .unwrap_or(0)
            })
            .collect();

        for (id, e) in &self.entities {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            h = mix(h, fnv1a(id.as_bytes()));
            h = mix(h, e.rev);
            h = mix(h, e.deleted as u64);
            h = mix(h, fnv1a(e.group_id.as_deref().unwrap_or("").as_bytes()));
            if let Some(g) = e.group_id.as_deref().and_then(|g| self.groups.get(g)) {
                h = mix(h, g.rev);
                h = mix(h, fnv1a(g.name.as_bytes()));
            }
            if let Some(def) = schema.entity_def(&e.kind) {
                for d in &def.directions {
                    for f in &d.clue_fields {
                        let value = e.get(f).or_else(|| {
                            e.group_id
                                .as_deref()
                                .and_then(|g| self.groups.get(g))
                                .and_then(|g| g.common.get(f))
                        });
                        let Some(v) = value else { continue };
                        h = mix(h, fnv1a(f.as_bytes()));
                        let field = field_keys
                            .entry((e.kind.as_str(), f.as_str()))
                            .or_insert_with(|| format!("{}.{}", e.kind, f))
                            .clone();
                        for t in v.terms() {
                            let n = match norm_cache.get(t) {
                                Some(n) => n.clone(),
                                None => {
                                    let n = normalize(t);
                                    norm_cache.insert(t, n.clone());
                                    n
                                }
                            };
                            if n.is_empty() {
                                continue;
                            }
                            let posting = self.index.lookup_norm(&field, &n);
                            h = mix(h, posting.len() as u64);
                            // 成员的交换和：只要「同线索的还有谁」变了就变，顺序无关。
                            // 刻意不掺 rev —— 别人改了卡片不引用的字段时，这里的卡不该失效。
                            let mut x = 0u64;
                            for slot in posting {
                                x ^= slot_fp[*slot as usize];
                            }
                            h = mix(h, x);
                        }
                    }
                }
            }
            out.insert(id.clone(), h);
        }
        out
    }

    /// 领域包指纹：换领域包必须整份重算（字段、方向、题面模板都变了）。
    pub fn schema_fp(&self) -> u64 {
        fnv1a(self.schema_toml.as_bytes())
    }

    pub fn stats(&self) -> super::Stats {
        super::Stats {
            entities: self.entities.values().filter(|e| !e.deleted).count(),
            groups: self.groups.len(),
            cards: self.cards_total,
            events: self.events.len(),
            rev: self.rev,
            device: self.device.clone(),
        }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn mix(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100_0000_01b3)
}
