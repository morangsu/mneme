# mneme · Android 原生工程（uniffi + Compose）

`mneme` 是「本地优先、可同步、领域无关」的记忆/复习引擎。领域逻辑（中文归一化、倒排反查、出题、FSRS、三方合并、上游冲突判定、GitHub 同步）**全部在 Rust 侧**；本工程只是 UI 与系统能力层：Kotlin + Jetpack Compose + Material3，通过 uniffi 生成的绑定调用 Rust。

- 包名 / applicationId / namespace：`io.github.morangsu.mneme`
- 五个底部标签：矩阵 / 反查 / 复习 / 同步 / 冲突
- 深色主题与 Win 端网页一致：`#16171A` / `#1E2024` / `#E6E7EA` / `#6AA9FF`

## 1. 当前状态（已接通，不是占位）

| 部件 | 状态 |
|---|---|
| `crates/mneme-ffi` | **已实现** 17 个 `#[uniffi::export]` 方法，全部返回 JSON 字符串 |
| Kotlin 绑定 | **已生成**：`app/src/main/java/io/github/morangsu/mneme/ffi/mneme_ffi.kt`（uniffi 0.29.5 顶层函数） |
| `MnemeApi.kt` | **已生成**：把顶层函数包成 `object MnemeApi`，APP 侧统一用 `MnemeApi.xxx()` |
| `Api.kt` 的 `MnemeBridge` | **已切到真实绑定**：按 method 分派到 `MnemeApi`；仅当缺 `.so` 抛 `UnsatisfiedLinkError` 时才回退 `MnemeApiStub` |
| `rustup target` | `aarch64-linux-android` 已安装 |
| `libmneme_ffi.so` | ✅ **已编译**：`jniLibs/arm64-v8a/libmneme_ffi.so`，5.9 MB，ELF64 AArch64，导出 16 个 uniffi 入口，仅依赖 `libdl/libm/libc` |
| APK | ✅ **已构建**：`app/build/outputs/apk/debug/app-debug.apk`，37 MB（仅 arm64-v8a），可直接 `adb install` |

也就是说：Rust 逻辑、Kotlin 绑定、APP 调用链、原生库、APK 打包**全部闭环并实测通过**（Gradle `BUILD SUCCESSFUL`）。

## 2. 打开工程

| 项 | 版本（本机实测） |
|---|---|
| Android Studio | Ladybug (2024.2)+ |
| JDK | **21 必需**：`D:\scoop\apps\corretto21-jdk\current`。本机默认 `JAVA_HOME` 是 JDK 27，Gradle 8.9 **无法启动**（报 `27.0.1.0.3`），必须显式切换 |
| Gradle / AGP / Kotlin | 8.9（wrapper 已生成）/ 8.7.3 / 2.0.21 |
| Compose | BOM 2024.12.01 / Material3 |
| compileSdk / targetSdk / minSdk | 35 / 35 / 26 |
| Android SDK | `D:\scoop\apps\android-clt\current`（platforms;android-35 + build-tools;35.0.0 已装） |
| Android NDK | `D:\scoop\apps\android-ndk\current`（r30，来自 scoop bucket `hoilc_scoop-lemon`） |

> 注意：`android-clt` 里的 `ndk/` 只是个空占位目录，真正的 NDK 是独立装的 `android-ndk` 包，别搞混。

命令行构建（无需 Android Studio）：

```bash
cd android
export JAVA_HOME="D:/scoop/apps/corretto21-jdk/current"
export PATH="/d/scoop/apps/corretto21-jdk/current/bin:$PATH"
./gradlew :app:assembleDebug          # 首次约 7 分钟（下依赖），之后 ~20 秒
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

用 Android Studio「Open」选择本 `android/` 目录。本地数据库由 Rust 侧 SQLite 管理（**不引入 Room**），网络同步由 Rust 侧完成（**不引入 Retrofit**）；APP 第三方库只有 AndroidX、ML Kit 中文 OCR、kotlinx-coroutines，`org.json` 为系统内置。

目录要点：

```
app/src/main/java/io/github/morangsu/mneme/
├── Api.kt                # ★ MnemeBridge（真实绑定分派）+ MnemeApiStub 兜底
├── MainActivity.kt       # 初始化 DB、通知权限、入队周期同步
├── MnemeApp.kt / Ui.kt   # Scaffold + 5 tab + 深色主题
├── MatrixScreen.kt / ReverseScreen.kt / QuizScreen.kt
├── SyncScreen.kt / ConflictScreen.kt / OcrInboxScreen.kt
├── SyncWorker.kt / Notify.kt / Db.kt
└── ffi/
    ├── mneme_ffi.kt      # uniffi 生成（勿手改）
    └── MnemeApi.kt       # 手写包装 object
app/src/main/jniLibs/arm64-v8a/   # ← libmneme_ffi.so 放这里
```

## 3. 生成 / 重新生成绑定与 .so

在仓库根 `mneme/` 执行：

```bash
# 1) Kotlin 绑定（已生成过；Rust FFI 接口变动后重跑）
cargo build -p mneme-ffi
cargo run -q -p mneme-ffi --bin generate-bindings -- generate \
  --library target/debug/mneme_ffi.dll \
  --language kotlin \
  --out-dir /tmp/mneme-kotlin
# 把 /tmp/mneme-kotlin/io/github/morangsu/mneme/ffi/mneme_ffi.kt
# 拷到 app/src/main/java/io/github/morangsu/mneme/ffi/
# Windows 用 target/debug/mneme_ffi.dll；Linux/macOS 分别是 libmneme_ffi.so / .dylib

# 2) arm64 动态库（NDK 已在 D:\scoop\apps\android-ndk\current）
#    路径在仓库根 .cargo/config.toml 的 [target.aarch64-linux-android] 与 [env] 里，
#    换机器时改那 4 条路径即可。
rustup target add aarch64-linux-android
cargo build --target aarch64-linux-android --release -p mneme-ffi
cp target/aarch64-linux-android/release/libmneme_ffi.so \
   android/app/src/main/jniLibs/arm64-v8a/libmneme_ffi.so
```

> 为什么不用 `cargo ndk`：本机 `cargo install cargo-ndk` 会因依赖编译失败（`E0463 can't find crate for url`，并行编译竞态），而且 NDK r30 在 Windows 上的 `aarch64-linux-android26-clang` 是 `.cmd` 批处理，rustc/cc 用 `CreateProcess` 直接调不起来。所以统一走 **`clang.exe` + 显式 `--target`**，等价且更可控。若你机器上 cargo-ndk 装得成功，用 `cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release -p mneme-ffi` 也可以。

APK 侧两条必要的构建约束（已在 `app/build.gradle.kts` 里）：

- **必须依赖 JNA**：uniffi 生成的 Kotlin 用 `com.sun.jna.{Structure,Pointer,Library}`，且要取 `@aar` 变体（内含各 ABI 的 `libjnidispatch.so`），否则 `Unresolved reference 'Structure'`。
- **`abiFilters += "arm64-v8a"`**：JNA 与 ML Kit 自带 5 个 ABI，不限制的话 72 MB，限制后 37 MB。


`proguard-rules.pro` 已保留 `io.github.morangsu.mneme.ffi.**` 与 JNI 方法；AGP 会自动打包 `src/main/jniLibs`。没有 `.so` 时 APP 仍能用 Stub 数据编译运行，方便先调 UI。

## 4. 三仓库拓扑

| 仓库 | 角色 | 权限 |
|---|---|---|
| `morangsu/mneme` | 公共：软件服务本体（源码 / 发布） | 公共只读 |
| `morangsu/mneme-data-public` | 公共：**对外开放的基础数据库**，只含教材基础字段，只读上游 | 公共只读（匿名 60/h，条件请求省配额） |
| `morangsu/mneme-data-private` | 私有：个性化内容 + 各设备事件分片 / 快照 / 同步日志 | 私有读写，`${ENV:MNEME_GITHUB_TOKEN}` |

设备间同步只连私有库；公共库**不会自动同步**，必须由用户主动「拉取」。拉取时做字段级三方合并，个人字段（掌握度、同义、批注、炮制…）一律不回传公共库。每次 PUT 在 GitHub 上就是一个 commit，天然可溯源。

`topology.json` 由 `mneme topology-example` 生成；同步配置 `sync.json` 由 `mneme config-example` 生成，私有库 target id 固定为 `github-private`。

## 5. 产品硬规则：冲突必须由用户逐项决定

- 三值**并排**：`base`（上次公共库值）/ `upstream`（新值）/ `local`（我的值），并显示教材页码；
- 每项三选一：`保留我的` / `采用上游` / `两者都留`，可填自定义值；
- `personalized`（个性化）与 `immutable`（危险区·原样）徽标；
- 建议 `suggestion` 只是批量填充，仍需用户主动提交；
- **未全部决定时提交按钮禁用**；Rust `apply_decisions` 同样返回 `Unresolved` 且**一项都不写库**；
- 判定「用户改过」是保守的：只要 `local != 上次公共库值` 就视为个性化调整，不依赖 provenance 是否被及时更新；
- `KeepMine` 会把 base 推进到本次上游值，避免同一冲突反复出现；
- 决策日志同时写本地与私有库 `sync/upstream-decisions.jsonl`。

`decisionsJson` 示例（`decision` 大小写、snake_case 都接受）：

```json
[
  {"conflict_id":"582b6fe1f3ce36edec7b","decision":"KeepMine","custom_value":null},
  {"conflict_id":"cf-3","decision":"take_upstream","custom_value":"发汗解肌，温通经脉"}
]
```

## 6. FFI 契约（全部返回 JSON 字符串）

```kotlin
MnemeApi.openDatabase(path: String, device: String)
MnemeApi.state() / views() / view(id) / reverse(cluesJson)
MnemeApi.quiz(limit: UInt) / answer(cardId) / review(cardId, grade: UByte) / judge(cardId, input)
MnemeApi.sync(targetId, dry) / importData(content, filename, dry) / exportData(format)
MnemeApi.upstreamPull() / upstreamResolve(planId, decisionsJson) / upstreamStatus() / upstreamAdopt()
```

主要返回形态：

| 方法 | JSON |
|---|---|
| `state` | `{schema,entities,groups,cards,events,rev,aliases,device,upstream_rev,sqlite}` |
| `view` | `{columns:[{key,label,immutable}],primary,hidden,group_by,rows:[{列名:值,_group}]}` |
| `reverse` | `{clues,items:[{id,name,group,score,summary}]}` |
| `quiz` | `{due,cards:[{id,prompt,direction,choices,boundary}]}`（`choices` 为同药群干扰项） |
| `answer` / `judge` | `{answer,accept,boundary}` / `{score,correct,matched}` |
| `sync` | `{ok,report,sha256,retries,conflicts,bytes_up,bytes_down,target,backend}` |
| `exportData` | `{format,content,bytes,sha256,filename}` |
| `upstreamPull` | `PullPlan`：`{id,from_rev,to_rev,auto,conflicts,additions,removals,summary,...}` |
| `upstreamResolve` | `{ok,plan_id,auto_applied,conflicts_resolved,added,removed,keep_mine,take_upstream,keep_both,new_rev,decision_log_dispatched_to_private,log}` |

`Conflict` 字段：`id,entity_id,entity_name,field,label,base,upstream,local,immutable,personalized,suggestion,kind,page`。

## 7. 已对齐清单（原「需要 Rust 侧对齐」项已全部解决）

1. `mneme-ffi` 已实现，`MnemeApi` object 已就位，错误统一 `{"error":"..."}`；
2. `quiz(UInt)` / `review(UByte)` 与 Rust `u32`/`u8` 对齐，grade 在 Rust 侧 `clamp(1,4)`；
3. `PullPlan` 已附加 `summary` 字段，UI 可直接显示；
4. `upstreamResolve` 接受 `KeepMine|TakeUpstream|KeepBoth` 与 `keep_mine|take_upstream|keep_both` 两种写法；
5. `Conflict.kind` 三值：`BothChanged|ImmutableClash|UpstreamRemovedLocallyEdited`；
6. `exportData` 已包装成 JSON 而非裸二进制；
7. `reverse(cluesJson)` 按「字段 label / key → 值」解析字符串数组；
8. `importData` / `sync` 回执含 `sha256` / `bytes_up` / `conflicts`；
9. Rust 侧全局状态用 `OnceLock<Mutex<App>>`，所有 FFI 调用线程安全；
10. 数据库路径 `filesDir/mneme/data`，`deviceId` 稳定存 SharedPreferences，Rust 按该 device 写 `events/<device>.jsonl`。
