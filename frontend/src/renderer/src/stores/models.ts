import { defineStore } from 'pinia'
import { providersBackend, type ApiFormat, type ProviderDto } from '@/services/providers'

/**
 * 模型配置(与后端约定:以 name 为唯一标识,不落 id)。
 * 排序:数组顺序即显示顺序,order 为显式排序字段(列表第一项即「默认」)。
 * 持久化:经 providersBackend 走主进程 safeStorage(Electron)或降级 localStorage(浏览器调试)。
 */
export interface AiModel {
  /** 模型名称(供应商内唯一,列表中可编辑) */
  name: string
  /** 排序字段:0 起 */
  order?: number
}

export interface ModelProvider {
  /** 供应商名称(全局唯一,后端以此为标识) */
  name: string
  /** Base URL */
  baseUrl: string
  /** API Key */
  apiKey: string
  /** API 协议(wire 格式);缺省 chat_completions */
  apiFormat: ApiFormat
  /** 该供应商下的模型 */
  models: AiModel[]
  /** 排序字段:0 起 */
  order?: number
}

/** 同步排序字段:数组顺序即排序,order 显式写入以便后端/配置引用 */
function reindex(list: Array<{ order?: number }>): void {
  list.forEach((item, i) => {
    item.order = i
  })
}

function sortByOrder<T extends { order?: number }>(list: T[]): T[] {
  return [...list].sort((a, b) => (a.order ?? Number.MAX_SAFE_INTEGER) - (b.order ?? Number.MAX_SAFE_INTEGER))
}

/** 后端 DTO → 本地模型(缺 order 按数组序) */
function fromDto(dto: ProviderDto, index: number): ModelProvider {
  return {
    name: dto.name,
    baseUrl: dto.baseUrl,
    apiKey: dto.apiKey,
    apiFormat: dto.apiFormat ?? 'chat_completions',
    order: typeof dto.order === 'number' ? dto.order : index,
    models: (Array.isArray(dto.models) ? dto.models : []).map((m, mi) => ({
      name: m.name,
      order: typeof m.order === 'number' ? m.order : mi
    }))
  }
}

/**
 * 模型配置:左侧供应商列表 + 右侧详情(供应商表单 + 模型列表)。
 * 以 name 为唯一标识;当前选中项(activeIndex)仅 UI 状态,不随配置落盘。
 */
export const useModelsStore = defineStore('models', {
  state: () => ({
    providers: [] as ModelProvider[],
    /** 当前选中供应商下标(仅 UI 状态,不写配置) */
    activeIndex: 0
  }),
  actions: {
    /** 应用启动/刷新:从主进程(或调试降级)拉取供应商配置 */
    async bootstrap(): Promise<void> {
      try {
        const list = await providersBackend.list()
        const providers = sortByOrder(list.map(fromDto))
        providers.forEach((p) => {
          p.models = sortByOrder(p.models)
        })
        this.providers = providers
        this.activeIndex = Math.min(this.activeIndex, Math.max(0, providers.length - 1))
      } catch (err) {
        console.error('[models] bootstrap failed:', err)
      }
    },
    /** 新增供应商:名称留空,保存时校验必填且不重名 */
    addProvider(): void {
      this.providers.push({ name: '', baseUrl: '', apiKey: '', apiFormat: 'chat_completions', models: [] })
      reindex(this.providers)
      this.activeIndex = this.providers.length - 1
    },
    selectProvider(index: number): void {
      if (index < 0 || index >= this.providers.length) return
      this.activeIndex = index
    },
    /**
     * 名称是否已被占用:以 name 为标识,存在 >=2 个同名供应商即为重复
     * (自身同名只计数 1,故可天然排除自身)
     */
    isProviderNameTaken(name: string): boolean {
      const n = name.trim()
      if (!n) return false
      return this.providers.filter((p) => p.name.trim() === n).length >= 2
    },
    /** 删除供应商,若删除的是当前选中项则就近选中下一条 */
    removeProvider(name: string): void {
      const idx = this.providers.findIndex((x) => x.name === name)
      if (idx === -1) return
      this.providers.splice(idx, 1)
      reindex(this.providers)
      if (this.activeIndex > idx) this.activeIndex -= 1
      else if (this.activeIndex === idx) this.activeIndex = Math.min(idx, this.providers.length - 1)
    },
    /** 拖拽排序:移动供应商(from → to);列表第一项即「默认」供应商 */
    moveProvider(from: number, to: number): void {
      if (from < 0 || to < 0 || from >= this.providers.length || to >= this.providers.length || from === to) return
      const [p] = this.providers.splice(from, 1)
      this.providers.splice(to, 0, p)
      reindex(this.providers)
      // 随拖拽同步当前选中下标(按位置追踪)
      if (this.activeIndex === from) this.activeIndex = to
      else if (from < this.activeIndex && this.activeIndex <= to) this.activeIndex -= 1
      else if (to <= this.activeIndex && this.activeIndex < from) this.activeIndex += 1
    },
    /** 在当前选中供应商下新增模型,名称留空由用户填写 */
    addModel(): void {
      const p = this.providers[this.activeIndex]
      if (!p) return
      p.models.push({ name: '' })
      reindex(p.models)
    },
    removeModel(modelName: string): void {
      const p = this.providers[this.activeIndex]
      if (!p) return
      p.models = p.models.filter((m) => m.name !== modelName)
      reindex(p.models)
    },
    /** 拖拽排序:移动当前选中供应商下的模型(from → to) */
    moveModel(from: number, to: number): void {
      const p = this.providers[this.activeIndex]
      if (!p || from < 0 || to < 0 || from >= p.models.length || to >= p.models.length || from === to) return
      const [m] = p.models.splice(from, 1)
      p.models.splice(to, 0, m)
      reindex(p.models)
    },
    /**
     * 同步到持久化层(主进程 safeStorage 或调试降级);失败不影响本地编辑。
     * 返回 Promise:引导页要等落盘 + 热更后端完成再放行(设置页/ watcher 不管返回值)。
     */
    persist(): Promise<void> {
      reindex(this.providers)
      this.providers.forEach((p) => reindex(p.models))
      return providersBackend
        .sync(
          this.providers.map((p) => ({
            name: p.name,
            baseUrl: p.baseUrl,
            apiKey: p.apiKey,
            apiFormat: p.apiFormat,
            models: p.models.map((m) => ({ name: m.name, order: m.order }))
          }))
        )
        .catch((err) => console.error('[models] persist failed:', err))
    }
  }
})