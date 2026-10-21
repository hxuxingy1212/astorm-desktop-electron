/**
 * 会话级设置的**纯映射层**(无 store / 无 window 依赖,可单独断言)。
 *
 * 模式 / 审批 / 思考强度 / 模型 / 供应商是**会话的持久化属性**,真相在后端会话记录里
 * (见 `stores/sessions.ts` 的 `settingsOf` / `setSessionSettings`)。本模块只管
 * 「前端措辞 ↔ 后端枚举」的换算,两侧名字不同容易写错,集中在这里并配断言。
 *
 * 后端枚举(napi string enum,见 docs/backend-contract.md):
 *   mode: plan | execute | auto     permission: manual | auto
 *   reasoning_effort: none | low | medium | high | max
 */

/** 对话模式:计划 / 执行 / 自动 */
export type ChatMode = 'plan' | 'build' | 'auto'
/** 权限模式:手动授权 / 自动授权 */
export type ChatPermission = 'ask' | 'allow'

/** 会话设置(UI 值;模式/权限是前端措辞,后端为 execute/manual) */
export interface SessionSettingsView {
  mode: ChatMode
  permission: ChatPermission
  /** 思考强度滑杆 0-10(后端只存五档,见 effortOf) */
  thinkingLevel: number
  provider: string
  model: string
}

/** 前端 → 后端 */
export const MODE_TO_BACKEND: Record<ChatMode, string> = {
  plan: 'plan',
  build: 'execute',
  auto: 'auto'
}
export const PERMISSION_TO_BACKEND: Record<ChatPermission, string> = {
  ask: 'manual',
  allow: 'auto'
}
/** 后端 → 前端(未知值由调用方 ?? 回落) */
export const MODE_FROM_BACKEND: Record<string, ChatMode> = {
  plan: 'plan',
  execute: 'build',
  auto: 'auto'
}
export const PERMISSION_FROM_BACKEND: Record<string, ChatPermission> = {
  manual: 'ask',
  auto: 'allow'
}
/** 后端五档思考强度 → 滑杆数值(仅在后端档位与本地档位不同档时用于回落) */
export const EFFORT_LEVEL: Record<string, number> = {
  none: 0,
  low: 3,
  medium: 6,
  high: 8,
  max: 10
}

/** 思考强度滑杆 0-10 → 后端五档(粗粒度;同档内的数值差别后端不保留) */
export function effortOf(level?: number): string {
  if (level === undefined || level <= 0) return 'none'
  if (level <= 3) return 'low'
  if (level <= 6) return 'medium'
  if (level <= 8) return 'high'
  return 'max'
}

/** UI 设置片段 → 后端 update_session_settings 请求体(只带显式给出的字段,缺省 = 后端保持) */
export function toBackendSettings(patch: Partial<SessionSettingsView>): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  if (patch.mode) out.mode = MODE_TO_BACKEND[patch.mode]
  if (patch.permission) out.permission = PERMISSION_TO_BACKEND[patch.permission]
  if (patch.thinkingLevel !== undefined) out.reasoningEffort = effortOf(patch.thinkingLevel)
  if (patch.provider !== undefined) out.provider = patch.provider
  if (patch.model !== undefined) out.model = patch.model
  return out
}
