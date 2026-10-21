/**
 * 系统目录解析（与 Rust AppDir 同源，唯一事实源 = napi getDataPaths）。
 *
 * 布局（系统目录 = 应用自身相关；用户目录 = 设置里配的工作区根，默认 ~/astorm）：
 *   <appData>/astorm/
 *     config/    providers.json / backend-settings.json / security.json / mcp_servers.json / config.json
 *     data/      sqlite.db / session/ / index/ / skill_index/（Rust 存储层）
 *     log/       按天滚动日志（Rust）
 *     plugins/   插件目录 + plugins.json
 *     skills/    技能（预留）
 *     cache/     应用缓存（icons/ 站点与邮件图标）
 *     electron/  Chromium 内部文件（userData 重定向目标）
 *   <用户目录>/{workspace/, knowledge/}
 */
import { app } from 'electron'
import { join } from 'path'
import { isNapiLoaded, napiCall } from './napi'

/** 系统目录根（astorm/）。优先取 napi getDataPaths（与 Rust 端严格一致）；
 *  .node 未加载的开发环境回退 appData/astorm（mac/win 与 dirs::data_dir 一致）。
 *  必须在 app ready 前调用（Chromium userData 重定向依赖）。 */
export function systemDir(): string {
  if (isNapiLoaded()) {
    try {
      const paths = napiCall('getDataPaths') as { appDir?: string } | null
      if (paths?.appDir) return paths.appDir
    } catch {
      /* fallthrough to fallback */
    }
  }
  // Linux 的 dirs::data_dir() 是 $XDG_DATA_HOME (~/.local/share)，而 Electron
  // app.getPath('appData') 是 ~/.config；这里必须跟 Rust 侧对齐。
  if (process.platform === 'linux') {
    const base = process.env.XDG_DATA_HOME || join(app.getPath('home'), '.local', 'share')
    return join(base, 'astorm')
  }
  return join(app.getPath('appData'), 'astorm')
}

/** 配置目录：providers.json / backend-settings.json / security.json / mcp_servers.json 等 JSON 配置。 */
export function configDir(): string {
  return join(systemDir(), 'config')
}

/** 插件目录：插件包目录 + plugins.json 清单。 */
export function pluginsDir(): string {
  return join(systemDir(), 'plugins')
}

/** Chromium 内部文件目录（userData 重定向目标）。 */
export function electronDir(): string {
  return join(systemDir(), 'electron')
}
