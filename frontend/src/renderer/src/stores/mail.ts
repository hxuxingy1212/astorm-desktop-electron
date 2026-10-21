/**
 * 邮箱 store：账号 / 文件夹 / 邮件列表 / 正文 / 标记整理。
 *
 * 数据来自 Rust 后端（astorm-mail）经 `backend:invoke` 的 mail_* 命令；
 * 列表与正文优先读本地缓存、后台再同步 IMAP，因此打开很快。
 */
import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'
import { invoke } from '@/services/backend'

export interface MailAccount {
  name: string
  displayName: string | null
  username: string
  imapHost: string
  imapPort: number
  imapTls: string
  smtpHost: string
  smtpPort: number
  smtpTls: string
}

export interface MailState {
  active: string | null
  presets: string[]
  accounts: MailAccount[]
}

export interface MailFolderCount {
  folder: string
  total: number
  unread: number
}

export interface MailMessage {
  uid: number
  folder: string
  seen: boolean
  flagged: boolean
  pinned: boolean
  category: string | null
  from: string
  to: string
  subject: string
  date: string
  size: number
  ts: number
  preview: string | null
}

export interface MailList {
  folder: string
  total: number
  unread: number
  hasMore: boolean
  syncedAt: number
  stale: boolean
  messages: MailMessage[]
}

export interface MailAttachmentMeta {
  filename: string
  size: number
}

export interface MailDetail {
  uid: number
  seen: boolean
  from: string
  to: string
  cc: string
  subject: string
  date: string
  text: string | null
  html: string | null
  attachments: MailAttachmentMeta[]
  /** 正文是否因超过缓存上限（2 MiB）被截断 */
  truncated?: boolean
}

export interface MailAttachmentBlob {
  filename: string
  contentType: string
  data: Uint8Array
}

export interface MailAddAccountInput {
  preset?: string
  name?: string
  username?: string
  password?: string
  displayName?: string
  imapHost?: string
  imapPort?: number
  imapTls?: string
  smtpHost?: string
  smtpPort?: number
  smtpTls?: string
}

export interface MailUpdateAccountInput {
  displayName?: string
  username?: string
  password?: string
  imapHost?: string
  imapPort?: number
  imapTls?: string
  smtpHost?: string
  smtpPort?: number
  smtpTls?: string
}

export interface MailSendInput {
  account?: string
  to: string[]
  cc: string[]
  bcc: string[]
  subject: string
  body: string
  /** 正文 HTML 版本（富文本产出）；与 body 同时给出时后端按 multipart/alternative 发送 */
  html?: string
  attachments: string[]
}

const PAGE_SIZE = 30

/** 标准文件夹类别（与后端 astorm_mail::service::folder_kind 同口径）。 */
export function folderKind(name: string): string {
  const f = name.toLowerCase()
  if (f === 'inbox' || f.includes('收件箱')) return 'inbox'
  if (f.includes('draft') || f.includes('草稿')) return 'draft'
  if (f.includes('sent') || f.includes('已发送')) return 'sent'
  if (f.includes('junk') || f.includes('spam') || f.includes('垃圾')) return 'junk'
  if (f.includes('trash') || f.includes('deleted') || f.includes('废纸篓')) return 'trash'
  return 'other'
}

export const useMailStore = defineStore('mail', () => {
  const state = ref<MailState>({ active: null, presets: [], accounts: [] })
  const folders = ref<string[]>([])
  const counts = ref<MailFolderCount[]>([])
  const curAccount = ref<string | null>(null)
  const curFolder = ref<string>('INBOX')
  const curCategory = ref<string | null>(null)
  const unreadOnly = ref(false)
  const messages = ref<MailMessage[]>([])
  const total = ref(0)
  const unread = ref(0)
  const hasMore = ref(false)
  const offset = ref(0)
  const loadingList = ref(false)
  /** 远端同步进行中（本地缓存可能偏旧/为空），用于「缓存优先 + 后台同步」的加载态 */
  const syncing = ref(false)
  const curUid = ref<number | null>(null)
  const curDetail = ref<MailDetail | null>(null)
  const loadingDetail = ref(false)
  const previews = reactive<Record<number, string>>({})

  const previewFetched = new Set<number>()

  /**
   * 会话内正文 LRU（`account|folder|uid` → 详情）：再次点开同一封**即时显示、0 IPC**。
   * 持久层（SQLite，30 天 TTL）由后端管理；这里只减少本会话的往返。
   * 键含 folder：同一 uid 在不同文件夹是不同邮件。
   */
  const DETAIL_CACHE_MAX = 30
  const detailCache = new Map<string, MailDetail>()
  /** 进行中的正文请求（同键只发一次；快速连点不重复 IPC）。 */
  const detailInflight = new Map<string, Promise<MailDetail>>()

  function detailKey(uid: number): string {
    return `${curAccount.value ?? ''}|${curFolder.value ?? ''}|${uid}`
  }

  function cacheDetail(d: MailDetail): void {
    const k = detailKey(d.uid)
    detailCache.delete(k)
    detailCache.set(k, d)
    while (detailCache.size > DETAIL_CACHE_MAX) {
      const oldest = detailCache.keys().next().value
      if (oldest === undefined) break
      detailCache.delete(oldest)
    }
  }

  function clearDetailCache(): void {
    detailCache.clear()
    detailInflight.clear()
  }

  function countOf(folder: string): MailFolderCount | undefined {
    return counts.value.find((c) => c.folder === folder)
  }

  async function loadState(): Promise<void> {
    state.value = await invoke('mail_state')
    if (!curAccount.value && state.value.active) curAccount.value = state.value.active
  }

  async function refreshSidebar(): Promise<void> {
    // 顺序执行：folder_counts 冷启动会做一次 LIST 并落库，随后 folders 直接命中缓存，
    // 避免并发触发两次 LIST（池化会话按账号串行，重复 LIST 会明显拖慢首屏）。
    const c = await invoke('mail_folder_counts', { account: curAccount.value ?? undefined })
    counts.value = c ?? []
    const f = await invoke('mail_folders', { account: curAccount.value ?? undefined })
    folders.value = f ?? []
  }

  async function selectAccount(name: string): Promise<void> {
    curAccount.value = name
    curFolder.value = 'INBOX'
    curCategory.value = null
    curUid.value = null
    curDetail.value = null
    messages.value = []
    previewsClear()
    clearDetailCache()
    resetStaleWatch()
    // 先出邮件列表（缓存优先），侧栏文件夹/计数随后台刷新，避免首屏被 LIST/STATUS 阻塞
    await loadMessages(true)
    void refreshSidebar()
  }

  /** 切换活动账号（仅更新账号状态，不切换邮箱界面的当前视图）。 */
  async function activateAccount(name: string): Promise<void> {
    state.value = await invoke('mail_activate_account', { name })
  }

  /** 懒加载正文摘要：只对进入可视区域的列表行按需拉取（IntersectionObserver 驱动）。 */
  const previewPending = new Map<number, string>()
  let previewTimer: ReturnType<typeof setTimeout> | null = null

  function previewsClear(): void {
    for (const k of Object.keys(previews)) delete previews[Number(k)]
    previewFetched.clear()
    previewPending.clear()
  }

  function scheduleFlush(): void {
    if (previewTimer !== null) return
    previewTimer = setTimeout(() => {
      previewTimer = null
      void flushPreviews()
    }, 60)
  }

  /** 请求某行的正文摘要（uid + 所在文件夹）；重复请求与已缓存会自动跳过。 */
  function requestPreview(uid: number, folder: string): void {
    if (!uid || previewFetched.has(uid) || previewPending.has(uid)) return
    previewPending.set(uid, folder)
    scheduleFlush()
  }

  async function flushPreviews(): Promise<void> {
    const batch = [...previewPending.entries()].slice(0, 30)
    if (batch.length === 0) return
    batch.forEach(([uid]) => {
      previewPending.delete(uid)
      previewFetched.add(uid)
    })
    try {
      // 按文件夹分组（分类视图跨文件夹）
      const byFolder = new Map<string, number[]>()
      for (const [uid, folder] of batch) {
        const list = byFolder.get(folder) ?? []
        list.push(uid)
        byFolder.set(folder, list)
      }
      for (const [folder, uids] of byFolder) {
        const list: { uid: number; preview: string }[] = await invoke('mail_previews', {
          account: curAccount.value ?? undefined,
          folder,
          uids
        })
        for (const item of list ?? []) {
          previews[item.uid] = item.preview
          // 同步写回列表行（列表渲染 m.preview）；否则懒加载的摘要在本会话不显示。
          const m = messages.value.find((x) => x.uid === item.uid && x.folder === folder)
          if (m && !m.preview) m.preview = item.preview
        }
      }
    } catch {
      batch.forEach(([uid]) => previewFetched.delete(uid))
    }
    if (previewPending.size) scheduleFlush()
  }

  /**
   * 后台同步进行中（本地数据偏旧）：稍后自动重取一次本地缓存，实现
   * 「先显示缓存 → 后台同步远程 → 差异补齐后刷新」。有次数上限，避免网络异常时无休止轮询。
   */
  let staleTimer: ReturnType<typeof setTimeout> | null = null
  let staleTries = 0

  function resetStaleWatch(): void {
    if (staleTimer) {
      clearTimeout(staleTimer)
      staleTimer = null
    }
    staleTries = 0
  }

  function scheduleStaleRefresh(): void {
    if (staleTimer || staleTries >= 8) return
    staleTries += 1
    const key = `${curAccount.value ?? ''}|${curCategory.value ?? ''}|${curFolder.value}`
    staleTimer = setTimeout(async () => {
      staleTimer = null
      const now = `${curAccount.value ?? ''}|${curCategory.value ?? ''}|${curFolder.value}`
      if (now !== key) return
      if (offset.value > PAGE_SIZE) return
      await loadMessages(true)
    }, 700)
  }

  async function loadMessages(reset = false, force = false): Promise<void> {
    if (loadingList.value) return
    loadingList.value = true
    try {
      if (reset) {
        offset.value = 0
        messages.value = []
        previewsClear()
      }
      const view: MailList = await invoke('mail_messages', {
        account: curAccount.value ?? undefined,
        folder: curCategory.value ? undefined : curFolder.value,
        category: curCategory.value ?? undefined,
        unread: unreadOnly.value,
        limit: PAGE_SIZE,
        offset: offset.value,
        refresh: force || undefined
      })
      const incoming = view.messages ?? []
      messages.value = reset ? incoming : [...messages.value, ...incoming]
      total.value = view.total
      unread.value = view.unread
      hasMore.value = view.hasMore
      offset.value = messages.value.length
      // 本地数据偏旧（后台正在同步）→ 稍后自动重取，差异补齐后即停止
      syncing.value = view.stale
      if (view.stale) scheduleStaleRefresh()
      // 正文摘要不再随列表一次性拉取：交给列表行的 IntersectionObserver 懒加载
    } finally {
      loadingList.value = false
    }
  }

  async function loadMore(): Promise<void> {
    if (!hasMore.value || loadingList.value) return
    await loadMessages(false)
  }

  async function selectFolder(folder: string): Promise<void> {
    curCategory.value = null
    curFolder.value = folder
    curUid.value = null
    curDetail.value = null
    clearDetailCache()
    resetStaleWatch()
    await loadMessages(true)
  }

  async function selectCategory(category: string): Promise<void> {
    curCategory.value = category
    curUid.value = null
    curDetail.value = null
    clearDetailCache()
    resetStaleWatch()
    await loadMessages(true)
  }

  /** 手动刷新：强制触发远端同步（后台）并重取缓存。 */
  async function refreshMessages(): Promise<void> {
    resetStaleWatch()
    await loadMessages(true, true)
  }

  async function selectMessage(uid: number): Promise<void> {
    curUid.value = uid
    // 会话内缓存命中 → 即时显示、0 IPC。
    const cached = detailCache.get(detailKey(uid))
    if (cached) {
      curDetail.value = cached
      loadingDetail.value = false
      const m = messages.value.find((x) => x.uid === uid)
      if (m) m.seen = true
      return
    }
    // 清空上一封内容 → 等待期间展示「加载中」占位（而不是旧内容或空状态）
    curDetail.value = null
    loadingDetail.value = true
    const key = detailKey(uid)
    try {
      // 并发去重：同键进行中只发一次请求。
      let p = detailInflight.get(key)
      if (!p) {
        p = invoke('mail_message', {
          uid,
          folder: curFolder.value,
          account: curAccount.value ?? undefined
        }) as Promise<MailDetail>
        detailInflight.set(key, p)
      }
      const d = await p
      cacheDetail(d)
      // 只有仍是当前选中才写回，避免快速切换时旧结果覆盖新选中。
      if (curUid.value === uid) curDetail.value = d
      const m = messages.value.find((x) => x.uid === uid)
      if (m) m.seen = true
    } finally {
      detailInflight.delete(key)
      if (curUid.value === uid) loadingDetail.value = false
    }
  }

  function patchMessage(uid: number, patch: Partial<MailMessage>): void {
    const m = messages.value.find((x) => x.uid === uid)
    if (m) Object.assign(m, patch)
    if (patch.seen !== undefined) {
      if (curDetail.value?.uid === uid) curDetail.value.seen = patch.seen
      // 同步详情缓存:否则重开时缓存命中会读到旧的已读状态
      const cached = detailCache.get(detailKey(uid))
      if (cached) cached.seen = patch.seen
    }
  }

  async function setMeta(
    uid: number,
    body: { flagged?: boolean; pinned?: boolean; category?: string; seen?: boolean }
  ): Promise<void> {
    await invoke('mail_set_meta', {
      uid,
      folder: curFolder.value,
      account: curAccount.value ?? undefined,
      body
    })
    patchMessage(uid, body as Partial<MailMessage>)
    // 只有会改变列表**成员/顺序**的字段才需要重取：pinned（置顶排序）、category
    // （分类视图过滤）。flagged 只影响徽标，本地已 patch，无需再走一次后端。
    if (body.pinned !== undefined || body.category !== undefined) {
      await loadMessages(true)
    } else if (body.seen !== undefined) {
      // 已读/未读:更新文件夹未读数;只看未读视图下列表成员会变,需重取
      void refreshSidebar()
      if (unreadOnly.value) await loadMessages(true)
    }
  }

  async function removeMessage(uid: number, dest: string): Promise<void> {
    await invoke('mail_move', {
      uid,
      folder: curFolder.value,
      account: curAccount.value ?? undefined,
      dest
    })
    messages.value = messages.value.filter((m) => m.uid !== uid)
    if (curUid.value === uid) {
      curUid.value = null
      curDetail.value = null
    }
    void refreshSidebar()
  }

  /** 按类别（trash / junk）解析目标文件夹并移动。 */
  async function moveToKind(uid: number, kind: 'trash' | 'junk'): Promise<void> {
    const dest = folders.value.find((f) => folderKind(f) === kind)
    if (!dest) throw new Error(kind === 'trash' ? '未找到废纸篓文件夹' : '未找到垃圾邮件文件夹')
    await removeMessage(uid, dest)
  }

  async function send(input: MailSendInput): Promise<string> {
    const sentAs: string = await invoke('mail_send', {
      ...input,
      account: input.account ?? curAccount.value ?? undefined
    })
    return sentAs
  }

  async function addAccount(input: MailAddAccountInput): Promise<void> {
    state.value = await invoke('mail_add_account', input as unknown as Record<string, unknown>)
  }

  async function updateAccount(name: string, input: MailUpdateAccountInput): Promise<void> {
    state.value = await invoke('mail_update_account', { name, input })
  }

  async function removeAccount(name: string): Promise<void> {
    state.value = await invoke('mail_remove_account', { name })
    if (curAccount.value === name) {
      curAccount.value = state.value.active ?? state.value.accounts[0]?.name ?? null
      if (!curAccount.value) {
        messages.value = []
        curDetail.value = null
        folders.value = []
        counts.value = []
      }
    }
  }

  async function downloadAttachment(uid: number, index: number): Promise<void> {
    const blob: MailAttachmentBlob = await invoke('mail_attachment', {
      uid,
      index,
      folder: curFolder.value,
      account: curAccount.value ?? undefined
    })
    const bytes = blob.data instanceof Uint8Array ? blob.data : new Uint8Array(blob.data as ArrayLike<number>)
    const url = URL.createObjectURL(
      new Blob([bytes as unknown as BlobPart], { type: blob.contentType || 'application/octet-stream' })
    )
    const a = document.createElement('a')
    a.href = url
    a.download = blob.filename || `attachment-${index}`
    document.body.appendChild(a)
    a.click()
    a.remove()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }

  /** 附件临时路径缓存（`account|folder|uid|index` → {name,path}）：重复预览免 IPC/解码。 */
  const attachTempCache = new Map<string, { name: string; path: string }>()

  /**
   * 附件 → 临时文件路径（后端解码后直接落盘，前端文件预览直连渲染）。
   * 后端复用内存中的原始报文缓存 + 落盘复用，避免每次点击都重新全量下载 / 写盘。
   * 前端再缓存一次路径：同一附件重复点开直接命中，连 IPC 都省。
   */
  async function attachmentToTemp(uid: number, index: number): Promise<{ name: string; path: string }> {
    const key = `${curAccount.value ?? ''}|${curFolder.value ?? ''}|${uid}|${index}`
    const hit = attachTempCache.get(key)
    if (hit) return hit
    const path: string = await invoke('mail_attachment_to_temp', {
      uid,
      index,
      folder: curFolder.value,
      account: curAccount.value ?? undefined
    })
    const name = path.split(/[\\/]/).pop() || `attachment-${index}`
    const entry = { name, path }
    attachTempCache.set(key, entry)
    return entry
  }

  return {
    state,
    folders,
    counts,
    curAccount,
    curFolder,
    curCategory,
    unreadOnly,
    messages,
    total,
    unread,
    hasMore,
    loadingList,
    syncing,
    curUid,
    curDetail,
    loadingDetail,
    previews,
    countOf,
    loadState,
    refreshSidebar,
    selectAccount,
    activateAccount,
    loadMessages,
    loadMore,
    refreshMessages,
    requestPreview,
    selectFolder,
    selectCategory,
    selectMessage,
    setMeta,
    removeMessage,
    moveToKind,
    send,
    addAccount,
    updateAccount,
    removeAccount,
    downloadAttachment,
    attachmentToTemp
  }
})
