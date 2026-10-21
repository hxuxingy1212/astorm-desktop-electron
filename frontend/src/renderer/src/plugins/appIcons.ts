/**
 * 应用图标集:以既有 Arco 命名(icon-xxx)注册**手写 SVG 字形**组件。
 *
 * - 字形来自 assets/ui-icons/(48 网格 / stroke-width 4 / currentColor / 圆角),
 *   与侧栏伸缩、页签伸缩按钮的手写内联 SVG 同一套造型
 * - 一个字形可服务多个注册名(icon-data / icon-db / icon-storage 都是数据库圆柱),
 *   映射表见 utils/uiIconGallery.ts 的 NAME_TO_GLYPH
 * - 组件 props 与替换前的 lucide 组件一致:size / stroke-width / color / class / style;
 *   stroke-width 沿用 lucide 的 24 网格语义,默认取全局标准 UI_ICON_STROKE_WIDTH
 * - 模板里的 `<icon-xxx />` 与 JS 侧 `<component :is="'icon-xxx'">` 都不需要改动
 *
 * 替换前的 lucide 映射保留在 plugins/lucide.ts,仅供图标总览页做前后对照。
 */
import { h, type App, type Component } from 'vue'
import AppIcon from '@/components/common/AppIcon.vue'
import { NAME_TO_GLYPH, UI_ICON_STROKE_WIDTH } from '@/utils/uiIconGallery'

/** icon-名称 → 组件(供 JS 侧按名取组件,如 registry 的 :icon 字符串) */
export const ICON_MAP: Record<string, Component> = {}

for (const [name, glyph] of Object.entries(NAME_TO_GLYPH)) {
  ICON_MAP[name] = {
    name,
    props: {
      size: { type: [Number, String], default: 18 },
      strokeWidth: { type: [Number, String], default: UI_ICON_STROKE_WIDTH },
      color: { type: String, default: '' }
    },
    setup(props) {
      // class / style 等剩余属性由 Vue 自动透传到 AppIcon(进而落到外壳元素)
      return () => h(AppIcon, { glyph, size: props.size, strokeWidth: props.strokeWidth, color: props.color })
    }
  } as Component
}

export function installAppIcons(app: App): void {
  for (const [name, comp] of Object.entries(ICON_MAP)) {
    app.component(name, comp)
  }
}

/** 供 JS 侧按名取组件(如 panels/registry 的 icon 字符串 → <component :is>) */
export function appIcon(name: string): Component | undefined {
  return ICON_MAP[name]
}
