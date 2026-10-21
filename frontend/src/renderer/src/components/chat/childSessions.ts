/**
 * 子智能体列表项（规划胶囊展开面板「子会话」一节）：
 * - `session`：后端 `sub_session` 已创建的子会话（有实时状态 / token / 时长）；
 * - `pending`：本轮的 `sub_session` 工具调用**尚未开始执行**（子会话还没创建），
 *   点击只在右侧展示任务文本（用户消息）。
 */
import type { Session } from '@/types'
import { localizedSessionTitle } from '@/utils/sessionTitle'

export type ChildSessionItem =
  | {
      key: string
      kind: 'session'
      session: Session
      title: string
    }
  | {
      key: string
      kind: 'pending'
      /** 发起本次委派的工具调用 id（页签去重 / 关联升级用） */
      toolCallId: string
      /** 父会话后端 id（等待子会话创建后把页签升级为实时子会话说用） */
      parentBackendId: string
      /** 该工具调用在本会话 sub_session 调用序列中的下标（与子会话创建顺序对齐） */
      callIndex: number
      title: string
      /** 委派任务原文（点击页签里作为用户消息展示） */
      task: string
    }

/** 委派任务首行：作为子智能体行的标题 */
export function delegationTitle(task: string): string {
  const line = task.split('\n').find((l) => l.trim().length > 0) ?? task
  return line.trim()
}

/** 合并「已创建子会话」与「未开始委派」为一个列表（已创建在前，未开始追加在后）。 */
export function mergeChildItems(
  children: Session[],
  pending: Array<{ toolCallId: string; parentBackendId: string; callIndex: number; task: string }>
): ChildSessionItem[] {
  const items: ChildSessionItem[] = children.map((session) => ({
    key: `s:${session.id}`,
    kind: 'session',
    session,
    title: localizedSessionTitle(session)
  }))
  for (const p of pending) {
    items.push({
      key: `p:${p.toolCallId}`,
      kind: 'pending',
      toolCallId: p.toolCallId,
      parentBackendId: p.parentBackendId,
      callIndex: p.callIndex,
      title: delegationTitle(p.task),
      task: p.task
    })
  }
  return items
}
