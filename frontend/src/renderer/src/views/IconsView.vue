<script setup lang="ts">
import { computed, ref } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { resolveSystemDark } from '@/theme'
import { FILE_ICON_GROUPS, FILE_ICONS } from '@/utils/fileIconGallery'
import { NAME_TO_GLYPH, UI_GLYPHS } from '@/utils/uiIconGallery'
import { LUCIDE_ICON_MAP } from '@/plugins/lucide'
import { fileGlyphUrl } from '@/utils/fileIcons'
import { getLegacyFileIconClass } from '@/utils/legacyVscodeFileIcons'
import MacFileIcon from '@/components/common/MacFileIcon.vue'

/**
 * 图标总览页(demo):把前端全部图标集中陈列,并在亮/暗两种底色下并排比对。
 *
 * - 手写文件图标(assets/file-icons/*.svg):按 fileIcons.ts 的扩展名分组陈列
 * - 手写 UI 图标(assets/ui-icons/*.svg):**当前生效**的应用图标集(plugins/appIcons.ts 注册)
 * - lucide 原图标:替换前的映射(plugins/lucide.ts),仅作前后对照
 * - 零散图标:UnoCSS i-lucide-* 类、文件夹 data-URI、内联折叠箭头、知识库 MacFileIcon
 * - 文件树实机预览:同一文件名并排给出「现有 vscode-icons ⟶ 手写 SVG」,便于验收切换
 *
 * 入口:`#/icons`(未挂侧边栏,避免污染正式导航)。
 */
const settings = useSettingsStore()

/** 顶部主题切换:写回 settings store,经 App.vue 的 watch 全局生效并持久化 */
const themeMode = computed({
  get: () => settings.themeMode,
  set: (mode: 'light' | 'dark' | 'system') => settings.setThemeMode(mode)
})
const isDark = computed(() => settings.themeMode === 'dark' || (settings.themeMode === 'system' && resolveSystemDark()))

/** 图标尺寸档位:文件树 16px / 页签与列表 16~20px / 卡片大图标 32px */
const SIZES = [16, 24, 32]

/** 双主题底色档:卡片底色写死(不随全局主题翻转),保证两栏始终可并排比对 */
const TONES = [
  { key: 'light', label: '亮色主题' },
  { key: 'dark', label: '暗色主题' }
] as const

/** 替换前的 lucide 映射表(icon-xxx → 组件):仅本页用作前后对照 */
const uiIcons = Object.entries(LUCIDE_ICON_MAP).map(([name, comp]) => ({ name, comp }))

/** 手写 UI 字形集的尺寸档位:侧栏分段 14 / 工具条 18 / 卡片与空态 24 */
const UI_SIZES = [14, 18, 24]

/** 覆盖自检:替换前的每个注册名都应能映射到手写字形(缺了才会非空) */
const unmappedNames = computed(() => Object.keys(LUCIDE_ICON_MAP).filter((name) => !NAME_TO_GLYPH[name]))

/**
 * 资源自检:空文件 / 非法 XML 的 svg 会静默显示成浏览器破图,
 * 这里把加载失败的图标名收集起来在页面上列出来(否则很容易漏掉)。
 */
const brokenIcons = ref<Set<string>>(new Set())
function markBroken(name: string): void {
  brokenIcons.value = new Set(brokenIcons.value).add(name)
}

/** 文件树实机模拟行:现用图标类由 fileIcons.ts 现算,手写图标按名取 URL */
interface TreeRow {
  file: string
  folder?: boolean
  /** 手写 SVG 图标名 */
  icon: string
}
const TREE_ROWS: TreeRow[] = [
  { file: 'src', folder: true, icon: 'folder' },
  { file: 'src/main.ts', icon: 'typescript' },
  { file: 'src/App.vue', icon: 'vue' },
  { file: 'src/Dashboard.tsx', icon: 'react-ts' },
  { file: 'index.html', icon: 'html' },
  { file: 'theme.css', icon: 'css' },
  { file: 'package.json', icon: 'json' },
  { file: 'pnpm-lock.yaml', icon: 'yaml' },
  { file: '.gitignore', icon: 'git' },
  { file: 'Dockerfile', icon: 'docker' },
  { file: 'Cargo.toml', icon: 'toml' },
  { file: 'Makefile', icon: 'shell' },
  { file: 'README.md', icon: 'markdown' },
  { file: 'LICENSE', icon: 'license' },
  { file: 'server/main.rs', icon: 'rust' },
  { file: 'scripts/deploy.py', icon: 'python' },
  { file: '.env.local', icon: 'ini' },
  { file: 'fixtures/报表.xlsx', icon: 'excel' },
  { file: 'assets/logo.png', icon: 'image' },
  { file: 'release/app.zip', icon: 'zip' },
  { file: 'notes.txt', icon: 'text' }
]
const treeRows = computed(() =>
  TREE_ROWS.map((row) => ({
    ...row,
    name: row.file.split('/').pop() ?? row.file,
    legacyClass: row.folder ? '' : getLegacyFileIconClass(row.file),
    url: fileGlyphUrl(row.icon) ?? ''
  }))
)

/** 文件夹图标 URL(与分组里的 folder 同一份资源) */
const folderUrl = fileGlyphUrl('folder') ?? ''

/** UnoCSS 的 i-lucide-* 类(侧栏/页签等处直接内联使用,不走组件注册表) */
const UNO_ICON_CLASSES = [
  'i-lucide-folder',
  'i-lucide-folder-open',
  'i-lucide-file-code',
  'i-lucide-settings',
  'i-lucide-terminal',
  'i-lucide-archive',
  'i-lucide-image',
  'i-lucide-table',
  'i-lucide-database',
  'i-lucide-lock'
]

/** 知识库 Mac Finder 风格图标配色(与 KnowledgeView 的 COLOR_OPTIONS 同源) */
const MAC_COLORS = ['#0a84ff', '#ff9f0a', '#30d158', '#bf5af2']
</script>

<template>
  <div class="icons-view">
    <header class="iv-head">
      <div class="iv-head-text">
        <h1>图标总览</h1>
        <p class="iv-sub">
          手写文件图标 <b>{{ FILE_ICONS.length }}</b> 个(覆盖文件树 / 页签 / 提及 / 知识库,<b>已生效</b>) · 手写 UI 图标 <b>{{ UI_GLYPHS.length }}</b> 个字形(覆盖
          <b>{{ Object.keys(NAME_TO_GLYPH).length }}</b> 个注册名,<b>已生效</b>) · 替换前 lucide 图标 <b>{{ uiIcons.length }}</b> 个 ·
          零散图标 <b>{{ UNO_ICON_CLASSES.length + 3 }}</b> 个。每节均以亮/暗两种底色并排陈列;入口 <code>#/icons</code>(未挂侧边栏)。
        </p>
      </div>
      <a-radio-group v-model="themeMode" type="button" size="small">
        <a-radio value="light">亮色</a-radio>
        <a-radio value="dark">暗色</a-radio>
        <a-radio value="system">跟随系统</a-radio>
      </a-radio-group>
    </header>

    <p class="iv-note">
      当前全局主题:<b>{{ isDark ? '暗色' : '亮色' }}</b> — 下面的预览卡片底色固定,不随全局主题变化,用于直接比对两套主题下的可读性。
      <span v-if="brokenIcons.size" class="iv-broken">
        资源加载失败 {{ brokenIcons.size }} 个:{{ [...brokenIcons].join('、') }}(svg 为空或 XML 非法)
      </span>
    </p>

    <!-- 1. 手写文件图标(按 fileIcons.ts 的扩展名分组) -->
    <section v-for="group in FILE_ICON_GROUPS" :key="group.title" class="iv-section">
      <h2>
        {{ group.title }}
        <span class="iv-count">{{ group.icons.length }}</span>
      </h2>
      <div class="iv-split">
        <div v-for="tone in TONES" :key="tone.key" class="iv-card" :class="`iv-card--${tone.key}`">
          <div class="iv-card-head">{{ tone.label }}</div>
          <div class="iv-grid">
            <div v-for="icon in group.icons" :key="icon.name" class="iv-cell" :title="`${icon.name}.svg`">
              <div class="iv-cell-icons">
                <img v-for="size in SIZES" :key="size" :src="icon.url" :width="size" :height="size" :alt="icon.name" @error="markBroken(icon.name)" />
              </div>
              <div class="iv-cell-name">{{ icon.name }}</div>
              <div class="iv-cell-covers">{{ icon.covers }}</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- 2. 文件树实机预览:现有 vscode-icons → 手写 SVG → 文件名 -->
    <section class="iv-section">
      <h2>
        文件树实机预览(16px 行高)
        <span class="iv-count">替换前 vscode-icons → 手写 SVG(已生效)</span>
      </h2>
      <div class="iv-split">
        <div v-for="tone in TONES" :key="tone.key" class="iv-card" :class="`iv-card--${tone.key}`">
          <div class="iv-card-head">{{ tone.label }}</div>
          <div class="iv-tree">
            <div v-for="row in treeRows" :key="row.file" class="iv-trow">
              <span class="iv-arrow" :class="row.folder && 'expanded'">
                <svg
                  v-if="row.folder"
                  viewBox="0 0 24 24"
                  width="16"
                  height="16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <polyline points="9 18 15 12 9 6" />
                </svg>
              </span>
              <span class="iv-glyph" :class="row.legacyClass" />
              <img class="iv-glyph-img" :src="row.url" width="16" height="16" :alt="row.name" />
              <span class="iv-trow-name">{{ row.name }}</span>
              <span class="iv-trow-path">{{ row.file }}</span>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- 3. 文件夹与特殊图标对照 -->
    <section class="iv-section">
      <h2>文件夹 / 特殊图标</h2>
      <div class="iv-split">
        <div v-for="tone in TONES" :key="tone.key" class="iv-card" :class="`iv-card--${tone.key}`">
          <div class="iv-card-head">{{ tone.label }}</div>
          <div class="iv-specials">
            <div class="iv-cell">
              <img :src="folderUrl" width="32" height="32" alt="folder" />
              <div class="iv-cell-name">folder.svg</div>
              <div class="iv-cell-covers">已生效(替代原 data-URI 文件夹)</div>
            </div>
            <div v-for="color in MAC_COLORS" :key="color" class="iv-cell">
              <MacFileIcon type="folder" :color="color" :size="32" />
              <div class="iv-cell-name">MacFileIcon</div>
              <div class="iv-cell-covers">知识库 · folder {{ color }}</div>
            </div>
            <div class="iv-cell">
              <MacFileIcon type="document" color="#8e8e93" :size="32" />
              <div class="iv-cell-name">MacFileIcon</div>
              <div class="iv-cell-covers">知识库 · document</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- 4. 手写 UI 图标:当前生效的应用图标集 -->
    <section class="iv-section">
      <h2>
        UI 图标 · 手写 SVG(当前生效)
        <span class="iv-count">{{ UI_GLYPHS.length }} 字形</span>
        <span class="iv-count">{{ Object.keys(NAME_TO_GLYPH).length }} 注册名</span>
        <span v-if="unmappedNames.length" class="iv-count iv-count--warn">未映射 {{ unmappedNames.length }}</span>
      </h2>
      <p class="iv-hint">
        由 plugins/appIcons.ts 以 icon-* 名全局注册,模板与 JS 侧用法保持不变。造型与侧栏伸缩 / 页签伸缩按钮的手写内联 SVG 同源:
        48 网格、源文件 stroke-width 4（运行时统一由 `UI_ICON_STROKE_WIDTH = 1.8` 覆盖，内部 ×2 换算）、currentColor、圆角端点 → 随 hover 与选中态变色。
        下面三档尺寸渲染的是**注册后的真实组件**(非图片),末尾灰色为替换前的 lucide 原图标,用于同尺寸对照。
      </p>
      <div class="iv-split">
        <div v-for="tone in TONES" :key="tone.key" class="iv-card" :class="`iv-card--${tone.key}`">
          <div class="iv-card-head">{{ tone.label }}</div>
          <div class="iv-grid iv-grid--ui">
            <div v-for="glyph in UI_GLYPHS" :key="glyph.glyph" class="iv-cell iv-cell--ui">
              <div class="iv-cell-row">
                <template v-if="glyph.names.length">
                  <component v-for="size in UI_SIZES" :key="size" :is="glyph.names[0]" :size="size" />
                </template>
                <template v-else>
                  <img
                    v-for="size in UI_SIZES"
                    :key="size"
                    :src="glyph.url"
                    :width="size"
                    :height="size"
                    :alt="glyph.glyph"
                    @error="markBroken(glyph.glyph)"
                  />
                </template>
                <component
                  :is="LUCIDE_ICON_MAP[glyph.names[0]]"
                  v-if="glyph.names.length"
                  :size="18"
                  :stroke-width="2"
                  class="iv-lucide-ref"
                />
              </div>
              <div class="iv-cell-name">{{ glyph.glyph }}</div>
              <div class="iv-cell-covers">{{ glyph.names.length ? glyph.names.join(' · ') : '备用字形(暂无注册名)' }}</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- 5. 替换前的 lucide 图标(已停用,保留对照) -->
    <section class="iv-section">
      <h2>
        替换前:lucide-vue-next 原图标
        <span class="iv-count">{{ uiIcons.length }}</span>
        <span class="iv-count">已停用</span>
      </h2>
      <p class="iv-hint">
        映射表保留在 plugins/lucide.ts(LUCIDE_ICON_MAP),应用启动时不再安装 —— 仅本页用于「替换前 / 替换后」对照。
      </p>
      <div class="iv-split">
        <div v-for="tone in TONES" :key="tone.key" class="iv-card" :class="`iv-card--${tone.key}`">
          <div class="iv-card-head">{{ tone.label }}</div>
          <div class="iv-grid iv-grid--ui">
            <div v-for="icon in uiIcons" :key="icon.name" class="iv-cell iv-cell--ui">
              <component :is="icon.comp" :size="18" :stroke-width="1.8" />
              <div class="iv-cell-name">{{ icon.name }}</div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- 6. 零散图标:UnoCSS 类 / 内联 SVG / 现有文件夹 data-URI -->
    <section class="iv-section">
      <h2>零散图标(UnoCSS 类 / 内联 SVG)</h2>
      <div class="iv-split">
        <div v-for="tone in TONES" :key="tone.key" class="iv-card" :class="`iv-card--${tone.key}`">
          <div class="iv-card-head">{{ tone.label }}</div>
          <div class="iv-grid iv-grid--ui">
            <div v-for="cls in UNO_ICON_CLASSES" :key="cls" class="iv-cell iv-cell--ui">
              <span class="iv-uno-glyph" :class="cls" />
              <div class="iv-cell-name">{{ cls }}</div>
            </div>
            <div class="iv-cell iv-cell--ui">
              <span class="iv-arrow expanded">
                <svg
                  viewBox="0 0 24 24"
                  width="16"
                  height="16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <polyline points="9 18 15 12 9 6" />
                </svg>
              </span>
              <div class="iv-cell-name">FileTreeNode 折叠箭头</div>
            </div>

          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<style scoped>
/* 页面底:沿用应用 token,跟着全局主题走 */
.icons-view {
  height: 100%;
  overflow-y: auto;
  padding: 20px 24px 48px;
  color: var(--text-primary);
}

.iv-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 8px;
}

.iv-head-text h1 {
  margin: 0 0 4px;
  font-size: var(--font-size-xl);
  font-weight: 600;
}

.iv-sub {
  margin: 0;
  font-size: var(--font-size-sm);
  line-height: 1.7;
  color: var(--text-secondary);
}

.iv-sub code {
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--bg-2);
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
}

.iv-note {
  margin: 0 0 18px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

/* 破图自检提示:红色高亮,避免加载失败被静默忽略 */
.iv-broken {
  display: inline-block;
  margin-left: 8px;
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--danger);
  color: #fff;
  font-size: var(--font-size-2xs);
}

.iv-section {
  margin-bottom: 26px;
}

.iv-section h2 {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 10px;
  font-size: var(--font-size-md);
  font-weight: 600;
}

.iv-count {
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--bg-2);
  color: var(--text-tertiary);
  font-size: var(--font-size-2xs);
  font-weight: 500;
}

.iv-count--warn {
  background: var(--danger);
  color: #fff;
}

.iv-hint {
  margin: -2px 0 10px;
  font-size: var(--font-size-xs);
  line-height: 1.7;
  color: var(--text-tertiary);
}

/* 双主题并排:内容一致,底色固定 */
.iv-split {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}

.iv-card {
  border-radius: var(--radius-lg);
  border: 1px solid transparent;
  overflow: hidden;
}

.iv-card--light {
  background: #ffffff;
  border-color: #e4e8ee;
  color: #1f2933;
}

.iv-card--dark {
  background: #16181d;
  border-color: #272c34;
  color: #e6eaf0;
}

.iv-card-head {
  padding: 7px 12px;
  font-size: var(--font-size-2xs);
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  opacity: 0.55;
  border-bottom: 1px solid rgba(128, 128, 128, 0.22);
}

/* 图标网格:每格三档尺寸 + 图标名 + 覆盖的扩展名 */
.iv-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(112px, 1fr));
  gap: 10px;
  padding: 12px;
}

.iv-grid--ui {
  grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
}

.iv-cell {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  min-width: 0;
}

.iv-cell--ui {
  align-items: center;
  text-align: center;
}

.iv-cell-icons {
  display: flex;
  align-items: flex-end;
  gap: 6px;
  height: 32px;
}

/* 手写 UI 字形行:三档尺寸 + 末尾 lucide 对照(降透明度区分) */
.iv-cell-row {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 28px;
}

.iv-cell-row img {
  display: block;
}

.iv-lucide-ref {
  margin-left: 3px;
  opacity: 0.4;
}

.iv-cell-icons img {
  display: block;
}

.iv-cell-name {
  width: 100%;
  font-size: var(--font-size-2xs);
  font-weight: 500;
  word-break: break-all;
}

.iv-cell-covers {
  width: 100%;
  font-size: var(--font-size-2xs);
  line-height: 1.45;
  opacity: 0.6;
  word-break: break-all;
}

/* 文件树实机预览:行样式对齐 FileTreeNode(32px 行高 / 15px 字号) */
.iv-tree {
  padding: 6px 8px 10px;
}

.iv-trow {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 30px;
  color: inherit;
}

.iv-arrow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
  opacity: 0.5;
}

.iv-arrow.expanded {
  transform: rotate(90deg);
}

/* 现有 vscode-icons 彩色图标:按卡片底色做对比度增强(同主题文件里的 --icon-filter) */
.iv-glyph {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

.iv-card--light .iv-glyph {
  filter: saturate(1.4) brightness(0.8);
}

.iv-card--dark .iv-glyph {
  filter: saturate(1.15) brightness(1.3);
}

.iv-glyph-img {
  display: block;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
}

.iv-trow-name {
  font-size: var(--font-size-md);
  font-weight: 500;
}

.iv-trow-path {
  margin-left: auto;
  font-size: var(--font-size-2xs);
  opacity: 0.45;
  white-space: nowrap;
}

/* 文件夹 / 特殊图标:单行横排 */
.iv-specials {
  display: flex;
  flex-wrap: wrap;
  gap: 14px;
  padding: 12px;
}

.iv-uno-glyph {
  display: inline-block;
  width: 18px;
  height: 18px;
}
</style>
