import { ipcMain, safeStorage } from 'electron'
import { configDir } from './systemPaths'
import { promises as fs } from 'fs'
import { writeFileAtomic } from './atomicWrite'
import { join } from 'path'
import type { NapiProviderConfig } from './napi'

export type ApiKeyStorage = 'safeStorage' | 'plaintext'

/** 供应商 API 协议（与 Rust llm_provider::Protocol 的 snake_case 取值一致） */
export type ApiFormat = 'chat_completions' | 'responses' | 'anthropic_messages'

const API_FORMATS: ApiFormat[] = ['chat_completions', 'responses', 'anthropic_messages']

/** 能力探测图：白底黑字 739（验证模型真的能读图，而不是只回显文字提示） */
const PROBE_IMAGE_BASE64 =
  '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'
/** 同时验证文字指令遵循 + 图片 OCR：必须回 PINEAPPLE 739 */
const PROBE_PROMPT = 'Read the number in the image. Reply exactly as: PINEAPPLE <number>'

/** 归一化协议取值；未知 / 缺省 → chat_completions（向后兼容） */
function normalizeApiFormat(v: unknown): ApiFormat {
  return typeof v === 'string' && (API_FORMATS as string[]).includes(v)
    ? (v as ApiFormat)
    : 'chat_completions'
}

export interface ProviderModelInput {
  name: string
  order?: number
}

/** 渲染层提交的供应商输入(明文 apiKey,由本模块加密落盘) */
export interface ProviderInput {
  name: string
  baseUrl: string
  apiKey: string
  /** API 协议;缺省 chat_completions */
  apiFormat?: ApiFormat
  models: ProviderModelInput[]
}

/** 返回渲染层的视图(apiKey 解密为明文供设置表单展示) */
export interface ProviderView extends ProviderInput {
  order: number
  storage: ApiKeyStorage
}

interface StoredProvider {
  name: string
  baseUrl: string
  /** apiKey:safeStorage → 加密后 hex;plaintext → 明文;空 = 无密钥 */
  apiKey: string | null
  storage: ApiKeyStorage
  apiFormat: ApiFormat
  models: ProviderModelInput[]
  order: number
}

interface ProvidersFile {
  schemaVersion: number
  providers: StoredProvider[]
}

const DEFAULT_FILE: ProvidersFile = { schemaVersion: 1, providers: [] }
let cache: ProvidersFile | null = null

function filePath(): string {
  return join(configDir(), 'providers.json')
}

/** 归一化:类型校验 + 丢弃多余字段(兼容旧数据) */
function normalizeFile(raw: unknown): ProvidersFile {
  const f = (raw ?? {}) as { schemaVersion?: unknown; providers?: unknown }
  const providers: StoredProvider[] = (Array.isArray(f.providers) ? f.providers : []).map((p, i) => {
    const x = (p ?? {}) as {
      name?: unknown
      baseUrl?: unknown
      apiKey?: unknown
      storage?: unknown
      apiFormat?: unknown
      models?: unknown
      order?: unknown
    }
    const storage: ApiKeyStorage = x.storage === 'safeStorage' ? 'safeStorage' : 'plaintext'
    const models = (Array.isArray(x.models) ? x.models : []).map((m, mi) => {
      const mm = (m ?? {}) as { name?: unknown; order?: unknown }
      return {
        name: typeof mm.name === 'string' ? mm.name : '',
        order: typeof mm.order === 'number' ? mm.order : mi
      }
    })
    return {
      name: typeof x.name === 'string' ? x.name : '',
      baseUrl: typeof x.baseUrl === 'string' ? x.baseUrl : '',
      apiKey: typeof x.apiKey === 'string' ? x.apiKey : null,
      storage,
      apiFormat: normalizeApiFormat(x.apiFormat),
      models,
      order: typeof x.order === 'number' ? x.order : i
    }
  })
  return { schemaVersion: 1, providers }
}

async function readFile(): Promise<ProvidersFile> {
  if (cache) return cache
  try {
    const text = await fs.readFile(filePath(), 'utf-8')
    cache = normalizeFile(JSON.parse(text))
  } catch {
    cache = DEFAULT_FILE
  }
  return cache
}

/** 确保供应商快照已加载(启动装配用;幂等)。 */
export async function ensureProvidersLoaded(): Promise<void> {
  await readFile()
}

/** 导出解密后的供应商配置快照(供 settings-sync 组装完整后端配置)。 */
export function getProvidersSnapshot(): NapiProviderConfig[] {
  return (cache?.providers ?? []).map((p) => ({
    name: p.name,
    baseUrl: p.baseUrl,
    apiKey: decryptKey(p),
    apiFormat: p.apiFormat,
    models: p.models.map((m) => ({
      // 后端 model.id = 前端引用(model 名),name = 发给 API 的模型名
      id: m.name,
      name: m.name
    })),
    order: p.order
  }))
}

async function writeFile(): Promise<void> {
  if (!cache) return
  await writeFileAtomic(filePath(), JSON.stringify(cache, null, 2), 'utf-8', 0o600)
}

/** safeStorage 加密明文密钥;不可用时降级明文并标注 storage=plaintext */
function encryptKey(plain: string): { key: string | null; storage: ApiKeyStorage } {
  if (!plain) return { key: null, storage: 'safeStorage' }
  if (safeStorage.isEncryptionAvailable()) {
    return { key: safeStorage.encryptString(plain).toString('hex'), storage: 'safeStorage' }
  }
  return { key: plain, storage: 'plaintext' }
}

function decryptKey(p: StoredProvider): string {
  if (!p.apiKey) return ''
  if (p.storage !== 'safeStorage') return p.apiKey
  try {
    return safeStorage.decryptString(Buffer.from(p.apiKey, 'hex'))
  } catch {
    return ''
  }
}

function toView(p: StoredProvider): ProviderView {
  return {
    name: p.name,
    baseUrl: p.baseUrl,
    apiKey: decryptKey(p),
    apiFormat: p.apiFormat,
    models: p.models,
    order: p.order,
    storage: p.storage
  }
}

/** 排序字段与数组顺序强制一致(数组顺序 = 排序 = 默认) */
function reindex(providers: StoredProvider[]): void {
  providers.forEach((p, i) => {
    p.order = i
    p.models.forEach((m, mi) => {
      m.order = mi
    })
  })
}

/** 供应商配置变更后热更新后端(运行中 updateConfig,不中断会话;未运行才 start)。
 *  完整后端配置由 settings-sync 组装(含会话上下文),动态导入避免模块环。 */
async function refreshBackendConfig(): Promise<void> {
  await import('./settings-sync').then((m) => m.syncBackendConfig())
}

/** 全量替换:校验 + 加密写盘 + 重推 napi */
async function replaceAll(input: ProviderInput[]): Promise<ProviderView[]> {
  const providers: StoredProvider[] = input.map((src) => {
    const enc = encryptKey(src.apiKey ?? '')
    return {
      name: src.name ?? '',
      baseUrl: src.baseUrl ?? '',
      apiKey: enc.key,
      storage: enc.storage,
      apiFormat: normalizeApiFormat(src.apiFormat),
      models: (Array.isArray(src.models) ? src.models : []).map((m) => ({
        name: m.name ?? '',
        order: 0
      })),
      order: 0
    }
  })
  reindex(providers)
  cache = { schemaVersion: 1, providers }
  await writeFile()
  await refreshBackendConfig()
  return providers.map(toView)
}

export function registerProvidersIpc(): void {
  ipcMain.handle('providers:list', async () => {
    const file = await readFile()
    return {
      providers: file.providers.map(toView),
      encryptionAvailable: safeStorage.isEncryptionAvailable()
    }
  })

  ipcMain.handle(
    'providers:sync',
    async (_e, input: { providers?: ProviderInput[] }): Promise<{ providers: ProviderView[] }> => {
      const list = Array.isArray(input?.providers) ? input.providers : []
      return { providers: await replaceAll(list) }
    }
  )

  // 连接检测:按协议真实发起一次最小请求,验证 baseUrl + apiKey + model 可用
  ipcMain.handle(
    'providers:test',
    async (_e, args: { name?: string; model?: string }): Promise<{ ok: boolean; message: string }> => {
      const file = await readFile()
      const p = file.providers.find((x) => x.name === args?.name)
      if (!p) return { ok: false, message: 'provider_not_found' }
      const key = decryptKey(p)
      if (!p.baseUrl.trim()) return { ok: false, message: 'test_need_base_url' }
      if (!key) return { ok: false, message: 'test_need_api_key' }
      const model = (args?.model ?? '').trim()
      if (!model) return { ok: false, message: 'test_need_model' }
      return await probeProvider(p.baseUrl.trim(), key, p.apiFormat, model)
    }
  )
}

/** 协议端点 URL:base 已带 /v1(Anthropic)或需要补路径时统一在这里拼接 */
function endpointUrl(base: string, apiFormat: ApiFormat): string {
  const b = base.replace(/\/+$/, '')
  if (apiFormat === 'anthropic_messages') {
    return b.endsWith('/v1') ? `${b}/messages` : `${b}/v1/messages`
  }
  return apiFormat === 'responses' ? `${b}/responses` : `${b}/chat/completions`
}

/** 从错误响应体提取可读信息(OpenAI/Anthropic 风格 error.message,或顶层 message) */
function errorMessage(text: string): string {
  try {
    const v = JSON.parse(text) as Record<string, unknown>
    const err = v.error as Record<string, unknown> | undefined
    if (err && typeof err.message === 'string') return err.message
    if (typeof v.message === 'string') return v.message
  } catch {
    /* 非 JSON:原样截断 */
  }
  const t = text.trim()
  return t.length > 300 ? `${t.slice(0, 300)}…` : t
}

/** 按协议构造能力探测请求：一张图 + 一句必须同时读文字和读图才能答对的指令 */
function probeBody(apiFormat: ApiFormat, model: string): string {
  const dataUrl = `data:image/png;base64,${PROBE_IMAGE_BASE64}`
  if (apiFormat === 'anthropic_messages') {
    return JSON.stringify({
      model,
      max_tokens: 48,
      messages: [
        {
          role: 'user',
          content: [
            { type: 'text', text: PROBE_PROMPT },
            {
              type: 'image',
              source: { type: 'base64', media_type: 'image/png', data: PROBE_IMAGE_BASE64 }
            }
          ]
        }
      ]
    })
  }
  if (apiFormat === 'responses') {
    return JSON.stringify({
      model,
      input: [
        {
          role: 'user',
          content: [
            { type: 'input_text', text: PROBE_PROMPT },
            { type: 'input_image', image_url: dataUrl }
          ]
        }
      ],
      max_output_tokens: 48
    })
  }
  return JSON.stringify({
    model,
    messages: [
      {
        role: 'user',
        content: [
          { type: 'text', text: PROBE_PROMPT },
          { type: 'image_url', image_url: { url: dataUrl } }
        ]
      }
    ],
    max_tokens: 48,
    stream: false
  })
}

/** 从三种协议的响应里取助手文本 */
function completionText(apiFormat: ApiFormat, body: Record<string, unknown>): string {
  const textOf = (content: unknown): string => {
    if (typeof content === 'string') return content
    if (!Array.isArray(content)) return ''
    return content
      .map((part) => {
        if (typeof part === 'string') return part
        if (part && typeof part === 'object' && typeof (part as { text?: unknown }).text === 'string') {
          return (part as { text: string }).text
        }
        return ''
      })
      .join(' ')
  }
  if (apiFormat === 'anthropic_messages') return textOf(body.content)
  if (apiFormat === 'responses') {
    if (typeof body.output_text === 'string') return body.output_text
    if (!Array.isArray(body.output)) return ''
    return body.output
      .map((item) => {
        if (!item || typeof item !== 'object') return ''
        return textOf((item as { content?: unknown }).content)
      })
      .join(' ')
  }
  const choices = body.choices as Array<{ message?: { content?: unknown } }> | undefined
  return textOf(choices?.[0]?.message?.content)
}

/** 真实探测供应商:必须同时通过文字指令 + 图片 OCR，才判定模型可用 */
async function probeProvider(
  baseUrl: string,
  apiKey: string,
  apiFormat: ApiFormat,
  model: string
): Promise<{ ok: boolean; message: string }> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 30000)
  try {
    const headers: Record<string, string> = { 'content-type': 'application/json' }
    if (apiFormat === 'anthropic_messages') {
      headers['x-api-key'] = apiKey
      headers['anthropic-version'] = '2023-06-01'
    } else {
      headers['authorization'] = `Bearer ${apiKey}`
    }
    const resp = await fetch(endpointUrl(baseUrl, apiFormat), {
      method: 'POST',
      headers,
      body: probeBody(apiFormat, model),
      signal: controller.signal
    })
    if (!resp.ok) {
      const text = await resp.text().catch(() => '')
      const lower = text.toLowerCase()
      if (
        [400, 415, 422].includes(resp.status) &&
        /image|vision|unsupported|content/.test(lower)
      ) {
        return { ok: false, message: 'vision_unsupported' }
      }
      return { ok: false, message: `HTTP ${resp.status}: ${errorMessage(text)}` }
    }
    const body = (await resp.json().catch(() => null)) as Record<string, unknown> | null
    const answer = body ? completionText(apiFormat, body).toLowerCase() : ''
    if (answer.includes('pineapple') && answer.includes('739')) {
      return { ok: true, message: 'ok' }
    }
    return { ok: false, message: 'vision_unsupported' }
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e)
    return { ok: false, message: msg === 'This operation was aborted' ? 'timeout' : msg }
  } finally {
    clearTimeout(timer)
  }
}
