use mneme_core::{
    DomainSchema, Entity, InvertedIndex, MergePolicy, Sm2Scheduler, TermTable, Value,
    diff_entities, fold_events, generate_cards, merge_events, normalize, schema::TermAliasDef,
    three_way,
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
