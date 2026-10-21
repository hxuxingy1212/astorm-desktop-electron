/**
 * 供应商配置持久化服务。
 * Electron 环境下走主进程(safeStorage 加密落盘,见 src/main/providers.ts);
 * 纯浏览器调试(Vite,无 preload)时降级 localStorage —— 与 mock 策略一致。
 */
export interface ProviderModelDto {
  name: string
  order?: number
}

/** 供应商 API 协议（与 Rust llm_provider::Protocol 的 snake_case 取值一致） */
export type ApiFormat = 'chat_completions' | 'responses' | 'anthropic_messages'

export interface ProviderDto {
  name: string
  baseUrl: string
  apiKey: string
  apiFormat?: ApiFormat
  models: ProviderModelDto[]
  order?: number
}

const LEGACY_KEY = 'astorm:models:v3'

function hasNativeBackend(): boolean {
  return typeof window.api?.providers?.list === 'function'
}

export const providersBackend = {
  async list(): Promise<ProviderDto[]> {
    if (hasNativeBackend()) {
      const res = await window.api.providers.list()
      return Array.isArray(res?.providers) ? res.providers : []
    }
    return loadLegacy()
  },

  async sync(providers: ProviderDto[]): Promise<void> {
    if (hasNativeBackend()) {
      await window.api.providers.sync(providers)
      return
    }
    saveLegacy(providers)
  }
}

/** 兼容旧 localStorage 形状(带 id/order 等多余字段,取 name/baseUrl/apiKey/models) */
function loadLegacy(): ProviderDto[] {
  try {
    const raw = localStorage.getItem(LEGACY_KEY)
    const parsed = raw ? (JSON.parse(raw) as { providers?: unknown }) : null
    const list = Array.isArray(parsed?.providers) ? parsed.providers : []
    return list.map((p) => {
      const x = (p ?? {}) as { name?: unknown; baseUrl?: unknown; apiKey?: unknown; apiFormat?: unknown; models?: unknown; order?: unknown }
      return {
        name: typeof x.name === 'string' ? x.name : '',
        baseUrl: typeof x.baseUrl === 'string' ? x.baseUrl : '',
        apiKey: typeof x.apiKey === 'string' ? x.apiKey : '',
        apiFormat: normalizeApiFormat(x.apiFormat),
        order: typeof x.order === 'number' ? x.order : 0,
        models: (Array.isArray(x.models) ? x.models : []).map((m) => {
          const mm = (m ?? {}) as { name?: unknown; order?: unknown }
          return { name: typeof mm.name === 'string' ? mm.name : '', order: typeof mm.order === 'number' ? mm.order : 0 }
        })
      }
    })
  } catch {
    return []
  }
}

function normalizeApiFormat(v: unknown): ApiFormat {
  return v === 'responses' || v === 'anthropic_messages' ? v : 'chat_completions'
}

function saveLegacy(providers: ProviderDto[]): void {
  try {
    localStorage.setItem(LEGACY_KEY, JSON.stringify({ providers }))
  } catch {
    /* 隐私模式等场景忽略 */
  }
}