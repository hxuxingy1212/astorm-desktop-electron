#!/usr/bin/env bash
# 「web 补齐功能」e2e（独立容器，不动 live 部署）
#
# 覆盖本轮补齐的四项能力（commit 4c411ba / ce60041）：
#   ① 目录选择器：/fs/list-dirs（默认起点=会话工作区根、只列目录、隐藏目录跳过、
#      越界拒绝）+ /fs/mkdir（新建/非法名/越界）
#   ② 审计导出：bash 执行与审批放行都落审计 → read_audit_log 返回可解析的 JSON
#      字符串（渲染层据此拼 ndjson 走 Blob 下载）
#   ③ 无文件管理器降级：web shim 的 `__web` 标记（驱动渲染层隐藏按钮/提示路径）
#      + shim 的 fs.openFolder 明确 unsupported + 服务端无「打开文件夹」类命令
#   ④ MCP 连接器（core 实现）：写配置即自动连接 → 工具进会话工具集 →
#      mock LLM 驱动模型调用 MCP 工具 → 结果经 stdio 子进程往返回流 → 断开后工具消失
#   ⑤ 浏览器工具（容器形态：服务端 headless Chromium）：
#      tool_load browser → navigate 本地夹具页 → snapshot 出 @eN 引用 → click →
#      read 得页面 JS 结果（证明点击真的生效）→ screenshot 落 PNG；
#      驱动 = 镜像内置的 agent-browser CLI（端点文件指向服务端 Chromium 的浏览器级 CDP WS），
#      缺失时回落服务端直连 CDP 实现——此处断言 CLI 路径确实在被使用；
#      另用 node 连面板画面流 WS：收帧 + 鼠标点击 + 文本输入（title 变化即证据）；
#      ⑤c 另测页面内 target=_blank / window.open → 本页导航（不落面板看不见的隐形页签）
#   ⑥ git 只读探测（bridge）：目录缺失 / 非仓库 → 空值（不把 run_git 的 stderr 文本当分支名），
#      真仓库照常返回分支名与本地分支列表（防修过头）
#
# 用法：bash scripts/e2e/run_gap_e2e.sh
#   ASTORM_E2E_IMAGE=astorm-server:dev  指定镜像（默认即此；不存在则报错提示先 build）
#   E2E_KEEP=1                          保留容器与卷（默认结束即清理）
#
# 与 run_e2e.sh 的隔离：独立容器名/端口/卷（live 部署常用 8787 + astorm-data 卷，
# 本脚本用 18787 + astorm-gap-e2e-data），**绝不 down -v 现有部署**。
#
# 风格约定：不用 set -e/-u（单步失败只记 FAIL 不中断）；脚本退出码 = 失败数。
# JSON 载荷一律由 python 构造（jbody/jpost/jbridge），避免 shell 嵌套引号踩坑。
cd "$(dirname "$0")/../.." # backend/
# 探针脚本目录（本脚本所在目录），供 WS 探针等按绝对路径调用
E2E_DIR="$(cd "$(dirname "$0")" && pwd)"

PORT=18787
MOCK_PORT=18670
IMAGE=${ASTORM_E2E_IMAGE:-astorm-server:dev}
NAME=astorm-gap-e2e
VOLUME=astorm-gap-e2e-data
TOKEN="gap-e2e-token-$(date +%s)"
CLI=${ASTORM_CLI:-target/debug/astorm}
PASS=0; FAIL=0

say()  { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
ok()   { printf '\033[1;32m[PASS]\033[0m %s\n' "$*"; PASS=$((PASS+1)); }
bad()  { printf '\033[1;31m[FAIL]\033[0m %s\n' "$*"; FAIL=$((FAIL+1)); }
check(){ if [ "$1" = "0" ]; then ok "$2"; else bad "$2"; fi }
# 断言 haystack 含 / 不含 needle（"该出现"/"不该出现"场景；case 模式比 grep 更省事）
has()    { case "$1" in *"$2"*) return 0;; *) return 1;; esac }
not_has(){ case "$1" in *"$2"*) return 1;; *) return 0;; esac }
# 单条断言：chk <条件函数与参数…> —— 直接用 has/not_has 的返回值
chk(){ local cond=$1; shift; if [ "$cond" = "has" ]; then has "$@"; else not_has "$@"; fi; }

cleanup() {
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null
  if [ "${E2E_KEEP:-0}" != "1" ]; then
    docker rm -f "$NAME" >/dev/null 2>&1
    docker volume rm "$VOLUME" >/dev/null 2>&1
  fi
  return 0
}
trap cleanup EXIT

AUTH="Authorization: Bearer $TOKEN"
BASE="http://127.0.0.1:$PORT/api/v1"
# jget <json 文本> <python 表达式（d = 解析结果）> → 求值输出
jget() { printf '%s' "$1" | python3 -c "import json,sys
try: d=json.load(sys.stdin)
except Exception: d=None
try: print($2)
except Exception: print('')"; }
# jbody <python 字典字面量> → JSON 文本（由 python 构造，无 shell 引号问题）
jbody() { python3 -c "import json; print(json.dumps($1))"; }
# jpost <path> <python 字典字面量> → 响应体
jpost() {
  local tmp; tmp=$(mktemp)
  jbody "$2" > "$tmp"
  curl -s -X POST "$BASE$1" -H "Content-Type: application/json" -H "$AUTH" --data-binary @"$tmp"
  rm -f "$tmp"
}
get() { curl -s "$BASE$1" -H "$AUTH"; }
# jpost_stdin <path>：载荷从 stdin 读（已是 JSON 文本）
jpost_stdin() {
  local tmp; tmp=$(mktemp)
  cat > "$tmp"
  curl -s -X POST "$BASE$1" -H "Content-Type: application/json" -H "$AUTH" --data-binary @"$tmp"
  rm -f "$tmp"
}
# jbridge <python 字典字面量> → /api/v1/bridge 响应体
# 注意：bridge 返回**裸值**（命令返回值本身；错误才是 {code,message}），
# 与 /fs/* 等 REST 端点的 {code,data,message} 信封不同。
jbridge() { jpost /bridge "$1"; }

docker image inspect "$IMAGE" >/dev/null 2>&1 || {
  echo "镜像 $IMAGE 不存在：cd backend && docker build -f Dockerfile -t $IMAGE .."; exit 2; }
[ -x "$CLI" ] || { echo "构建 CLI…"; cargo build -q -p astorm-cli; }
export ASTORM_SERVER="http://127.0.0.1:$PORT"
export ASTORM_TOKEN="$TOKEN"

# ---------- 0. mock LLM + 独立容器 ----------
say "起 mock LLM(:$MOCK_PORT) 与独立容器($NAME, 端口 $PORT)"
pkill -f "mock_llm.py $MOCK_PORT" 2>/dev/null; sleep 0.3
python3 scripts/e2e/mock_llm.py "$MOCK_PORT" >/tmp/gap-e2e-mock.log 2>&1 & MOCK_PID=$!
sleep 0.5

docker rm -f "$NAME" >/dev/null 2>&1
docker volume rm "$VOLUME" >/dev/null 2>&1
# macOS Docker Desktop 自带 host.docker.internal；Linux 需显式映射
HOSTMAP=""
[ "$(uname -s)" = "Linux" ] && HOSTMAP="--add-host=host.docker.internal:host-gateway"
docker run -d --name "$NAME" -p "$PORT:8787" -v "$VOLUME:/data" $HOSTMAP \
  -e ASTORM_TOKEN="$TOKEN" \
  -e ASTORM_LLM_BASE_URL="http://host.docker.internal:$MOCK_PORT/v1" \
  -e ASTORM_LLM_API_KEY=e2e-key \
  -e ASTORM_LLM_MODEL=mock-model \
  "$IMAGE" >/dev/null 2>&1
HEALTH=1
for i in $(seq 1 40); do
  curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 && { HEALTH=0; break; }
  sleep 1
done
check "$HEALTH" "容器健康检查 /health"
[ "$HEALTH" = "0" ] || { say "汇总"; printf '%s passed, %s failed\n' "$PASS" "$FAIL"; exit 1; }

CODE=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/v1/status")
[ "$CODE" = "401" ]; check $? "未带令牌被拒 [401 实际 $CODE]"
"$CLI" status >/dev/null 2>&1; check $? "CLI status 直连容器"

# ---------- 1. 目录选择器 ----------
say "① 目录选择器（/fs/list-dirs + /fs/mkdir）"
ROOT=/data
WSROOT=/data/workspace
docker exec "$NAME" mkdir -p "$WSROOT" >/dev/null 2>&1

L=$(get "/fs/list-dirs")
L_PATH=$(jget "$L" "d['data']['path']")
L_ROOT=$(jget "$L" "d['data']['root']")
[ "$L_PATH" = "$WSROOT" ]; check $? "缺省起点 = 会话工作区根 [实际 $L_PATH]"
[ "$L_ROOT" = "$ROOT" ]; check $? "可浏览根 = 数据根 [实际 $L_ROOT]"

# 只列目录 / 隐藏目录跳过：造 普通文件 + 隐藏目录 + 普通目录
docker exec "$NAME" sh -c "mkdir -p $WSROOT/e2e-picker-demo $WSROOT/.e2e-hidden && echo x > $WSROOT/e2e-plain.txt" >/dev/null 2>&1
L=$(get "/fs/list-dirs?path=$WSROOT")
NAMES=$(jget "$L" "[x['name'] for x in d['data']['dirs']]")
chk has "$NAMES" "e2e-picker-demo"; check $? "目录进列表 [$NAMES]"
chk not_has "$NAMES" "e2e-plain.txt"; check $? "普通文件不进列表（只列目录）"
chk not_has "$NAMES" "e2e-hidden"; check $? "隐藏目录不进列表"
PARENT=$(jget "$L" "d['data']['parent']")
[ "$PARENT" = "$ROOT" ]; check $? "上级可回退到数据根 [实际 $PARENT]"

# 新建文件夹（选择器「新建」路径）
M=$(jpost /fs/mkdir "{'parent':'$WSROOT','name':'e2e-mkdir-demo'}")
M_PATH=$(jget "$M" "d['data']['path']")
[ "$M_PATH" = "$WSROOT/e2e-mkdir-demo" ]; check $? "新建文件夹成功 [实际 $M_PATH]"
docker exec "$NAME" test -d "$WSROOT/e2e-mkdir-demo"; check $? "新建目录真实落在容器卷"

# 非法名 / 越界拒绝
R=$(jpost /fs/mkdir "{'parent':'$WSROOT','name':'.evil'}")
C=$(jget "$R" "d['code']")
[ "$C" = "40001" ]; check $? "非法文件夹名被拒 [code=$C]"
R=$(jpost /fs/mkdir "{'parent':'$WSROOT','name':'a/b'}")
C=$(jget "$R" "d['code']")
[ "$C" = "40001" ]; check $? "含分隔符的文件夹名被拒 [code=$C]"
R=$(jpost /fs/mkdir "{'parent':'/etc','name':'x'}")
C=$(jget "$R" "d['code']")
[ "$C" = "40001" ]; check $? "越界新建被拒（/etc）[code=$C]"
R=$(get '/fs/list-dirs?path=/etc')
C=$(jget "$R" "d['code']")
[ "$C" = "40001" ]; check $? "越界浏览被拒（/etc）[code=$C]"
# --path-as-is：不让 curl 自己先把 ../ 规范化掉，验服务端词法规范化后的越界判定
R=$(curl -s --path-as-is "$BASE/fs/list-dirs?path=/data/workspace/../../etc" -H "$AUTH")
C=$(jget "$R" "d['code']")
[ "$C" = "40001" ]; check $? "词法规范化后越界仍被拒（../.. 绕行）[code=$C]"

# ---------- 2. 审计导出（bash 执行 + 审批放行 → read_audit_log 载荷）----------
say "② 审计导出：审批流 → read_audit_log 可下载载荷"
"$CLI" newtask "APPROVAL gap e2e" --mode execute --detach >/dev/null 2>&1
SID_A=""
for i in $(seq 1 30); do
  SID_A=$("$CLI" pending --json 2>/dev/null | python3 -c "
import json,sys
try: d=json.load(sys.stdin); print(d[0]['session'] if d else '')
except Exception: print('')" 2>/dev/null)
  [ -n "$SID_A" ] && break
  sleep 1
done
[ -n "$SID_A" ]; check $? "审批中断出现 [会话 $SID_A]"
"$CLI" approve "$SID_A" >/dev/null 2>&1
ST=""
for i in $(seq 1 60); do
  ST=$("$CLI" tasks --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(next((s['status'] for s in d if s['id']=='$SID_A'), ''))" 2>/dev/null)
  [ "$ST" = "done" ] && break
  sleep 1
done
[ "$ST" = "done" ]; check $? "批准后续跑至 done [终态=$ST]"

RAW=$(jbridge "{'command':'read_audit_log','args':{'session_id':'$SID_A'}}")
N=$(jget "$RAW" "len(json.loads(d)) if isinstance(d, str) else -1")
[ "${N:-0}" -ge 1 ]; check $? "read_audit_log 返回 JSON 字符串且非空 [${N} 条]"
APPROVALS=$(jget "$RAW" "sorted({str(e.get('approval')) for e in json.loads(d)})")
chk has "$APPROVALS" "agree"; check $? "含审批放行记录 [approval=$APPROVALS]"
HAS_NET=$(jget "$RAW" "any(e.get('category')=='network' for e in json.loads(d))")
[ "$HAS_NET" = "True" ]; check $? "含 network 分类条目（原拦截）[$HAS_NET]"
LINES=$(jget "$RAW" "len([json.dumps(e, ensure_ascii=False) for e in json.loads(d)])")
[ "${LINES:-0}" -eq "${N:-0}" ]; check $? "ndjson 载荷可拼装 [行数 $LINES = 条目数 $N]"

# ---------- 3. 无文件管理器降级（契约级）----------
say "③ 无文件管理器降级：shim 契约 + 服务端无对应命令"
SHIM=$(curl -s "http://127.0.0.1:$PORT/web-bridge.js")
chk has "$SHIM" "__web: true"; check $? "web shim 声明 __web（驱动 isWebShell 降级分支）"
chk has "$SHIM" "openFolder: function () { return unsupported"; check $? "shim 的 fs.openFolder 明确 unsupported"
chk has "$SHIM" "listDirs"; check $? "shim 暴露 listDirs（目录选择器走服务端）"
R=$(jbridge "{'command':'open_folder','args':{'path':'/data'}}")
C=$(jget "$R" "d['code']")
[ "$C" = "40001" ]; check $? "服务端无「打开文件夹」命令（未知命令）[code=$C]"

# ---------- 4. MCP 连接器 ----------
say "④ MCP：配置 → 自动连接 → 工具进会话 → 模型调用 → 断开"
docker cp crates/astorm-core/tests/fixtures/mock-mcp-server.mjs "$NAME:/tmp/mock-mcp-server.mjs" >/dev/null 2>&1
check $? "mock MCP 服务器脚本进容器"
CFG_FILE=/tmp/gap-e2e-mcp.json
cat > "$CFG_FILE" <<'JSON'
{"mcpServers": {"mock": {"command": "node", "args": ["/tmp/mock-mcp-server.mjs"]}}}
JSON
# content 是「JSON 文本」而非对象：由 python 读文件后作为字符串塞进载荷
R=$(python3 -c "
import json
cfg = open('$CFG_FILE').read()
print(json.dumps({'command': 'write_mcp_config_file', 'args': {'content': cfg}}))" | jpost_stdin /bridge)
OK=$(jget "$R" "d.get('ok')")
[ "$OK" = "True" ]; check $? "写 MCP 配置（落盘 + 重载 + 自动连接）"

CONN=""
for i in $(seq 1 30); do
  R=$(jbridge "{'command':'list_mcp_servers','args':{}}")
  CONN=$(jget "$R" "d[0]['connected'] if d else ''")
  [ "$CONN" = "True" ] && break
  sleep 1
done
[ "$CONN" = "True" ]; check $? "服务器自动连接 [connected=$CONN]"
docker exec "$NAME" test -f /data/config/mcp_servers.json; check $? "配置落在卷内 /data/config/mcp_servers.json"

TOOLS=$(jbridge "{'command':'list_tools','args':{'category':'mcp'}}")
NAMES=$(jget "$TOOLS" "sorted(t['name'] for t in d)")
chk has "$NAMES" "'echo'"; check $? "MCP 工具进工具目录 [$NAMES]"
chk has "$NAMES" "'mock__bash'"; check $? "与内置重名的 MCP 工具加前缀（mock__bash）"
SRC=$(jget "$TOOLS" "d[0]['source']['type'] if d else ''")
[ "$SRC" = "mcp" ]; check $? "工具来源标记为 mcp [source.type=$SRC]"

# 模型调用 MCP 工具（mock LLM：用户消息含 MCP → tool_call echo{"text":"MCP_ECHO_OK"}）
IDS_BEFORE=$("$CLI" tasks --json 2>/dev/null | python3 -c "import json,sys; print(' '.join(s['id'] for s in json.load(sys.stdin)))")
OUT=$(perl -e 'alarm 120; exec @ARGV' -- "$CLI" newtask "MCP gap e2e" --auto --mode execute 2>&1)
chk has "$OUT" "E2E_PASS"; check $? "MCP 会话跑到最终回复（含 E2E_PASS）"
chk has "$OUT" "[工具] echo"; check $? "工具调用事件可见（echo）"
export IDS_BEFORE
SID_M=$("$CLI" tasks --json 2>/dev/null | python3 -c "
import json,sys,os
before=set(os.environ.get('IDS_BEFORE','').split())
print(next((s['id'] for s in json.load(sys.stdin) if s['id'] not in before), ''))" 2>/dev/null)
[ -n "$SID_M" ]; check $? "MCP 会话入库 [$SID_M]"
SHOW=$("$CLI" show "$SID_M" --json 2>/dev/null)
chk has "$SHOW" "echo: MCP_ECHO_OK"; check $? "MCP 工具结果回流（stdio 子进程往返 echo: MCP_ECHO_OK）"

# 断开 → 工具从目录消失（连接态与工具目录一致）
jbridge "{'command':'disconnect_mcp_server','args':{'name':'mock'}}" >/dev/null
R=$(jbridge "{'command':'list_mcp_servers','args':{}}")
CONN=$(jget "$R" "d[0]['connected'] if d else ''")
[ "$CONN" = "False" ]; check $? "断开后 connected=false [$CONN]"
R=$(jbridge "{'command':'list_tools','args':{'category':'mcp'}}")
NTOOLS=$(jget "$R" "len(d or [])")
[ "${NTOOLS:-1}" = "0" ]; check $? "断开后 MCP 工具不再下发 [${NTOOLS} 个]"
# 重连（UI 的「连接」按钮路径）
R=$(jbridge "{'command':'connect_mcp_server','args':{'name':'mock'}}")
MSG=$(jget "$R" "d")
chk has "$MSG" "已连接"; check $? "重连命令返回可读回执 [$MSG]"

# ---------- 5. 浏览器工具（服务端 headless Chromium）----------
say "⑤ 浏览器工具：tool_load → navigate → snapshot → click → read → screenshot"
# 夹具页：写在容器卷内（file:// 加载，不依赖外网）
docker exec "$NAME" sh -c 'mkdir -p /data/workspace && cat > /data/workspace/e2e-page.html <<'"'"'HTML'"'"'
<!doctype html><html><head><meta charset="utf-8"><title>E2E 测试页</title></head>
<body><h1>E2E 浏览器测试页</h1>
<button id="btn" onclick="document.getElementById('"'"'out'"'"').textContent='"'"'E2E_CLICK_OK'"'"'">点我</button>
<div id="out"></div><input id="name" placeholder="姓名" /></body></html>
HTML' >/dev/null 2>&1
check $? "夹具页写入容器卷（file:///data/workspace/e2e-page.html）"

# mock LLM 的 BROWSER 分支需要 URL（重启 mock 带上环境变量）
kill "$MOCK_PID" 2>/dev/null; sleep 0.5
E2E_BROWSER_URL="file:///data/workspace/e2e-page.html" python3 scripts/e2e/mock_llm.py "$MOCK_PORT" >/tmp/gap-e2e-mock.log 2>&1 & MOCK_PID=$!
sleep 0.8

IDS_BEFORE=$("$CLI" tasks --json 2>/dev/null | python3 -c "import json,sys; print(' '.join(s['id'] for s in json.load(sys.stdin)))")
OUT=$(perl -e 'alarm 240; exec @ARGV' -- "$CLI" newtask "BROWSER gap e2e" --auto --mode execute 2>&1)
chk has "$OUT" "E2E_PASS"; check $? "浏览器会话跑到最终回复（含 E2E_PASS）"
chk has "$OUT" "[工具] browser"; check $? "browser 工具调用事件可见"
export IDS_BEFORE
SID_B=$("$CLI" tasks --json 2>/dev/null | python3 -c "
import json,sys,os
before=set(os.environ.get('IDS_BEFORE','').split())
print(next((s['id'] for s in json.load(sys.stdin) if s['id'] not in before), ''))" 2>/dev/null)
[ -n "$SID_B" ]; check $? "浏览器会话入库 [$SID_B]"
SHOW=$("$CLI" show "$SID_B" --json 2>/dev/null)
chk has "$SHOW" "file:///data/workspace/e2e-page.html"; check $? "navigate 结果含目标 URL"
SNAP_REF=$(printf '%s' "$SHOW" | python3 -c "
import json,re,sys
t=json.dumps(json.load(sys.stdin), ensure_ascii=False)
print('yes' if re.search(r'button', t) and re.search(r'(@e[0-9]+|ref=e[0-9]+)', t) else 'no')" 2>/dev/null)
[ "$SNAP_REF" = "yes" ]; check $? "snapshot 出可点元素引用（@eN / [ref=eN] 两种驱动都认）"
chk has "$SHOW" "E2E_CLICK_OK"; check $? "click + read 得到页面 JS 结果（点击真的生效）"
SHOT=$(printf '%s' "$SHOW" | python3 -c "
import json,sys,re
m = re.search(r'browser-snapshot/([^\"]+\.png)', json.dumps(json.load(sys.stdin)))
print(m.group(1) if m else '')")
[ -n "$SHOT" ]; check $? "screenshot 返回落盘路径 [$SHOT]"
docker exec "$NAME" sh -c "test -s \$(find /data/workspace -name '$SHOT' | head -1)"; check $? "截图文件真实落在容器卷（非空）"
PNG_MAGIC=$(docker exec "$NAME" sh -c "f=\$(find /data/workspace -name '$SHOT' | head -1); head -c 4 \"\$f\" | od -An -tx1 | tr -d ' \n'" 2>/dev/null)
[ "$PNG_MAGIC" = "89504e47" ]; check $? "截图是合法 PNG（magic=$PNG_MAGIC）"

# 工具侧驱动 = agent-browser CLI（镜像内置）——端点文件 + daemon 痕迹即证据
CLIV=$(docker exec "$NAME" sh -c 'agent-browser --version 2>/dev/null' || true)
chk has "$CLIV" "agent-browser"; check $? "镜像内置 agent-browser CLI [$CLIV]"
EP=$(docker exec "$NAME" sh -c 'cat /data/electron/browser-agent.json 2>/dev/null' || true)
chk has "$EP" '"wsUrl"'; check $? "浏览器桥接端点落盘（工具走 CLI 的前提）"
chk has "$EP" "$SID_B"; check $? "端点绑定本会话（sessionId 命中）"
WS_URL=$(printf '%s' "$EP" | python3 -c "import json,sys; print(json.load(sys.stdin).get('wsUrl',''))" 2>/dev/null)
chk has "$WS_URL" "ws://127.0.0.1:"; check $? "端点 wsUrl 指向服务端 Chromium [$WS_URL]"
DSOCK=$(docker exec "$NAME" sh -c 'ls /root/.agent-browser/astorm-*.sock 2>/dev/null | head -1' || true)
[ -n "$DSOCK" ]; check $? "agent-browser daemon 已建立（动作确经 CLI）[$DSOCK]"

# 面板画面流 WS：收帧 + 鼠标点击 + 文本输入（title 变化即证据）
say "⑤b 面板画面流：帧下行 + 鼠标 / 键盘上行"
docker exec "$NAME" sh -c 'cat > /data/workspace/ws-page.html <<'"'"'HTML'"'"'
<!doctype html><html><head><meta charset="utf-8"><title>WS 输入测试</title></head>
<body style="margin:0">
<div style="position:fixed;inset:0;background:#eef" onclick="document.title='"'"'CLICK:'"'"'+Math.round(event.clientX)+'"'"','"'"'+Math.round(event.clientY)"></div>
<input style="position:fixed;left:20px;top:200px;width:300px;height:30px" oninput="document.title='"'"'TEXT:'"'"'+this.value" />
</body></html>
HTML' >/dev/null 2>&1
WS_JSON=$(node "$E2E_DIR/browser-ws-probe.mjs" frames "$PORT" "$TOKEN" "$SID_B")
WF=$(jget "$WS_JSON" "d.get('frames')")
[ "${WF:-0}" -ge 1 ]; check $? "画面流收到帧 [frames=$WF]"
WT=$(jget "$WS_JSON" "str(d.get('titles'))")
chk has "$WT" "TEXT:ZCode"; check $? "鼠标聚焦 + 文本输入生效 [titles=$WT]"
# 端点 frameUrl 跟随页面（面板导航后服务端回写；审批 origin/展示读它）
EPF=$(docker exec "$NAME" sh -c 'cat /data/electron/browser-agent.json 2>/dev/null' || true)
FRAME_URL=$(printf '%s' "$EPF" | python3 -c "import json,sys; print(json.load(sys.stdin).get('frameUrl',''))" 2>/dev/null)
chk has "$FRAME_URL" "ws-page.html"; check $? "端点 frameUrl 跟随面板导航 [$FRAME_URL]"
WE=$(jget "$WS_JSON" "len(d.get('errors') or [])")
[ "${WE:-1}" = "0" ]; check $? "画面流无错误 [errors=$WE]"
# 视口固定 1280×900：state 与帧尺寸一致（面板坐标映射按它换算，不一致就会点偏）
WS_ST=$(jget "$WS_JSON" "sorted(set(d.get('stateSizes') or []))[-1] if d.get('stateSizes') else ''")
[ "$WS_ST" = "1280x900" ]; check $? "state 视口固定 1280x900 [state=$WS_ST]"
WS_FS=$(jget "$WS_JSON" "sorted(set(d.get('frameSizes') or []))[-1] if d.get('frameSizes') else ''")
[ "$WS_FS" = "1280x900" ]; check $? "画面帧尺寸随视口（非 800x600 默认窗口）[frame=$WS_FS]"

# ---------- 浏览器页面内的「开新窗口」→ 本页导航 ----------
say "⑤c 页面内 target=_blank / window.open：本页导航（面板看得见）"
docker exec "$NAME" sh -c 'cat > /data/workspace/popup-page.html <<'"'"'HTML'"'"'
<!doctype html><html><head><meta charset="utf-8"><title>POPUP_SRC</title></head>
<body style="margin:0">
<a id="lk" href="file:///data/workspace/popup-target.html" target="_blank"
   style="position:fixed;left:0;top:0;width:100%;height:120px;font-size:22px;background:#eef">新窗口链接</a>
<button id="bt" style="position:fixed;left:20px;top:200px;font-size:20px"
  onclick="window.open(&quot;file:///data/workspace/popup-target.html&quot;,&quot;_blank&quot;)">window.open</button>
</body></html>
HTML' >/dev/null 2>&1
docker exec "$NAME" sh -c 'cat > /data/workspace/popup-target.html <<'"'"'HTML'"'"'
<!doctype html><html><head><meta charset="utf-8"><title>POPUP_TARGET</title></head>
<body><h1>POPUP_TARGET</h1></body></html>
HTML' >/dev/null 2>&1
check $? "夹具页写入容器卷（popup-page/popup-target）"
POP_JSON=$(node "$E2E_DIR/browser-ws-probe.mjs" popup "$PORT" "$TOKEN" "$SID_B")
POP_T=$(jget "$POP_JSON" "str(d.get('titles'))")
chk has "$POP_T" "POPUP_TARGET"; check $? "页面内 target=_blank → 本页导航到目标页 [titles=$POP_T]"
POP_U=$(jget "$POP_JSON" "str(d.get('urls'))")
chk has "$POP_U" "popup-target.html"; check $? "window.open 同样本页导航（活动页 URL 变了）"

# ---------- git 探测健壮性（普通任务不显示 git / 不吐 fatal 文本） ----------
say "⑥ git 探测：非仓库 / 目录缺失 → 空；真仓库照常"
R=$(jbridge "{'command':'git_branch_state','args':{'dir':'/data/workspace/not-exist-dir'}}")
CUR=$(jget "$R" "d.get('current')")
[ -z "$CUR" ]; check $? "目录缺失 → current 为空（不再把 fatal 文本当分支名）[$CUR]"
NLOC=$(jget "$R" "len(d.get('local') or [])")
[ "${NLOC:-1}" = "0" ]; check $? "目录缺失 → local 空 [$NLOC 项]"
R=$(jbridge "{'command':'git_list_branches','args':{'dir':'/data/workspace/not-exist-dir'}}")
NB=$(jget "$R" "len(d or [])")
[ "${NB:-1}" = "0" ]; check $? "目录缺失 → 分支列表空 [$NB 项]"
docker exec "$NAME" sh -c 'mkdir -p /data/workspace/plain-task' >/dev/null 2>&1
R=$(jbridge "{'command':'git_branch_state','args':{'dir':'/data/workspace/plain-task'}}")
CUR=$(jget "$R" "d.get('current')")
[ -z "$CUR" ]; check $? "非仓库目录 → current 为空 [$CUR]"
R=$(jbridge "{'command':'git_is_repo','args':{'dir':'/data/workspace/plain-task'}}")
[ "$(jget "$R" "d")" = "False" ]; check $? "非仓库目录 → is_repo=false"
# 别修过头：真仓库仍要正常返回（新建一个最小仓库）
docker exec "$NAME" sh -c 'cd /data/workspace/plain-task && git init -q && git -c user.email=e2e@astorm -c user.name=e2e commit -q --allow-empty -m init && git checkout -q -b feature-x' >/dev/null 2>&1
check $? "夹具：容器内建最小 git 仓库（feature-x）"
R=$(jbridge "{'command':'git_branch_state','args':{'dir':'/data/workspace/plain-task'}}")
CUR=$(jget "$R" "d.get('current')")
[ "$CUR" = "feature-x" ]; check $? "真仓库 → 分支名照常 [$CUR]"
NLOC=$(jget "$R" "len(d.get('local') or [])")
[ "${NLOC:-0}" -ge 2 ]; check $? "真仓库 → 本地分支列表非空 [$NLOC 项]"

# ---------- 汇总 ----------
say "汇总"
printf '%s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" = "0" ]
