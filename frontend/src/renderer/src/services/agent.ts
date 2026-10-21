/**
 * 流式请求上下文:与后端契约为准。
 * 前端不内置 mock agent,消息流式输出全部由真实后端(Rust napi)驱动。
 */
export interface StreamContext {
  provider?: string
  model?: string
  thinkingLevel?: number
  /** 会话模式:plan | build | auto(前端)→ 后端 plan | execute | auto */
  mode?: string
  /** 权限模式:ask | allow(前端)→ 后端 manual | auto */
  permission?: string
}