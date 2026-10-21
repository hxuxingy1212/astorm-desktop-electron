import type { MentionItem } from './types'

/**
 * 内置提及配置清单(TS 内置 + 运行时注册 API,见 registry.ts)
 *
 * 字段说明(与方案配置清单一致):
 *   id     唯一标识,不含触发符;token = 触发符 + id('/goal')
 *   labelKey/label 名称(i18n key 优先)
 *   descriptionKey/description 列表副标题
 *   icon   Arco 图标名;输入框 chip 内走 chip.ts 的内联 SVG 映射,未收录回退为彩色圆点
 *   color  标签色:CSS 变量或 #hex,自动随明暗主题切换
 *   category 二级分组(命令按 category 分组)
 *
 * 注意:技能('$name')与智能体('$agent/id')不再硬编码,由 registry 的 SkillProvider
 * 运行时从后端 list_skills / list_agents 拉取(见 source.ts)。
 */
/**
 * 内置快捷命令：**只保留已实现的功能**（未实现的占位命令一律不列出）。
 * 处理入口见 `components/chat/ChatInput.vue`：
 *   - 文本宏：`/init`、`/plan`（替换为预置提示词后发送）
 *   - 控制命令（`ACTION_COMMANDS`）：`/undo`、`/fork`、`/compact`
 *
 * 展示约定：命令名用**原始英文**（`label = '/' + id`，不随语言变化）；描述走 i18n，
 * 按界面语言呈现。命令不再分类（弹层里归为单一「命令」组）；图标为手写字形（assets/ui-icons），
 * **统一单色 `--text-primary`**，与标签文字一致。
 */
const COMMAND_COLOR = 'var(--text-primary)'

export const BUILTIN_COMMANDS: MentionItem[] = [
  {
    type: 'command',
    id: 'init',
    label: '/init',
    descriptionKey: 'mentions.initDesc',
    icon: 'icon-init',
    color: COMMAND_COLOR,
    keywords: ['初始化', 'init', 'agents']
  },
  {
    type: 'command',
    id: 'plan',
    label: '/plan',
    descriptionKey: 'mentions.planDesc',
    icon: 'icon-plan',
    color: COMMAND_COLOR,
    keywords: ['计划', '规划', 'plan']
  },
  {
    type: 'command',
    id: 'goal',
    label: '/goal',
    descriptionKey: 'mentions.goalDesc',
    icon: 'icon-goal',
    color: COMMAND_COLOR,
    keywords: ['目标', 'goal', 'objective']
  },
  {
    type: 'command',
    id: 'compact',
    label: '/compact',
    descriptionKey: 'mentions.compactDesc',
    icon: 'icon-compact',
    color: COMMAND_COLOR,
    keywords: ['摘要', '压缩', 'compact', 'summarize']
  },
  {
    type: 'command',
    id: 'undo',
    label: '/undo',
    descriptionKey: 'mentions.undoDesc',
    icon: 'icon-undo',
    color: COMMAND_COLOR,
    keywords: ['撤回', '撤销', 'undo', 'rewind']
  },
  {
    type: 'command',
    id: 'fork',
    label: '/fork',
    descriptionKey: 'mentions.forkDesc',
    icon: 'icon-fork',
    color: COMMAND_COLOR,
    keywords: ['分支', '派生', 'fork', 'branch']
  }
]
