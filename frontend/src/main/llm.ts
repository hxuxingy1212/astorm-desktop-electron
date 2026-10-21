/**
 * LLM 单次补全 IPC（插件 SDK `llm.chat` 通路）。
 *
 * 渲染层 `window.api.llm.ask(request, onChunk)`：
 * - 触发 `llm:ask` → napi `askLlm(request, callId)`（启动即返回）；
 * - 补全事件经后端**全局事件总线**（napi:event，`type:"llm"`）回传，
 *   渲染层 preload 按 `call_id` 过滤路由（不再为主进程维护独立流）；
 * - 后端未加载（.node 缺失）时返回 { ok:false, error }，由渲染层作为 error 事件透传。
 */
import { ipcMain } from 'electron'
import { isNapiLoaded, napiCall } from './napi'

export function registerLlmIpc(): void {
  ipcMain.handle(
    'llm:ask',
    async (
      _event,
      args: { callId?: string; request?: Record<string, unknown> }
    ): Promise<{ ok: boolean; callId?: string; error?: string }> => {
      const callId =
        typeof args?.callId === 'string' && args.callId
          ? args.callId
          : `llm-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
      try {
        if (!isNapiLoaded()) {
          return { ok: false, error: 'BACKEND_UNAVAILABLE: backend not loaded (askLlm)' }
        }
        napiCall('askLlm', (args?.request ?? {}) as Record<string, unknown>, callId)
        return { ok: true, callId }
      } catch (err) {
        return { ok: false, error: (err as Error).message }
      }
    }
  )
}