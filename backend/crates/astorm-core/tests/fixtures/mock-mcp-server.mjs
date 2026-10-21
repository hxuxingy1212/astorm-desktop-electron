// 最小 MCP stdio 服务器（测试夹具）：换行分隔 JSON-RPC，协议版本 2024-11-05。
// 工具：echo（回显）/ add（求和）/ boom（返回 isError）。
import { createInterface } from 'node:readline'

const TOOLS = [
  {
    name: 'echo',
    description: '回显给定文本（测试用）',
    inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] }
  },
  {
    name: 'add',
    description: '两数求和（测试用）',
    inputSchema: {
      type: 'object',
      properties: { a: { type: 'number' }, b: { type: 'number' } },
      required: ['a', 'b']
    }
  },
  { name: 'bash', description: '与内置工具同名（命名冲突用例）', inputSchema: { type: 'object', properties: {} } },
  { name: 'boom', description: '总是返回错误（isError 用例）', inputSchema: { type: 'object', properties: {} } }
]

const send = (msg) => process.stdout.write(JSON.stringify(msg) + '\n')

const rl = createInterface({ input: process.stdin })
rl.on('line', (line) => {
  const text = line.trim()
  if (!text) return
  let msg
  try {
    msg = JSON.parse(text)
  } catch {
    return
  }
  // 通知（无 id）：不回
  if (msg.id === undefined) return
  const reply = (result) => send({ jsonrpc: '2.0', id: msg.id, result })
  switch (msg.method) {
    case 'initialize':
      reply({
        protocolVersion: '2024-11-05',
        capabilities: { tools: {} },
        serverInfo: { name: 'mock-echo', version: '0.0.1' }
      })
      break
    case 'tools/list':
      reply({ tools: TOOLS })
      break
    case 'tools/call': {
      const name = msg.params?.name
      const args = msg.params?.arguments ?? {}
      if (name === 'echo') reply({ content: [{ type: 'text', text: `echo: ${args.text}` }] })
      else if (name === 'add') reply({ content: [{ type: 'text', text: String(Number(args.a) + Number(args.b)) }] })
      else if (name === 'bash') reply({ content: [{ type: 'text', text: `mcp-bash(${JSON.stringify(args)})` }] })
      else if (name === 'boom') reply({ isError: true, content: [{ type: 'text', text: '工具执行失败（测试）' }] })
      else reply({ content: [{ type: 'text', text: `unknown tool: ${name}` }], isError: true })
      break
    }
    default:
      send({ jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: `Method not found: ${msg.method}` } })
  }
})
