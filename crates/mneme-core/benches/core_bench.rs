use criterion::{Criterion, criterion_group, criterion_main};
use mneme_core::{DomainSchema, Entity, InvertedIndex, Value, normalize};

fn make(n: usize) -> (DomainSchema, Vec<Entity>) {
    let s = DomainSchema::from_toml(
        r#"
id="bench"
name="bench"
[[entities]]
kind="herb"
primary="name"
[[entities.fields]]
key="name"
[[entities.fields]]
key="effects"
indexable=true
reversible=true
"#,
    )
    .unwrap();
    let list: Vec<Entity> = (0..n)
        .map(|i| {
            let mut e = Entity::new(format!("药{i}"), "herb", "bench", 1);
            e.set("name", Value::Text(format!("药{i}")), 1);
            e.set(
                "effects",
                Value::Tags(vec![format!("功效{}", i % 64), format!("归经{}", i % 16)]),
                1,
            );
            e
        })
        .collect();
    (s, list)
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("core");
    group.bench_function("normalize_1000_terms", |b| {
        b.iter(|| {
            for i in 0..1000 {
                let _ = normalize(&format!("Ａ. 发汗解表（{i}）！"));
            }
        })
    });

    let (s, list) = make(10_000);
    group.bench_function("index_rebuild_10k", |b| {
        b.iter(|| {
            let mut idx = InvertedIndex::default();
            idx.rebuild(&list, &s);
            idx
        })
    });

    let mut idx = InvertedIndex::default();
    idx.rebuild(&list, &s);
    group.bench_function("reverse_lookup_10k", |b| {
        b.iter(|| idx.reverse(&[("herb.effects".into(), "功效7".into())]))
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
