# astorm CLI 用户手册

`astorm` 是 astorm 无头服务端（astorm-server）的命令行客户端：在任何能访问服务端的机器上派任务、跟进度、处理审批、做自动化集成。

> 版本要求：CLI 与服务端同仓发布（`--version` 查看版本）。服务端部署见
> [deployment.md](./deployment.md)；HTTP API 细节见 [cloud-server.md](./cloud-server.md)。

---

## 1. 安装与连接

```bash
# 构建（仓库根目录；需要 Rust ≥1.85）
cd backend && cargo build --release -p astorm-cli
# 产物：backend/target/release/astorm
```

连接参数（三选一组合，均可被命令行旗标覆盖）：

| 方式 | 变量/旗标 | 说明 |
|---|---|---|
| 服务端地址 | env `ASTORM_SERVER` 或 `--server` | 缺省 `http://127.0.0.1:8787` |
| 访问令牌 | env `ASTORM_TOKEN` 或 `--token` | 服务端 `ASTORM_TOKEN` 的值 |

推荐把两个环境变量写进 shell profile 或 CI 的 secret：

```bash
export ASTORM_SERVER=http://your-server:8787
export ASTORM_TOKEN=your-token
astorm status        # 通了会打印服务端版本/模型配置
```

> 首次部署时服务端若未显式配置令牌，会随机生成一个并打印在启动日志里
> （`ASTORM_TOKEN=…`），复制它即可。**生产环境务必显式设置令牌。**

---

## 2. 命令总览

| 命令 | 作用 |
|---|---|
| `astorm newtask <消息>` | 创建任务并执行（默认跟随输出直到完成；`--detach` 只创建） |
| `astorm tasks` | 列出全部任务（id / 状态 / 标题） |
| `astorm show <id>` | 查看任务历史消息；`--follow` 实时观战 |
| `astorm stop <id>` | 取消任务当前循环 |
| `astorm pending` | 列出等待审批的任务（含命令、原因、request_id） |
| `astorm approve <id>` | 同意审批（`--scope session` 本会话内同目标不再询问） |
| `astorm deny <id>` | 拒绝审批 |
| `astorm status` | 服务端状态（版本 / 数据目录 / 供应商与模型） |

全局旗标（所有命令可用）：

- `--server <URL>`：服务端地址
- `--token <TOKEN>`：访问令牌
- `--json`：输出原始 JSON（脚本消费；全局旗标，写在子命令前后均可）

退出码：`0` 成功；`1` 失败（错误信息走 stderr；`--json` 时输出可解析 JSON）。

---

## 3. 日常使用

### 3.1 派任务

```bash
# 前台跟随：实时流式输出，直到任务完成（Ctrl-C 只断开跟随，任务继续跑）
astorm newtask "审查 backend/crates 的代码质量并输出报告" --mode execute

# 后台执行：创建即返回，之后用 show --follow 观战
astorm newtask "把这份日志按错误类型归纳" --mode execute --detach
astorm show <任务id> --follow

# 指定工作区（agent 在该目录内读写文件；缺省用服务端默认工作区）
astorm newtask "跑一遍测试并修复失败" --mode execute -w /data/workspace/myproj

# 指定模型 / 供应商
astorm newtask "总结这份需求" --model your-model --provider default

# 自动授权：工具调用不再逐次弹审批（危险命令/黑名单仍被硬拦）
astorm newtask "初始化项目脚手架" --mode execute --auto
```

**模式与权限（重要）**：

| 参数 | 取值 | 语义 |
|---|---|---|
| `--mode` | `plan`（默认）/ `execute` / `auto` | plan=只读（写操作被硬拒）；execute=可写；auto=可写+更放手 |
| `--auto` | 旗标 | 权限 permission=auto：工具免逐次审批（安全静态预检仍生效） |

想真正让 agent 改文件，**必须 `--mode execute`（或 auto）**——这是 plan 模式的安全设计，不是故障。

### 3.2 查看与观战

```bash
astorm tasks                       # 任务表
astorm tasks --json                # 脚本用：原始 JSON 数组
astorm show <id>                   # 历史消息（用户/助手/工具结果摘要）
astorm show <id> --json            # 完整消息记录（含 toolCalls/工具结果全文）
astorm show <id> --follow          # 实时跟随（流式正文 + 工具卡 + 终态）
```

跟随输出中会出现的标记：`[工具] bash`（工具调用）、`[等待审批]`（需要你处理）、
`[完成]` / `[已取消]` / `[错误]`（终态）。跟随中 Ctrl-C 只断开本地输出，任务不受影响。

### 3.3 停止任务

```bash
astorm stop <id>                   # 取消当前循环；会话状态 → cancelled
```

### 3.4 审批（plan/execute + 手动权限时）

agent 触发需要授权的操作（联网、工作区外写入等）时会中断等待审批：

```bash
astorm pending                     # 待审批列表：分类/目标/完整命令/原因
astorm approve <id>                # 同意一次（本次放行，目标进沙箱白名单重跑）
astorm approve <id> --scope session # 本会话内同目标不再询问
astorm deny <id>                   # 拒绝（agent 收到拒绝原因，会改换思路）
```

审批卡内容示例：

```
任务 450e4226…（执行curl命令）
  类型 approval · request_id ca4343cb…
  分类 network · 目标 ["example.com"]
  命令: curl -sS https://example.com -o /dev/null && echo NET_OK
  原因: 命令需要联网（curl（网络客户端）），而当前策略默认拒绝所有出站连接…
```

不想逐次审批：任务创建时用 `--auto`；或 Web UI 设置页调整安全策略。

### 3.5 服务端状态

```bash
astorm status
# astorm-server 0.1.0（linux-aarch64）
# 数据目录 /data/data · 工作空间 /data/workspace
# 供应商 default（https://…/v1）模型: your-model
```

---

## 4. 脚本与自动化

`--json` 输出机器可读结果，配合 jq 可编排完整流程：

```bash
# 创建任务并取 id（注意用 --json 提取 id，纯文本输出含多处 UUID 不可靠）
SID=$(astorm newtask "每日巡检" --mode execute --auto --detach --json | jq -r .id)

# 轮询到终态
while :; do
  ST=$(astorm tasks --json | jq -r --arg id "$SID" '.[] | select(.id==$id) | .status')
  [ "$ST" = "done" ] || [ "$ST" = "error" ] || [ "$ST" = "cancelled" ] && break
  sleep 10
done

# 取最终回复（最后一条 assistant 消息）
astorm show "$SID" --json | jq -r '[.[] | select(.role=="assistant")][-1].content'

# 审批自动化：pending 有就全批（谨慎；或先 deny 再人工介入）
astorm pending --json | jq -r '.[].session' | xargs -r -n1 astorm approve
```

CI 场景建议：`--detach --json` 创建 → 轮询终态 → `show --json` 取产物与结论；
超时兜底用 `astorm stop`。

---

## 5. 疑难排查

| 现象 | 原因与处理 |
|---|---|
| `[401 Unauthorized] 缺少或无效的 Bearer 令牌` | 令牌错/未设。核对服务端 `ASTORM_TOKEN` 与本地 `ASTORM_TOKEN` |
| `error sending request for url …` | 服务端不可达：地址、端口、防火墙；`curl $ASTORM_SERVER/health` 先探 |
| 任务一直「运行中」无输出 | 多为等待审批：`astorm pending` 查看；或模型在长思考，用 `show --follow` 观战 |
| `会话不存在` | 服务端重启后内存注册表为空——CLI 会自动重建重试一次；若仍报错说明该会话属旧数据卷 |
| 模型报 `HTTP 400 … is not a multimodal model` | 给非多模态模型发了图片附件（与桌面版一致的限制）；换多模态模型或不带图 |
| plan 模式写文件被拒 | 用 `--mode execute` / `--auto`；已存在的会话在 Web UI 会话设置里切换 |
| 任务报无模型 / 模型不可用 | LLM 供应商未配置：先在 Web 设置页「模型」添加（容器不预置），或用服务端 env `ASTORM_LLM_*` 预置；`astorm status` 可查当前供应商与默认模型 |

---

## 6. 与 Web UI 的关系

CLI 与 Web UI（`http://<host>:8787/`）操作的是**同一个服务端、同一批会话**：
CLI 创建的任务在 Web UI 侧栏可见，Web 里等待审批的任务也可以用 `astorm approve`
处理。编排器/脚本用 CLI，人机交互用 Web，两者按需混用。
