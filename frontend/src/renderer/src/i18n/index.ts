import { createI18n } from 'vue-i18n'
import zhCN from './locales/zh-CN'
import enUS from './locales/en-US'
import zhCNArco from '@arco-design/web-vue/es/locale/lang/zh-cn'
import enUSArco from '@arco-design/web-vue/es/locale/lang/en-us'
import type { ArcoLang } from '@arco-design/web-vue/es/locale/interface'

/** 语言列表:新增语言在此追加并新增 locales/<key>.ts 即可 */
export const LOCALES = [
  { key: 'zh-CN', label: '简体中文', arco: zhCNArco },
  { key: 'en-US', label: 'English', arco: enUSArco }
] as const

export type LocaleKey = (typeof LOCALES)[number]['key']

export const i18n = createI18n({
  legacy: false,
  locale: 'zh-CN',
  fallbackLocale: 'zh-CN',
  messages: {
    'zh-CN': zhCN,
    'en-US': enUS
  }
})

export function getArcoLocale(key: LocaleKey): ArcoLang {
  return LOCALES.find((l) => l.key === key)?.arco ?? zhCNArco
}
