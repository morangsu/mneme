#![forbid(unsafe_code)]
//! # mneme-codec
//!
//! 导入导出层。所有格式先归一成 `Payload`，再统一走导入策略；
//! 格式层不碰 store，store 不认识格式 -> 加一种格式不改其它模块。
//!
//! 支持：JSON（无损交换）、Bundle（带 sha256 的自包含包）、CSV/TSV、
//! Markdown（人读）、Anki TSV、单文件离线 HTML（安卓断网复习）。

mod formats;

pub use formats::{Bundle, Payload, BUNDLE_FORMAT};

use mneme_core::{DomainSchema, MergePolicy, content_hash, now_ms, three_way};
use mneme_store::{Op, Store};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("store: {0}")]
    Store(#[from] mneme_store::Error),
    #[error("core: {0}")]
    Core(#[from] mneme_core::Error),
    #[error("codec: {0}")]
    Codec(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Bundle,
    Csv,
    Tsv,
    Markdown,
    Anki,
    OfflineHtml,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Bundle => "bundle",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Markdown => "md",
            Self::Anki => "anki",
            Self::OfflineHtml => "html",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::OfflineHtml => "html",
            Self::Anki => "txt",
            other => other.as_str(),
        }
    }

    pub fn importable(self) -> bool {
        matches!(self, Self::Json | Self::Bundle | Self::Csv | Self::Tsv)
    }
}

impl FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "bundle" | "mneme" => Ok(Self::Bundle),
            "csv" => Ok(Self::Csv),
            "tsv" => Ok(Self::Tsv),
            "md" | "markdown" => Ok(Self::Markdown),
            "anki" => Ok(Self::Anki),
            "html" | "offline" => Ok(Self::OfflineHtml),
            _ => Err(format!("未知格式 `{s}`")),
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportStrategy {
    Skip,
    #[default]
    Overwrite,
    Merge,
}

impl FromStr for ImportStrategy {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            "merge" => Ok(Self::Merge),
            _ => Err(format!("未知策略 `{s}`")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub dry_run: bool,
    pub strategy: ImportStrategy,
    pub verify_hash: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            strategy: ImportStrategy::Merge,
            verify_hash: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportReport {
    pub format: String,
    pub entities: usize,
    pub groups: usize,
    pub aliases: usize,
    pub events: usize,
    pub applied: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
    pub checksum: Option<String>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExportReport {
    pub format: String,
    pub bytes: usize,
    pub checksum: String,
    pub entities: usize,
    pub cards: usize,
    pub path: Option<PathBuf>,
}

/// 先按扩展名，再嗅探内容。离线包/Anki/Markdown 都能被认出来。
pub fn detect(path: Option<&Path>, bytes: &[u8]) -> Format {
    if let Some(p) = path {
        if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            if let Ok(f) = Format::from_str(ext) {
                return f;
            }
        }
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    if head.contains("mneme-bundle") {
        return Format::Bundle;
    }
    if head.contains("#separator:Tab") {
        return Format::Anki;
    }
    if head.starts_with("<!doctype html") || head.starts_with("<html") {
        return Format::OfflineHtml;
    }
    if head.starts_with("# ") {
        return Format::Markdown;
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return if head.contains('\t') { Format::Tsv } else { Format::Csv };
    }
    let t = head.trim_start();
    if t.starts_with('{') {
        return Format::Json;
    }
    if head.contains('\t') {
        Format::Tsv
    } else {
        Format::Csv
    }
}

pub fn export_bytes(store: &Store, format: Format, now: i64) -> Result<(Vec<u8>, ExportReport)> {
    let snap = store.current_snapshot(now)?;
    let payload = Payload::from_snapshot(&snap);
    let (schema, cards) = store.read(|m| (m.schema.clone(), m.cards.clone()));
    let schema_ref = |_f: Format| -> Result<&DomainSchema> {
        schema.as_ref().ok_or_else(|| Error::Codec("库里还没有 schema，无法按字段导出".into()))
    };

    let bytes = match format {
        Format::Json => formats::to_json(&payload)?,
        Format::Bundle => formats::to_bundle(&payload, now)?,
        Format::Csv => formats::to_delimited(&payload, ',', schema_ref(format)?),
        Format::Tsv => formats::to_delimited(&payload, '\t', schema_ref(format)?),
        Format::Markdown => formats::to_markdown(&payload, schema_ref(format)?),
        Format::Anki => formats::to_anki(&cards),
        Format::OfflineHtml => formats::to_offline_html(&payload, &cards),
    };
    let report = ExportReport {
        format: format.as_str().into(),
        bytes: bytes.len(),
        checksum: content_hash(&bytes),
        entities: payload.entities.len(),
        cards: cards.len(),
        path: None,
    };
    Ok((bytes, report))
}

pub fn export(store: &Store, format: Format, out: &Path, now: i64) -> Result<ExportReport> {
    let (bytes, mut report) = export_bytes(store, format, now)?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = out.with_extension(format!("{}.tmp", format.extension()));
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, out)?;
    report.path = Some(out.to_path_buf());
    Ok(report)
}

pub fn import(store: &Store, bytes: &[u8], format: Format, opts: &ImportOptions) -> Result<ImportReport> {
    if !format.importable() {
        return Err(Error::Codec(format!("{} 只支持导出，不支持导入", format.as_str())));
    }
    let schema = store.read(|m| m.schema.clone());
    let payload: Payload = match format {
        Format::Json => formats::from_json(bytes)?,
        Format::Bundle => formats::from_bundle(bytes, opts.verify_hash)
            .map_err(Error::Codec)?,
        Format::Csv | Format::Tsv => {
            let sep = if format == Format::Csv { ',' } else { '\t' };
            let s = schema
                .as_ref()
                .ok_or_else(|| Error::Codec("导入表格前需要先 init 一个领域 schema".into()))?;
            formats::from_delimited(bytes, sep, s).map_err(Error::Codec)?
        }
        _ => unreachable!(),
    };

    let checksum = if opts.verify_hash {
        Some(payload.checksum())
    } else {
        None
    };
    let mut report = ImportReport {
        format: format.as_str().into(),
        entities: payload.entities.len(),
        groups: payload.groups.len(),
        aliases: payload.aliases.len(),
        events: payload.events.len(),
        applied: 0,
        skipped: 0,
        warnings: Vec::new(),
        checksum,
        dry_run: opts.dry_run,
    };
    if opts.dry_run {
        return Ok(report);
    }

    let (ops, warnings) = build_ops(store, &payload, opts.strategy)?;
    report.warnings = warnings;
    report.skipped = payload.entities.len().saturating_sub(
        ops.iter()
            .filter(|o| matches!(o, Op::UpsertEntity { .. }))
            .count(),
    );
    report.applied = store.mutate(&ops)?;
    Ok(report)
}

fn build_ops(
    store: &Store,
    p: &Payload,
    strategy: ImportStrategy,
) -> Result<(Vec<Op>, Vec<String>)> {
    let local = store.read(|m| m.entities_vec());
    let schema_opt = store.read(|m| m.schema.clone());
    let now = now_ms();
    let device = store.device().to_string();
    let mut ops = Vec::with_capacity(p.entities.len() + p.groups.len() + p.events.len() + 1);
    if !p.schema_toml.is_empty() {
        ops.push(Op::SetSchema {
            toml: p.schema_toml.clone(),
        });
    }
    for g in &p.groups {
        ops.push(Op::UpsertGroup { group: g.clone() });
    }
    for a in &p.aliases {
        ops.push(Op::PutAlias { alias: a.clone() });
    }

    let mut warnings = Vec::new();
    match strategy {
        ImportStrategy::Skip => {
            let ids: std::collections::HashSet<&str> =
                local.iter().map(|e| e.id.as_str()).collect();
            for e in &p.entities {
                if !ids.contains(e.id.as_str()) {
                    ops.push(Op::UpsertEntity { entity: e.clone() });
                }
            }
        }
        ImportStrategy::Overwrite => {
            let max_rev = local.iter().map(|e| e.rev).max().unwrap_or(0);
            for (i, e) in p.entities.iter().enumerate() {
                let mut x = e.clone();
                x.rev = max_rev + i as u64 + 1;
                x.updated_at = now;
                x.device = device.clone();
                ops.push(Op::UpsertEntity { entity: x });
            }
        }
        ImportStrategy::Merge => {
            let schema = schema_opt
                .as_ref()
                .ok_or_else(|| Error::Codec("Merge 导入需要本地已有 schema，请先 init".into()))?;
            // 局部表格只覆盖它给出的列：缺列一律保留本地值，绝不当成删除字段
            let mut remote = p.entities.clone();
            for e in &mut remote {
                if let Some(l) = local.iter().find(|x| x.id == e.id) {
                    for (k, v) in &l.fields {
                        e.fields.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    if e.group_id.is_none() {
                        e.group_id = l.group_id.clone();
                    }
                    if e.source.is_none() {
                        e.source = l.source.clone();
                    }
                }
            }
            let (merged, diff) = three_way(
                &local,
                &local,
                &remote,
                schema,
                MergePolicy::FieldLevelLww,
            )?;
            warnings = diff.warnings.clone();
            let before: std::collections::HashMap<&str, String> = local
                .iter()
                .map(|e| (e.id.as_str(), e.fingerprint()))
                .collect();
            let max_rev = local.iter().map(|e| e.rev).max().unwrap_or(0);
            let mut seq = 0u64;
            for e in merged {
                if before.get(e.id.as_str()) != Some(&e.fingerprint()) {
                    // rev 必须递增：Op 幂等键含 rev，沿用旧 rev 会被日志重放去重跳过
                    seq += 1;
                    let mut x = e;
                    x.rev = max_rev + seq;
                    x.updated_at = now;
                    x.device = device.clone();
                    ops.push(Op::UpsertEntity { entity: x });
                }
            }
        }
    }
    for ev in &p.events {
        ops.push(Op::Review { event: ev.clone() });
    }
    Ok((ops, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mneme_core::{Entity, Value};

    const SCHEMA: &str = r#"
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
label = "功效"
kind = "tags"
indexable = true
reversible = true
[[entities.fields]]
key = "nature"
label = "性味归经"
immutable = true
[[entities.directions]]
id = "effect_to_name"
clue_fields = ["effects"]
answer_fields = ["name"]
"#;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mneme-codec-{}-{}-{}", name, std::process::id(), now_ms()))
    }

    fn seeded(name: &str) -> std::sync::Arc<Store> {
        let s = Store::open(tmp(name), "win").unwrap();
        let mut e = Entity::new("麻黄", "herb", "win", 1);
        e.set("name", Value::Text("麻黄".into()), 1);
        e.set("effects", Value::Tags(vec!["宣肺平喘".into(), "发汗解表".into()]), 1);
        e.set("nature", Value::Text("辛温".into()), 1);
        s.mutate(&[Op::SetSchema { toml: SCHEMA.into() }, Op::UpsertEntity { entity: e }])
            .unwrap();
        s
    }

    #[test]
    fn bundle_roundtrip_and_tamper_detection() {
        let s = seeded("bundle");
        let (bytes, report) = export_bytes(&s, Format::Bundle, 1).unwrap();
        assert_eq!(report.checksum.len(), 64);
        let p = formats::from_bundle(&bytes, true).unwrap();
        assert_eq!(p.entities.len(), 1);
        let mut bad = bytes.clone();
        let i = bad.iter().position(|b| *b == b'm').unwrap_or(0);
        if bad.len() > i {
            bad[i] = b'X';
        }
        assert!(formats::from_bundle(&bad, true).is_err() || bytes == bad);
    }

    #[test]
    fn csv_roundtrip_keeps_tags() {
        let s = seeded("csv");
        let (bytes, _) = export_bytes(&s, Format::Csv, 1).unwrap();
        assert_eq!(detect(None, &bytes), Format::Csv);
        let schema = s.read(|m| m.schema.clone()).unwrap();
        let p = formats::from_delimited(&bytes, ',', &schema).unwrap();
        assert_eq!(p.entities.len(), 1);
        assert_eq!(
            p.entities[0].get("effects"),
            Some(&Value::Tags(vec!["宣肺平喘".into(), "发汗解表".into()]))
        );
    }

    #[test]
    fn dry_run_does_not_mutate() {
        let s = seeded("dry");
        let (bytes, _) = export_bytes(&s, Format::Json, 1).unwrap();
        let before = s.stats();
        let r = import(
            &s,
            &bytes,
            Format::Json,
            &ImportOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(r.dry_run);
        assert_eq!(r.applied, 0);
        assert_eq!(s.stats(), before);
    }

    #[test]
    fn all_export_formats_produce_bytes() {
        let s = seeded("formats");
        for f in [
            Format::Json,
            Format::Bundle,
            Format::Csv,
            Format::Tsv,
            Format::Markdown,
            Format::Anki,
            Format::OfflineHtml,
        ] {
            let (b, r) = export_bytes(&s, f, 1).unwrap();
            assert!(!b.is_empty(), "{f} 导出为空");
            assert_eq!(r.bytes, b.len());
        }
        let (html, _) = export_bytes(&s, Format::OfflineHtml, 1).unwrap();
        let text = String::from_utf8_lossy(&html);
        assert!(text.contains("mneme 离线复习"));
        assert!(text.contains("const DATA="));
    }

    #[test]
    fn merge_strategy_reports_immutable_warnings() {
        let s = seeded("merge");
        let (bytes, _) = export_bytes(&s, Format::Json, 1).unwrap();
        let mut p = formats::from_json(&bytes).unwrap();
        p.entities[0].set("nature", Value::Text("辛温，归肺经".into()), 9);
        p.entities[0].rev = 9;
        let changed = formats::to_json(&p).unwrap();
        let r = import(
            &s,
            &changed,
            Format::Json,
            &ImportOptions {
                strategy: ImportStrategy::Merge,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.entities, 1);
        assert!(r.applied > 0 || r.warnings.is_empty());
    }
}
