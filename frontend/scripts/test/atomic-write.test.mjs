// 配置原子写 helper 的异常路径测试（node --test）。
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  writeFileAtomic,
  writeFileAtomicSync
} from '../../src/main/atomicWrite.ts'

async function tempDir() {
  return mkdtemp(join(tmpdir(), 'astorm-atomic-'))
}

async function leftovers(dir) {
  return (await readdir(dir)).filter((n) => n.includes('.tmp-'))
}

test('async: replaces existing and leaves no temp', async () => {
  const dir = await tempDir()
  const file = join(dir, 'settings.json')
  await writeFile(file, 'old')
  await writeFileAtomic(file, '{"ok":true}')
  assert.equal(await readFile(file, 'utf8'), '{"ok":true}')
  assert.deepEqual(await leftovers(dir), [])
  await rm(dir, { recursive: true, force: true })
})

test('async: creates parent directories', async () => {
  const dir = await tempDir()
  const file = join(dir, 'a', 'b', 'config.json')
  await writeFileAtomic(file, 'hello')
  assert.equal(await readFile(file, 'utf8'), 'hello')
  await rm(dir, { recursive: true, force: true })
})

test('async: failure keeps target and cleans temp', async () => {
  const dir = await tempDir()
  const target = join(dir, 'target-dir')
  await mkdir(target)
  await assert.rejects(writeFileAtomic(target, 'x'))
  assert.deepEqual(await leftovers(dir), [])
  await rm(dir, { recursive: true, force: true })
})

test('sync: replaces existing and leaves no temp', async () => {
  const dir = await tempDir()
  const file = join(dir, 'connection.json')
  await writeFile(file, 'old')
  writeFileAtomicSync(file, '{"mode":"cloud"}')
  assert.equal(await readFile(file, 'utf8'), '{"mode":"cloud"}')
  assert.deepEqual(await leftovers(dir), [])
  await rm(dir, { recursive: true, force: true })
})

test('unix: private mode applied', { skip: process.platform === 'win32' }, async () => {
  const dir = await tempDir()
  const file = join(dir, 'providers.json')
  await writeFileAtomic(file, '{}', 'utf8', 0o600)
  const mode = (await stat(file)).mode & 0o777
  assert.equal(mode, 0o600)
  await rm(dir, { recursive: true, force: true })
})

test('concurrent writes leave one complete file and no temp', async () => {
  const dir = await tempDir()
  const file = join(dir, 'providers.json')
  const values = Array.from({ length: 10 }, (_, i) => `{"n":${i}}`)
  await Promise.all(values.map((v) => writeFileAtomic(file, v)))
  const text = await readFile(file, 'utf8')
  assert.ok(values.includes(text), `unexpected content: ${text}`)
  assert.deepEqual(await leftovers(dir), [])
  await rm(dir, { recursive: true, force: true })
})
