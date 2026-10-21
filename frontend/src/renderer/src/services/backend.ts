/**
 * 后端调用适配层。
 *
 * 经 preload 的 window.api.backend.invoke 走真实后端:
 * - 技能 / 工具 / 定时任务 → 主进程转发 Rust napi(astorm-backend.node)
 * - 设置 / MCP 服务器 → 主进程本地实现(settings.json / mcp_servers.json)
 *
 * 无 mock 降级:后端未加载时抛错,由调用方展示失败原因(桌面应用必须真实后端)。
 */

export type BackendArgs = Record<string, unknown>

/** 后端未加载错误前缀(main/backend.ts napiCall 抛出) */
const UNAVAILABLE = 'BACKEND_UNAVAILABLE'

export async function invoke(command: string, args?: BackendArgs): Promise<any> {
  const bridge = window.api?.backend?.invoke
  if (!bridge) throw new Error(`${UNAVAILABLE}: backend bridge missing (${command})`)
  try {
    // 序列化兜底:store 直接透传 Vue reactive proxy(数组字段如 category/tags)
    // 会导致 ipcRenderer 结构化克隆失败("An object could not be cloned"),统一剥成纯数据
    const plain: BackendArgs = args === undefined ? {} : JSON.parse(JSON.stringify(args))
    return await bridge(command, plain)
  } catch (err) {
    throw err
  }
}