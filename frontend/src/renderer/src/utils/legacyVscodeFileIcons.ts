/**
 * 【历史对照】替换前的 vscode-icons 文件图标映射。
 *
 * 运行时已改用 utils/fileIcons.ts 的手写 SVG 字形;本模块仅供 `#/icons` 图标总览页
 * 做「替换前 / 替换后」对照 —— 这里的类名需要 UNOCSS 在构建期预生成,故 uno.config.ts
 * 的 safelist 仍引用 ALL_ICON_CLASSES。
 */

type VscodeIconId = string

/** 扩展名 → vscode-icons 图标 id */
const EXT_MAP: Record<string, VscodeIconId> = {
  // 代码
  ts: 'file-type-typescript', mts: 'file-type-typescript', cts: 'file-type-typescript', tsx: 'file-type-reactts',
  js: 'file-type-js', mjs: 'file-type-js', cjs: 'file-type-js', jsx: 'file-type-reactjs',
  py: 'file-type-python', rs: 'file-type-rust', go: 'file-type-go', java: 'file-type-java', class: 'file-type-class',
  c: 'file-type-c', h: 'file-type-c', cpp: 'file-type-cpp', cc: 'file-type-cpp', cxx: 'file-type-cpp', hpp: 'file-type-cpp',
  cs: 'file-type-csharp', csproj: 'file-type-csproj', rb: 'file-type-ruby', erb: 'file-type-ruby',
  php: 'file-type-php', phtml: 'file-type-php', swift: 'file-type-swift',
  kt: 'file-type-kotlin', kts: 'file-type-kotlin', scala: 'file-type-scala', dart: 'file-type-dartlang',
  lua: 'file-type-lua', ex: 'file-type-elixir', exs: 'file-type-elixir', erl: 'file-type-erlang', hrl: 'file-type-erlang',
  r: 'file-type-r', jl: 'file-type-julia', pl: 'file-type-perl', pm: 'file-type-perl',
  groovy: 'file-type-groovy', clj: 'file-type-clojure', cljs: 'file-type-clojure',
  hs: 'file-type-haskell', lhs: 'file-type-haskell',
  vue: 'file-type-vue', svelte: 'file-type-svelte', astro: 'file-type-astro',
  html: 'file-type-html', htm: 'file-type-html', cshtml: 'file-type-html',
  css: 'file-type-css', scss: 'file-type-scss', sass: 'file-type-sass', less: 'file-type-less',
  xml: 'file-type-xml', plist: 'file-type-xml', sql: 'file-type-sql', graphql: 'file-type-graphql', gql: 'file-type-graphql',
  // 数据 / 配置
  json: 'file-type-json', jsonc: 'file-type-json',
  yaml: 'file-type-yaml', yml: 'file-type-yaml', toml: 'file-type-toml',
  ini: 'file-type-ini', cfg: 'file-type-ini', conf: 'file-type-ini', properties: 'file-type-ini', env: 'file-type-ini',
  // 脚本 / 终端
  sh: 'file-type-shell', bash: 'file-type-shell', zsh: 'file-type-shell', fish: 'file-type-shell',
  ps1: 'file-type-powershell', psm1: 'file-type-powershell', psd1: 'file-type-powershell',
  bat: 'file-type-bat', cmd: 'file-type-bat', mk: 'file-type-shell',
  // 文档 / 办公
  md: 'file-type-markdown', mdx: 'file-type-markdown', markdown: 'file-type-markdown',
  txt: 'file-type-text', text: 'file-type-text', log: 'file-type-text',
  doc: 'file-type-word', docx: 'file-type-word', rtf: 'file-type-word', odt: 'file-type-word',
  pdf: 'file-type-pdf2',
  xls: 'file-type-excel', xlsx: 'file-type-excel', csv: 'file-type-excel', ods: 'file-type-excel',
  ppt: 'file-type-powerpoint', pptx: 'file-type-powerpoint', odp: 'file-type-powerpoint',
  // 媒体 / 资源
  png: 'file-type-image', jpg: 'file-type-image', jpeg: 'file-type-image', gif: 'file-type-image',
  webp: 'file-type-image', bmp: 'file-type-image', ico: 'file-type-image',
  mp3: 'file-type-audio', wav: 'file-type-audio', flac: 'file-type-audio', ogg: 'file-type-audio',
  mp4: 'file-type-video', mkv: 'file-type-video', avi: 'file-type-video', webm: 'file-type-video', mov: 'file-type-video',
  // 归档 / 密钥 / 工程
  zip: 'file-type-zip', rar: 'file-type-zip', '7z': 'file-type-zip', tar: 'file-type-zip',
  gz: 'file-type-zip', bz2: 'file-type-zip', xz: 'file-type-zip',
  jar: 'file-type-jar', war: 'file-type-jar',
  key: 'file-type-key', pem: 'file-type-key',
  sln: 'file-type-sln', slnx: 'file-type-sln', csproj2: 'file-type-csproj',
  gemfile: 'file-type-ruby'
}

/** 特殊文件名 → 图标 id */
const NAME_MAP: Record<string, VscodeIconId> = {
  dockerfile: 'file-type-docker',
  makefile: 'file-type-shell',
  license: 'file-type-license',
  gitignore: 'file-type-git',
  gemfile: 'file-type-ruby',
  '.env': 'file-type-ini'
}

const ALL_IDS = [...new Set<VscodeIconId>([...Object.values(EXT_MAP), ...Object.values(NAME_MAP), 'default-file'])]

/** 类名全集(uno safelist 用;彩色图标带 ?bg 后缀保留多色) */
export const ALL_ICON_CLASSES: string[] = ALL_IDS.map((id) => `i-vscode-icons-${id}?bg`)

function cls(id: VscodeIconId): string {
  return `i-vscode-icons-${id}?bg`
}

/** 文件名 → vscode-icons 类名(仅供总览页对照) */
export function getLegacyFileIconClass(filename: string | undefined | null): string {
  const name = (filename || '').split('/').pop() || ''
  const lower = name.toLowerCase()
  if (NAME_MAP[lower]) return cls(NAME_MAP[lower])
  if (lower.startsWith('.env')) return cls('file-type-ini')
  const dot = name.lastIndexOf('.')
  const ext = dot === -1 || dot === 0 ? '' : name.slice(dot + 1).toLowerCase()
  return cls(EXT_MAP[ext] ?? 'default-file')
}
