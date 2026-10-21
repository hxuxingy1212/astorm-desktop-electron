import { invoke } from '@/services/backend'

/**
 * 会话工作区（会话记录里的值）→ 绝对目录。
 *
 * 与后端**同口径**（`AppDir.workspace_dir = <用户目录>/workspace`，
 * `tool::resolve_workspace` 把相对工作区接在该目录下）：
 * - 绝对路径：原样返回（用户选目录的项目会话）
 * - 相对路径（默认时间戳工作区，如 `2026_09_11_14_50_53`）：
 *   `<用户目录>/workspace/<相对路径>`
 *
 * 说明：`get_settings().workspace` 是**用户目录**（后端再拼 `workspace` 子目录），
 * 早前前端把它当基目录直接接相对路径，漏掉 `workspace` 这一层，
 * 得到不存在的路径（文件树 / 打开目录报 ENOENT）。此处收敛为唯一实现，避免再次走偏。
 */
export async function resolveSessionWorkspaceDir(
  workspace?: string | null
): Promise<string> {
  const raw = (workspace ?? '').trim()
  if (!raw) return ''
  // Windows 盘符开头的绝对路径 / POSIX 根路径
  if (/^[a-zA-Z]:[\\/]/.test(raw) || /^\//.test(raw)) return raw
  try {
    const settings = (await invoke('get_settings')) as { workspace?: string }
    const base = (settings.workspace || '').trim().replace(/[\\/]+$/, '')
    if (!base) return ''
    return `${base}/workspace/${raw.replace(/^[\\/]+/, '')}`
  } catch {
    return ''
  }
}
