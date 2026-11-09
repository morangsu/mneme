#!/usr/bin/env bash
# 宿主机侧包装：保证 adb server 活着，再执行传入的命令。
#
# 为什么需要它：这台机器上 adb server 是沙箱/终端会话的子进程，
# 命令一结束就被回收；而容器里的 adb client 必须连到宿主机 127.0.0.1:5037
# （Docker Desktop 会把 host.docker.internal 代理到宿主 loopback）。
# 所以「起 server」和「跑容器」必须在同一个 shell 生命周期内完成。
#
# 用法（在仓库根目录）：
#   bash docker/scripts/host-adb.sh docker compose -f docker/compose.yaml run --rm device
#   bash docker/scripts/host-adb.sh adb devices
set -euo pipefail

if ! command -v adb >/dev/null 2>&1; then
  echo "宿主机没有 adb，请先装 platform-tools 或加入 PATH" >&2
  exit 1
fi

# 已经在跑就不重复起（start-server 是幂等的，这里只是省一次输出）
adb start-server >/dev/null 2>&1 || true

# 等一小会儿，确认真的在监听（沙箱里启动有时会慢半拍）
for _ in $(seq 1 20); do
  if adb devices >/dev/null 2>&1; then break; fi
  sleep 0.25
done

exec "$@"
