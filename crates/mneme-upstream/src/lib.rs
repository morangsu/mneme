#![forbid(unsafe_code)]
//! # mneme-upstream · 三仓库拓扑
//!
//! ```text
//! 公共库 A  软件服务本体（源码 / 发布）
//! 公共库 B  对外开放的基础数据库 —— 只读上游，只含教材基础字段
//! 私有库 C  个性化内容 + 设备同步落点 —— 读写，带 token
//! ```
//!
//! 同步**不**直接从公共库 B 发生：用户主动「拉取」B 的内容到 C，
//! 拉取时做字段级三方合并：
//! - 用户没动过的字段 -> 自动采用上游；
//! - **用户个性化调整过的字段与上游都改了 -> 逐项交给用户决定**；
//! - 其余自动合并；所有冲突裁决完之前**一项都不写库**。
//!
//! 判定「用户有没有改过」靠 `ProvenanceStore` 的字段级溯源，不靠猜。

pub mod model;
pub mod pull;
pub mod source;

pub use model::*;
pub use pull::{adopt, apply_decisions, build_public_bundle, plan_pull};
pub use source::{FilePublicSource, GitHubPublicSource, PublicSource};

#[cfg(test)]
mod tests {
    use super::*;
    use mneme_core::{DomainSchema, Entity, SourceAnchor, Value, now_ms};
    use std::fs;

    const SCHEMA: &str = r#"
id = "t"
name = "t"
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
key = "canonical"
label = "教材主表述"
kind = "text"
[[entities.fields]]
key = "nature"
label = "性味归经"
kind = "text"
immutable = true
[[entities.fields]]
key = "mastery"
label = "掌握度"
kind = "text"
[[entities.fields]]
key = "synonyms"
label = "可替换说法"
kind = "tags"
"#;

    fn schema() -> DomainSchema {
        DomainSchema::from_toml(SCHEMA).unwrap()
    }
    fn topo() -> Topology {
        Topology::example("tester")
    }
    fn ent(id: &str, canonical: &str, effects: &str, nature: &str) -> Entity {
        let mut e = Entity::new(id, "herb", "phone", 1);
        e.set("name", Value::Text(id.into()), 1);
        e.set("canonical", Value::Text(canonical.into()), 1);
        e.set("effects", Value::Tags(vec![effects.into()]), 1);
        e.set("nature", Value::Text(nature.into()), 1);
        e.set("mastery", Value::Text("熟练".into()), 1);
        e.source = Some(SourceAnchor {
            page: Some(12),
            ..Default::default()
        });
        e
    }
    fn bundle(rev: u64, items: Vec<Entity>) -> PublicBundle {
        PublicBundle::new(rev, "t", items, vec![], 1000 + rev as i64)
    }

    #[test]
    fn adopt_marks_user_personalization() {
        let public = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "我的整合写法", "发汗解表", "辛温")];
        let prov = adopt(&local, &public, &topo(), 100);
        assert_eq!(prov.upstream_rev, 1);
        assert_eq!(prov.entities["麻黄"].fields["canonical"].origin, Origin::User);
        assert_eq!(prov.entities["麻黄"].fields["effects"].origin, Origin::Upstream);
        assert!(prov.entities["麻黄"].fields.get("mastery").is_none());
    }

    #[test]
    fn unedited_fields_auto_merge_without_conflict() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "教材原句", "发汗解表", "辛温")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![ent("麻黄", "教材原句", "发汗解表|利水消肿", "辛温")]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        assert!(plan.conflicts.is_empty());
        assert!(plan.auto.iter().any(|a| a.field == "effects"));
        assert!(plan.summary().contains("待裁决 0"));
    }

    #[test]
    fn personalized_field_conflict_is_reported_and_suggests_keep_mine() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "我的整合写法", "发汗解表", "辛温")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![ent("麻黄", "教材新版表述", "发汗解表", "辛温")]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        assert_eq!(plan.conflicts.len(), 1);
        let c = &plan.conflicts[0];
        assert_eq!(c.field, "canonical");
        assert!(c.personalized);
        assert_eq!(c.suggestion, Decision::KeepMine);
        assert_eq!(c.local, "我的整合写法");
        assert_eq!(c.upstream, "教材新版表述");
        assert_eq!(c.page, Some(12));
    }

    #[test]
    fn unresolved_conflicts_block_all_writes() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "我的整合写法", "发汗解表", "辛温")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![ent("麻黄", "新版", "发汗解表", "辛温")]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        let r = apply_decisions(&plan, &[], &local, &schema(), &prov, &topo(), "phone", 300);
        assert!(matches!(r, Err(Error::Unresolved(_))));
    }

    #[test]
    fn keep_mine_advances_base_so_conflict_does_not_repeat() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "我的整合写法", "发汗解表", "辛温")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![ent("麻黄", "教材新版表述", "发汗解表", "辛温")]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        let res = vec![Resolution {
            conflict_id: plan.conflicts[0].id.clone(),
            decision: Decision::KeepMine,
            custom_value: None,
        }];
        let (entities, new_prov, report) =
            apply_decisions(&plan, &res, &local, &schema(), &prov, &topo(), "phone", 300).unwrap();
        assert_eq!(entities[0].get("canonical").unwrap().display(), "我的整合写法");
        assert_eq!(new_prov.upstream_rev, 2);
        assert_eq!(new_prov.entities["麻黄"].fields["canonical"].upstream_value, "教材新版表述");
        assert_eq!(report.conflicts_resolved, 1);
        assert_eq!(report.log.len(), 1);

        // 上游没再变 -> 不应重复报同一冲突
        let same_upstream = bundle(3, vec![ent("麻黄", "教材新版表述", "发汗解表", "辛温")]);
        let plan2 = plan_pull(&entities, &same_upstream, &new_prov, &schema(), &topo(), "phone", 400).unwrap();
        assert!(plan2.conflicts.is_empty(), "base 推进后不应重复冲突");
    }

    #[test]
    fn keep_both_preserves_local_and_appends_upstream_to_synonyms() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "我的整合写法", "发汗解表", "辛温")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![ent("麻黄", "教材新版表述", "发汗解表", "辛温")]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        let res = vec![Resolution {
            conflict_id: plan.conflicts[0].id.clone(),
            decision: Decision::KeepBoth,
            custom_value: None,
        }];
        let (entities, _, report) =
            apply_decisions(&plan, &res, &local, &schema(), &prov, &topo(), "phone", 300).unwrap();
        assert_eq!(entities[0].get("canonical").unwrap().display(), "我的整合写法");
        let syn = entities[0].get("synonyms").unwrap().display();
        assert!(syn.contains("教材新版表述"));
        assert_eq!(report.keep_both, 1);
    }

    #[test]
    fn immutable_clash_always_goes_to_user() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温归肺")]);
        let local = vec![ent("麻黄", "教材原句", "发汗解表", "我改的归经")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![ent("麻黄", "教材原句", "发汗解表", "新版归经")]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        let c = plan.conflicts.iter().find(|c| c.field == "nature").unwrap();
        assert!(c.immutable);
        assert_eq!(c.kind, ConflictKind::ImmutableClash);
        assert_eq!(c.suggestion, Decision::KeepMine);
    }

    #[test]
    fn upstream_removal_of_edited_entity_is_a_conflict() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let local = vec![ent("麻黄", "我的整合写法", "发汗解表", "辛温")];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        assert!(plan.conflicts.iter().any(|c| c.kind == ConflictKind::UpstreamRemovedLocallyEdited));
        assert!(plan.removals.is_empty());
    }

    #[test]
    fn self_created_entity_is_never_deleted_by_upstream() {
        let base = bundle(1, vec![ent("麻黄", "教材原句", "发汗解表", "辛温")]);
        let mut mine = ent("我的笔记", "自拟", "自拟功效", "自拟");
        mine.device = "phone".into();
        let local = vec![ent("麻黄", "教材原句", "发汗解表", "辛温"), mine];
        let prov = adopt(&local, &base, &topo(), 100);
        let upstream = bundle(2, vec![]);
        let plan = plan_pull(&local, &upstream, &prov, &schema(), &topo(), "phone", 200).unwrap();
        assert!(!plan.removals.contains(&"我的笔记".to_string()));
        assert!(!plan.conflicts.iter().any(|c| c.entity_id == "我的笔记"));
    }

    #[test]
    fn public_bundle_excludes_personal_fields() {
        let mut e = ent("麻黄", "教材原句", "发汗解表", "辛温");
        e.set("synonyms", Value::Tags(vec!["解表".into()]), 1);
        let b = build_public_bundle(&[e], &[], 7, &topo(), 1);
        assert!(b.entities[0].fields.get("mastery").is_none());
        assert!(b.entities[0].fields.get("synonyms").is_none());
        assert!(b.entities[0].fields.get("canonical").is_some());
        assert_eq!(b.entities[0].device, "public");
        b.verify().unwrap();
    }

    #[test]
    fn topology_validation() {
        let mut t = topo();
        assert!(t.validate().is_ok());
        t.public_data.token_env = Some("SECRET".into());
        assert!(t.validate().is_err());
    }

    #[tokio::test]
    async fn file_source_deduplicates_unchanged_content() {
        let p = std::env::temp_dir().join(format!("mneme-upstream-{}.json", now_ms()));
        fs::write(&p, br#"{"a":1}"#).unwrap();
        let s = FilePublicSource::new(&p);
        assert!(s.fetch_if_changed().await.unwrap().is_some());
        assert!(s.fetch_if_changed().await.unwrap().is_none());
        let _ = fs::remove_file(&p);
    }
}
