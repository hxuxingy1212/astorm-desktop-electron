/**
 * 原子文件写入：同目录临时文件 + rename。
 *
 * 配置文件（settings / providers / MCP / OAuth / KB / plugins / connection）
 * 直接 writeFile 在进程崩溃或断电时会留下截断 JSON；这里统一写临时文件，
 * fsync 后 rename 覆盖目标。rename 在同一文件系统内原子（Windows 也替换已有文件）。
 */

import { mkdir, open, rename, unlink, type FileHandle } from 'node:fs/promises'
import {
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  renameSync,
  unlinkSync,
  writeSync
} from 'node:fs'
import { basename, dirname, join } from 'node:path'

let seq = 0

function tempPath(file: string): string {
  return join(
    dirname(file),
    `.${basename(file)}.tmp-${process.pid}-${Date.now()}-${seq++}`
  )
}

function retryable(code: string | undefined): boolean {
  return code === 'EPERM' || code === 'EBUSY' || code === 'EACCES'
}

/** 异步原子写。`data` 为字符串时按 `encoding` 编码。 */
export async function writeFileAtomic(
  file: string,
  data: string | Uint8Array,
  encoding: BufferEncoding = 'utf8',
  mode?: number
): Promise<void> {
  await mkdir(dirname(file), { recursive: true })
  const tmp = tempPath(file)
  let handle: FileHandle | undefined
  try {
    handle = await open(tmp, 'w', mode)
    if (typeof data === 'string') {
      await handle.writeFile(data, { encoding })
    } else {
      await handle.writeFile(data)
    }
    await handle.sync()
    await handle.close()
    handle = undefined
    for (let i = 0; ; i++) {
      try {
        await rename(tmp, file)
        return
      } catch (err) {
        if (i >= 3 || !retryable((err as NodeJS.ErrnoException).code)) throw err
        await new Promise((resolve) => setTimeout(resolve, 30))
      }
    }
  } catch (err) {
    if (handle) await handle.close().catch(() => {})
    await unlink(tmp).catch(() => {})
    throw err
  }
}

/** 同步原子写（主进程里少数同步持久化路径，如 connection.json）。 */
export function writeFileAtomicSync(
  file: string,
  data: string | Uint8Array,
  encoding: BufferEncoding = 'utf8',
  mode?: number
): void {
  mkdirSync(dirname(file), { recursive: true })
  const tmp = tempPath(file)
  let fd: number | undefined
  try {
    fd = openSync(tmp, 'w', mode)
    if (typeof data === 'string') {
      writeSync(fd, data, 0, encoding)
    } else {
      writeSync(fd, data)
    }
    fsyncSync(fd)
    closeSync(fd)
    fd = undefined
    for (let i = 0; ; i++) {
      try {
        renameSync(tmp, file)
        return
      } catch (err) {
        if (i >= 3 || !retryable((err as NodeJS.ErrnoException).code)) throw err
        const end = Date.now() + 30
        while (Date.now() < end) {
          /* 短暂让出：同步路径不能 await */
        }
      }
    }
  } catch (err) {
    if (fd !== undefined) {
      try {
        closeSync(fd)
      } catch {
        /* already closed */
      }
    }
    try {
      unlinkSync(tmp)
    } catch {
      /* no temp */
    }
    throw err
  }
}
