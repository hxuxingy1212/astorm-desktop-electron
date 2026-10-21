/**
 * 文件图标清单(图标 demo 页用):分组元数据 + 资源 URL(URL 解析统一委托 utils/fileIcons.ts)。
 *
 * - 资源由 fileIcons.ts 的 `import.meta.glob(..., { query: '?url' })` 解析:带 hash 且随 base
 *   相对解析,打包后 file:// 加载(hash 路由)也能直接命中,不依赖 public 目录
 * - 分组与 `covers` 文案与 utils/fileIcons.ts 的 EXT_GLYPH / NAME_GLYPH 一一对照,
 *   便于在 demo 页验收「扩展名覆盖度」。
 */

import { fileGlyphUrl } from './fileIcons'

export interface GalleryIcon {
  /** 图标名(= svg 文件名,不含后缀) */
  name: string
  /** 解析后的静态资源 URL */
  url: string
  /** 该图标覆盖的扩展名 / 文件名(取自 fileIcons.ts 的映射) */
  covers: string
}

export interface IconGroup {
  title: string
  icons: GalleryIcon[]
}

function icon(name: string, covers: string): GalleryIcon {
  const url = fileGlyphUrl(name)
  if (!url) throw new Error(`[fileIconGallery] 缺少图标资源:${name}.svg`)
  return { name, url, covers }
}

/** 手写文件图标分组(顺序即 demo 页展示顺序) */
export const FILE_ICON_GROUPS: IconGroup[] = [
  {
    title: '代码语言',
    icons: [
      icon('typescript', 'ts · mts · cts'),
      icon('react-ts', 'tsx'),
      icon('javascript', 'js · mjs · cjs'),
      icon('react', 'jsx'),
      icon('python', 'py'),
      icon('rust', 'rs'),
      icon('go', 'go'),
      icon('java', 'java'),
      icon('java-class', 'class'),
      icon('c', 'c · h'),
      icon('cpp', 'cpp · cc · cxx · hpp'),
      icon('csharp', 'cs'),
      icon('csproj', 'csproj'),
      icon('ruby', 'rb · erb'),
      icon('php', 'php · phtml'),
      icon('swift', 'swift'),
      icon('kotlin', 'kt · kts'),
      icon('scala', 'scala'),
      icon('dart', 'dart'),
      icon('lua', 'lua'),
      icon('elixir', 'ex · exs'),
      icon('erlang', 'erl · hrl'),
      icon('r', 'r'),
      icon('julia', 'jl'),
      icon('perl', 'pl · pm'),
      icon('groovy', 'groovy'),
      icon('clojure', 'clj · cljs'),
      icon('haskell', 'hs · lhs')
    ]
  },
  {
    title: '前端 / 标记语言',
    icons: [
      icon('vue', 'vue'),
      icon('svelte', 'svelte'),
      icon('astro', 'astro'),
      icon('html', 'html · htm · cshtml'),
      icon('css', 'css'),
      icon('scss', 'scss'),
      icon('sass', 'sass'),
      icon('less', 'less'),
      icon('xml', 'xml · plist'),
      icon('sql', 'sql'),
      icon('graphql', 'graphql · gql')
    ]
  },
  {
    title: '数据 / 配置',
    icons: [
      icon('json', 'json · jsonc'),
      icon('yaml', 'yaml · yml'),
      icon('toml', 'toml'),
      icon('ini', 'ini · cfg · conf · properties · env')
    ]
  },
  {
    title: '脚本 / 终端 / 工程约定',
    icons: [
      icon('shell', 'sh · bash · zsh · fish · mk · makefile'),
      icon('powershell', 'ps1 · psm1 · psd1'),
      icon('bat', 'bat · cmd'),
      icon('docker', 'dockerfile'),
      icon('git', '.gitignore')
    ]
  },
  {
    title: '文档 / 办公',
    icons: [
      icon('markdown', 'md · mdx · markdown'),
      icon('text', 'txt · text · log'),
      icon('word', 'doc · docx · rtf · odt'),
      icon('pdf', 'pdf'),
      icon('excel', 'xls · xlsx · csv · ods'),
      icon('powerpoint', 'ppt · pptx · odp'),
      icon('license', 'license')
    ]
  },
  {
    title: '媒体 / 资源',
    icons: [
      icon('image', 'png · jpg · jpeg · gif · webp · bmp · ico'),
      icon('audio', 'mp3 · wav · flac · ogg'),
      icon('video', 'mp4 · mkv · avi · webm · mov')
    ]
  },
  {
    title: '归档 / 密钥 / 工程文件',
    icons: [
      icon('zip', 'zip · rar · 7z · tar · gz · bz2 · xz'),
      icon('jar', 'jar · war'),
      icon('key', 'key · pem'),
      icon('sln', 'sln · slnx')
    ]
  },
  {
    title: '兜底 / 目录',
    icons: [
      icon('default-file', '未命中扩展名'),
      icon('folder', '文件夹(32 网格,双层渐变)')
    ]
  }
]

/** 全部手写图标(demo 页统计用) */
export const FILE_ICONS: GalleryIcon[] = FILE_ICON_GROUPS.flatMap((g) => g.icons)
