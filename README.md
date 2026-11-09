# mneme · 多领域记忆地基

Rust 写的**本地优先、可同步、领域无关**的记忆引擎。中药学是第一个实例；内经、植物学、食疗学以及后续任何"概念多、表述像、需要部分推整体"的学科，只需加一份 `domains/*.toml`，不改一行 Rust 代码。

针对的具体痛点：功效/应用/用法大量相同、各自又有特点；考试给功效、应用、特点、禁忌、药名中的一部分要答其余；一张大表不可读；Win 与安卓双端；网络同步与导入导出是硬需求；不额外付费、数据不锁死。

---

## 1. 五分钟跑起来

```bash
cargo build --release                 # 或 cargo build
MN=./target/release/mneme             # Windows: mneme.exe

# 加载领域包（字段/出题方向/视图/同义表述都来自 TOML）
$MN --data data/demo init --domain domains/materia-medica.toml

# 导入自己的数据（示例给了 6 味药）
$MN --data data/demo import --file domains/sample-herbs.csv

# 给部分推整体：功效 + 禁忌 两个线索做 AND 反查
$MN --data data/demo query --clue "功效:宣肺平喘" --clue "禁忌:表虚自汗"

# 看自动派生的反向卡
$MN --data data/demo quiz --limit 5 --reveal

# 起服务（Win 常驻；安卓浏览器接入）
$MN --data data/demo serve --port 8321
```

实测：`6 味药 → 自动派生 77 张反向卡`；AND 反查精确命中「麻黄」。

---

## 2. 架构：严格单向的 crate 依赖

```
mneme-core    schema / 实体 / 倒排索引 / 中文归一化 / 出题 / SRS / 三方合并   （零 IO、零网络）
     ↑
mneme-store   append-only 事件日志 + 不可变快照 + 内存索引 + 分片读写
     ↑
mneme-codec   JSON / Bundle / CSV / TSV / Markdown / Anki TSV / 离线 HTML
     ↑
mneme-sync    SyncTarget trait + 5 个后端 + 412 自愈引擎
     ↑
mneme-server (axum) / mneme-cli     ← 只有这两层碰进程与网络
```

**加一门学科不改代码，加一个同步后端不改引擎，加一种格式不改存储。** 这就是"解耦"的落点。

数据分四层，永远不重复维护：

| 层 | 内容 | 是否同步 |
|---|---|---|
| 主数据 | `Entity`（一药一行）、`Group`（共性只存一份）、`TermAlias` | 快照，双向合并 |
| 派生 | 倒排索引、`Card`（一药 4–6 张反向卡）、SRS 状态 | **不同步**，各端自行重算 |
| 事件 | `ReviewEvent` append-only，键 `(card_id, ts, device, seq)` | 每设备一个分片，幂等合并 |
| 视图 | 同一份数据的列配置 / 筛选 / 分组，不是多张表 | 配置随 schema |

---

## 3. 领域包：加学科只写 TOML

`domains/` 下四个包，字段、方向、视图各不相同，共用同一个二进制：

| 包 | 实体 | 可反查线索 | immutable 危险区 |
|---|---|---|---|
| `materia-medica.toml` | 药物 | 功效 / 应用 / 特点 / 禁忌 | 性味归经、毒性、用量、配伍禁忌、特殊煎法、相似药鉴别 |
| `neijing.toml` | 篇章 | 原文 / 主题 / 注家 | 原文、出处 |
| `botany.toml` | 物种 | 形态 / 分布 / 用途 / 生境 | 拉丁学名、科属、相似种鉴别 |
| `dietetics.toml` | 食材 | 功效 / 适宜 / 方例 | 性味、禁忌、用法用量 |

三个关键声明：

- `reversible = true`（必须同时 `indexable = true`）：该字段可作为线索被反查；
- `[[entities.directions]]`：`clue_fields` → `answer_fields`，就是老师说的"知道部分回答其余"。出题器会做**唯一性收敛**：候选 >1 自动追加 `scope`（药群/类别），仍不唯一就转成"枚举 + 说差异"题，绝不硬给单答案；
- `immutable = true`：禁止同义替换，合并冲突时记 warning 并保留本地值交人工裁决，视图默认显示，出题器会附提示。

同义表述在 `[[aliases]]` 里声明：`canonical` 唯一，`aliases` 只是可命中关键词，`boundary` 写清不能替换的限定。

---

## 4. 同步协议：为什么不搬数据库文件

> 按文件同步一个"活的" SQLite 会撕裂索引（快照/版本号机制与文件拷贝语义不同，同类工具明确禁止）。所以同步的最小单元是**不可变 JSON 文件 + 每设备一份只追加日志**，任何后端只要会读写文件就能接。

一次同步的流程：

1. `prepare()`（Git 在此 `pull --rebase`）；
2. 拉其它设备的 `events/<device>.jsonl` 分片，只追加本地缺的行；**每台设备只写自己那一份，连文件级冲突都不存在**；
3. 拉远端最新 `snapshots/snap-*.json`，校验 sha256；
4. 以上次成功同步状态为 base 做**字段级三方合并**，产出差异预览；
5. 覆盖前自动备份（`backups/`）；
6. 上传：事件分片 → 不可变快照 → `latest.json` 指针（唯一的条件写）；
7. `If-Match` / GitHub `sha` 不匹配 → `PreconditionFailed` → **重拉 + 重新合并 + 重试**，最多 `max_retries` 次（412 自愈）；
8. `finalize()`（Git 在此 `commit` + `push`），产出含字节数、sha256、冲突数、重试数的回执。

三条硬约束：

- **只传事件，不传 SRS 状态**。排期由事件在各端重算（`fold_events`），两端算法状态不会出现分叉；
- **删除只认显式墓碑**（`deleted = true`），绝不因"远端整份没有"推断删除；
- **组件排除用显式列表**：`exclude = ["events", "kind:secret"]`，只过滤上传副本，本地数据永远完整，且把排除项写进 `latest.json` 让对端不必猜。

---

## 5. 五个同步后端

| 后端 | 覆盖 | 条件写 |
|---|---|---|
| `LocalDir` | 本地目录；指向 Syncthing / 坚果云 / OneDrive 挂载目录即可借它们的传输 | 内容 sha256 作 ETag |
| `WebDav` | 坚果云、Nextcloud、群晖、ownCloud | `If-Match`，412 自愈 |
| `GitHub` | Contents API，私有仓库存数据 | 文件 `sha`，409/422 自愈 |
| `Git` | 任意 remote（GitHub / Gitee / 自建） | 工作树 + `pull --rebase` / `push` |
| `S3` | AWS S3 / Cloudflare R2 / MinIO，手写 AWS SigV4 | `If-Match`，412 自愈 |

```bash
$MN config-example --out data/sync.json     # 生成 5 种后端示例
$MN sync --config data/sync.json --target local
$MN sync --config data/sync.json --target github --dry
$MN sync --config data/sync.json --all
```

凭据**不写死在配置里**：用 `${ENV:VAR}` 占位，运行时读环境变量（`MNEME_WEBDAV_TOKEN`、`MNEME_GITHUB_TOKEN`、`MNEME_S3_ACCESS_KEY` / `MNEME_S3_SECRET_KEY`）。后端是惰性构建的，只同步 `local` 时不会因为没配 GitHub token 而失败。

加第六种服务：实现 `SyncTarget` 的 5 个方法，在 `targets/mod.rs` 的工厂加一个分支即可；也可用 `SyncEngine::inject_target()` 运行时注入。

---

## 6. Win + 安卓双端

| 档 | 做法 | 关键点 |
|---|---|---|
| **A（推荐）** | USB 线 + `adb reverse tcp:8321 tcp:8321`，手机浏览器开 `http://localhost:8321` | `localhost` 是安全上下文，Service Worker / IndexedDB /「添加到主屏幕」都可用；不联网、零配置 |
| B | 服务加 `--lan`，同 WiFi 访问 `http://192.168.x.x:8321` | ⚠️ http + 局域网 IP **不是安全上下文，SW 注册会静默失败**；在线用可以，离线交给 C |
| C | `$MN export --format html --out offline.html`，把单文件存到手机本地打开 | 数据内嵌 + 内联 JS，完全断网复习；进度存 localStorage，回连后用 `sync` 合并 |

开 B 档建议加 `--token <口令>`（前端从 `?token=` 读取并自动带 `Authorization`）。默认只听 `127.0.0.1`，不做公网穿透。

---

## 7. 导入导出

| 格式 | 用途 | 导入 |
|---|---|---|
| `bundle` | 带 sha256 的自包含包（推荐做备份/迁移） | ✅ |
| `json` | 无损交换 | ✅ |
| `csv` / `tsv` | 表格；Tags 用 `\|` 分隔，UTF-8 BOM 保证 Excel 不乱码 | ✅ |
| `md` | 人读，按药群/篇章分组，危险区标注 | 仅导出 |
| `anki` | Anki TSV（正面/背面/标签） | 仅导出 |
| `html` | 单文件离线复习包 | 仅导出 |

导入支持 `--dry`（只预演不写库）、`--strategy skip|overwrite|merge`（merge 走三方合并、报告 immutable 冲突）、校验和核验、格式自动嗅探。数据全部可导出，不锁死。

---

## 8. HTTP API

`GET /api/state`、`/api/views`、`/api/view/{id}`、`/api/reverse?clue=功效:宣肺平喘|禁忌:表虚自汗`、`/api/quiz`、`/api/quiz/answer/{id}`、`/api/export?format=...`、`/api/sync/status`；`POST /api/review`、`/api/judge`（同义归一 + Dice 容错判分）、`/api/import`、`/api/sync/{id}`。前端 `web/` 三个文件用 `include_str!` 编进二进制，单文件分发。

---

## 9. 性能设计：对 `新建文本文档.txt` 清单的逐条对照

| 清单条目 | 本项目 | 判断 |
|---|---|---|
| 最新稳定版工具链 | Rust 1.98.1 / edition 2024 / resolver 3 | 采用 |
| `opt-level=3` `lto="fat"` `codegen-units=1` `panic="abort"` | `[profile.release]` 全开，另加 `strip` | 采用 |
| `RUSTFLAGS="-C target-cpu=native"` | **不默认开**，命令写在 `.cargo/config.toml` | 理由：锁死 CPU 会破坏可复现与跨机分发。要榨本机性能时显式启用，并只用同环境对照组比较 |
| PGO | 命令写在 `.cargo/config.toml` | 可选进阶，不默认 |
| Criterion.rs | `crates/mneme-core/benches/core_bench.rs` | 采用 |
| `mneme bench` | 内置 10k 实体基准（写入/重建/反查/快照） | 采用，不依赖外部工具 |
| perf / cargo-flamegraph | Linux 系；Windows 用 `samply` / WPR | 平台适配 |
| hyperfine / cargo-bloat / cargo-show-asm | 见下方命令 | 采用为可选工具 |
| Tokio | 异步运行时 | 采用 |
| Axum / Hyper | HTTP 层 | 选 Axum 而非 Actix：与 Tokio/Tower 同生态、模块化、依赖更轻 |
| Rayon | 倒排索引构建时并行分词，合并阶段串行免锁 | 采用 |
| mimalloc | `--features fast-alloc` 可选全局分配器 | **默认关闭**：本机 MSVC 编译 C 代码不稳定（构建中 `ring` 曾瞬时失败），默认保持零 C 依赖 |
| jemalloc / arena(bumpalo) | 不采用 | Windows MSVC 需 C 编译；且本库数据长期存活，不符合 arena 的短生命周期前提 |
| `Vec::with_capacity` / `Cow` | 索引、日志、卡片生成路径均预分配 | 采用；`Cow` 仅在确有收益处使用，不为凑清单而加间接层 |
| `std::simd` / pulp / wide | 不默认 | 稳定版不可用；字符串扫描改用 `memchr`（内部已有 SIMD 加速） |
| rkyv / sonic-rs / nom | 预留扩展点，不引入 | 序列化全部隔离在 `mneme-codec`，快照格式可替换；当前 JSON 优先可读、可调试、可校验 |
| DPDK | 不适用 | 本机/局域网服务，不需要内核旁路 |
| 缓存友好的数据布局 | postings 用 `Vec<u32>` + 独立 `ids` 数组，交集在紧凑整数上做 | 采用 |
| 减少锁竞争 | `parking_lot::RwLock`，索引与卡片**在锁外**算好再整体换入 | 采用 |
| `unsafe` | **全部库 crate `#![forbid(unsafe_code)]`** | 有意与清单不同：地基项目的可靠性优先，性能靠数据布局/并行/索引，不靠绕过检查；若将来 bench 证明某处必须 unsafe，应隔离在单独 crate 并加守卫 |
| Cargo workspace | 6 个 crate | 采用 |

```bash
cargo xtest                       # 全 workspace 测试
cargo xbench                      # Criterion 基准
cargo bloat --release --crates    # 体积
cargo asm --release --lib mneme_core::index::InvertedIndex::reverse
hyperfine 'target/release/mneme --data data/demo query --clue 功效:宣肺平喘'
samply record target/release/mneme bench --entities 50000
RUSTFLAGS="-C target-cpu=native" cargo build --release   # 本机极致档
```

---

## 10. 测试与实测回执（2026-11-09，本机 Node/Rust 环境）

**46 项测试全绿**：core 25（含 7 项集成）、store 5、codec 5、sync 11（含 6 项双设备/412/组件排除）。`cargo build --release` 1m55s 通过。

| 项目 | 实测 |
|---|---|
| 领域包加载 | 17 字段 / 5 出题方向 / 5 视图 / 2 同义表述 |
| 导入 6 味药 | 6 实体 → **77 张派生卡**；CSV sha256 `1fa7a373…` |
| AND 反查「宣肺平喘 + 表虚自汗」 | 命中 1：麻黄 |
| Bundle 导出 | 13,047 B，sha256 `713cb045…` |
| 离线 HTML / Anki | 43,372 B / 7,957 B |
| 同步 local | 上行 22,182 B，快照 `snap-00000002-387d50d9.json`，冲突 0、重试 0 |
| 校验 | 1 份快照 checksum 通过 |
| 10k 实体基准 | 写入+落盘+派生 2.01 s；index rebuild **25.78 ms**；reverse ×1000 48.52 ms（**0.0485 ms/次**）；snapshot 143.12 ms / 2.72 MB |
| release 产物 | `mneme.exe` 4.4 MB / `mneme-server.exe` 4.3 MB（LTO fat + strip），单二进制含内嵌前端 |
| HTTP | `/api/state` 6 实体 77 卡；反查命中麻黄；危险区视图 immutable 标记正确；静态首页 3,278 B；CSV 导出 2,720 B；复习/判分端点 200 |

> 性能数字只适合同环境对照，跨机器/跨负载的绝对值不可相减。

---

## 11. 边界与不做的事

- 只拍自己的实体书做局部数字化，**本地保存、不外传、不分发扫描件**；本项目不含任何教材正文。
- OCR 生僻字（燀、煨、烊化、十八反十九畏等）必须人工校一遍；归一化已覆盖 CJK 扩展 A/B，但不代替核对教材原文与页码。
- 危险区字段（性味归经、毒性、用量、配伍禁忌、特殊煎法、鉴别点）代码级禁止同义替换，不靠文档提醒。
- 不引 npm/前端构建链、不上云与账号、不做 Electron 或安卓原生工程、不自研 OCR、不生成整本电子书。
- 不动工作区里其它项目；`tiku/`（题库网关）仍是独立可用的工程，本项目只是借鉴了它的零依赖网关思路与中文归一化算法。

---

# v0.2 · 手机 APP / 三仓库拓扑 / 上游冲突裁决（2026-11-09）

> 本节是最新状态。上文 §1–§11 是 v0.1 基线；其中「6 个 crate」「手机用浏览器」「Win 常开做服务」等描述已被本节取代。

## A. 模块数：6 → 8 个 crate + 1 个 Android 工程

| 新增 | 职责 |
|---|---|
| `mneme-upstream` | 三仓库拓扑、字段级 provenance、`adopt` / `plan_pull` / `apply_decisions`、公共库只读源 |
| `mneme-ffi` | uniffi 绑定，17 个 Kotlin 可调用方法（含 `upstreamAdopt`） |
| `android/` | Compose 原生 APP（矩阵/反查/复习/同步/冲突裁决 + WorkManager + ML Kit OCR） |

原有 6 个 crate 也都做了增强：`core`（FSRS、干扰项）、`store`（增量派生、SQLite 物化镜像）、`codec`（局部 CSV 合并）、`sync`（manifest/增量分片/gzip/delta/同步日志）、`server`（6 个上游 API）、`cli`（7 个上游命令）。

## B. 三仓库拓扑（GitHub，免费可溯源）

```
公共 A  morangsu/mneme                 软件服务本体（源码/发布）
公共 B  morangsu/mneme-data-public     对外开放的基础数据库，只读上游，只含基础字段
私有 C  morangsu/mneme-data-private    个性化内容 + 设备同步落点，token 读写
```

- **设备间同步只连私有库 C**；公共库 B 永不自动同步，必须由用户主动拉取；
- `PublicBundle` 只含 `base_fields`（13 个），`mastery/synonyms/boundary/notes/processing` 等个人字段一律剔除，绝不外发；
- GitHub Contents API 每次 PUT 都是一个 commit；`manifest.json`、`sync/log.jsonl`、`sync/upstream-decisions.jsonl` 让每次同步与每次裁决都可溯源；
- 公共库用 `If-None-Match` 条件请求（匿名 60/h 配额），命中 304 不解析；命中限流明确报错而非重试打爆；
- 凭据写 `${ENV:MNEME_GITHUB_TOKEN}`，不落明文；私有库 target id = `github-private`。

## C. 冲突规则（用户特别强调的部分，已做成硬约束）

```bash
mneme topology-example --out data/topology.json
mneme upstream-publish --out data/upstream/public-bundle.json --rev 1   # 维护者生成公共包
mneme upstream-adopt   --public ...                                      # 首次接入，建立字段级溯源
# … 用户改本地字段（如把相似表述整合成一条）…
mneme upstream-pull    --public ...      # 只生成 staging 计划，不写库
mneme upstream-conflicts --plan <id>     # 三值并列：base / 上游新值 / 我的值 + 页码
mneme upstream-resolve --plan <id> --all keep_mine   # 或逐项 --decision <id>:<动作>
```

判定与执行：

1. **保守识别个性化**：只要 `local != 上次公共库值` 就视为用户改过，不依赖 provenance 是否被及时更新；
2. 用户没动过的字段 → 自动采用上游；**两边都改且不同 → 冲突，逐项交用户决定**；
3. `immutable`（性味归经/毒性/用量/配伍禁忌…）与 `personalized` 冲突默认建议 `KeepMine`；
4. **所有冲突都有决定之前，一项都不写库**（`Unresolved` 错误），staging 计划与联网解耦；
5. `KeepBoth` 把上游值追加到个人字段 `synonyms`，主字段保留用户值；
6. `KeepMine` 会把 base 推进到本次上游值，避免同一冲突反复出现；
7. 上游移除但本地改过 → 冲突；本地自建实体（无溯源记录）→ 永不删；
8. 决策日志写私有库 `sync/upstream-decisions.jsonl`。

## D. 手机 APP（uniffi + Compose）

- Win 端网页；**安卓端是原生 Compose APP**，不再是浏览器；
- APP 本地是**真 SQLite**（`index.sqlite` 物化镜像：entities/cards/events 三表 + 索引 + `integrity_check`），但它是**可重建的派生视图**，同步仍不搬 `.sqlite` 文件；
- Rust 逻辑全部复用：归一化、反查、出题、判分、三方合并、冲突判定、GitHub 同步都在 `mneme-core/store/sync/upstream`，Kotlin 只做 UI 与系统能力；
- 已生成 `mneme_ffi.kt`（uniffi 0.29.5，50 KB）+ 手写 `MnemeApi.kt` 包装，`Api.kt` 的 `MnemeBridge` 已切到真实绑定，缺 `.so` 时才回退样例 Stub；
- WorkManager 每小时、仅联网时后台同步；ML Kit 中文 OCR → 收件箱 → 复核入库；到期卡片通知。

## E. P0/P1 优化落地

| 项 | 实现 |
|---|---|
| 增量派生 | `Memory.dirty` 脏标记，只重生成受影响实体的卡片；纯复习事件连索引重建都跳过 |
| SQLite 镜像 | `mneme-store/sqlite` feature（默认开，rusqlite bundled），事务批量、可 `rebuild`、可 `integrity_check` |
| 增量同步 | `manifest.json` 记各分片 sha256 + 长度（offset）；相同则跳过，不同只取新增字节；`delta-*.json` 只传变化实体；纯 Rust gzip 压缩快照 |
| FSRS | `FsrsScheduler`（DSR：稳定性/难度/可提取度），`fit()` 用本人复习记录网格调参 |
| 干扰项 | `generate_choices()` 从同药群确定性挑 3 个相似药做选择题选项 |
| 可溯源日志 | `sync/log.jsonl` 每次同步一行；决策日志单独成文件 |

## F. 实测回执（2026-11-09）

- **68 项测试全绿**（core 27 / store 7 / codec 5 / sync 15 / upstream 12 / ffi 2）；`cargo check --workspace`、release 构建通过；
- 端到端冲突流程实测：r1 发布 → 用户把「麻黄·教材主表述」改成自己的整合写法 → 上游 r2 也改同一字段 → 拉取得到 **1 项待裁决**（`personalized=true`、建议 `KeepMine`、页码 45、三值并列）→ 未决提交被拒（`Unresolved`）→ `--all keep_mine` 裁决成功 → 导出 CSV 确认本地值**未被覆盖** → 再次拉取 `unchanged`（base 已推进）；
- uniffi Kotlin 绑定生成成功：`mneme_ffi.kt` 50,567 B，17 个导出方法；
- **arm64 `.so` 编译成功**：`jniLibs/arm64-v8a/libmneme_ffi.so` 5.9 MB，ELF64 AArch64，导出 16 个 uniffi 入口，仅依赖 `libdl/libm/libc`；
- **APK 打包成功**：`android/app/build/outputs/apk/debug/app-debug.apk` 37 MB（`abiFilters` 收到 arm64-v8a），`aapt2 badging` 确认 `applicationId=io.github.morangsu.mneme`、minSdk 26 / targetSdk 35；
- 实测中修掉的真问题：SQLite 连接锁非重入死锁、`Op` 幂等键沿用旧 rev 导致局部导入静默失效、局部 CSV 把缺列当字段删除、`adopt` 后用户改字段未识别为个性化、APP 决策值大小写不兼容、`StdMutexGuard` 跨 `.await` 使 axum Future 非 Send。

## G. 构建环境（已全部打通）

| 部件 | 位置 / 取值 |
|---|---|
| Android NDK | `D:\scoop\apps\android-ndk\current`（r30，scoop bucket `hoilc_scoop-lemon`） |
| Android SDK | `D:\scoop\apps\android-clt\current`（android-35 + build-tools 35.0.0） |
| JDK | **必须 21**：`D:\scoop\apps\corretto21-jdk\current`（默认的 JDK 27 会让 Gradle 8.9 起不来） |
| Gradle | 8.9，wrapper 已生成（`./gradlew` 直接用） |

一条命令出包：

```bash
cargo build --target aarch64-linux-android --release -p mneme-ffi
cp target/aarch64-linux-android/release/libmneme_ffi.so \
   android/app/src/main/jniLibs/arm64-v8a/libmneme_ffi.so
cd android && JAVA_HOME=D:/scoop/apps/corretto21-jdk/current ./gradlew :app:assembleDebug
```

坑位：`android-clt/ndk` 是空占位目录（真 NDK 在 `apps/android-ndk`）；NDK 的 `aarch64-linux-android26-clang` 在 Windows 上是 `.cmd`，rustc 不能直接调用，故项目 `.cargo/config.toml` 用 `clang.exe` + `--target`；uniffi 生成代码依赖 JNA 且必须 `@aar`，否则 Kotlin 侧 `Unresolved reference 'Structure'`。

## H. 仓库与联网状态（2026-11-09 晚更新）

三个仓库已建好并推上首个提交：

| 仓库 | 可见性 | 内容 | 状态 |
|---|---|---|---|
| [`morangsu/mneme`](https://github.com/morangsu/mneme) | 公开 | 软件本体（Rust 8 crate + Android 工程 + web） | 已推送 |
| [`morangsu/mneme-data-public`](https://github.com/morangsu/mneme-data-public) | 公开 | 开放基础库：4 个领域包 + 示例数据 + `manifest.json` 校验 | 已推送 |
| [`morangsu/mneme-data-private`](https://github.com/morangsu/mneme-data-private) | 私有 | 个性化内容 + 各设备同步落点 | 已由真机写入（rev 27） |

**这台机器上 `github.com:443` 被阻断**，`git push/clone` 走不通；但 `api.github.com` 与 `codeload.github.com` 可达。
因此首推用 `tools/gh-commit.mjs`（Git Data API：blob → tree → commit → ref，一个提交写完所有文件，读的是 git 索引里的 blob，所以 LF 归一化与 `git push` 一致）。
**这条通道与 App 的同步后端是同一个 API**，等于顺带验证了 `mneme-sync` 的 GitHub 目标可行。

```bash
# 首次推送 / 之后补推（github.com 不可达时）
GH_TOKEN=xxx node tools/gh-commit.mjs . morangsu/mneme "feat: ..."
```

想恢复原生 `git push`：在 Clash 白名单里加 `github.com`（当前 `MATCH,DIRECT` 兜底，所以 github.com 直连被墙；`api.github.com` 命中 gfw 规则集才走代理）。

## I. 手机实测回执（真机 V2445EA / Android 16 / arm64-v8a）

设备：iQOO `V2445EA`，Android 16，`arm64-v8a`，USB 调试直连。
APK 44.6 MB（含内置领域包与 `libmneme_ffi.so`），`pm install` 成功，冷启动 353 ms。

| 环节 | 实测结果 |
| --- | --- |
| FFI 加载 | 真实 uniffi 绑定生效（未回退 Stub），16 → 19 个导出方法 |
| 首次建库 | 界面列出内置 4 个领域包 → 选「中药学」→ **实体 6 / 卡片 77 / rev 6** |
| 矩阵 | 按药群分组，列 `药名/功效/特点锚/应用`，immutable 列标「原样」 |
| AND 反查 | 线索 `功效:宣肺平喘` + `禁忌:表虚自汗` → **命中 1 项 · 麻黄 · 匹配 3.0** |
| 复习 | 到期 0 / 本轮 5 张，出题来自真实数据（「具有『疏肝行气』的是？」） |
| 首次上行 | `backend=github / bytes_up=32022 / conflicts=0 / retries=0`，私有库 rev 7 |
| 公共库拉取 r1 | `上游 r0 → r1：自动合并 0 / 新增 0 / 待裁决 0`（本地与公共一致） |
| 公共库拉取 r2 | `上游 r0 → r2：新增 1 / 待裁决 0` → 无需裁决，**直接落库**，矩阵出现「生姜」，实体 7 / 卡片 89 |
| 增量上行 | rev 7 → 27，事件分片 9700 B → 15550 B，新增 `deltas/delta-7-27.json`（旧 delta 保留） |

本地落盘：`files/mneme/data/` 下 `events/` `snapshots/` `sync.json` `index.sqlite` `upstream/{provenance.json,decisions.jsonl,staging-*.json,last-report.json}`（`provenance.json` 记录 `upstream_rev=2` 与逐字段 `origin`，是「本地改过」的判定基线）。

远端结构（私有库 `data/`）：`manifest.json` / `latest.json` / `events/<device>.jsonl` / `snapshots/snap-*.{json,gz}` / `deltas/delta-<from>-<to>.json` / `sync/log.jsonl`。

### 这轮实测抓到的两个真 bug

1. **`github.com:443` 走不通时首推没路**：本机 `git push` 被阻断（`api.github.com` 通）。补了 `tools/gh-commit.mjs` —— 用 Git Data API（blob → tree → commit → ref）一次提交写完所有文件，**读的是 git 索引里的 blob**，所以 LF 归一化、可执行位与 `git push` 完全一致。
2. **事件分片第二次上传必 422**：`events/<device>.jsonl` 是**可变文件**，原来用无条件 PUT，文件已存在时 GitHub 要求带 `sha`，返回 422 且拿不到 412 自愈。改为先 `head()` 取 sha 再条件写，412 自愈随之生效。同时把 409/412/422 的响应体带进错误信息——只写状态码会让人误判成并发冲突。

## J. 三仓库用法速查

```bash
# 公共库克隆成 domains/（github.com 的 git 传输若被阻断，用 API 取 tarball）
git clone https://github.com/morangsu/mneme-data-public domains

# 本项目推送（github.com 不可达时）
GH_TOKEN=xxx node tools/gh-commit.mjs . morangsu/mneme "feat: ..."

# 手机端：同步页填 token / owner / repo（默认 morangsu/mneme-data-private，路径 data）
#        → 「保存并启用」写 sync.json 并重建引擎 → 「立即同步」
android\sync-assets.cmd     # 把 domains/ 的领域包同步进 APK assets（改完数据再打包前跑）
```

**公开库是只读上游**：App 里「拉取公共基础库」只读、不写；无冲突时 Rust 侧直接落库（不留空确认步骤），有冲突则一项都不写、跳到「冲突」页逐项裁决。公开库读取复用私有库 token（手机上没有环境变量，匿名 60/h 且部分网络会被掐断）。

## K. 尚未完成

1. **冲突裁决的真机回执**：冲突判定逻辑已有桌面端实测（1 项待裁决、`personalized=true`、三值并列、未决拒绝提交），但**手机端界面**还没跑过一次真实冲突 —— 需要一个「本地改过、上游也改」的场景；当前 App 没有编辑入口，只能靠导入改过的 CSV 制造。
2. **编辑能力**：矩阵目前只读；改字段要走导入，后续补单元格编辑 + `immutable` 字段的「不可改，需新建」提示。
3. **`upstream-adopt` 尚未需要**：拉取落库时 `apply_decisions` 已写入字段级 provenance（`upstream/provenance.json` 里 `upstream_rev=2`、每个字段带 `upstream_value` + `origin`），所以「本地改过」的判定基线已经存在；`adopt` 只在「先有本地数据、后接公共库」时才用得上。
4. **桌面 CLI 的 release 二进制未刷新**：仓库里已含 422 修复源码，但本机 `target/release/mneme.exe` 还是修复前的。
