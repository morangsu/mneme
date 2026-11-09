use crate::{
    config::SyncConfig,
    error::{Error, Result},
    targets::{build_target, unknown_target},
    traits::SyncTarget,
};
use mneme_core::{DomainSchema, MergePolicy, content_hash, merge_events, now_ms, three_way};
use mneme_store::{FORMAT, Snapshot, Store};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt, fs,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Latest {
    snapshot: String,
    sha256: String,
    rev: u64,
    updated_at: i64,
    device: String,
    /// 显式写出被排除的组件，避免对端靠「没收到」去做启发式推断
    excluded: Vec<String>,
}

/// 一次同步的完整回执：文件数、字节数、sha256、冲突、重试、耗时。
/// 可复现是硬要求，所以每次同步都留一份可核对的数字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub target: String,
    pub backend: String,
    pub pulled_events: usize,
    pub pushed_event_bytes: usize,
    pub remote_snapshot: Option<String>,
    pub local_snapshot: String,
    pub diff_preview: String,
    pub conflicts: usize,
    pub retries: u32,
    pub bytes_up: usize,
    pub bytes_down: usize,
    pub sha256: String,
    pub excluded: Vec<String>,
    pub dry_run: bool,
    pub duration_ms: u64,
}

impl fmt::Display for SyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "同步目标 {} ({})", self.target, self.backend)?;
        writeln!(f, "  拉取事件 {} 行 / 上传事件 {} B", self.pulled_events, self.pushed_event_bytes)?;
        writeln!(f, "  远端快照 {:?} -> 本地 {}", self.remote_snapshot, self.local_snapshot)?;
        writeln!(f, "  上行 {} B / 下行 {} B / 重试 {} 次 / 冲突 {}", self.bytes_up, self.bytes_down, self.retries, self.conflicts)?;
        writeln!(f, "  sha256 {}", self.sha256)?;
        if !self.excluded.is_empty() {
            writeln!(f, "  显式排除 {}", self.excluded.join(","))?;
        }
        if self.dry_run {
            writeln!(f, "  [dry-run] 未上传、未覆盖本地")?;
        }
        if !self.diff_preview.is_empty() {
            writeln!(f, "{}", self.diff_preview)?;
        }
        Ok(())
    }
}

pub struct SyncEngine {
    store: Arc<Store>,
    cfg: SyncConfig,
    /// 惰性构建：只同步 local 时，不会因为 GitHub token 没配而失败
    cache: Mutex<HashMap<String, Arc<dyn SyncTarget>>>,
}

impl SyncEngine {
    pub fn new(store: Arc<Store>, cfg: SyncConfig) -> Self {
        Self {
            store,
            cfg,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &SyncConfig {
        &self.cfg
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn target_ids(&self) -> Vec<String> {
        self.cfg.targets.iter().map(|t| t.id().to_string()).collect()
    }

    /// 运行时注入一个自定义后端（也用于测试里注入故障后端验证 412 自愈）。
    /// 这正是「可插拔」的落点：加新同步服务不必改引擎。
    pub fn inject_target(&self, id: &str, t: Arc<dyn SyncTarget>) {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), t);
    }

    /// 公开访问后端，供 server / FFI 在提交上游决策时把日志写进私有库。
    pub fn target(&self, id: &str) -> Result<Arc<dyn SyncTarget>> {
        {
            let c = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = c.get(id) {
                return Ok(t.clone());
            }
        }
        let cfg = self.cfg.target(id).ok_or_else(|| unknown_target(id))?;
        let t = build_target(cfg)?;
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), t.clone());
        Ok(t)
    }

    fn base_path(&self, target: &str) -> std::path::PathBuf {
        self.store
            .root()
            .join("sync")
            .join(format!("base-{}.json", crate::traits::safe_rel(target).unwrap_or_default()))
    }

    fn load_base(&self, target: &str) -> Vec<mneme_core::Entity> {
        let p = self.base_path(target);
        fs::read(&p)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<mneme_core::Entity>>(&b).ok())
            .unwrap_or_default()
    }

    fn save_base(&self, target: &str, entities: &[mneme_core::Entity]) -> Result<()> {
        let p = self.base_path(target);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(entities)?)?;
        fs::rename(tmp, p)?;
        Ok(())
    }

    pub async fn sync(&self, target_id: &str, dry: bool) -> Result<SyncReport> {
        let t = self.target(target_id)?;
        let started = std::time::Instant::now();
        t.prepare().await?;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.attempt(&*t, target_id, dry).await {
                Ok(mut r) => {
                    r.retries = attempt - 1;
                    r.duration_ms = started.elapsed().as_millis() as u64;
                    if !dry {
                        // 同步回执写进远端 log.jsonl；GitHub 每次 PUT 都是一个 commit -> 可溯源
                        let mut entry = crate::incremental::SyncLogEntry::from_report(&r, now_ms());
                        entry.device = self.store.device().to_string();
                        let _ = crate::incremental::append_log(&*t, &entry).await;
                        t.finalize().await?;
                    }
                    return Ok(r);
                }
                // ETag / sha / If-Match 冲突：重新拉远端并重新三方合并，再试一次
                Err(Error::PreconditionFailed(msg))
                    if attempt <= self.cfg.max_retries && !dry =>
                {
                    eprintln!("[mneme-sync] 412 自愈：第 {attempt} 次重试（{msg}）");
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn sync_all(&self, dry: bool) -> Vec<(String, Result<SyncReport>)> {
        let mut out = Vec::new();
        for id in self.target_ids() {
            out.push((id.clone(), self.sync(&id, dry).await));
        }
        out
    }

    async fn attempt(&self, t: &dyn SyncTarget, target_id: &str, dry: bool) -> Result<SyncReport> {
        let now = now_ms();
        let device = self.store.device().to_string();
        let mut bytes_down = 0usize;
        let mut bytes_up = 0usize;

        // ---- 1. 增量拉其它设备的事件分片（append-only，幂等合并） ----
        let mut pulled_events = 0usize;
        if !self.cfg.excludes_events() {
            let manifest = crate::incremental::read_manifest(t).await?;
            let names: Vec<String> = self
                .store
                .log()
                .list_files()?
                .iter()
                .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
                .collect();
            let local_shards = crate::incremental::local_shards(
                |d| self.store.log().read_device_bytes(d).ok(),
                &names,
            );
            let inc = crate::incremental::incremental_events(t, &manifest, &local_shards).await?;
            for (dev, slice) in inc {
                if dev == device || dev.is_empty() {
                    continue; // 自己那一份只由自己写
                }
                bytes_down += slice.len();
                pulled_events += self.store.log().merge_device_bytes(&dev, &slice)?;
            }
            // 兼容没有 manifest 的老远端：退回列目录全量拉
            if manifest.events.is_empty() {
                for f in t.list("events/").await? {
                    if !f.ends_with(".jsonl") {
                        continue;
                    }
                    let dev = f
                        .trim_start_matches("events/")
                        .trim_end_matches(".jsonl")
                        .to_string();
                    if dev == device || dev.is_empty() {
                        continue;
                    }
                    let bytes = t.get(&f).await?;
                    bytes_down += bytes.len();
                    pulled_events += self.store.log().merge_device_bytes(&dev, &bytes)?;
                }
            }
            if pulled_events > 0 && !dry {
                self.store.replay()?;
            }
        }

        // ---- 2. 拉远端最新快照 ----
        let mut snap_files = t.list("snapshots/").await?;
        snap_files.retain(|s| s.contains("snap-") && s.ends_with(".json"));
        snap_files.sort();
        let remote_name = snap_files.last().cloned();
        let remote_snap: Option<Snapshot> = match &remote_name {
            Some(n) => {
                let b = t.get(n).await?;
                bytes_down += b.len();
                Some(Snapshot::from_bytes(&b)?)
            }
            None => None,
        };

        // ---- 3. 三方合并（base = 上次成功同步的共同状态） ----
        let local_snap = self.store.current_snapshot(now)?;
        let schema: DomainSchema = self
            .store
            .read(|m| m.schema.clone())
            .ok_or_else(|| Error::Config("库里没有 schema，先执行 mneme init".into()))?;
        let base = self.load_base(target_id);

        let (entities, diff_preview, conflicts) = match &remote_snap {
            Some(rs) => {
                let (merged, diff) = three_way(
                    &base,
                    &local_snap.entities,
                    &rs.entities,
                    &schema,
                    MergePolicy::FieldLevelLww,
                )?;
                (merged, diff.summary(), diff.warnings.len())
            }
            None => (
                local_snap.entities.clone(),
                "首次同步：远端为空，以上传本地为主".into(),
                0,
            ),
        };

        // 事件按幂等键合并，不传 SRS 状态
        let events = match &remote_snap {
            Some(rs) => merge_events(&local_snap.events, &rs.events),
            None => local_snap.events.clone(),
        };

        // 组/同义表按 id、concept 取较新者
        let mut groups = local_snap.groups.clone();
        if let Some(rs) = &remote_snap {
            for g in &rs.groups {
                if let Some(cur) = groups.iter_mut().find(|x| x.id == g.id) {
                    if g.rev > cur.rev {
                        *cur = g.clone();
                    }
                } else {
                    groups.push(g.clone());
                }
            }
        }
        let mut aliases = local_snap.aliases.clone();
        if let Some(rs) = &remote_snap {
            for a in &rs.aliases {
                aliases.retain(|x| x.concept != a.concept);
                aliases.push(a.clone());
            }
        }

        let rev = local_snap
            .rev
            .max(remote_snap.as_ref().map(|s| s.rev).unwrap_or(0))
            + 1;
        let schema_toml = if local_snap.schema_toml.is_empty() {
            remote_snap
                .as_ref()
                .map(|s| s.schema_toml.clone())
                .unwrap_or_default()
        } else {
            local_snap.schema_toml.clone()
        };
        let mut merged = Snapshot {
            format: FORMAT.into(),
            version: 1,
            rev,
            created_at: now,
            device: device.clone(),
            schema_toml,
            entities,
            groups,
            aliases,
            events,
            checksum: String::new(),
        };
        merged.entities.sort_by(|a, b| a.id.cmp(&b.id));
        merged.recheck();

        if dry {
            return Ok(SyncReport {
                target: target_id.into(),
                backend: t.kind().into(),
                pulled_events,
                pushed_event_bytes: 0,
                remote_snapshot: remote_name,
                local_snapshot: format!("rev-{}", merged.rev),
                diff_preview,
                conflicts,
                retries: 0,
                bytes_up: 0,
                bytes_down,
                sha256: content_hash(&merged.to_bytes()?),
                excluded: self.cfg.exclude.clone(),
                dry_run: true,
                duration_ms: 0,
            });
        }

        // ---- 4. 覆盖前备份 + 装入合并结果（apply_remote_snapshot 内部会备份） ----
        self.store.apply_remote_snapshot(&merged)?;

        // ---- 5. 上传：事件分片 -> 不可变快照 -> latest.json 指针 ----
        let mut pushed_event_bytes = 0usize;
        if !self.cfg.excludes_events() {
            let own = self.store.log().read_device_bytes(&device)?;
            if !own.is_empty() {
                pushed_event_bytes = own.len();
                bytes_up += own.len();
                t.put(&format!("events/{device}.jsonl"), &own, None).await?;
            }
        }

        // 组件级排除只作用于**上传副本**，本地数据永远完整
        let excluded_kinds = self.cfg.excluded_kinds();
        let mut upload = merged.clone();
        if !excluded_kinds.is_empty() {
            upload.entities.retain(|e| !excluded_kinds.contains(&e.kind));
        }
        upload.recheck();
        let snap_bytes = upload.to_bytes()?;
        let snap_hash = content_hash(&snap_bytes);
        let snap_path = format!("snapshots/snap-{:08}-{}.json", upload.rev, &snap_hash[..8]);
        t.put(&snap_path, &snap_bytes, None).await?;
        bytes_up += snap_bytes.len();

        let head = t.head("latest.json").await?;
        let if_match = head.as_ref().map(|m| m.etag.clone());
        let latest = Latest {
            snapshot: snap_path.clone(),
            sha256: snap_hash.clone(),
            rev: upload.rev,
            updated_at: now,
            device: device.clone(),
            excluded: self.cfg.exclude.clone(),
        };
        let latest_bytes = serde_json::to_vec_pretty(&latest)?;
        bytes_up += latest_bytes.len();
        // 唯一的条件写；冲突 -> PreconditionFailed -> 外层 412 自愈
        t.put("latest.json", &latest_bytes, if_match.as_deref())
            .await?;

        // ---- 5b. 压缩快照 / delta / manifest：增量同步与 GitHub 配额治理 ----
        let packed = crate::incremental::gzip(&snap_bytes)?;
        let gz_path = format!(
            "snapshots/{}.gz",
            snap_path
                .trim_start_matches("snapshots/")
                .trim_end_matches(".json")
        );
        t.put(&gz_path, &packed, None).await?;
        bytes_up += packed.len();

        let from_rev = remote_snap.as_ref().map(|s| s.rev).unwrap_or(0);
        let delta = crate::incremental::DeltaSnapshot::between(
            &base,
            &merged.entities,
            from_rev,
            upload.rev,
        );
        let delta_bytes = serde_json::to_vec(&delta)?;
        let delta_path = format!("deltas/delta-{}-{}.json", from_rev, upload.rev);
        t.put(&delta_path, &delta_bytes, None).await?;
        bytes_up += delta_bytes.len();

        let names: Vec<String> = self
            .store
            .log()
            .list_files()?
            .iter()
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
            .collect();
        let shards = crate::incremental::local_shards(
            |d| self.store.log().read_device_bytes(d).ok(),
            &names,
        );
        let mut mf = crate::incremental::read_manifest(t).await.unwrap_or_default();
        mf.format = crate::incremental::MANIFEST_FORMAT.into();
        mf.version = 1;
        mf.rev = upload.rev;
        mf.updated_at = now;
        mf.device = device.clone();
        mf.snapshot = Some(snap_path.clone());
        mf.snapshot_sha256 = snap_hash.clone();
        mf.events = shards;
        mf.excluded = self.cfg.exclude.clone();
        mf.deltas.push(crate::incremental::DeltaMeta {
            path: delta_path,
            from_rev,
            to_rev: upload.rev,
            sha256: content_hash(&delta_bytes),
        });
        if mf.deltas.len() > 20 {
            mf.deltas.remove(0);
        }
        bytes_up += mf.to_bytes()?.len();
        // 条件写：并发同步时后写者收到 412，回到外层自愈重跑
        crate::incremental::write_manifest(t, &mf).await?;

        self.save_base(target_id, &merged.entities)?;
        self.cleanup_old_snapshots(t, &snap_path).await?;

        Ok(SyncReport {
            target: target_id.into(),
            backend: t.kind().into(),
            pulled_events,
            pushed_event_bytes,
            remote_snapshot: remote_name,
            local_snapshot: snap_path,
            diff_preview,
            conflicts,
            retries: 0,
            bytes_up,
            bytes_down,
            sha256: snap_hash,
            excluded: self.cfg.exclude.clone(),
            dry_run: false,
            duration_ms: 0,
        })
    }

    async fn cleanup_old_snapshots(&self, t: &dyn SyncTarget, keep: &str) -> Result<()> {
        let keep_n = self.cfg.keep_snapshots;
        if keep_n == 0 {
            return Ok(());
        }
        let mut files = t.list("snapshots/").await?;
        files.retain(|s| s.contains("snap-") && s.ends_with(".json"));
        files.sort();
        if files.len() <= keep_n {
            return Ok(());
        }
        for f in files.iter().take(files.len() - keep_n) {
            if f != keep {
                let _ = t.delete(f).await;
            }
        }
        Ok(())
    }
}
