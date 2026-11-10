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
    /// 全文检索：跨字段 + 同义名 + 拼音首字母（mneme search mh）
    Search {
        /// 关键词，空格分隔表示「都要命中」（AND）
        query: String,
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
    /// 冷启动预算：open 各阶段 + 首屏查询 + 单条编辑的真实耗时
    BenchStartup {
        /// 构造多少条实体
        #[arg(long, default_value_t = 500)]
        entities: usize,
        /// 重复打开几次（取中位数与 p95）
        #[arg(long, default_value_t = 7)]
        rounds: usize,
        /// 用哪个领域包做字段形状（决定字段数量与文本长度）
        #[arg(long, default_value = "domains/materia-medica.toml")]
        schema: PathBuf,
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

        Cmd::Search { query, limit } => {
            let store = open(&cli)?;
            let (schema, entities, groups) = store.read(|m| {
                (
                    m.schema.clone(),
                    m.entities_vec(),
                    m.groups.values().cloned().collect::<Vec<_>>(),
                )
            });
            let Some(schema) = schema else {
                anyhow::bail!("还没有加载领域 schema，先执行 mneme init");
            };
            let out = mneme_core::search(&schema, &entities, &groups, query, None, *limit);
            println!(
                "命中 {} 条（{} 条已显示，模式 {}，词 {:?}）:",
                out.total,
                out.hits.len(),
                out.mode,
                out.terms
            );
            for h in &out.hits {
                let via = match h.via {
                    mneme_core::SearchVia::Name => "药名",
                    mneme_core::SearchVia::Synonym => "别名",
                    mneme_core::SearchVia::Pinyin => "拼音",
                    mneme_core::SearchVia::Field => "字段",
                    mneme_core::SearchVia::Group => "药群",
                };
                if h.snippet.is_empty() || h.snippet == h.name {
                    println!("  {} [{via}] {}", h.name, h.group);
                } else {
                    println!("  {} [{via}·{}] {}", h.name, h.label, h.snippet);
                }
            }
            if out.mode == "or" {
                println!("（没有一条同时满足所有词，已改为命中任意一个词）");
            }
        }

        Cmd::Quiz { limit, reveal } => {
            let store = open(&cli)?;
            store.ensure_cards()?;
            let cards = store.read(|m| m.cards_flat());
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

        Cmd::BenchStartup {
            entities,
            rounds,
            schema,
        } => {
            bench_startup(*entities, *rounds, schema)?;
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


// ---------------------------------------------------------------- 冷启动基准

fn median(xs: &mut [f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs[xs.len() / 2]
}

fn p95(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[((v.len() as f64 * 0.95) as usize).min(v.len() - 1)]
}

/// 一段固定长度的中文填充文本，长度按字段类型给，模拟真实教材字段的体量。
///
/// 关键在**术语多样性**：真实 500 味药大约共享 600~900 个不同功效词，平均每个词
/// 只出现在 2~6 味药上。如果只用十来个词循环，倒排 posting 会被放大几十倍，
/// 卡片生成里「这条线索是否有歧义」的判定就会被人为拖慢——那是测出来的假瓶颈。
const POOL: &[&str] = &[
    "发汗解表", "宣肺平喘", "利水消肿", "温通经脉", "助阳化气", "清热泻火",
    "除烦止渴", "凉血解毒", "润肠通便", "降气止咳", "疏散风热", "清利头目",
];
/// 词表规模：12 个词根 × 60 个编号 = 720 个不同术语
const VARIANTS: usize = 60;

fn term_at(i: usize) -> String {
    format!("{}{}", POOL[i % POOL.len()], (i / POOL.len()) % VARIANTS)
}

fn filler(seed: usize, chars: usize) -> String {
    let mut out = String::with_capacity(chars * 3);
    let mut i = seed;
    while out.chars().count() < chars {
        out.push_str(&term_at(i));
        i += 1;
    }
    out.chars().take(chars).collect()
}

/// 按 schema 的字段形状把一条实体填满——只填 schema 里真实存在的字段，
/// 这样换领域包（内经/植物学）也能用同一条命令量。
fn fill_entity(schema: &DomainSchema, idx: usize) -> Entity {
    let kind = schema.entities.first().map(|e| e.kind.as_str()).unwrap_or("item");
    let primary = schema
        .entity_def(kind)
        .and_then(|d| d.primary.as_deref())
        .unwrap_or("name");
    let mut e = Entity::new(format!("药{idx}"), kind, "bench", 1);
    for f in schema.entity_def(kind).map(|d| d.fields.as_slice()).unwrap_or(&[]) {
        if f.key == primary {
            e.set(primary, Value::Text(format!("药{idx}")), 1);
            continue;
        }
        match f.kind {
            mneme_core::FieldKind::Tags => {
                let n = 3 + idx % 3;
                e.set(
                    f.key.clone(),
                    Value::Tags((0..n).map(|k| filler(idx + k, 4)).collect()),
                    1,
                );
            }
            mneme_core::FieldKind::Tag => {
                e.set(f.key.clone(), Value::Text(filler(idx, 4)), 1);
            }
            mneme_core::FieldKind::Number => {
                e.set(f.key.clone(), Value::Number((idx % 30) as f64), 1);
            }
            mneme_core::FieldKind::Bool => {
                e.set(f.key.clone(), Value::Bool(idx % 2 == 0), 1);
            }
            mneme_core::FieldKind::Text => {
                // 长文本字段（应用/笔记/鉴别）按 80 字，其余 20 字
                let long = matches!(f.key.as_str(), "indications" | "notes" | "identification" | "applications");
                e.set(f.key.clone(), Value::Text(filler(idx, if long { 80 } else { 20 })), 1);
            }
        }
    }
    e
}

fn dir_size(p: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = fs::read_dir(p) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += dir_size(&path);
            } else if let Ok(m) = entry.metadata() {
                total += m.len();
            }
        }
    }
    total
}

fn bench_startup(n: usize, rounds: usize, schema_path: &std::path::Path) -> anyhow::Result<()> {
    let toml = fs::read_to_string(schema_path).map_err(|e| {
        anyhow::anyhow!(
            "读不到领域包 {}：{e}\n（这是本地克隆的公共数据仓库，git clone 到 domains/ 即可）",
            schema_path.display()
        )
    })?;
    let schema = DomainSchema::from_toml(&toml)?;
    let field_count = schema
        .entities
        .first()
        .map(|e| e.fields.len())
        .unwrap_or(0);

    let root = std::env::temp_dir().join(format!("mneme-startup-{}-{}", n, now_ms()));
    // 造库：一次批量写（等价于导入一份完整数据），让事件日志与镜像都达到目标规模
    {
        let store = Store::open(&root, "bench")?;
        let mut ops = vec![Op::SetSchema { toml: toml.clone() }];
        for i in 0..n {
            ops.push(Op::UpsertEntity {
                entity: fill_entity(&schema, i),
            });
        }
        store.mutate(&ops)?;
    }

    let mut walls: Vec<f64> = Vec::with_capacity(rounds);
    let mut snaps: Vec<f64> = Vec::with_capacity(rounds);
    let mut replays: Vec<f64> = Vec::with_capacity(rounds);
    let mut derives: Vec<f64> = Vec::with_capacity(rounds);
    let mut caches: Vec<f64> = Vec::with_capacity(rounds);
    let mut sqlites: Vec<f64> = Vec::with_capacity(rounds);
    let mut walls_detail: Vec<String> = Vec::with_capacity(rounds);

    let mut last: Option<std::sync::Arc<Store>> = None;
    for _ in 0..rounds {
        let t = Instant::now();
        let (store, timing) = Store::open_timed(&root, "bench")?;
        walls.push(t.elapsed().as_secs_f64() * 1000.0);
        snaps.push(timing.snapshot_load.as_secs_f64() * 1000.0);
        replays.push(timing.replay.as_secs_f64() * 1000.0);
        derives.push(timing.derive.as_secs_f64() * 1000.0);
        caches.push(timing.cards_cache.as_secs_f64() * 1000.0);
        sqlites.push(timing.sqlite_rebuild.as_secs_f64() * 1000.0);
        walls_detail.push(format!("{:.0}", walls.last().copied().unwrap_or(0.0)));
        last = Some(store);
    }
    let store = last.expect("at least one round");

    // 懒加载：open 只装索引，卡片是「第一次进复习/导出」才装
    let t = Instant::now();
    store.ensure_cards()?;
    let cards_lazy = t.elapsed();
    let lazy_n = store.read(|m| m.cards_len());

    // 首屏数据：矩阵要的是整表投影（FFI view() 的等价物）
    let t = Instant::now();
    let list = store.read(|m| m.entities_vec());
    let first_paint = t.elapsed();

    // 检索：一次典型查询（药名首字母 + 字段子串）
    let t = Instant::now();
    let rounds_q = 50;
    let mut hits = 0usize;
    for i in 0..rounds_q {
        let q = if i % 2 == 0 { "mh".to_string() } else { filler(i, 4) };
        let out = mneme_core::search(&schema, &list, &[], &q, None, 50);
        hits += out.hits.len();
    }
    let search_each = t.elapsed().as_secs_f64() * 1000.0 / rounds_q as f64;

    // 派生拆解：索引单独、索引+卡片
    let t = Instant::now();
    let mut idx = InvertedIndex::default();
    idx.rebuild(&list, &schema);
    let index_only = t.elapsed();
    // 全量派生（冷路径）：索引已单独测过，这里测「索引 + 全部卡片」
    let t = Instant::now();
    store.refresh()?;
    let derive_all = t.elapsed();
    let card_count = store.read(|m| m.cards_len());
    let (idx_fields, idx_terms) = store.read(|m| m.index.shape());

    // 归因：单次 reverse 与单条实体卡片生成，各自多快
    let groups_map = store.read(|m| m.groups_map());
    let sample_entity = list.first().cloned().expect("entity");
    let clue: Vec<(String, String)> = {
        let def = schema.entity_def(&sample_entity.kind).expect("def");
        let d = def.directions.first().expect("direction");
        let f = d.clue_fields.first().expect("clue field");
        let t = sample_entity
            .get(f)
            .and_then(|v| v.terms().first().map(|x| x.to_string()))
            .unwrap_or_default();
        vec![(format!("{}.{}", sample_entity.kind, f), t)]
    };
    let t = Instant::now();
    for _ in 0..200 {
        let _ = idx.reverse(&clue);
    }
    let reverse_each = t.elapsed().as_secs_f64() * 1e6 / 200.0;
    let t = Instant::now();
    let mut sample_cards = 0;
    let mut sample_candidates = 0;
    for _ in 0..20 {
        if let Ok(cs) = mneme_core::generate_cards(&schema, &sample_entity, &groups_map, &idx) {
            sample_cards = cs.len();
            sample_candidates = cs.iter().map(|c| c.candidates.len()).sum();
        }
    }
    let cards_each = t.elapsed().as_secs_f64() * 1e6 / 20.0;

    // 单条编辑：连做 3 次，第 2/3 次才是稳态（第 1 次会带上 SQLite 首次事务与 WAL 增长）
    let mut edits: Vec<f64> = Vec::new();
    for round in 0..3 {
        let t = Instant::now();
        let mut edit = store.read(|m| m.entities.get("药0").cloned()).expect("entity");
        edit.set("notes", Value::Text(filler(7 + round, 80)), now_ms());
        edit.rev += 1;
        store.mutate(&[Op::UpsertEntity { entity: edit }])?;
        edits.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let one_edit = edits[1];

    let events_bytes = dir_size(&root.join("events"));
    let sqlite_bytes = dir_size(&root);

    let mut w = walls.clone();
    let mut r = replays.clone();
    let mut d = derives.clone();
    let mut sq = sqlites.clone();
    println!("mneme bench-startup（n={n} 实体 × {field_count} 字段，rounds={rounds}，领域包 {}）", schema_path.display());
    let db = fs::metadata(root.join("index.sqlite")).map(|m| m.len()).unwrap_or(0);
    let wal = fs::metadata(root.join("index.sqlite-wal")).map(|m| m.len()).unwrap_or(0);
    let snaps_bytes = dir_size(&root.join("snapshots"));
    println!(
        "  磁盘           事件 {:.1} MB / sqlite {:.1} MB / wal {:.1} MB / 快照 {:.1} MB / 合计 {:.1} MB",
        events_bytes as f64 / 1048576.0,
        db as f64 / 1048576.0,
        wal as f64 / 1048576.0,
        snaps_bytes as f64 / 1048576.0,
        sqlite_bytes as f64 / 1048576.0
    );
    println!(
        "  冷启动 wall     中位 {:>7.1} ms   p95 {:>7.1} ms   （{} 次，含进程内新建 Store）",
        median(&mut w), p95(&walls), rounds
    );
    println!("    快照读取      {:>7.2} ms", median(&mut snaps.clone()));
    println!("    重放日志      {:>7.2} ms", median(&mut r));
    println!("    派生索引+卡   {:>7.2} ms", median(&mut d));
    println!("    卡片缓存      {:>7.2} ms   ← 命中则只校验块指纹", median(&mut caches.clone()));
    println!("    同步 SQLite   {:>7.2} ms   ← 只补差异", median(&mut sq));
    println!("  逐次 wall       {}", walls_detail.join(" / "));
    println!(
        "  首次装卡        {:>7.1} ms（{} 张，之后常驻）",
        cards_lazy.as_secs_f64() * 1000.0,
        lazy_n
    );
    println!("  首屏整表投影    {:>7.2} ms（{} 行）", first_paint.as_secs_f64() * 1000.0, list.len());
    println!("  派生拆解        索引 {:>6.1} ms / 索引+卡片 {:>6.1} ms / {} 张卡 / 倒排 {} 字段 {} 词条",
        index_only.as_secs_f64() * 1000.0,
        derive_all.as_secs_f64() * 1000.0,
        card_count, idx_fields, idx_terms);
    println!("  单次 reverse    {:>7.2} µs（1 条线索）", reverse_each);
    println!(
        "  单条卡片生成    {:>7.1} µs（{sample_cards} 张，歧义候选合计 {sample_candidates} 个）",
        cards_each
    );
    println!("  检索            {:>7.3} ms/次（50 次共命中 {hits} 条）", search_each);
    println!("  单条编辑        首次 {:>6.1} ms / 稳态 {:>6.1} ms / 第三次 {:>6.1} ms", edits[0], one_edit, edits[2]);

    drop(store);
    let _ = fs::remove_dir_all(&root);
    Ok(())
}
