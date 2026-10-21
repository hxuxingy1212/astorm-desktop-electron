import { resolve } from 'path'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import UnoCSS from 'unocss/vite'

/**
 * GitHub Pages 预览站构建配置。
 *
 * 与 electron.vite.config.ts 共用 renderer 源码（真实组件/样式/图标），
 * 区别只有三点：
 * - root 指到 src/preview，产出两个页面：介绍页（index.html）与应用预览（app/index.html）
 * - base 用相对路径，GitHub Pages 部署在 /<repo>/ 子路径下也能直接加载
 * - 应用预览页在入口处先装 mock 桥（preview-bridge.js），不依赖 Electron preload
 */
const previewRoot = resolve(__dirname, 'src/preview')

export default defineConfig({
  root: previewRoot,
  base: './',
  plugins: [vue(), UnoCSS({ configFile: resolve(__dirname, 'uno.config.ts') })],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer/src'),
      '@astorm/office-viewer/style.css': resolve(
        __dirname,
        'resources/office/viewer/office-viewer.css'
      ),
      '@astorm/office-viewer': resolve(__dirname, 'resources/office/viewer')
    }
  },
  build: {
    outDir: resolve(__dirname, 'preview-dist'),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        site: resolve(previewRoot, 'index.html'),
        docs: resolve(previewRoot, 'docs.html'),
        app: resolve(previewRoot, 'app/index.html')
      }
    }
  }
})
