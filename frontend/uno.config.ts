import { defineConfig, presetIcons, presetUno, transformerDirectives, transformerVariantGroup } from 'unocss'
// 运行时文件图标已改为手写 SVG(utils/fileIcons.ts),这里的 vscode-icons 类只为
// 图标总览页的「替换前」对照保留 → safelist 指向 legacy 模块
import { ALL_ICON_CLASSES } from './src/renderer/src/utils/legacyVscodeFileIcons'

export default defineConfig({
  presets: [
    presetUno(),
    // 图标类:i-lucide-* / i-vscode-icons-*?bg(legacy 对照);动态类名靠 safelist 保编译
    presetIcons({ scale: 1, warn: false })
  ],
  safelist: [...ALL_ICON_CLASSES],
  transformers: [transformerDirectives(), transformerVariantGroup()],
  // 基础样式(reset)交给 Arco Design,避免相互覆盖
  preflight: false
})