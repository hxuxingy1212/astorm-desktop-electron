import type { ToolCallView } from '@/types'

/**
 * 工具卡状态判定(对话区多处共用,抽出来避免语义被复制走样):
 *   - ChatMessageItem 决定卡片图标/默认展开
 *   - ChatTurnGroup 决定收纳块是否因中断而强制展开
 *
 * 语义要点:失败分两类——「安全拦截/用户拒绝/待决中断」必须让用户看见,
 * 「纯执行失败」只回给模型改道(界面保持中性灰),两者不能混。
 */

/** 工具是否正等待用户决定(审批卡 / 提问表单):这类失败必须显式展示 */
export function needsAttention(tool: ToolCallView): boolean {
  return tool.status === 'error' && (!!tool.denial || !!tool.interrupt)
}

/** 工具卡是否正等待用户决定:此时结果区是中断信封 JSON,不直接展示 */
export function awaitingDecision(tool: ToolCallView): boolean {
  return tool.interrupt?.status === 'pending'
}

/**
 * 纯执行失败(静默态):失败但既非安全拦截也非待决中断——如路径不存在、格式不支持、
 * 命令抛错。这类是已知可预测异常,错误文本已回给模型由它换一条路走,用户无需感知,
 * 因此卡片不标红、不自动展开。
 */
export function isQuietFailure(tool: ToolCallView): boolean {
  return tool.status === 'error' && !tool.denial && !tool.interrupt && !awaitingDecision(tool)
}
