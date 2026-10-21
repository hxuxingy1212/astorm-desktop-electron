/**
 * 会话标题按当前界面语言取中/英版本。
 *
 * 后端为每次标题生成（首条消息 / 摘要后）写入 `title_zh` + `title_en`；旧记录没有
 * 这两列，回退到 `title`。调用处直接放在模板/computed 里即可：这里读取
 * `i18n.global.locale.value`，语言切换会触发重新渲染。
 */
import { i18n } from '@/i18n'

export interface SessionTitleFields {
  title?: string | null
  titleZh?: string | null
  titleEn?: string | null
}

export function localizedSessionTitle(
  session: SessionTitleFields | null | undefined,
  locale: string = i18n.global.locale.value
): string {
  if (!session) return ''
  const legacy = (session.title ?? '').trim()
  const zh = (session.titleZh ?? legacy).trim()
  const en = (session.titleEn ?? legacy).trim()
  return locale.toLowerCase().startsWith('en') ? en || zh : zh || en
}
