/**
 * 浏览器数据服务：本机 Chrome Cookie 导入 / 缓存与站点数据清理。
 *
 * 命令走主进程 `browser-data.ts`（Rust 解密 → Electron 会话写入/清理），
 * 这里只负责调用与「本进程只自动导入一次」的守卫。
 *
 * 为什么需要守卫：macOS 首次导入会弹系统钥匙串授权框，若每次打开网页页签都尝试，
 * 用户点「拒绝」后会反复弹框。因此自动导入路径**成功或失败都只跑一次**；
 * 设置页的手动按钮不受守卫限制（用户明确点击 → 允许重试）。
 */
import { invoke } from '@/services/backend'
import type { ChromeCookieImportResult, ClearBrowserDataResult } from '@/services/dto'

export type { ChromeCookieImportResult, ClearBrowserDataResult }

/** 自动导入是否已尝试过（模块级，随进程生命周期）。 */
let autoImportAttempted = false

/** 自动导入路径是否已经跑过（成功或失败都算）。 */
export function isAutoImportAttempted(): boolean {
  return autoImportAttempted
}

/** 导入本机 Chrome Cookie 到内置浏览器会话（一次性，不修改 Chrome 数据）。 */
export async function importChromeCookies(): Promise<ChromeCookieImportResult> {
  const result = (await invoke('import_chrome_cookies')) as ChromeCookieImportResult
  autoImportAttempted = true
  return result
}

/**
 * 自动导入（打开网页页签时触发）：本进程只跑一次，异常向上抛给调用方提示。
 * 不在此处 catch —— 调用方需要区分「已跑过」与「失败」并给出不同提示。
 */
export async function autoImportChromeCookies(): Promise<ChromeCookieImportResult | null> {
  if (autoImportAttempted) return null
  return importChromeCookies()
}

/** 清除 HTTP 缓存 / Cache Storage / Service Worker，保留 Cookie 与站点数据。 */
export async function clearBrowserCache(): Promise<void> {
  await invoke('clear_browser_cache')
}

/** 清除全部浏览器数据（Cookie + 站点数据 + 缓存），不可撤销。 */
export async function clearBrowserData(): Promise<ClearBrowserDataResult> {
  return (await invoke('clear_browser_data')) as ClearBrowserDataResult
}
