#!/usr/bin/env bash
# 无头服务端 + 单用户云容器 e2e（本地 Docker 部署测试）
#
# 覆盖：compose 部署（/data 卷 + 单令牌鉴权）→ mock LLM 驱动完整 ReAct 循环
# （工具调用/执行/结果回流）→ SSE 跟随与环形缓冲回放 → 审批链路（Network 类
# 中断 + CLI approve）→ 容器重启后持久化（sqlite 卷）。
#
# 用法：bash scripts/e2e/run_e2e.sh
#   E2E_KEEP=1 保留部署（默认结束即 down -v 清理）
#
# 风格约定：不用 set -e/-u（单步失败只记 FAIL 不中断）；脚本退出码 = 失败数。
cd "$(dirname "$0")/../.." # backend/

PORT=18669
TOKEN="e2e-token-$(date +%s)"
CLI=${ASTORM_CLI:-target/debug/astorm}
PASS=0; FAIL=0

say()  { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()   { printf '\033[1;32m[PASS]\033[0m %s\n' "$*"; PASS=$((PASS+1)); }
bad()  { printf '\033[1;31m[FAIL]\033[0m %s\n' "$*"; FAIL=$((FAIL+1)); }
# check <exit-code> <描述>：<非零记 FAIL（0 值入参时描述里的变量已就地展开）
check(){ if [ "$1" = "0" ]; then ok "$2"; else bad "$2"; fi }
# 上一步是否成功（配合 `cmd; check $? …` 使用）

cleanup() {
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null
  if [ "${E2E_KEEP:-0}" != "1" ]; then
    docker compose --env-file .env.e2e down -v --remove-orphans >/dev/null 2>&1
    rm -f .env.e2e
  fi
  return 0
}
trap cleanup EXIT

[ -x "$CLI" ] || { echo "构建 CLI…"; cargo build -q -p astorm-cli; }
export ASTORM_SERVER=http://127.0.0.1:8787
export ASTORM_TOKEN="$TOKEN"

# ---------- 1. mock LLM（宿主机，容器经 host.docker.internal 访问）----------
pkill -f "mock_llm.py" 2>/dev/null; sleep 0.3
python3 scripts/e2e/mock_llm.py "$PORT" & MOCK_PID=$!
sleep 0.5

# ---------- 2. compose 部署（单用户云容器）----------
say "compose 部署（fresh 卷 + 单令牌）"
cat > .env.e2e <<EOF
ASTORM_TOKEN=$TOKEN
ASTORM_LLM_BASE_URL=http://host.docker.internal:$PORT/v1
ASTORM_LLM_API_KEY=e2e-key
ASTORM_LLM_MODEL=mock-model
EOF
docker compose --env-file .env.e2e down -v --remove-orphans >/dev/null 2>&1
docker compose --env-file .env.e2e up -d --build >/dev/null 2>&1
HEALTH=1
for i in $(seq 1 30); do
  curl -fsS http://127.0.0.1:8787/health >/dev/null 2>&1 && { HEALTH=0; break; }
  sleep 1
done
check "$HEALTH" "容器健康检查（/health）"

CODE=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8787/api/v1/status)
[ "$CODE" = "401" ]; check $? "未带令牌被拒（401，实际 $CODE）"
"$CLI" status >/dev/null 2>&1; check $? "CLI status（Bearer 鉴权直连容器）"

# ---------- 3. 完整 ReAct 循环（工具调用 → 执行 → 结果回流 → 最终回复）----------
say "e2e 任务：工具调用全链路（auto + execute）"
OUT=$(perl -e 'alarm 120; exec @ARGV' -- "$CLI" newtask "帮我完成 e2e" --auto --mode execute 2>&1)
echo "$OUT" | grep -q "E2E_PASS"; check $? "最终回复含 E2E_PASS（经 SSE 流式跟随）"
echo "$OUT" | grep -q "\[工具\] bash"; check $? "工具调用事件（bash）可见"
echo "$OUT" | grep -q "\[完成\]"; check $? "done 终态事件"

# 工具真实落在容器文件系统（/data 卷内会话工作区）
MARKER=""
MARKER=$(docker compose --env-file .env.e2e exec -T astorm-server \
  sh -c 'find /data/workspace -name e2e-marker.txt 2>/dev/null -exec cat {} \;' 2>/dev/null | head -1 || true)
[ "$MARKER" = "E2E_TOOL_OK" ]; check $? "工具在容器 FS 实际执行（卷内容=$MARKER）"

# 历史回读：工具结果回流进消息
SID=""
SID=$("$CLI" tasks --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d[0]['id'] if d else '')" 2>/dev/null)
"$CLI" show "$SID" --json 2>/dev/null | grep -q "E2E_TOOL_OK"; check $? "show --json 含工具结果（E2E_TOOL_OK，会话 $SID）"

# ---------- 4. SSE 环形缓冲回放（after 游标续传，须在重启前）----------
say "SSE 回放：after 游标"
AUTH="Authorization: Bearer $TOKEN"
FIRST=$(curl -sN -m 2 -H "$AUTH" "http://127.0.0.1:8787/api/v1/sessions/$SID/events" 2>/dev/null | grep "^id:" | head -1 | tr -dc '0-9')
FIRST=${FIRST:-0}
NEXT=$(curl -sN -m 2 -H "$AUTH" "http://127.0.0.1:8787/api/v1/sessions/$SID/events?after=$FIRST" 2>/dev/null | grep "^id:" | head -1 | tr -dc '0-9')
NEXT=${NEXT:-0}
[ "$NEXT" -gt "$FIRST" ]; check $? "after 游标续传（$FIRST → $NEXT）"

# ---------- 5. 审批链路（Network 类中断 → CLI approve → 续跑）----------
say "审批 e2e：manual 权限 + 联网命令 → 中断 → 批准"
"$CLI" newtask "APPROVAL e2e" --mode execute --detach >/dev/null 2>&1
SID2=""
for i in $(seq 1 30); do
  SID2=$("$CLI" pending --json 2>/dev/null | python3 -c "
import json,sys
try: d=json.load(sys.stdin); print(d[0]['session'] if d else '')
except Exception: print('')" 2>/dev/null)
  [ -n "$SID2" ] && break
  sleep 1
done
[ -n "$SID2" ]; check $? "待审批任务出现在 pending（会话 $SID2）"
PENDING_DETAIL=$("$CLI" pending 2>/dev/null)
echo "$PENDING_DETAIL" | grep -qE "curl|联网|Network|网络|category"; check $? "pending 详情可读（命令/原因）"
"$CLI" approve "$SID2" >/dev/null 2>&1; check $? "CLI approve 提交决议（$SID2）"
ST=""
for i in $(seq 1 60); do
  ST=$("$CLI" tasks --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(next((s['status'] for s in d if s['id']=='$SID2'), ''))" 2>/dev/null)
  [ "$ST" = "done" ] && break
  sleep 1
done
[ "$ST" = "done" ]; check $? "批准后循环续跑至 done（终态=$ST）"
"$CLI" show "$SID2" --json 2>/dev/null | grep -q "E2E_PASS"; check $? "审批会话最终回复含 E2E_PASS"

# ---------- 6. 持久化（容器重启，/data 卷）----------
say "持久化：restart 后会话仍在"
docker compose --env-file .env.e2e restart >/dev/null 2>&1
HEALTH=1
for i in $(seq 1 30); do
  curl -fsS http://127.0.0.1:8787/health >/dev/null 2>&1 && { HEALTH=0; break; }
  sleep 1
done
check "$HEALTH" "重启后健康检查"
N=$("$CLI" tasks --json 2>/dev/null | python3 -c "import json,sys; print(len(json.load(sys.stdin)))" 2>/dev/null)
[ "${N:-0}" -ge 2 ]; check $? "重启后 sqlite 卷会话仍在（$N 个）"
"$CLI" show "$SID" >/dev/null 2>&1; check $? "重启后历史消息回读正常"

# 重启后 SSE 可连；回放为空属预期（环形缓冲是内存态，历史走 show/message.jsonl）
SSE_CODE=$(curl -s -o /dev/null -w '%{http_code}' -m 2 -H "$AUTH" \
  "http://127.0.0.1:8787/api/v1/sessions/$SID/events")
[ "$SSE_CODE" = "200" ]; check $? "重启后 SSE 端点可连（HTTP $SSE_CODE；回放为空属预期）"

# ---------- 汇总 ----------
say "汇总"
printf '%s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" = "0" ]
