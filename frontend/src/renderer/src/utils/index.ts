/** 简单唯一 id */
export function uid(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8)
}

/** 当天日期键（本地时区 yyyy-MM-dd）：会话未指定目录时的默认工作区名 */
export function todayKey(): string {
  const d = new Date()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${d.getFullYear()}-${m}-${day}`
}
