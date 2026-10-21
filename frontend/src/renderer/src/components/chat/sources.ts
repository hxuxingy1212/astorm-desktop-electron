/**
 * 「相关资料 / 相关记忆」：把**本轮**（最近一次用户消息 → 结尾无工具助手消息期间）里
 * 网页搜索（`search` action=websearch）与记忆搜索（`memory` action=search）的**结果元数据**
 * 汇总成卡片。
 *
 * 数据来自工具结果元数据（后端 `web_results` / `memory_results`，随 tool_result 事件下发、
 * 落消息 metadata，冷启动可恢复）——前端**不做任何文本解析**。
 */
import type { TurnStep } from './turnGrouping'
import type { MemorySource, SourceItem, WebSource } from '@/types'
import { siteIconUrl } from '@/utils/siteIcons'

export type { MemorySource, SourceItem, WebSource }

function domainOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return ''
  }
}

/** 站点图标；桌面端经主进程 7 天缓存（`astorm-icon://`），浏览器部署回落直连。 */
export function siteFavicon(url: string, size = 64): string {
  return siteIconUrl(url, size)
}

/** 一轮里所有 web / 记忆检索结果（读工具结果元数据，按 url / concept_id 去重，保持出现顺序） */
export function collectTurnSources(steps: TurnStep[]): SourceItem[] {
  const out: SourceItem[] = []
  const seen = new Set<string>()
  for (const step of steps) {
    for (const tool of step.msg.tools ?? []) {
      for (const r of tool.webResults ?? []) {
        const key = `web:${r.url}`
        if (seen.has(key)) continue
        seen.add(key)
        out.push({
          kind: 'web',
          title: r.title,
          url: r.url,
          snippet: r.snippet,
          source: domainOf(r.url)
        })
      }
      for (const r of tool.memoryResults ?? []) {
        const key = `memory:${r.concept_id}`
        if (seen.has(key)) continue
        seen.add(key)
        out.push({
          kind: 'memory',
          conceptId: r.concept_id,
          title: r.title,
          snippet: r.snippet,
          observedAt: r.observed_at ?? undefined
        })
      }
    }
  }
  return out
}
