/**
 * stealth 反爬预加载脚本（在页面脚本运行前执行）。
 *
 * 与 `page-render.cjs` 配合：窗口以 `contextIsolation:false` 加载本脚本，
 * 因此这里的改动直接落在**页面世界**，抹掉常见的自动化/无头指纹：
 *   - navigator.webdriver / window.chrome / plugins / mimeTypes / languages
 *   - navigator.hardwareConcurrency / deviceMemory
 *   - WebGL UNMASKED_VENDOR/RENDERER
 *   - permissions.query('notifications') 与 Notification.permission 对齐
 *
 * 目标不是「绕过所有风控」，而是抹掉一眼可辨的自动化特征，配合真实 UA / 语言 / 头，
 * 让百度、谷歌等对爬虫较敏感的站点把我们当作普通 Chromium 访问。
 */
;(() => {
  try {
    const nav = navigator
    const proto = Object.getPrototypeOf(nav)

    const def = (obj, prop, getter) => {
      try {
        Object.defineProperty(obj, prop, { get: getter, configurable: true })
      } catch {
        /* 某些属性不可配置则跳过 */
      }
    }

    // webdriver
    def(proto, 'webdriver', () => false)
    def(nav, 'webdriver', () => false)

    // window.chrome（真实 Chrome 有该对象；Electron 默认没有）
    if (!window.chrome) {
      Object.defineProperty(window, 'chrome', { value: {}, configurable: true, writable: true })
    }
    if (window.chrome && !window.chrome.runtime) {
      Object.defineProperty(window.chrome, 'runtime', { value: {}, configurable: true, writable: true })
    }

    // 语言
    def(proto, 'languages', () => ['zh-CN', 'zh', 'en'])
    def(proto, 'language', () => 'zh-CN')

    // plugins / mimeTypes：非空数组（无头环境常为空 → 明显特征）
    const plugin = {
      name: 'Chrome PDF Plugin',
      filename: 'internal-pdf-viewer',
      description: 'Portable Document Format'
    }
    def(proto, 'plugins', () => [plugin, plugin, plugin])
    def(proto, 'mimeTypes', () => [])

    // 硬件指纹
    def(proto, 'hardwareConcurrency', () => 8)
    def(proto, 'deviceMemory', () => 8)

    // WebGL 厂商/渲染器
    try {
      const patch = (Ctor) => {
        if (!Ctor) return
        const orig = Ctor.prototype.getParameter
        Ctor.prototype.getParameter = function (p) {
          if (p === 37445) return 'Intel Inc.' // UNMASKED_VENDOR_WEBGL
          if (p === 37446) return 'Intel Iris OpenGL Engine' // UNMASKED_RENDERER_WEBGL
          return orig.apply(this, [p])
        }
      }
      patch(window.WebGLRenderingContext)
      patch(window.WebGL2RenderingContext)
    } catch {
      /* ignore */
    }

    // 权限与通知状态保持一致（无头环境两者常矛盾）
    try {
      const origQuery = window.navigator.permissions && window.navigator.permissions.query
      if (origQuery) {
        window.navigator.permissions.query = (params) =>
          params && params.name === 'notifications'
            ? Promise.resolve({ state: Notification.permission, onchange: null })
            : origQuery.call(window.navigator.permissions, params)
      }
    } catch {
      /* ignore */
    }
  } catch {
    /* stealth 失败不应影响页面加载 */
  }
})()
