/**
 * 选目录的统一入口：桌面端走原生目录对话框，浏览器部署走应用内模态
 * （`DirectoryPickerModal.vue`，服务端目录浏览——浏览器没有原生目录对话框）。
 *
 * ```ts
 * const dir = await pickDirectory()          // 取消/失败 → ''
 * if (!dir) return
 * ```
 */
import { createApp, h, type App } from 'vue'
import DirectoryPickerModal from '@/components/common/DirectoryPickerModal.vue'
import { i18n } from '@/i18n'
import { isWebShell } from '@/utils/platform'

export interface DirectoryPickerOptions {
  /** 模态标题（缺省「选择目录」） */
  title?: string
  /** 起始目录（缺省 = 服务端工作空间根） */
  startPath?: string
}

/** 同一时刻只允许一个选择器（重复调用直接返回空，避免遮罩叠加） */
let active = false

export async function pickDirectory(opts: DirectoryPickerOptions = {}): Promise<string> {
  if (!isWebShell) {
    return (await window.api.dialog.openDirectory()) || ''
  }
  if (active) return ''
  active = true
  return new Promise<string>((resolve) => {
    const host = document.createElement('div')
    document.body.appendChild(host)
    let settled = false
    const done = (value: string): void => {
      if (settled) return
      settled = true
      app.unmount()
      host.remove()
      active = false
      resolve(value)
    }
    const app: App = createApp({
      render: () =>
        h(DirectoryPickerModal, {
          title: opts.title,
          startPath: opts.startPath,
          onConfirm: (path: string) => done(path),
          onCancel: () => done('')
        })
    })
    app.use(i18n)
    app.mount(host)
  })
}
