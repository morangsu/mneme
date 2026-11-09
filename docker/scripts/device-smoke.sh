#!/usr/bin/env bash
# 真机冒烟：容器内 adb client → 宿主机 adb server → USB 设备。
# 做四件事：确认设备在线 → 装最新 APK（若存在）→ 冷启动截图 → 扫崩溃日志。
# 不做 UI 断言（点击坐标会随版本变化），交互回归放在 test/ 下的脚本里。
set -euo pipefail
cd /work

ADB_HOST="${ADB_HOST:-host.docker.internal}"
ADB_PORT="${ADB_PORT:-5037}"
PKG="${PKG:-io.github.morangsu.mneme}"
ACTIVITY="${ACTIVITY:-.MainActivity}"
APK="android/app/build/outputs/apk/debug/app-debug.apk"

A() { adb -H "$ADB_HOST" -P "$ADB_PORT" "$@"; }

echo "== 1/5 连接宿主机 adb server ($ADB_HOST:$ADB_PORT) =="
if ! A version >/dev/null 2>&1; then
  echo "连不上 adb server。宿主机需要先起 adb（默认 127.0.0.1:5037），"
  echo "且 Docker Desktop 的 host.docker.internal 能转发到 loopback。"
  exit 1
fi
A version | head -2

echo "== 2/5 等待设备 =="
for i in $(seq 1 30); do
  LINE=$(A devices | sed -n '2p')
  case "$LINE" in
    *"\tdevice"*) break;;
    *unauthorized*) echo "设备未授权：请在手机上点「允许 USB 调试」"; exit 1;;
  esac
  sleep 2
done
A devices -l | sed -n '1,3p'
SERIAL=$(A devices | sed -n '2p' | awk '{print $1}')
[ -n "$SERIAL" ] || { echo "没有在线设备"; exit 1; }

echo "== 3/5 设备信息 =="
printf '  model=%s android=%s abi=%s\n' \
  "$(A -s "$SERIAL" shell getprop ro.product.model | tr -d '\r')" \
  "$(A -s "$SERIAL" shell getprop ro.build.version.release | tr -d '\r')" \
  "$(A -s "$SERIAL" shell getprop ro.product.cpu.abi | tr -d '\r')"

if [ -f "$APK" ]; then
  echo "== 4/5 安装 APK ($(du -h "$APK" | cut -f1)) =="
  # 直接 adb install 在这台设备上会挂住，改 push + pm install
  A -s "$SERIAL" push "$APK" /data/local/tmp/mneme.apk >/dev/null
  A -s "$SERIAL" shell pm install -r -d /data/local/tmp/mneme.apk | tail -1
else
  echo "== 4/5 跳过安装（$APK 不存在，先在宿主机打包） =="
fi

echo "== 5/5 冷启动 + 截图 + 崩溃扫描 =="
A -s "$SERIAL" shell input keyevent KEYCODE_WAKEUP >/dev/null 2>&1 || true
A -s "$SERIAL" shell am force-stop "$PKG" >/dev/null 2>&1 || true
A -s "$SERIAL" logcat -c >/dev/null 2>&1 || true
A -s "$SERIAL" shell am start -W -n "$PKG/$ACTIVITY" | sed -n '1,4p'
sleep 4

mkdir -p /work/.shots
SHOT="/work/.shots/docker-$(date +%H%M%S).png"
A -s "$SERIAL" exec-out screencap -p > "$SHOT"
printf '  截图 %s (%s)\n' "$SHOT" "$(du -h "$SHOT" | cut -f1)"

CRASH=$(A -s "$SERIAL" logcat -d -b crash 2>/dev/null | grep -cE "FATAL EXCEPTION|AndroidRuntime.*mneme" || true)
FATAL=$(A -s "$SERIAL" logcat -d 2>/dev/null | grep -cE "FATAL EXCEPTION" || true)
printf '  崩溃日志: crash_buffer=%s fatal=%s\n' "${CRASH:-0}" "${FATAL:-0}"

# 未捕获异常只允许 0 条；有就贴出来并失败
if [ "${FATAL:-0}" != "0" ]; then
  echo "--- 崩溃栈 ---"
  A -s "$SERIAL" logcat -d | grep -A 20 "FATAL EXCEPTION" | head -30
  exit 1
fi

echo "真机冒烟通过"
