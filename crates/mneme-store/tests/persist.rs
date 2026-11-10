//! 持久化与增量派生的一致性测试。
//!
//! 这一组的存在理由：为了把冷启动从 3 秒压到 0.2 秒，卡片改成了「按实体分块 + 落盘缓存 +
//! 依赖指纹校验」，SQLite 镜像改成了「按 rev 水位补差异」。这些都是**性能优化**，
//! 一旦和「全量重算」结果不一致，就会表现成「编辑过的库和重开过的库不一样」——
//! 最难查的一类 bug。所以这里做的是等价性断言。

use mneme_core::{DomainSchema, Entity, Value};
use mneme_store::{Op, Store};
use std::collections::BTreeSet;
use std::path::PathBuf;

const SCHEMA: &str = r#"
id = "test"
name = "测试"
[[entities]]
kind = "herb"
primary = "name"
[[entities.fields]]
key = "name"
label = "药名"
kind = "text"
indexable = true
[[entities.fields]]
key = "effects"
label = "功效"
kind = "tags"
indexable = true
reversible = true
[[entities.fields]]
key = "nature"
label = "性味"
kind = "text"
[[entities.directions]]
id = "effect_to_name"
clue_fields = ["effects"]
answer_fields = ["name"]
"#;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mneme-persist-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn herb(id: &str, effects: &[&str], nature: &str) -> Entity {
    let mut e = Entity::new(id, "herb", "t", 1);
    e.set("name", Value::Text(id.into()), 1);
    e.set(
        "effects",
        Value::Tags(effects.iter().map(|s| s.to_string()).collect()),
        1,
    );
    e.set("nature", Value::Text(nature.into()), 1);
    e
}

fn seed(dir: &PathBuf) {
    let store = Store::open(dir, "t").unwrap();
    let ops = vec![
        Op::SetSchema {
            toml: SCHEMA.into(),
        },
        Op::UpsertEntity {
            entity: herb("麻黄", &["发汗解表", "宣肺平喘"], "辛温"),
        },
        Op::UpsertEntity {
            entity: herb("桂枝", &["发汗解肌"], "辛甘温"),
        },
        Op::UpsertEntity {
            entity: herb("石膏", &["清热泻火"], "甘寒"),
        },
    ];
    store.mutate(&ops).unwrap();
}

fn card_ids(store: &Store) -> BTreeSet<String> {
    store.read(|m| m.cards_flat().into_iter().map(|c| c.id).collect())
}

#[test]
fn cards_survive_reopen_and_cache_is_used() {
    let dir = tmp("reopen");
    seed(&dir);
    let (first, _) = Store::open_timed(&dir, "t").unwrap();
    first.ensure_cards().unwrap();
    let n1 = first.stats().cards;
    let ids1 = card_ids(&first);
    assert!(n1 > 0, "应该有卡片");
    drop(first);

    assert!(dir.join("cards.bin").exists(), "首次打开应写出卡片缓存");
    let (second, timing) = Store::open_timed(&dir, "t").unwrap();
    second.ensure_cards().unwrap();
    assert_eq!(second.stats().cards, n1, "重开后卡数必须一致");
    assert_eq!(card_ids(&second), ids1, "重开后卡 id 集合必须一致");
    assert!(
        timing.cards_cache.as_secs_f64() < 1.0,
        "卡片缓存阶段不该退回全量生成：{:?}",
        timing.cards_cache
    );
}

#[test]
fn editing_one_entity_only_rebuilds_its_cards() {
    let dir = tmp("edit");
    seed(&dir);
    let (store, _) = Store::open_timed(&dir, "t").unwrap();
    store.ensure_cards().unwrap();
    let before = card_ids(&store);
    let guizhi_before: Vec<_> = store.read(|m| m.cards.get("桂枝").cloned().unwrap_or_default());
    let mahuang_before = store.read(|m| m.cards.get("麻黄").cloned().unwrap_or_default());
    assert!(!mahuang_before.is_empty());

    // 改麻黄的功效：它自己的卡要变，桂枝的卡要一字不动
    let mut e = store.read(|m| m.entities.get("麻黄").cloned().unwrap());
    // 注意：`set` 会自动 rev+1；这里不能再手工把 rev 调小，否则会被 `apply`
    // 当成过期写入直接丢掉（old.rev > new.rev），测试就成了空跑
    e.set("effects", Value::Tags(vec!["利水消肿".into()]), 2);
    store.mutate(&[Op::UpsertEntity { entity: e }]).unwrap();

    let after = card_ids(&store);
    let guizhi_after = store.read(|m| m.cards.get("桂枝").cloned().unwrap_or_default());
    assert_eq!(guizhi_before, guizhi_after, "别人的卡片不该被牵连重算");
    assert_ne!(before, after, "改过的那条卡 id 应该变（线索词变了）");

    // 索引也要跟着走：新词能查到，旧词查不到
    store.read(|m| {
        assert_eq!(
            m.index.lookup("herb.effects", "利水消肿").len(),
            1,
            "新功效应有命中"
        );
        assert!(
            m.index.lookup("herb.effects", "宣肺平喘").is_empty(),
            "旧功效应失效"
        );
    });

    // 重开之后结果必须一致（缓存不能把旧卡留下来）
    drop(store);
    let (again, _) = Store::open_timed(&dir, "t").unwrap();
    again.ensure_cards().unwrap();
    assert_eq!(card_ids(&again), after, "重开后卡片应与编辑后完全一致");
    assert!(again.read(|m| m.find_card("不存在").is_none()));
}

#[test]
fn incremental_index_matches_full_rebuild_after_edits() {
    let dir = tmp("index");
    seed(&dir);
    let store = Store::open_timed(&dir, "t").unwrap().0;
    store.ensure_cards().unwrap();
    let mut e = store.read(|m| m.entities.get("桂枝").cloned().unwrap());
    e.set("effects", Value::Tags(vec!["温通经脉".into()]), 2);
    store.mutate(&[Op::UpsertEntity { entity: e }]).unwrap();

    // 增量维护出来的索引 vs 从内存态全量重建的索引
    let (inc, entities, schema) = store.read(|m| {
        let mut inc = Vec::new();
        for term in ["宣肺平喘", "温通经脉", "发汗解肌", "清热泻火"] {
            inc.push((term, m.index.lookup("herb.effects", term).to_vec()));
        }
        (inc, m.entities_vec(), m.schema.clone().unwrap())
    });
    let mut fresh = mneme_core::InvertedIndex::default();
    fresh.rebuild(&entities, &schema);
    for (term, mut got) in inc {
        let mut want = fresh.lookup("herb.effects", term).to_vec();
        got.sort();
        want.sort();
        assert_eq!(got, want, "词条 {term} 的 posting 与全量重建不一致");
    }
}

#[test]
fn sqlite_mirror_is_idempotent_across_reopens() {
    let dir = tmp("mirror");
    seed(&dir);
    let (store, _) = Store::open_timed(&dir, "t").unwrap();
    let counts1 = store.sqlite().expect("mirror").counts().unwrap();
    drop(store);

    let (store, _) = Store::open_timed(&dir, "t").unwrap();
    let counts2 = store.sqlite().expect("mirror").counts().unwrap();
    assert_eq!(counts1.entities, counts2.entities, "镜像不应重复写入");
    assert_eq!(counts1.cards, counts2.cards);
    assert!(store.sqlite().unwrap().integrity_check().unwrap());
}

#[test]
fn cards_rebuild_when_cache_is_missing() {
    let dir = tmp("nocache");
    seed(&dir);
    let (store, _) = Store::open_timed(&dir, "t").unwrap();
    store.ensure_cards().unwrap();
    let expected = store.stats().cards;
    drop(store);
    let _ = std::fs::remove_file(dir.join("cards.bin"));

    let (store, _) = Store::open_timed(&dir, "t").unwrap();
    store.ensure_cards().unwrap();
    assert_eq!(store.stats().cards, expected, "无缓存时应重建出同样多的卡");
    assert!(dir.join("cards.bin").exists(), "重建后应重新写出缓存");
    let schema: DomainSchema = store.read(|m| m.schema.clone()).expect("schema");
    assert_eq!(schema.id, "test");
}

#[test]
fn cards_are_lazy_but_count_is_accurate() {
    let dir = tmp("lazy");
    seed(&dir);
    // 先跑一次完整装载，让 cards_n 水位落进镜像
    {
        let (s, _) = Store::open_timed(&dir, "t").unwrap();
        s.ensure_cards().unwrap();
        assert!(s.stats().cards > 0);
    }

    let (s, _) = Store::open_timed(&dir, "t").unwrap();
    // 打开时不该装载卡片：矩阵页只要数字
    assert!(!s.read(|m| m.cards_loaded), "open 不该装卡");
    assert!(s.read(|m| m.cards.is_empty()), "内存里不该有卡片块");
    assert!(s.stats().cards > 0, "但卡片总数要靠镜像水位给出（不能显示 0）");

    s.ensure_cards().unwrap();
    assert!(s.read(|m| m.cards_loaded));
    assert!(!s.read(|m| m.cards.is_empty()));
    assert_eq!(s.stats().cards, s.read(|m| m.cards_flat().len()));
}

#[test]
fn mutate_loads_cards_first() {
    // 写路径要替换卡片块，所以必须先装载——不能出现「编辑后卡片统计变成 0」
    let dir = tmp("mutate");
    seed(&dir);
    let (s, _) = Store::open_timed(&dir, "t").unwrap();
    assert!(!s.read(|m| m.cards_loaded));

    let mut e = s.read(|m| m.entities.get("石膏").cloned().unwrap());
    e.set("effects", Value::Tags(vec!["除烦止渴".into()]), 5);
    s.mutate(&[Op::UpsertEntity { entity: e }]).unwrap();

    assert!(s.read(|m| m.cards_loaded), "写之前应自动装载");
    assert!(s.stats().cards > 0);
    assert_eq!(
        s.sqlite().unwrap().counts().unwrap().cards,
        s.stats().cards,
        "镜像卡片数应与内存一致"
    );
}

#[test]
fn opening_without_cards_does_not_wipe_mirror_cards() {
    // 懒加载最容易踩的坑：open 时卡片是空的，如果照常写镜像就会把 cards 表清空
    let dir = tmp("keepcards");
    seed(&dir);
    let before = {
        let (s, _) = Store::open_timed(&dir, "t").unwrap();
        s.ensure_cards().unwrap();
        s.sqlite().unwrap().counts().unwrap().cards
    };
    assert!(before > 0);

    // 再打开一次（不装卡），然后强制同步镜像
    let (s, _) = Store::open_timed(&dir, "t").unwrap();
    assert!(!s.read(|m| m.cards_loaded));
    s.sync_sqlite().unwrap();
    assert_eq!(
        s.sqlite().unwrap().counts().unwrap().cards,
        before,
        "不装卡时同步镜像不能删掉 cards"
    );
}
