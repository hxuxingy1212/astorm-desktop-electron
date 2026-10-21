/**
 * 浏览器部署下的「另存为」：Blob 下载。
 *
 * 桌面端走原生保存对话框（`dialog.saveFile` + `fs.writeTextFile`），浏览器没有该能力，
 * 统一在这里降级为 `a[download]` 下载（文件名即 download 属性）。
 */

function triggerDownload(name: string, blob: Blob): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.style.display = 'none'
  document.body.appendChild(a)
  a.click()
  a.remove()
  // 交给浏览器开始下载后再回收，避免部分浏览器取消下载
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

export function downloadText(name: string, text: string, mime = 'text/plain;charset=utf-8'): void {
  triggerDownload(name, new Blob([text], { type: mime }))
}

export function downloadBytes(name: string, bytes: ArrayBuffer | Uint8Array, mime = 'application/octet-stream'): void {
  const buf = bytes instanceof Uint8Array ? bytes.slice().buffer : bytes
  triggerDownload(name, new Blob([buf], { type: mime }))
}
