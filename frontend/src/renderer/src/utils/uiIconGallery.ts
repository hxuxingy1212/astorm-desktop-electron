/**
 * 手写 UI 图标清单(图标 demo 页用):assets/ui-icons/*.svg + 注册名 → 字形映射。
 *
 * 字形集与项目内已有的手写内联 SVG(AppSidebar / StatusBar 的「侧栏伸缩 / 页签伸缩」)
 * 同一套造型:48 网格 / stroke-width 4 / currentColor / 圆角,可随 hover 与选中态变色。
 * 一个字形可服务多个注册名(如 icon-data / icon-db / icon-storage 都是数据库圆柱)。
 */

/**
 * 全局手写 UI 图标线宽（lucide 24 网格语义；AppIcon 内部 ×2 换算到 48 网格）。
 *
 * 唯一标准：页面不要再逐图标传 `stroke-width`，需要整体调整时只改这里。
 */
export const UI_ICON_STROKE_WIDTH = 1.8

export interface UiGlyphEntry {
  /** 字形名(= svg 文件名,不含后缀) */
  glyph: string
  /** 解析后的静态资源 URL */
  url: string
  /** 该字形服务的注册名(ICON_MAP 的 key);空数组 = 备用字形 */
  names: string[]
}

/** 载入全部字形资源(eager:渲染期同步可用;打包后为 data URI,file:// 下同样可加载) */
const modules = import.meta.glob('../assets/ui-icons/*.svg', {
  query: '?url',
  import: 'default',
  eager: true
}) as Record<string, string>

/** 字形源码(raw):供 AppIcon 内联渲染,使 currentColor / :deep(svg) 等既有样式继续生效 */
const rawModules = import.meta.glob('../assets/ui-icons/*.svg', {
  query: '?raw',
  import: 'default',
  eager: true
}) as Record<string, string>

const RAW_BY_GLYPH: Record<string, string> = Object.fromEntries(
  Object.entries(rawModules).map(([path, raw]) => [path.slice(path.lastIndexOf('/') + 1, -'.svg'.length), raw])
)

/**
 * 取字形的内联 SVG 源码,并把尺寸/线宽写进 svg 属性。
 * 字形源码固定为 `width="48" height="48"` + `stroke-width="4"`,替换失败会静默保持原值(不致命)。
 */
export function uiGlyphMarkup(glyph: string, size: number | string, strokeWidth: number): string {
  const raw = RAW_BY_GLYPH[glyph]
  if (!raw) throw new Error(`[uiIconGallery] 缺少字形源码:${glyph}.svg`)
  return raw
    .replace('width="48" height="48"', `width="${size}" height="${size}"`)
    .replace('stroke-width="4"', `stroke-width="${strokeWidth}"`)
}

const URL_BY_GLYPH: Record<string, string> = Object.fromEntries(
  Object.entries(modules).map(([path, url]) => [path.slice(path.lastIndexOf('/') + 1, -'.svg'.length), url])
)

/**
 * 注册名(plugins/lucide.ts 的 ICON_MAP key)→ 手写字形。
 * 表长即覆盖率:ICON_MAP 的每个名字都必须在这里出现,漏了会在模块加载期报错。
 */
export const NAME_TO_GLYPH: Record<string, string> = {
  // 基础符号
  'icon-plus': 'plus',
  'icon-minus': 'minus',
  'icon-close': 'close',
  'icon-check': 'check',
  'icon-edit-check': 'check',
  'icon-check-circle': 'check-circle',
  'icon-exclamation-circle': 'alert-circle',
  'icon-alert-circle': 'alert-circle',
  'icon-warning': 'alert-circle',
  'icon-info': 'info',
  'icon-info-circle': 'info',
  'icon-help': 'help',
  'icon-down': 'chevron-down',
  'icon-chevron-down': 'chevron-down',
  'icon-chevron-up': 'chevron-up',
  'icon-caret': 'chevron-down',
  'icon-left': 'chevron-left',
  'icon-right': 'chevron-right',
  'icon-up': 'arrow-up',
  'icon-upload': 'arrow-up',
  'icon-to-top': 'arrow-up',
  'icon-more': 'more-horizontal',
  'icon-list': 'more-horizontal',
  'icon-drag-dot-vertical': 'grip-vertical',
  'icon-square': 'square',
  'icon-play': 'play',
  'icon-star': 'star',
  'icon-thunderbolt': 'zap',
  'icon-compass': 'compass',
  'icon-globe': 'globe',
  'icon-eye': 'eye',
  'icon-filter': 'filter',
  'icon-search': 'search',
  'icon-link': 'link',
  'icon-unlink': 'unlink',
  'icon-launch': 'external-link',

  // 文件 / 数据操作
  'icon-copy': 'copy',
  'icon-save': 'save',
  'icon-delete': 'trash',
  'icon-archive': 'archive',
  'icon-empty': 'inbox',
  'icon-inbox': 'inbox',
  'icon-mail': 'mail',
  'icon-download': 'download',
  'icon-send': 'send',
  'icon-pushpin': 'pin',
  'icon-pushpin-off': 'pin-off',

  // 窗口 / 视图
  'icon-expand': 'expand',
  'icon-shrink': 'shrink',
  'icon-hover': 'mouse-pointer',
  'icon-pick-element': 'mouse-pointer-click',
  'icon-kanban': 'kanban',
  'icon-deck': 'table',
  'icon-layers': 'layers',
  'icon-free-size': 'monitor-smartphone',

  // 文件 / 目录
  'icon-file': 'file',
  'icon-file-text': 'file-text',
  'icon-doc': 'file-text',
  'icon-sheet': 'file-spreadsheet',
  'icon-folder': 'folder',
  'icon-folder-add': 'folder-plus',
  'icon-mind-mapping': 'folder-tree',
  'icon-clipboard-list': 'clipboard',
  'icon-book': 'book',
  'icon-library': 'library',
  'icon-tags': 'tags',

  // 编辑 / 工具
  'icon-edit': 'pencil',
  'icon-brush': 'brush',
  'icon-eraser': 'eraser',
  'icon-tool': 'wrench',
  'icon-build': 'hammer',
  'icon-settings': 'settings',
  'icon-config': 'settings-2',
  'icon-code': 'code',
  'icon-code-square': 'code-square',
  'icon-shell': 'terminal',
  'icon-data': 'database',
  'icon-db': 'database',
  'icon-storage': 'database',
  'icon-cpu': 'cpu',
  'icon-puzzle': 'puzzle',
  'icon-box': 'box',

  // 进程 / 状态
  'icon-loading': 'loading',
  'icon-refresh': 'refresh',
  'icon-sync': 'refresh-ccw',
  'icon-rotate-left': 'rotate',
  'icon-reply': 'rotate',
  'icon-clock': 'history',
  'icon-history': 'history',
  'icon-clock-circle': 'clock',
  'icon-bell': 'bell',
  'icon-safe': 'shield-check',
  'icon-lock': 'lock',
  'icon-idcard': 'id-card',
  'icon-user-group': 'users',
  'icon-robot': 'bot',
  'icon-memory': 'brain',
  'icon-gauge': 'gauge',
  'icon-think': 'think',
  'icon-summary': 'summary',
  'icon-camera': 'camera',
  'icon-video': 'video',
  'icon-media': 'image',
  'icon-bar-chart': 'chart-bar',
  'icon-wrap': 'wrap-text',
  'icon-message': 'message',
  'icon-calendar': 'calendar',
  'icon-schedule': 'calendar-clock',
  'icon-branch': 'git-branch',
  'icon-git-branch': 'git-branch',
  'icon-gitlab': 'git-compare',
  'icon-github': 'github',

  // 提及命令(/undo /goal /init /plan /compact /fork):单色,与正文一致
  'icon-undo': 'undo',
  'icon-goal': 'goal',
  'icon-init': 'init',
  'icon-plan': 'plan',
  'icon-compact': 'compact',
  'icon-fork': 'fork',
  // 技能(skill-creator 专用星光)
  'icon-sparkles': 'sparkles'
}

/** 字形展示顺序(与生成器内分组顺序一致) */
const GLYPH_ORDER = [
  'plus', 'minus', 'close', 'check', 'check-circle', 'alert-circle', 'info',
  'chevron-down', 'chevron-up', 'chevron-left', 'chevron-right', 'arrow-up', 'arrow-down',
  'more-horizontal', 'grip-vertical', 'square', 'play', 'star', 'zap', 'compass', 'globe', 'eye', 'filter', 'search',
  'link', 'unlink', 'external-link', 'copy', 'save', 'trash', 'archive', 'inbox', 'mail', 'download', 'send', 'pin', 'pin-off',
  'panel-left', 'panel-right', 'expand', 'shrink', 'mouse-pointer', 'mouse-pointer-click', 'kanban', 'table', 'layers',
  'file', 'file-text', 'file-spreadsheet', 'folder', 'folder-plus', 'folder-tree', 'clipboard', 'book', 'library', 'tags',
  'pencil', 'brush', 'eraser', 'wrench', 'hammer', 'settings', 'settings-2', 'code', 'code-square', 'terminal',
  'database', 'cpu', 'puzzle', 'loading', 'refresh', 'refresh-ccw', 'rotate', 'clock', 'history', 'bell',
  'shield-check', 'lock', 'id-card', 'users', 'bot', 'camera', 'video', 'image', 'monitor-smartphone',
  'chart-bar', 'wrap-text', 'message', 'calendar', 'calendar-clock', 'git-branch', 'git-compare', 'github',
  'gauge',
  'brain'
]

/** 名称 → 字形 的反查(字形 → 服务名称列表) */
const NAMES_BY_GLYPH: Record<string, string[]> = {}
for (const [name, glyph] of Object.entries(NAME_TO_GLYPH)) {
  ;(NAMES_BY_GLYPH[glyph] ??= []).push(name)
}

function entry(glyph: string): UiGlyphEntry {
  const url = URL_BY_GLYPH[glyph]
  if (!url) throw new Error(`[uiIconGallery] 缺少字形资源:${glyph}.svg`)
  return { glyph, url, names: NAMES_BY_GLYPH[glyph] ?? [] }
}

/** 全部手写字形(按 GLYPH_ORDER;未列入顺序表的字形追加在末尾,保证不漏展示) */
export const UI_GLYPHS: UiGlyphEntry[] = [
  ...GLYPH_ORDER.filter((g) => g in URL_BY_GLYPH).map(entry),
  ...Object.keys(URL_BY_GLYPH)
    .filter((g) => !GLYPH_ORDER.includes(g))
    .sort()
    .map(entry)
]

/** 未被任何注册名引用的备用字形 */
export const UNUSED_GLYPHS = UI_GLYPHS.filter((g) => g.names.length === 0)

/** 按名取字形 URL(后续切换 UI 图标时可直接复用) */
export function uiGlyphUrl(name: string): string | undefined {
  const glyph = NAME_TO_GLYPH[name]
  return glyph ? URL_BY_GLYPH[glyph] : undefined
}
