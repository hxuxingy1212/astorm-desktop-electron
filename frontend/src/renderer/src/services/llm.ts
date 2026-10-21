/**
 * LLM 单次补全服务（插件 SDK `llm.chat` 通路）。
 *
 * 事件契约与后端 `ask_llm` / 主进程 llm.ts 对齐：
 * - `chunk` / `reasoning`：增量（delta）
 * - `end`：流正常结束（usage / finishReason）
 * - `error`：错误（message），其后必跟 `end`
 */

export interface AskLlmMessage {
  role: 'system' | 'user' | 'assistant'
  content: string
}

export interface AskLlmRequest {
  /** 模型引用 id（缺省用默认模型） */
  model?: string
  /** 供应商名（缺省用默认供应商） */
  provider?: string
  messages: AskLlmMessage[]
  /** 思考强度（可选）：low / medium / high */
  reasoningEffort?: string
}

export interface LlmChunkEvent {
  type: 'chunk' | 'reasoning' | 'end' | 'error'
  delta?: string
  message?: string
  usage?: { prompt: number; completion: number; total: number } | null
  finishReason?: string
}

/** 请求一次流式补全；onEvent 依序收到事件（end/error 后自动停止）；返回取消函数。 */
export function askLlm(request: AskLlmRequest, onEvent: (ev: LlmChunkEvent) => void): () => void {
  return window.api.llm.ask(request, (payload) => {
    onEvent(payload.chunk as LlmChunkEvent)
  })
}