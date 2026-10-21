/**
 * 文件图标映射:扩展名 / 特殊文件名 → **手写 SVG 字形**(assets/file-icons/*.svg)。
 *
 * - 字形为设计稿:16 网格、硬编码品牌实色填充、扁平剪影(虚线描边/无 currentColor),
 *   因此以 <img> 渲染即可,自带明暗两种主题下的可读性,无需滤镜增强
 * - 一个字形服务多个扩展名(如 ts/mts/cts → typescript)
 * - 未命中 → default-file;文件夹 → folder(32 网格双层渐变)
 *
 * 替换前的 vscode-icons 映射保留在 utils/legacyVscodeFileIcons.ts,仅供图标总览页做前后对照。
 */

/** 扩展名 → 手写字形名(与 assets/file-icons/<glyph>.svg 同名) */
const EXT_GLYPH: Record<string, string> = {
  // 代码
  ts: 'typescript', mts: 'typescript', cts: 'typescript',
  tsx: 'react-ts',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript',
  jsx: 'react',
  py: 'python', rs: 'rust', go: 'go', java: 'java', class: 'java-class',
  c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp',
  cs: 'csharp', csproj: 'csproj', rb: 'ruby', erb: 'ruby',
  php: 'php', phtml: 'php', swift: 'swift',
  kt: 'kotlin', kts: 'kotlin', scala: 'scala', dart: 'dart',
  lua: 'lua', ex: 'elixir', exs: 'elixir', erl: 'erlang', hrl: 'erlang',
  r: 'r', jl: 'julia', pl: 'perl', pm: 'perl',
  groovy: 'groovy', clj: 'clojure', cljs: 'clojure',
  hs: 'haskell', lhs: 'haskell',
  vue: 'vue', svelte: 'svelte', astro: 'astro',
  html: 'html', htm: 'html', cshtml: 'html',
  css: 'css', scss: 'scss', sass: 'sass', less: 'less',
  xml: 'xml', plist: 'xml', sql: 'sql', graphql: 'graphql', gql: 'graphql',
  // 数据 / 配置
  json: 'json', jsonc: 'json',
  yaml: 'yaml', yml: 'yaml', toml: 'toml',
  ini: 'ini', cfg: 'ini', conf: 'ini', properties: 'ini', env: 'ini',
  // 脚本 / 终端
  sh: 'shell', bash: 'shell', zsh: 'shell', fish: 'shell', mk: 'shell',
  ps1: 'powershell', psm1: 'powershell', psd1: 'powershell',
  bat: 'bat', cmd: 'bat',
  // 文档 / 办公
  md: 'markdown', mdx: 'markdown', markdown: 'markdown',
  txt: 'text', text: 'text', log: 'text',
  doc: 'word', docx: 'word', rtf: 'word', odt: 'word',
  pdf: 'pdf',
  xls: 'excel', xlsx: 'excel', csv: 'excel', ods: 'excel',
  ppt: 'powerpoint', pptx: 'powerpoint', odp: 'powerpoint',
  // 媒体 / 资源
  png: 'image', jpg: 'image', jpeg: 'image', gif: 'image',
  webp: 'image', bmp: 'image', ico: 'image',
  mp3: 'audio', wav: 'audio', flac: 'audio', ogg: 'audio',
  mp4: 'video', mkv: 'video', avi: 'video', webm: 'video', mov: 'video',
  // 归档 / 密钥 / 工程
  zip: 'zip', rar: 'zip', '7z': 'zip', tar: 'zip',
  gz: 'zip', bz2: 'zip', xz: 'zip',
  jar: 'jar', war: 'jar',
  key: 'key', pem: 'key',
  sln: 'sln', slnx: 'sln',
  gemfile: 'ruby'
}

/** 特殊文件名 → 手写字形名 */
const NAME_GLYPH: Record<string, string> = {
  dockerfile: 'docker',
  makefile: 'shell',
  license: 'license',
  gitignore: 'git',
  gemfile: 'ruby',
  '.env': 'ini'
}

/** 未命中扩展名时的兜底字形 */
export const DEFAULT_FILE_GLYPH = 'default-file'
/** 文件夹字形(32 网格,双层渐变) */
export const FOLDER_GLYPH = 'folder'

/** 字形资源:Vite 解析为带 hash 的 URL(base 相对,file:// 与打包后均可加载) */
const glyphModules = import.meta.glob('../assets/file-icons/*.svg', {
  query: '?url',
  import: 'default',
  eager: true
}) as Record<string, string>

const URL_BY_GLYPH: Record<string, string> = Object.fromEntries(
  Object.entries(glyphModules).map(([path, url]) => [path.slice(path.lastIndexOf('/') + 1, -'.svg'.length), url])
)

/** 按字形名取资源 URL(找不到返回 undefined,调用方可据此回退) */
export function fileGlyphUrl(glyph: string): string | undefined {
  return URL_BY_GLYPH[glyph]
}

function extensionOf(filename: string): string {
  const dot = filename.lastIndexOf('.')
  if (dot === -1 || dot === 0) return ''
  return filename.slice(dot + 1).toLowerCase()
}

/** 文件名 → 字形名(先查特殊文件名,再查扩展名,未命中 → default-file) */
export function getFileGlyph(filename: string | undefined | null): string {
  const name = (filename || '').split('/').pop() || ''
  const lower = name.toLowerCase()
  if (NAME_GLYPH[lower]) return NAME_GLYPH[lower]
  if (lower.startsWith('.env')) return 'ini'
  return EXT_GLYPH[extensionOf(name)] ?? DEFAULT_FILE_GLYPH
}

/** 文件名 → 图标 URL(文件树 / 页签 / 提及 / 知识库共用) */
export function getFileIconUrl(filename: string | undefined | null): string {
  return fileGlyphUrl(getFileGlyph(filename)) ?? fileGlyphUrl(DEFAULT_FILE_GLYPH)!
}

/** 文件夹图标 URL */
export function getFolderIconUrl(): string {
  return fileGlyphUrl(FOLDER_GLYPH)!
}
