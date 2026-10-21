#!/usr/bin/env python3
"""mock_llm.py — OpenAI /v1/chat/completions 兼容的最小 mock（SSE 流式，仅标准库）。

供 astorm 无头服务端 e2e 使用（无需真实 LLM 凭据）。脚本化两轮行为：

- 会话消息里**没有** role=tool（首轮）→ 下发一次工具调用：
  - 用户消息含 "BROWSER" → 调用 **browser 工具** 打开页面并读文本（浏览器 e2e：
    验证服务端 headless Chromium 后端 / 桌面 iframe 桥都能真的驱动页面）
  - 用户消息含 "MCP" → 调用 **MCP 服务器工具** `echo`（补齐功能 e2e：验证连接后的
    MCP 工具真的进了会话工具集、调用经 stdio 子进程往返）
  - 用户消息含 "APPROVAL" → bash 命令带 `curl https://example.com`（manual 权限下
    触发 Network 类审批，默认策略拒绝所有出站）
  - 否则 → bash 纯工作区写读命令（直接执行，不触发审批）
- 有 role=tool（工具结果已回）→ 输出含 `E2E_PASS` 的最终文本并 stop。

协议：`data: {chunk}` 行 + `data: [DONE]` 哨兵；tool_calls 增量按 index 聚合，
一个 chunk 携带完整 id/name/arguments（与 llm-provider 的解析约定一致）。

用法：python3 mock_llm.py [port]（默认 18669）
"""

import json
import os
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18669

# 两轮命令模板（容器内工作区执行）
CMD_PLAIN = "echo E2E_TOOL_OK > e2e-marker.txt && cat e2e-marker.txt"
CMD_NET = (
    "echo E2E_TOOL_PENDING > e2e-net.txt && "
    "curl -sS https://example.com -o /dev/null && "
    "echo E2E_TOOL_OK >> e2e-net.txt && cat e2e-net.txt"
)


# 浏览器 e2e：默认打开内置的本地页面（容器/宿主都用 127.0.0.1 上的 e2e 静态服务）
BROWSER_URL = os.environ.get("E2E_BROWSER_URL", "https://example.com")

# MCP e2e：工具名 = mock MCP 服务器（tests/fixtures/mock-mcp-server.mjs）暴露的 echo
MCP_TOOL_NAME = "echo"
MCP_TOOL_ARGS = {"text": "MCP_ECHO_OK"}


def button_ref(tool_results):
    """从上一条 snapshot 工具结果里解析「按钮」的引用 —— 模拟真实模型读快照再点的行为。

    两种驱动都要认：
    - 服务端直连 CDP（回落实现）：`@e1 button "点我"`
    - agent-browser CLI（工具侧默认）：`- button "点我" [ref=e2]`（**eN 含标题等非交互元素**，
      故不能写死 @e1）
    解析不到则退回 CSS 选择器（CLI 与回落实现都支持）。
    """
    for msg in reversed(tool_results):
        text = msg.get("content") or ""
        if not isinstance(text, str):
            text = json.dumps(text, ensure_ascii=False)
        for line in text.splitlines():
            if "button" not in line and "点我" not in line:
                continue
            m = re.search(r"\[ref=(e\d+)\]", line) or re.search(r"@(e\d+)\b", line)
            if m:
                return "@" + m.group(1)
    return "#btn"


def chunk(delta, finish=None):
    return {
        "id": "chatcmpl-e2e",
        "object": "chat.completion.chunk",
        "model": "mock-model",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        try:
            body = json.loads(self.rfile.read(length) or b"{}")
        except json.JSONDecodeError:
            body = {}
        messages = body.get("messages", [])
        tool_results = [m for m in messages if m.get("role") == "tool"]
        has_tool_result = bool(tool_results)
        last_user = next(
            (m.get("content", "") for m in reversed(messages) if m.get("role") == "user"),
            "",
        )

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        try:
            if "BROWSER" in str(last_user):
                # browser 是「按需加载」工具：先 tool_load，再按脚本走一串动作
                # （navigate → snapshot → read text → screenshot），逐轮校验动作词汇与后端
                steps = [
                    ("tool_load", {"action": "load", "tools": ["browser"]}),
                    ("browser", {"action": "navigate", "url": BROWSER_URL, "description": "打开测试页"}),
                    ("browser", {"action": "snapshot", "description": "看页面结构"}),
                    ("browser", {"action": "click", "target": "@e1", "description": "点击按钮"}),
                    ("browser", {"action": "read", "what": "text", "target": "#out", "description": "读点击结果"}),
                    ("browser", {"action": "read", "what": "text", "target": "body", "description": "读页面正文"}),
                    ("browser", {"action": "screenshot", "filename": "e2e-browser.png", "description": "截图"}),
                ]
                if len(tool_results) >= len(steps):
                    self.emit([
                        {"role": "assistant", "content": None},
                        {"content": "E2E_PASS: 浏览器动作已完成。"},
                    ], finish="stop")
                    return
                name, arguments = steps[len(tool_results)]
                if name == "browser" and arguments.get("action") == "click":
                    arguments = {**arguments, "target": button_ref(tool_results)}
                call = {
                    "index": 0,
                    "id": f"call_e2e_browser_{len(tool_results)}",
                    "type": "function",
                    "function": {"name": name, "arguments": json.dumps(arguments)},
                }
                self.emit([
                    {"role": "assistant", "content": None},
                    {"tool_calls": [call]},
                ], finish="tool_calls")
                return
            if not has_tool_result:
                if "MCP" in str(last_user):
                    call = {
                        "index": 0,
                        "id": "call_e2e_mcp",
                        "type": "function",
                        "function": {
                            "name": MCP_TOOL_NAME,
                            "arguments": json.dumps(MCP_TOOL_ARGS),
                        },
                    }
                else:
                    command = CMD_NET if "APPROVAL" in str(last_user) else CMD_PLAIN
                    call = {
                        "index": 0,
                        "id": "call_e2e_1",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": json.dumps({
                                "command": command,
                                "description": "e2e 标记文件写入与读回",
                            }),
                        },
                    }
                self.emit([
                    {"role": "assistant", "content": None},
                    {"tool_calls": [call]},
                ], finish="tool_calls")
            else:
                self.emit([
                    {"role": "assistant", "content": None},
                    {"content": "E2E_PASS: 工具已执行，"},
                    {"content": "标记文件已写入会话工作区。"},
                ], finish="stop")
        except (BrokenPipeError, ConnectionResetError):
            pass  # 客户端提前断开（如取消）

    def emit(self, deltas, finish):
        for d in deltas:
            self.wfile.write(f"data: {json.dumps(chunk(d))}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(f"data: {json.dumps(chunk({}, finish))}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, *args):  # 静默访问日志
        pass


if __name__ == "__main__":
    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    print(f"mock-llm listening on :{PORT}", flush=True)
    server.serve_forever()
