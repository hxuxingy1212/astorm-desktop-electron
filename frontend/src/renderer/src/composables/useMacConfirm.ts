/**
 * 命令式 macOS 风格弹窗（`MacConfirm.vue` 的宿主）。
 *
 * 调用手感对齐原 Arco `Modal.confirm` / `Modal.warning`，但外壳换成项目已定型的 macOS 浮层：
 *
 * ```ts
 * const ok = await macConfirm({ title, content, tone: 'danger', confirmText: t('common.delete') })
 * if (!ok) return
 * await doWork()
 *
 * await macAlert({ title, content, tone: 'warning' })   // 单按钮「知道了」
 * ```
 *
 * **并发策略 = 排队（FIFO）**：同一时刻只显示一个弹窗，后来的调用排队等待前一个决议后
 * 再显示。理由：两个确认卡叠加会互相抢焦点、遮罩互相覆盖，且用户无法分辨该操作哪一条；
 * 排队既不丢调用也不互相干扰（不采用「替换」，替换会让先调用的 Promise 永远悬空）。
 * 注意：不要在 `onConfirm` 里再调用本 API 并 await——那会排在自己后面，形成死锁。
 *
 * 异步确认仍走 Arco 语义：`onConfirm` 返回 Promise 时确认按钮转 loading，
 * resolve 后关闭；resolve 为 `false` 或 reject 时**保持打开**（错误提示由调用方 Message 呈现）。
 * 无论走哪条路径（确认 / 取消 / 异常 / 重复决议），实例都会 unmount 且宿主节点被移除。
 */
import { createApp, h, ref, type App } from 'vue'
import MacConfirm from '@/components/common/MacConfirm.vue'
import { i18n } from '@/i18n'

export type MacConfirmTone = 'danger' | 'warning' | 'primary'

export interface MacConfirmOptions {
  title?: string
  content?: string
  tone?: MacConfirmTone
  confirmText?: string
  cancelText?: string
  /**
   * 异步确认（对齐 Arco `onOk`）：
   * - 返回/解析 `false` → 不关闭（调用方自行提示原因）
   * - 抛错/reject → 不关闭（错误提示由调用方 Message 负责）
   * - 其余 → 关闭
   */
  onConfirm?: () => void | boolean | Promise<void | boolean>
}

export interface MacAlertOptions {
  title?: string
  content?: string
  tone?: 'warning' | 'primary'
  /** 单按钮文案，默认 `common.gotIt`（「知道了」） */
  confirmText?: string
}

/* ---------- 串行队列：一个时刻只显示一个弹窗 ---------- */

let running = false
const waiting: Array<() => void> = []

function serialize<T>(task: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const start = (): void => {
      running = true
      task()
        .then(resolve, reject)
        .finally(() => {
          running = false
          waiting.shift()?.()
        })
    }
    if (running) waiting.push(start)
    else start()
  })
}

/* ---------- 单次弹窗：独立 app 实例，决议即销毁 ---------- */

function show(opts: MacConfirmOptions, alert: boolean): Promise<boolean> {
  return serialize(
    () =>
      new Promise<boolean>((resolve) => {
        const host = document.createElement('div')
        document.body.appendChild(host)

        const loading = ref(false)
        let settled = false

        const done = (value: boolean): void => {
          if (settled) return
          settled = true
          // 先卸载再移除容器：卸载期间不会有残留的响应式更新
          app.unmount()
          host.remove()
          resolve(value)
        }

        const { onConfirm, ...rest } = opts
        const app: App = createApp({
          render: () =>
            h(MacConfirm, {
              ...rest,
              hideCancel: alert,
              loading: loading.value,
              onConfirm: () => {
                if (!onConfirm) {
                  done(true)
                  return
                }
                let result: void | boolean | Promise<void | boolean>
                try {
                  result = onConfirm()
                } catch (err) {
                  // 同步抛错 = 失败：保持打开（与 Arco 的 onOk reject 行为一致）
                  console.error('[macConfirm] onConfirm threw:', err)
                  return
                }
                if (!(result instanceof Promise)) {
                  if (result !== false) done(true)
                  return
                }
                loading.value = true
                result
                  .then((r) => {
                    loading.value = false
                    if (r !== false) done(true)
                  })
                  .catch((err: unknown) => {
                    loading.value = false
                    console.error('[macConfirm] onConfirm rejected:', err)
                  })
              },
              onCancel: () => done(false)
            })
        })
        // 命令式挂载的独立 app：手动装上 i18n 单例（组件内 useI18n / 默认文案依赖它）
        app.use(i18n)
        app.mount(host)
      })
  )
}

/** 确认弹窗：确认 → `true`，取消 / Esc / 点遮罩 → `false` */
export function macConfirm(opts: MacConfirmOptions): Promise<boolean> {
  return show(opts, false)
}

/** 提示弹窗（单按钮「知道了」，标题/按钮的默认文案由 MacConfirm 走 i18n）：关闭后 resolve */
export function macAlert(opts: MacAlertOptions): Promise<void> {
  return show(opts, true).then(() => undefined)
}
