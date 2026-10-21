import { resolve } from 'path'
import { defineConfig, externalizeDepsPlugin } from 'electron-vite'
import vue from '@vitejs/plugin-vue'
import UnoCSS from 'unocss/vite'

export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin()]
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    build: {
      rollupOptions: {
        // 两个 preload：index = 本机桌面形态（window.api 全量 IPC 桥）；
        // cloud = 云端壳子形态（只把访问令牌写进远端页面 localStorage，**刻意不暴露
        // window.api**——远端 Web UI 的 web-bridge 以「window.api 存在 = 桌面 preload 桥」
        // 自退，挂了它远端页面会误判成桌面形态去调本机 IPC；详见 src/preload/cloud.ts）
        input: {
          index: resolve(__dirname, 'src/preload/index.ts'),
          cloud: resolve(__dirname, 'src/preload/cloud.ts')
        }
      }
    }
  },
  renderer: {
    resolve: {
      alias: {
        '@': resolve(__dirname, 'src/renderer/src'),
        // @astorm/office-viewer:随包分发的 office 预览组件库(scripts/sync-office.mjs 从
        // astorm-office 仓库 office-kit 拷贝到 resources/office/viewer;kit 缺失为抛错占位)
        // 长键在前:rollup alias 语义下 '@astorm/office-viewer' 也会前缀匹配 './style.css' 子路径
        '@astorm/office-viewer/style.css': resolve(__dirname, 'resources/office/viewer/office-viewer.css'),
        '@astorm/office-viewer': resolve(__dirname, 'resources/office/viewer')
      }
    },
    plugins: [vue(), UnoCSS()]
  }
})
