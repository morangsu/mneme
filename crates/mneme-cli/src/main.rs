#![forbid(unsafe_code)]
//! mneme CLI：初始化 / 导入导出 / 反查 / 出题 / 同步 / 基准 / 校验 / 起服务

#[cfg(feature = "fast-alloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use clap::{Parser, Subcommand};
use mneme_codec::{self as codec, Format, ImportOptions, ImportStrategy};
use mneme_core::{
    DomainSchema, Entity, InvertedIndex, TermAlias, Value, content_hash, now_ms,
};
use mneme_store::{Op, SnapshotStore, Store};
use mneme_sync::{SyncConfig, SyncEngine};
use mneme_upstream::{
    Decision, ProvenanceStore, PublicBundle, PullPlan, Resolution, Topology, adopt,
    apply_decisions, build_public_bundle, plan_pull,
};
use std::{
    fs,
    path::PathBuf,
    str::FromStr,
    time::Instant,
};

#[derive(Parser, Debug)]
#[command(name = "mneme", version = "0.1.0", about = "多领域记忆地基：中药学 / 内经 / 植物学 / 食疗学 ...")]
struct Cli {
    #[arg(long, default_value = "data", global = true)]
    data: PathBuf,
    #[arg(long, global = true)]
    device: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 加载一个领域 TOML（domain 决定字段、方向、视图，不改代码）
    Init {
        #[arg(long)]
        domain: PathBuf,
    },
    /// 查看当前库状态
    Stats,
    /// 导入 JSON / Bundle / CSV / TSV
    Import {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        format: Option<String>,
        #[arg(long, default_value = "merge")]
        strategy: String,
        #[arg(long)]
        dry: bool,
    },
    /// 导出（json/bundle/csv/tsv/md/anki/html）
    Export {
        #[arg(long, default_value = "bundle")]
        format: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// 给线索反推实体：mneme query --clue "功效:宣肺平喘" --clue "禁忌:表虚自汗"
    Query {
        #[arg(long)]
        clue: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 打印反向卡（默认不显示答案）
    Quiz {
        #[arg(long, default_value_t = 5)]
        limit: usize,
        #[arg(long)]
        reveal: bool,
    },
    /// 同步到指定目标 / 全部目标
    Sync {
        #[arg(long, default_value = "data/sync.json")]
        config: PathBuf,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry: bool,
    },
    /// 生成一份带环境变量占位符的同步配置
    ConfigExample {
        #[arg(long, default_value = "data/sync.json")]
        out: PathBuf,
    },
    /// 内存索引 / 反查 / 快照的同环境基准
    Bench {
        #[arg(long, default_value_t = 10000)]
        entities: usize,
    },
    /// 校验所有快照与日志
    Verify,
    /// 生成三仓库拓扑配置示例（2 个公共库 + 1 个私有库）
    TopologyExample {
        #[arg(long, default_value = "data/topology.json")]
        out: PathBuf,
        #[arg(long, default_value = "morangsu")]
        owner: String,
    },
    /// 维护者侧：导出只含基础内容的公共库包
    UpstreamPublish {
        #[arg(long, default_value = "data/upstream/public-bundle.json")]
        out: PathBuf,
        #[arg(long, default_value_t = 1)]
        rev: u64,
    },
    /// 首次接入公共库：建立字段级溯源，标记已有个性化调整
    UpstreamAdopt {
        #[arg(long, default_value = "data/upstream/public-bundle.json")]
        public: PathBuf,
    },
    /// 从公共库拉取：只生成 staging 计划，不写库
    UpstreamPull {
        #[arg(long, default_value = "data/upstream/public-bundle.json")]
        public: PathBuf,
    },
    /// 查看某次拉取的待裁决冲突
    UpstreamConflicts {
        #[arg(long)]
        plan: String,
    },
    /// 提交裁决：所有冲突都决定后才写库
    UpstreamResolve {
        #[arg(long)]
        plan: String,
        /// 逐项决定，格式 `冲突ID:keep_mine|take_upstream|keep_both`
        #[arg(long)]
        decision: Vec<String>,
        /// 显式一键决定（仍是用户选择），例如 --all keep_mine
        #[arg(long)]
        all: Option<String>,
    },
    /// 查看上游状态
    UpstreamStatus,
    /// 启动 HTTP 服务（等价于 mneme-server）
    Serve {
        #[arg(long, default_value_t = 8321)]
        port: u16,
        #[arg(long)]
        lan: bool,
        #[arg(long)]
        token: Option<String>,
        #[arg(long, default_value = "data/sync.json")]
        config: PathBuf,
    },
}

fn device_of(cli: &Cli) -> String {
    cli.device
        .clone()
        .unwrap_or_else(|| std::env::var("COMPUTERNAME").unwrap_or_else(|_| "win".into()))
}

fn open(cli: &Cli) -> anyhow::Result<std::sync::Arc<Store>> {
    Ok(Store::open(&cli.data, &device_of(&cli))?)
}

fn resolve_field(schema: &DomainSchema, raw: &str) -> Option<String> {
    if raw.contains('.') {
        return Some(raw.to_string());
    }
    for e in &schema.entities {
        for f in &e.fields {
            if f.key == raw || f.label == raw {
                return Some(format!("{}.{}", e.kind, f.key));
            }
        }
    }
    None
}

fn parse_clues(schema: &DomainSchema, raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .flat_map(|s| s.split('|'))
        .filter_map(|part| {
            let (lhs, rhs) = part.split_once(':').unwrap_or(("", part));
            let field = if lhs.is_empty() {
                schema.entities.first().and_then(|e| {
                    e.fields
                        .iter()
                        .find(|f| f.reversible)
                        .map(|f| format!("{}.{}", e.kind, f.key))
                })
            } else {
                resolve_field(schema, lhs)
            };
            field.map(|f| (f, rhs.trim().to_string())).filter(|(_, v)| !v.is_empty())
        })
        .collect()
}

fn topology_of(cli: &Cli) -> Topology {
    fs::read(cli.data.join("topology.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Topology>(&b).ok())
        .unwrap_or_else(|| Topology::example("morangsu"))
}

fn provenance_of(cli: &Cli) -> ProvenanceStore {
    fs::read(cli.data.join("upstream/provenance.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<ProvenanceStore>(&b).ok())
        .unwrap_or_default()
}

fn save_rel(cli: &Cli, rel: &str, v: &impl serde::Serialize) -> anyhow::Result<()> {
    let p = cli.data.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    fs::rename(tmp, p)?;
    Ok(())
}

fn plan_of(cli: &Cli, id: &str) -> anyhow::Result<PullPlan> {
    let p = cli.data.join(format!("upstream/staging-{id}.json"));
    let b = fs::read(&p).map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", p.display()))?;
    Ok(serde_json::from_slice(&b)?)
}

fn print_conflicts(p: &PullPlan) {
    println!("{}", p.summary_text);
    println!("plan_id={}", p.id);
    if p.conflicts.is_empty() {
        println!("没有冲突，可直接用 --all take_upstream 提交自动合并部分。");
        return;
    }
    println!("\n待你逐项决定（未决之前不会写入任何数据）：");
    for c in &p.conflicts {
        println!(
            "  ! {} · {} · {} [个性化={} 危险区={} 建议={:?}]",
            c.id, c.entity_name, c.label, c.personalized, c.immutable, c.suggestion
        );
        println!("      base    : {}", c.base);
        println!("      上游新值: {}", c.upstream);
        println!("      我的值  : {}", c.local);
        if let Some(page) = c.page {
            println!("      教材页码: {page}");
        }
    }
}

const BENCH_SCHEMA: &str = r#"
id = "bench"
name = "bench"
[[entities]]
kind = "item"
primary = "name"
[[entities.fields]]
key = "name"
[[entities.fields]]
key = "effects"
kind = "tags"
indexable = true
reversible = true
"#;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Init { domain } => {
            let toml = fs::read_to_string(domain)?;
            let schema = DomainSchema::from_toml(&toml)?;
            let store = open(&cli)?;
            let mut ops = vec![Op::SetSchema { toml: toml.clone() }];
            // 领域包里的同义表述一并入库（教材主表述唯一，同义只作命中关键词）
            for a in &schema.aliases {
                ops.push(Op::PutAlias {
                    alias: TermAlias {
                        concept: a.concept.clone(),
                        canonical: a.canonical.clone(),
                        aliases: a.aliases.clone(),
                        boundary: a.boundary.clone(),
                        page: a.page,
                    },
                });
            }
            store.mutate(&ops)?;
            let def = &schema.entities[0];
            println!(
                "领域 {} 已加载：{} 个字段 / {} 个出题方向 / {} 个视图 / {} 条同义表述",
                schema.name,
                def.fields.len(),
                def.directions.len(),
                schema.views.len(),
                schema.aliases.len()
            );
            println!("数据目录 {}", cli.data.display());
        }

        Cmd::Stats => {
            let s = open(&cli)?;
            println!("{:?}", s.stats());
        }

        Cmd::Import {
            file,
            format,
            strategy,
            dry,
        } => {
            let store = open(&cli)?;
            let bytes = fs::read(file)?;
            let fmt = match format {
                Some(f) => Format::from_str(f).map_err(|e| anyhow::anyhow!(e))?,
                None => codec::detect(Some(file), &bytes),
            };
            let opts = ImportOptions {
                dry_run: *dry,
                strategy: ImportStrategy::from_str(strategy).unwrap_or(ImportStrategy::Merge),
                verify_hash: true,
            };
            let r = codec::import(&store, &bytes, fmt, &opts)?;
            println!("格式 {} / 读取 {} 条实体 / 应用 {} 个操作 / 跳过 {}", r.format, r.entities, r.applied, r.skipped);
            if let Some(h) = &r.checksum {
                println!("sha256 {h}");
            }
            for w in &r.warnings {
                println!("! {w}");
            }
            if r.dry_run {
                println!("[dry-run] 未写入");
            }
            println!("当前 {:?}", store.stats());
        }

        Cmd::Export { format, out } => {
            let store = open(&cli)?;
            let fmt = Format::from_str(format).map_err(|e| anyhow::anyhow!(e))?;
            let path = out
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("mneme-export.{}", fmt.extension())));
            let r = codec::export(&store, fmt, &path, now_ms())?;
            println!(
                "已导出 {} -> {} ({} B, {} 实体, {} 卡) sha256 {}",
                r.format,
                path.display(),
                r.bytes,
                r.entities,
                r.cards,
                r.checksum
            );
        }

        Cmd::Query { clue, limit } => {
            let store = open(&cli)?;
            let (schema, hits, entities) = store.read(|m| {
                let s = m.schema.clone();
                let clues = s.as_ref().map(|x| parse_clues(x, clue)).unwrap_or_default();
                let hits = m.index.reverse(&clues);
                let ids: Vec<String> = hits
                    .iter()
                    .filter_map(|h| m.index.id_of(h.idx).map(String::from))
                    .collect();
                (s, ids, m.entities_vec())
            });
            let Some(schema) = schema else {
                anyhow::bail!("还没有加载领域 schema，先执行 mneme init");
            };
            let kind = schema.entities.first().map(|e| e.kind.as_str()).unwrap_or("");
            let primary = schema
                .entity_def(kind)
                .and_then(|d| d.primary.as_deref())
                .unwrap_or("name");
            println!("命中 {} 个，显示前 {}:", hits.len(), limit);
            for id in hits.iter().take(*limit) {
                if let Some(e) = entities.iter().find(|x| &x.id == id) {
                    let fields: Vec<String> = e
                        .fields
                        .iter()
                        .filter(|(k, _)| k.as_str() != primary)
                        .take(4)
                        .map(|(k, v)| format!("{k}={}", v.display()))
                        .collect();
                    println!("  {} [{}] {}", e.display_of(primary), e.group_id.as_deref().unwrap_or("-"), fields.join(" / "));
                }
            }
        }

        Cmd::Quiz { limit, reveal } => {
            let store = open(&cli)?;
            let cards = store.read(|m| m.cards.clone());
            for (i, c) in cards.iter().take(*limit).enumerate() {
                println!("\n{}. [{}] {}", i + 1, c.direction, c.prompt);
                if *reveal {
                    println!("   -> {}", c.answer);
                }
                for b in &c.immutable_boundary {
                    println!("   ! {b}");
                }
            }
            println!("\n共 {} 张卡（显示 {}）", cards.len(), limit);
        }

        Cmd::Sync {
            config,
            target,
            all,
            dry,
        } => {
            let store = open(&cli)?;
            let cfg = SyncConfig::from_file(config)
                .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}（可先 mneme config-example）", config.display()))?;
            let engine = SyncEngine::new(store.clone(), cfg);
            if *all {
                for (id, r) in engine.sync_all(*dry).await {
                    match r {
                        Ok(rep) => println!("--- {id} ---\n{rep}"),
                        Err(e) => eprintln!("--- {id} 失败: {e}"),
                    }
                }
            } else {
                let id = target.clone().unwrap_or_else(|| "local".into());
                let rep = engine.sync(&id, *dry).await?;
                print!("{rep}");
            }
        }

        Cmd::ConfigExample { out } => {
            let cfg = SyncConfig::example(&device_of(&cli));
            cfg.to_file(out)?;
            println!("已写入 {}（5 种后端示例，凭据请用 ${{ENV:VAR}} 占位）", out.display());
        }

        Cmd::Bench { entities } => {
            let n = *entities;
            let root = std::env::temp_dir().join(format!("mneme-bench-{}", now_ms()));
            let store = Store::open(&root, "bench")?;
            let mut ops = vec![Op::SetSchema { toml: BENCH_SCHEMA.into() }];
            for i in 0..n {
                let mut e = Entity::new(format!("药{i}"), "item", "bench", 1);
                e.set("name", Value::Text(format!("药{i}")), 1);
                e.set(
                    "effects",
                    Value::Tags(vec![format!("功效{}", i % 64), format!("归经{}", i % 16)]),
                    1,
                );
                ops.push(Op::UpsertEntity { entity: e });
            }
            let t = Instant::now();
            store.mutate(&ops)?;
            let insert = t.elapsed();

            let list = store.read(|m| m.entities_vec());
            let schema = DomainSchema::from_toml(BENCH_SCHEMA)?;
            let t = Instant::now();
            let mut idx = InvertedIndex::default();
            idx.rebuild(&list, &schema);
            let rebuild = t.elapsed();

            let t = Instant::now();
            let rounds = 1000;
            for i in 0..rounds {
                let _ = idx.reverse(&[("item.effects".into(), format!("功效{}", i % 64))]);
            }
            let lookup = t.elapsed();

            let t = Instant::now();
            let snap = store.current_snapshot(now_ms())?;
            let bytes = snap.to_bytes()?;
            let snapshot = t.elapsed();

            println!("mneme bench (n={n}, 同环境对照用，跨机器不可相减)");
            println!("  写入+落盘+派生   {:>9.2?}", insert);
            println!("  index rebuild    {:>9.2?}", rebuild);
            println!("  reverse x{rounds:<5} {:>9.2?}  ({:.4} ms/次)", lookup, lookup.as_secs_f64() * 1000.0 / rounds as f64);
            println!("  snapshot         {:>9.2?}  ({} B)", snapshot, bytes.len());
            println!("  sha256           {}", content_hash(&bytes));
            let _ = fs::remove_dir_all(&root);
        }

        Cmd::TopologyExample { out, owner } => {
            let t = Topology::example(owner);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(out, serde_json::to_vec_pretty(&t)?)?;
            println!("已写入 {}：", out.display());
            println!("  公共源码库   {}", t.app_repo.slug());
            println!("  公共基础库   {}（只读，只放基础内容）", t.public_data.slug());
            println!("  私有同步库   {}（个性化 + 设备同步）", t.private_data.slug());
            println!("  基础字段 {} 个 / 个人字段 {} 个", t.base_fields.len(), t.personal_fields.len());
        }

        Cmd::UpstreamPublish { out, rev } => {
            let store = open(&cli)?;
            let (entities, groups) = store.read(|m| {
                (m.entities_vec(), m.groups.values().cloned().collect::<Vec<_>>())
            });
            let topo = topology_of(&cli);
            let b = build_public_bundle(&entities, &groups, *rev, &topo, now_ms());
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let bytes = b.to_bytes()?;
            fs::write(out, &bytes)?;
            println!(
                "公共库包 r{} 已生成 -> {} ({} 实体 / {} B / sha256 {})",
                rev, out.display(), b.entities.len(), bytes.len(), b.checksum
            );
            println!("个人字段（掌握度/同义/批注等）已剔除，可安全推送到公共仓库。");
        }

        Cmd::UpstreamAdopt { public } => {
            let store = open(&cli)?;
            let b = PublicBundle::from_bytes(&fs::read(public)?)?;
            let local = store.read(|m| m.entities_vec());
            let topo = topology_of(&cli);
            let prov = adopt(&local, &b, &topo, now_ms());
            let personalized = prov
                .entities
                .values()
                .flat_map(|e| e.fields.values())
                .filter(|f| f.origin == mneme_upstream::Origin::User)
                .count();
            save_rel(&cli, "upstream/provenance.json", &prov)?;
            println!("已接入公共库 r{}：追踪 {} 条记录，其中 {} 个字段标记为「用户个性化调整」", prov.upstream_rev, prov.entities.len(), personalized);
            println!("之后每次拉取都会把这些字段的冲突交你逐项决定，其余自动合并。");
        }

        Cmd::UpstreamPull { public } => {
            let store = open(&cli)?;
            let b = PublicBundle::from_bytes(&fs::read(public)?)?;
            let prov = provenance_of(&cli);
            if b.rev <= prov.upstream_rev {
                println!("unchanged：公共库仍是 r{}", prov.upstream_rev);
                return Ok(());
            }
            let schema = store
                .read(|m| m.schema.clone())
                .ok_or_else(|| anyhow::anyhow!("还没有加载领域 schema，先执行 init"))?;
            let local = store.read(|m| m.entities_vec());
            let topo = topology_of(&cli);
            let plan = plan_pull(&local, &b, &prov, &schema, &topo, &device_of(&cli), now_ms())?;
            save_rel(&cli, &format!("upstream/staging-{}.json", plan.id), &plan)?;
            print_conflicts(&plan);
        }

        Cmd::UpstreamConflicts { plan } => {
            let p = plan_of(&cli, plan)?;
            print_conflicts(&p);
        }

        Cmd::UpstreamResolve { plan, decision, all } => {
            let store = open(&cli)?;
            let p = plan_of(&cli, plan)?;
            let mut resolutions: Vec<Resolution> = Vec::new();
            if let Some(a) = all {
                let d = Decision::from_str(a).map_err(|e| anyhow::anyhow!(e))?;
                for c in &p.conflicts {
                    resolutions.push(Resolution {
                        conflict_id: c.id.clone(), decision: d, custom_value: None,
                    });
                }
            } else {
                for item in decision {
                    let (id, dstr) = item.split_once(':').unwrap_or(("", item.as_str()));
                    let d = Decision::from_str(dstr).map_err(|e| anyhow::anyhow!(e))?;
                    if id.is_empty() {
                        for c in &p.conflicts {
                            resolutions.push(Resolution {
                                conflict_id: c.id.clone(), decision: d, custom_value: None,
                            });
                        }
                    } else {
                        resolutions.push(Resolution {
                            conflict_id: id.to_string(), decision: d, custom_value: None,
                        });
                    }
                }
            }
            let schema = store
                .read(|m| m.schema.clone())
                .ok_or_else(|| anyhow::anyhow!("还没有加载领域 schema"))?;
            let local = store.read(|m| m.entities_vec());
            let topo = topology_of(&cli);
            let prov = provenance_of(&cli);
            let now = now_ms();
            let (entities, new_prov, report) = apply_decisions(
                &p, &resolutions, &local, &schema, &prov, &topo, &device_of(&cli), now,
            )?;
            let max_rev = store.stats().rev;
            let device = device_of(&cli);
            let ops: Vec<Op> = entities
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let mut x = e.clone();
                    x.rev = max_rev + i as u64 + 1;
                    x.updated_at = now;
                    x.device = device.clone();
                    Op::UpsertEntity { entity: x }
                })
                .collect();
            store.mutate(&ops)?;
            save_rel(&cli, "upstream/provenance.json", &new_prov)?;
            save_rel(&cli, "upstream/last-report.json", &report)?;
            println!("裁决完成：自动合并 {} / 冲突 {} / 新增 {} / 移除 {}", report.auto_applied, report.conflicts_resolved, report.added, report.removed);
            println!("  保留我的 {} · 采用上游 {} · 两者都留 {} · 上游版本 r{}", report.keep_mine, report.take_upstream, report.keep_both, report.new_rev);
            println!("  决策日志 {} 条，同步时随私有库 commit 留痕", report.log.len());
        }

        Cmd::UpstreamStatus => {
            let prov = provenance_of(&cli);
            let topo = topology_of(&cli);
            println!("上游 r{} / 追踪 {} 条记录 / sha {}", prov.upstream_rev, prov.entities.len(), &prov.upstream_checksum.chars().take(12).collect::<String>());
            println!("  公共源码库 {} · 公共基础库 {} · 私有同步库 {}", topo.app_repo.slug(), topo.public_data.slug(), topo.private_data.slug());
        }

        Cmd::Verify => {
            let store = open(&cli)?;
            let snaps = SnapshotStore::open(&cli.data)?;
            let mut count = 0;
            for name in snaps.list()? {
                let s = snaps.read(&name)?;
                s.verify()?;
                count += 1;
                println!("  ok {name} rev={} sha={}", s.rev, &s.checksum[..12]);
            }
            let st = store.stats();
            println!("校验通过：{count} 份快照 / {} 实体 / {} 卡片 / {} 事件", st.entities, st.cards, st.events);
        }

        Cmd::Serve { port, lan, token, config } => {
            let store = open(&cli)?;
            let engine = mneme_server::load_sync_config(&store, config);
            let state = mneme_server::AppState::new(store, engine, token.clone());
            mneme_server::serve(state, *port, *lan).await?;
        }
    }
    Ok(())
}
