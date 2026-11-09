# Docker 测试环境

把「不需要 Windows GUI」的验证全部搬进容器，跑出来的结论与宿主机无关（同一份 Rust 代码、同一份领域包）。

## 一眼看懂

| 服务 | 干什么 | 需要什么 |
| --- | --- | --- |
| `test` | `cargo test --workspace`（当前 75 项） | 无 |
| `lint` | `cargo clippy -- -D warnings` | 无 |
| `server` | 建库 → 导入 → 起 axum → 12 条 HTTP 断言 | 无 |
| `device` | 装 APK → 冷启动 → 截图 → 崩溃扫描 | 宿主机 adb + USB 手机 |
| `shell` | 进容器手动折腾 | 无 |

## 命令

```bash
# 建镜像（首次 ~2 分钟，之后命中缓存）
docker compose -f docker/compose.yaml build

# 内核 / 服务端
docker compose -f docker/compose.yaml run --rm test
docker compose -f docker/compose.yaml run --rm lint
docker compose -f docker/compose.yaml run --rm server

# 真机（必须用包装脚本，见下）
bash docker/scripts/host-adb.sh docker compose -f docker/compose.yaml run --rm device

# 进容器
docker compose -f docker/compose.yaml run --rm shell
```

缓存：`cargo-registry` / `cargo-git` / `target-linux` 三个命名卷。`target-linux` 独立于宿主机的
`target/`，因为宿主机是 Windows/MSVC 产物，Linux 容器复用不了反而会互相踩。

## 真机测试为什么需要 `host-adb.sh`

容器里的 adb 是 **client**，它连的是宿主机的 adb **server**（USB 设备在宿主机上，容器拿不到）。

- Docker Desktop 会把 `host.docker.internal` 代理到宿主机的 loopback，所以 server 只绑
  `127.0.0.1:5037` 也能被容器访问——**不需要 `adb -a` 监听全网卡**，也就不会把手机暴露给同网段。
- 但这台机器上 adb server 是终端会话的子进程，终端一结束就被回收。所以「起 server」和
  「跑容器」必须在同一个 shell 生命周期内完成，`host-adb.sh` 就是把这两步串起来。

```bash
bash docker/scripts/host-adb.sh docker compose -f docker/compose.yaml run --rm device
```

## 为什么不把 Android SDK 也塞进镜像

打 APK 需要 `platforms;android-36` + `build-tools` + NDK（约 3–4 GB），而宿主机上的 NDK/Gradle
缓存已经是热的（一次打包 30 秒）。容器负责**跑测试**，宿主机负责**打包**——这是有意的分工。

如果将来确实要全容器化，另建 `docker/Dockerfile.android`：
1. `dl.google.com` 在这台机器上被掐断，SDK 包必须走 `https://mirrors.cloud.tencent.com/AndroidSDK/`
   （已验证可用，`platform-tools_r37.0.2-linux.zip` 能下）；
2. Gradle / AGP 走 `maven.google.com`（可用）与 Maven Central（可用）；
3. NDK 交叉编译要重设 `.cargo/config.toml` 里的 linker 路径（现在是 Windows 绝对路径），
   建议容器版用 `cargo-ndk` 而不是手写 linker。

## 踩过的坑（都已修，别再踩）

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `rustc: command not found` | 用 `bash -lc` 起容器，登录 shell 读 `/etc/profile` 把 PATH 重置，丢掉 `/usr/local/cargo/bin` | 用 `bash -c`；脚本用 `#!/usr/bin/env bash`（非登录） |
| `adb: Network is unreachable` | compose 里给 `host.docker.internal` 加了 `extra_hosts: host-gateway`，Docker Desktop 解析成 **IPv6** 网关 | 删掉 `extra_hosts`，用 Docker Desktop 自带 DNS |
| `adb: Connection refused` | 宿主机 adb server 没在跑（终端退出被回收） | 用 `host-adb.sh` 包一层 |
| `adb install` 挂住不动 | 这台 iQOO 上流式安装会卡 | 改 `adb push` + `pm install -r -d`（脚本已按这个写） |
| 端口 5037 被占 | 宿主机已有 adb server | 包装脚本会复用，不会重复起 |

## 已验证的回执（2026-11-09）

```
docker compose run --rm test    → passed=75 failed=0
docker compose run --rm server  → 12/12 断言通过（state/views/view/reverse/quiz/judge/review/export）
device（容器 → 宿主 adb）        → adb 37.0.2 连上 server，adb devices 正常返回
```
