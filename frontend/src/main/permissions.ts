import { ipcMain, shell, systemPreferences } from 'electron'

/** 跨平台系统权限项标识 */
export type PermissionKind = 'microphone' | 'camera' | 'screen' | 'accessibility' | 'fullDisk'

/** 权限状态:macOS 由系统查询;其他平台无统一接口 → unsupported(需在系统设置中管理) */
export type PermissionState =
  | 'granted'
  | 'denied'
  | 'not-determined'
  | 'restricted'
  | 'unknown'
  | 'unsupported'

export interface PermissionItem {
  kind: PermissionKind
  state: PermissionState
  /** 是否支持主动弹窗请求(仅 macOS 麦克风/摄像头) */
  canRequest: boolean
  /** 是否能跳转到系统设置对应页 */
  canOpenSettings: boolean
}

const KINDS: PermissionKind[] = ['microphone', 'camera', 'screen', 'accessibility', 'fullDisk']

function mediaKeyOf(kind: PermissionKind): 'microphone' | 'camera' | 'screen' {
  if (kind === 'microphone' || kind === 'camera') return kind
  return 'screen'
}

function currentState(kind: PermissionKind): PermissionState {
  if (process.platform !== 'darwin') return 'unsupported'
  if (kind === 'accessibility') {
    return systemPreferences.isTrustedAccessibilityClient(false) ? 'granted' : 'denied'
  }
  // 完全磁盘访问无公开查询 API:只能引导用户到系统设置确认
  if (kind === 'fullDisk') return 'unknown'
  return systemPreferences.getMediaAccessStatus(mediaKeyOf(kind)) as PermissionState
}

function canRequest(kind: PermissionKind): boolean {
  return process.platform === 'darwin' && (kind === 'microphone' || kind === 'camera')
}

function settingsLink(kind: PermissionKind): string | null {
  if (process.platform === 'darwin') {
    const map: Record<PermissionKind, string> = {
      microphone: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone',
      camera: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Camera',
      screen: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
      accessibility: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility',
      fullDisk: 'x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles'
    }
    return map[kind]
  }
  if (process.platform === 'win32') {
    const map: Record<PermissionKind, string> = {
      microphone: 'ms-settings:privacy-microphone',
      camera: 'ms-settings:privacy-webcam',
      screen: 'ms-settings:privacy',
      accessibility: 'ms-settings:easeofaccess',
      fullDisk: 'ms-settings:privacy-broadfilesystemaccess'
    }
    return map[kind]
  }
  return null
}

function listStatuses(): PermissionItem[] {
  return KINDS.map((kind) => ({
    kind,
    state: currentState(kind),
    canRequest: canRequest(kind),
    canOpenSettings: settingsLink(kind) !== null
  }))
}

/** 注册系统权限 IPC:查询状态 / 请求授权 / 打开系统设置对应页 */
export function registerPermissionIpc(): void {
  ipcMain.handle('permission:status', () => listStatuses())

  ipcMain.handle('permission:request', async (_event, kind: PermissionKind) => {
    if (canRequest(kind)) {
      try {
        await systemPreferences.askForMediaAccess(kind as 'microphone' | 'camera')
      } catch {
        /* 系统拒绝弹窗或非 macOS:忽略,回读状态即可 */
      }
    }
    return listStatuses()
  })

  ipcMain.handle('permission:open-settings', async (_event, kind: PermissionKind) => {
    const link = settingsLink(kind)
    if (!link) return false
    await shell.openExternal(link)
    return true
  })
}
