/**
 * 提及(mention)系统类型定义
 *
 * 触发符约定:
 *   '/' → 快捷命令(command)
 *   '$' → 技能(skill) / 子智能体(agent)
 *   '@' → 文件引用(file,数据源为会话工作目录的 list_directory 扁平化,见 registry/fileSource)
 *
 * 静态条目以 TS 配置 + 注册 API 维护(见 config.ts / registry.ts),
 * 动态条目通过实现 MentionProvider 运行时注入。
 */
export type MentionType = 'command' | 'skill' | 'agent' | 'file'
export type MentionTrigger = '/' | '$' | '@'

export interface MentionItem {
  type: MentionType
  /** 唯一标识(不含触发符),token = 触发符 + id,如 '/goal'、'$code-review' */
  id: string
  /** 名称:i18n key 优先;缺省时用 label */
  labelKey?: string
  label?: string
  /** 列表副标题 */
  descriptionKey?: string
  description?: string
  /** Arco 图标名(弹层/消息侧用 <component :is>);输入框 chip 内使用 chip.ts 的内联 SVG 映射,未收录则回退为彩色圆点 */
  icon?: string
  /** 彩色图标 URL(手写 SVG 字形);设置时弹层/chip 优先于 icon 的线框图标 */
  iconUrl?: string
  /** 标签色:CSS 变量(如 var(--warning))或 #hex */
  color: string
  /** 二级分组(缺省按 type 归组) */
  category?: string
  /** 过滤关键词 */
  keywords?: string[]
  /** 选中后紧随 chip 插入的模板文本(可选) */
  apply?: (ctx: MentionContext) => string
}

export interface MentionGroup {
  id: string
  /** 分组头:i18n key 优先;缺省用 label */
  labelKey?: string
  label?: string
  items: MentionItem[]
}

export interface MentionContext {
  /** 触发符后已输入部分 */
  query: string
  /** 当前会话工作目录(@ 文件引用的数据源,绝对路径;缺省时弹层提示先选目录) */
  sessionDir?: string
}

export interface MentionProvider {
  trigger: MentionTrigger
  fetch(query: string, ctx: MentionContext): Promise<MentionGroup[]> | MentionGroup[]
}

/** 触发符 → 条目类型(用于拼接 token 与解析) */
export const TRIGGER_BY_TYPE: Record<MentionType, MentionTrigger> = {
  command: '/',
  skill: '$',
  agent: '$',
  file: '@'
}

/**
 * 发送时从输入框 chip 收集的引用(原文 + 机器可读标识)。
 * 内容正文保留「原文」(路径 / 名称 / 编号标签);这里收集的 id/token 作为请求元数据下发,
 * 后端据此在喂模型与冷启动重建时生成「引用说明」。
 */
export interface ChipMentionRef {
  kind: 'file' | 'agent'
  /** 文件相对路径 / 智能体裸 id */
  id: string
  /** 原文(文件路径 / 智能体名称),用于说明文本展示 */
  token: string
  label?: string
}

/** 输入框内联 chip 收集结果:引用(文件/智能体) + 元素围栏原文 + office 路径围栏原文 */
export interface CollectedChipRefs {
  mentions: ChipMentionRef[]
  elements: string[]
  /** office 预览拾取的 ```office-ref 围栏原文(路径 + 文件信息) */
  officeRefs: string[]
}

/**
 * 对话正文引用(「添加到当前任务」):正文只保留截断预览,这里下发完整文本 + 来源消息,
 * 后端据此生成「引用说明」并作为消息元数据落盘(喂模型 / 冷启动重建时拼回 user 消息)。
 */
export interface QuoteRefPayload {
  /** 选中正文原文(完整) */
  text: string
  /** 来源消息序号(#seq) */
  seq?: number
  /** 来源角色 */
  role?: 'assistant' | 'user'
}