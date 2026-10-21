import { ipcMain, powerSaveBlocker } from 'electron'

/**
 * 当前生效的电源抑制器 id(null = 未抑制)。
 *
 * 用 `prevent-app-suspension`(macOS 对应 PreventUserIdleSystemSleep):
 * 只阻止**因空闲导致的系统休眠**,屏幕仍可息屏;
 * 手动睡眠 / 合盖休眠属于强制睡眠,不受此抑制影响。
 */
let blockerId: number | null = null

/** 应用「保持电脑运行」偏好;返回应用后是否处于抑制中 */
function setKeepAwake(enabled: boolean): boolean {
  if (enabled) {
    if (blockerId === null || !powerSaveBlocker.isStarted(blockerId)) {
      blockerId = powerSaveBlocker.start('prevent-app-suspension')
    }
    return powerSaveBlocker.isStarted(blockerId)
  }
  if (blockerId !== null) {
    if (powerSaveBlocker.isStarted(blockerId)) powerSaveBlocker.stop(blockerId)
    blockerId = null
  }
  return false
}

/** 注册电源抑制 IPC:渲染层切换「保持电脑运行」时调用 */
export function registerPowerIpc(): void {
  ipcMain.handle('power:set-keep-awake', (_event, enabled: unknown) => setKeepAwake(Boolean(enabled)))
}
