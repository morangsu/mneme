use mneme_core::{
    DomainSchema, Entity, InvertedIndex, MergePolicy, ReviewEvent, Sm2Scheduler, SrsState, TermTable,
    Value, daily_queue, diff_entities, fold_events, generate_cards, mastery_label, merge_events,
    normalize, schema::TermAliasDef, suggest_grade, weak_cards, three_way,
};

fn full_schema() -> DomainSchema {
    DomainSchema::from_toml(
        r#"
id = "materia-medica"
name = "中药学"
version = 1

[[entities]]
kind = "herb"
label = "药物"
primary = "name"
group_kind = "group"

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
weight = 2.0

[[entities.fields]]
key = "indications"
label = "应用"
kind = "tags"
indexable = true
reversible = true

[[entities.fields]]
key = "features"
label = "特点"
kind = "tags"
indexable = true
reversible = true

[[entities.fields]]
key = "contraindications"
label = "禁忌"
kind = "tags"
indexable = true
reversible = true

[[entities.fields]]
key = "nature"
label = "性味归经"
kind = "text"
immutable = true

[[entities.fields]]
key = "usage"
label = "特殊用法"
kind = "text"
immutable = true

[[entities.fields]]
key = "page"
label = "页码"
kind = "number"

[[entities.fields]]
key = "group_name"
label = "药群"
kind = "text"
indexable = true

[[entities.directions]]
id = "effect_to_name"
label = "功效推药名"
clue_fields = ["effects", "features", "contraindications"]
answer_fields = ["name"]
scope = "group_name"
enumerate_if_ambiguous = true

[[entities.directions]]
id = "name_to_chain"
label = "药名推全链"
clue_fields = ["name"]
answer_fields = ["effects", "indications", "features"]

[[aliases]]
concept = "解表药"
canonical = "以发散表邪、解除表证为主要功效"
aliases = ["发散表邪", "解表"]
boundary = "不能写成发汗解肌作总定义"
page = 12

[[views]]
id = "matrix"
label = "药群矩阵"
columns = ["name", "effects", "features", "page"]
group_by = "group_name"

[[views]]
id = "danger"
label = "危险区"
columns = ["name", "nature", "usage", "contraindications"]
"#,
    )
    .unwrap()
}

fn herb(id: &str, group: &str, effects: &[&str], features: &[&str]) -> Entity {
    let mut e = Entity::new(id, "herb", "win", 1);
    e.set("name", Value::Text(id.into()), 1);
    e.set("group_name", Value::Text(group.into()), 1);
    e.set("effects", Value::Tags(effects.iter().map(|s| s.to_string()).collect()), 1);
    e.set("features", Value::Tags(features.iter().map(|s| s.to_string()).collect()), 1);
    e.set("nature", Value::Text("辛温，归肺膀胱经".into()), 1);
    e
}

#[test]
fn schema_validation_catches_bad_direction() {
    let bad = r#"
id="x"
name="x"
[[entities]]
kind="a"
[[entities.directions]]
id="d"
clue_fields=["nope"]
answer_fields=["name"]
"#;
    assert!(DomainSchema::from_toml(bad).is_err());
    let bad2 = r#"
id="x"
name="x"
[[entities]]
kind="a"
[[entities.fields]]
key="f"
reversible=true
"#;
    assert!(DomainSchema::from_toml(bad2).is_err());
}

#[test]
fn reverse_lookup_narrows_with_scope() {
    let s = full_schema();
    let list = vec![
        herb("麻黄", "发散风寒药", &["发汗解表", "宣肺平喘"], &["表实无汗"]),
        herb("桂枝", "发散风寒药", &["发汗解表", "温通经脉"], &["表虚有汗"]),
        herb("薄荷", "发散风热药", &["发汗解表", "清利头目"], &["风热表证"]),
    ];
    let mut idx = InvertedIndex::default();
    idx.rebuild(&list, &s);

    let all = idx.reverse(&[("herb.effects".into(), "发汗解表".into())]);
    assert_eq!(all.len(), 3);
    let scoped = idx.reverse(&[
        ("herb.effects".into(), "发汗解表".into()),
        ("herb.group_name".into(), "发散风寒药".into()),
    ]);
    assert_eq!(scoped.len(), 2);
    assert_eq!(
        idx.reverse(&[("herb.effects".into(), "宣肺平喘".into())])
            .len(),
        1
    );
}

#[test]
fn cards_generated_are_derived_and_stable() {
    let s = full_schema();
    let list = vec![
        herb("麻黄", "发散风寒药", &["发汗解表", "宣肺平喘"], &["表实无汗"]),
        herb("桂枝", "发散风寒药", &["发汗解表"], &["表虚有汗"]),
    ];
    let mut idx = InvertedIndex::default();
    idx.rebuild(&list, &s);
    let groups = Default::default();
    let a = generate_cards(&s, &list[0], &groups, &idx).unwrap();
    let b = generate_cards(&s, &list[0], &groups, &idx).unwrap();
    assert_eq!(a, b, "同一份主数据生成的卡片必须完全一致");
    assert!(a.iter().any(|c| c.direction == "name_to_chain"));
    assert!(a.iter().any(|c| c.clue_term == "宣肺平喘"));
}

#[test]
fn immutable_fields_never_silently_merge() {
    let s = full_schema();
    let base = vec![herb("麻黄", "发散风寒药", &["发汗解表"], &[])];
    let mut local = base[0].clone();
    local.set("nature", Value::Text("辛温 归肺经".into()), 2);
    local.rev = 3;
    let mut remote = base[0].clone();
    remote.set("nature", Value::Text("辛温 归膀胱经".into()), 3);
    remote.rev = 3;
    let (_, diff) = three_way(&base, &[local], &[remote], &s, MergePolicy::default()).unwrap();
    assert!(diff.warnings.iter().any(|w| w.contains("性味归经") || w.contains("nature")));
}

#[test]
fn diff_preview_and_event_merge() {
    let s = full_schema();
    let base = vec![herb("麻黄", "发散风寒药", &["发汗解表"], &[])];
    let mut changed = base[0].clone();
    changed.set("effects", Value::Tags(vec!["发汗解表".into(), "利水消肿".into()]), 2);
    let diff = diff_entities(&base, &[changed], &s);
    assert!(!diff.summary().is_empty());
    assert!(diff.summary().contains("共"));
}

#[test]
fn srs_replay_survives_duplicate_delivery() {
    let mk = |ts, grade, dev: &str| mneme_core::ReviewEvent {
        card_id: "c1".into(),
        ts,
        grade,
        device: dev.into(),
        seq: 0,
    };
    let events = merge_events(
        &[mk(1000, 4, "win"), mk(2000, 3, "phone")],
        &[mk(1000, 4, "win"), mk(3000, 2, "phone")],
    );
    assert_eq!(events.len(), 3);
    let states = fold_events(&events, &Sm2Scheduler::default());
    assert_eq!(states["c1"].reps, 2);
    assert_eq!(states["c1"].lapses, 1);
}

#[test]
fn term_table_and_normalization_cover_rare_chars() {
    assert_eq!(normalize("ＡＢＣ１２３"), "abc123");
    assert!(normalize("䗪虫、燀桃仁").contains("燀"));
    let t = TermTable::from_defs(&[TermAliasDef {
        concept: "解表药".into(),
        canonical: "以发散表邪、解除表证为主要功效".into(),
        aliases: vec!["解表".into()],
        boundary: Some("不能写成发汗解肌".into()),
        page: Some(12),
    }]);
    assert!(t.canonical("解表").starts_with("以发散表邪"));
}


// ---- A2 复习闭环：队列 / 自动评分 / 错题本 / 掌握度派生 ----

fn bare_card(id: &str, entity: &str, direction: &str) -> mneme_core::Card {
    // Card 是派生数据，没有 new 构造器；测试只需要 id/entity/direction 三件事
    let mut c = mneme_core::Card {
        id: id.into(),
        entity_id: entity.into(),
        direction: direction.into(),
        clue_field: String::new(),
        clue_term: String::new(),
        prompt: format!("{entity} · {direction}"),
        answer: String::new(),
        accept: vec![],
        answer_fields: vec![],
        scope: None,
        candidates: vec![],
        enumerate: false,
        immutable_boundary: vec![],
    };
    c.prompt = format!("{entity} · {direction}");
    c
}

#[test]
fn daily_queue_orders_due_first_then_clusters_new() {
    let cards = vec![
        bare_card("c-new-1", "薄荷", "effects_to_name"),
        bare_card("c-due-1", "麻黄", "name_to_effects"),
        bare_card("c-new-2", "麻黄", "effects_to_name"),
        bare_card("c-due-2", "桂枝", "name_to_effects"),
    ];
    let mut states: std::collections::HashMap<String, SrsState> = Default::default();
    let now = 10 * DAY;
    let mut s1 = SrsState::default();
    s1.due = now - DAY; // 桂枝更逾期，应排最前
    states.insert("c-due-2".into(), s1);
    let mut s2 = SrsState::default();
    s2.due = now - 100; // 麻黄刚到期
    states.insert("c-due-1".into(), s2);

    let q = daily_queue(&cards, &states, now, 10, 10);
    assert_eq!(q.due, vec!["c-due-2".to_string(), "c-due-1".to_string()]);
    // 新卡按实体聚簇（entity_id 字典序：薄(U+8584) < 麻(U+9EBB)，薄荷在前）
    assert_eq!(q.new, vec!["c-new-1".to_string(), "c-new-2".to_string()]);
    assert_eq!(q.items.len(), 4);
    assert_eq!(q.items[0], "c-due-2");
}

#[test]
fn daily_queue_respects_limits() {
    let cards: Vec<_> = (0..5).map(|i| bare_card(&format!("c-new-{i}"), "薄荷", "d")).collect();
    let empty: std::collections::HashMap<String, SrsState> = Default::default();
    let q = daily_queue(&cards, &empty, DAY, 2, 3);
    assert!(q.due.is_empty());
    assert_eq!(q.new.len(), 2, "新卡上限生效");
    assert_eq!(q.items.len(), 2);
}

#[test]
fn suggest_grade_follows_correctness_and_elapsed() {
    assert_eq!(suggest_grade(false, 1000, 0.9).0, 1);
    assert_eq!(suggest_grade(true, 20_000, 1.0).0, 2, "想太久");
    assert_eq!(suggest_grade(true, 8_000, 1.0).0, 3);
    assert_eq!(suggest_grade(true, 2_000, 1.0).0, 4);
    assert_eq!(suggest_grade(true, 2_000, 0.86).0, 2, "擦边判对");
}

fn ev(card: &str, ts: i64, grade: u8) -> ReviewEvent {
    ReviewEvent { card_id: card.into(), ts, grade, device: "t".into(), seq: 0 }
}

#[test]
fn weak_cards_need_two_consecutive_misses() {
    let now = 100 * DAY;
    let events = vec![
        ev("a", now - DAY, 1),
        ev("a", now - DAY / 2, 1),          // a：末尾连错 2 次 -> 薄弱
        ev("b", now - DAY, 1),
        ev("b", now - DAY / 2, 3),          // b：最后答对了 -> 不薄弱
        ev("c", now - DAY, 1),
        ev("c", now - DAY / 2, 1),
        ev("c", now - DAY / 4, 1),          // c：连错 3 次 -> 薄弱且排前
        ev("d", now - 40 * DAY, 1),          // d：窗口外 -> 不算
        ev("d", now - 39 * DAY, 1),
    ];
    let w = weak_cards(&events, now, 10);
    assert_eq!(w.iter().map(|x| x.card_id.as_str()).collect::<Vec<_>>(), vec!["c", "a"]);
    assert_eq!(w[0].misses, 3);
    // d 的错都在 30 天窗口外，且没有窗口内事件 -> 不出现
    assert!(!w.iter().any(|x| x.card_id == "d"));
}

#[test]
fn mastery_label_follows_stability_and_lapses() {
    assert_eq!(mastery_label(0.5, 0, Some(4)), "生疏", "stability 太低");
    assert_eq!(mastery_label(2.0, 0, None), "熟悉");
    assert_eq!(mastery_label(9.0, 0, Some(3)), "熟练");
    assert_eq!(mastery_label(9.0, 2, Some(3)), "生疏", "lapses >= 2 直接生疏");
    assert_eq!(mastery_label(9.0, 0, Some(1)), "熟悉", "刚答错还不够熟练");
}

#[test]
fn mastery_aggregates_across_entity_cards() {
    // 实体有两张卡，一张很稳一张很生疏：均值落在「熟悉」
    let mut states: std::collections::HashMap<String, SrsState> = Default::default();
    let mut a = SrsState::default();
    a.stability = 12.0;
    a.reps = 3;
    let mut b = SrsState::default();
    b.stability = 0.5;
    b.reps = 1;
    states.insert("a".into(), a);
    states.insert("b".into(), b);
    let vals: Vec<&SrsState> = ["a", "b"].iter().filter_map(|k| states.get(*k)).collect();
    let avg = vals.iter().map(|s| s.stability).sum::<f64>() / vals.len() as f64;
    let lapses: u32 = vals.iter().map(|s| s.lapses).sum();
    assert_eq!(mastery_label(avg, lapses, Some(3)), "熟悉");
}

const DAY: i64 = 86_400_000;
