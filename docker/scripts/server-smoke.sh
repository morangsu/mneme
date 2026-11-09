#!/usr/bin/env bash
# 服务端 HTTP 冒烟：建库 → 导入 → 起 axum → 打真实请求并断言。
# 目的不是替代 cargo test，而是验证「桌面端要调的那条链路」在 Linux 容器里也是通的。
set -euo pipefail
cd /work

PORT="${PORT:-18080}"
BASE="http://127.0.0.1:${PORT}"
DB="$(mktemp -d)"

DOMAIN="domains/materia-medica.toml"
[ -f "$DOMAIN" ] || DOMAIN="android/app/src/main/assets/domains/materia-medica.toml"
CSV="domains/sample-herbs.csv"
[ -f "$CSV" ] || CSV="android/app/src/main/assets/domains/materia-medica.csv"
[ -f "$DOMAIN" ] || { echo "找不到领域包（domains/ 与 assets 都没有）"; exit 1; }
[ -f "$CSV" ]   || { echo "找不到示例数据"; exit 1; }

cleanup() {
  [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null || true
  rm -rf "$DB"
}
trap cleanup EXIT

echo "== 1/6 构建 CLI 与 server =="
cargo build -q -p mneme-cli -p mneme-server

echo "== 2/6 建库并导入示例 =="
./target/debug/mneme --data "$DB" init --domain "$DOMAIN" >/dev/null
./target/debug/mneme --data "$DB" import --file "$CSV" | tail -2

echo "== 3/6 启动服务 :$PORT =="
./target/debug/mneme-server --data "$DB" --port "$PORT" >/tmp/server.log 2>&1 &
SRV=$!
for i in $(seq 1 60); do
  if curl -sf "$BASE/api/health" >/dev/null 2>&1; then break; fi
  if ! kill -0 "$SRV" 2>/dev/null; then echo "server 启动失败："; cat /tmp/server.log; exit 1; fi
  sleep 0.5
done

fail=0
check() { # check <名字> <期望> <实际>
  if [ "$2" = "$3" ]; then
    printf '  ✓ %-34s %s\n' "$1" "$3"
  else
    printf '  ✗ %-34s 期望[%s] 实际[%s]\n' "$1" "$2" "$3"
    fail=$((fail+1))
  fi
}
check_ge() {
  if [ "$3" -ge "$2" ] 2>/dev/null; then
    printf '  ✓ %-34s %s (>=%s)\n' "$1" "$3" "$2"
  else
    printf '  ✗ %-34s 期望>=%s 实际[%s]\n' "$1" "$2" "$3"
    fail=$((fail+1))
  fi
}

echo "== 4/6 基础接口 =="
check "GET /api/health" "ok" "$(curl -s "$BASE/api/health")"
STATE=$(curl -s "$BASE/api/state")
check "state.schema" "中药学" "$(echo "$STATE" | jq -r '.schema')"
check_ge "state.entities" 6 "$(echo "$STATE" | jq -r '.entities')"
check_ge "state.cards" 1 "$(echo "$STATE" | jq -r '.cards')"
N_ENT=$(echo "$STATE" | jq -r '.entities')

echo "== 5/6 视图 / 反查 / 复习 / 判分 / 导出 =="
check_ge "views 数量" 3 "$(curl -s "$BASE/api/views" | jq -r '.views | length')"
check "view.matrix 行数 == 实体数" "$N_ENT" "$(curl -s "$BASE/api/view/matrix" | jq -r '.rows | length')"

REV=$(curl -s -G "$BASE/api/reverse" --data-urlencode "clue=功效:宣肺平喘")
check_ge "反查命中数" 1 "$(echo "$REV" | jq -r '.items | length')"
check "反查首位药名" "麻黄" "$(echo "$REV" | jq -r '.items[0].name')"

QUIZ=$(curl -s "$BASE/api/quiz?limit=3")
check_ge "quiz 取卡" 1 "$(echo "$QUIZ" | jq -r '.cards | length')"
CARD=$(echo "$QUIZ" | jq -r '.cards[0].id')

JUDGE=$(curl -s -X POST "$BASE/api/judge" -H 'content-type: application/json' \
  -d "{\"card_id\":\"$CARD\",\"input\":\"麻黄\"}")
check "judge 返回 score" "true" "$(echo "$JUDGE" | jq -r 'has("score")')"

REVIEW=$(curl -s -X POST "$BASE/api/review" -H 'content-type: application/json' \
  -d "{\"card_id\":\"$CARD\",\"grade\":3}")
check "review 落库" "1" "$(echo "$REVIEW" | jq -r '.applied')"

check "导出 CSV 含药名" "true" "$(curl -s "$BASE/api/export?format=csv" | grep -q '麻黄' && echo true || echo false)"

echo "== 6/6 结果 =="
if [ "$fail" -ne 0 ]; then
  echo "失败 $fail 项"
  echo "--- server.log 尾部 ---"
  tail -20 /tmp/server.log
  exit 1
fi
echo "服务端冒烟全通过"
