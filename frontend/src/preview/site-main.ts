import '@/styles/themes/base.css'
import '@/styles/themes/color-scheme.css'
import '@/theme/tokens.css'
import './site.css'
import logoUrl from '@/assets/logo.png'

document.querySelectorAll<HTMLImageElement>('img[data-logo]').forEach((img) => {
  img.src = logoUrl
})

/* ------------------------------------------------------------------ *
 * 明暗主题：站点与 iframe 内应用分开控制，默认同步
 * ------------------------------------------------------------------ */
type Theme = 'light' | 'dark'
const SITE_THEME_KEY = 'astorm:preview:site-theme'
const root = document.documentElement
const frame = document.getElementById('preview-frame') as HTMLIFrameElement | null

function storedTheme(): Theme {
  try {
    return localStorage.getItem(SITE_THEME_KEY) === 'dark' ? 'dark' : 'light'
  } catch {
    return 'light'
  }
}

let siteTheme: Theme = storedTheme()
let appTheme: Theme = siteTheme

function applySiteTheme(theme: Theme, persist = true): void {
  siteTheme = theme
  root.dataset.siteTheme = theme
  root.style.colorScheme = theme
  if (persist) {
    try {
      localStorage.setItem(SITE_THEME_KEY, theme)
    } catch {
      /* 隐私模式忽略 */
    }
  }
}

function sendAppTheme(theme: Theme): void {
  appTheme = theme
  try {
    frame?.contentWindow?.postMessage(
      { type: 'astorm-preview-theme', theme },
      window.location.origin
    )
  } catch {
    /* iframe 尚未就绪 */
  }
}

applySiteTheme(siteTheme, false)

document.getElementById('site-theme')?.addEventListener('click', () => {
  const next: Theme = siteTheme === 'dark' ? 'light' : 'dark'
  applySiteTheme(next)
  sendAppTheme(next)
})

document.getElementById('app-theme')?.addEventListener('click', () => {
  sendAppTheme(appTheme === 'dark' ? 'light' : 'dark')
})

/* ------------------------------------------------------------------ *
 * iframe 就绪 / 重置
 * ------------------------------------------------------------------ */
const loading = document.getElementById('frame-loading')

function markFrameReady(): void {
  loading?.classList.add('is-hidden')
}

window.addEventListener('message', (event) => {
  if (event.origin !== window.location.origin) return
  const data = event.data as { type?: string } | null
  if (data?.type === 'astorm-preview-ready') {
    markFrameReady()
    sendAppTheme(appTheme)
  }
})

frame?.addEventListener('load', () => {
  window.setTimeout(() => {
    markFrameReady()
    sendAppTheme(appTheme)
  }, 260)
})

document.getElementById('app-reset')?.addEventListener('click', () => {
  loading?.classList.remove('is-hidden')
  try {
    frame?.contentWindow?.postMessage({ type: 'astorm-preview-reset' }, window.location.origin)
  } catch {
    if (frame) frame.src = frame.src
  }
})

/* ------------------------------------------------------------------ *
 * GitHub 地址与下载入口
 * ------------------------------------------------------------------ */
function repoUrl(): string {
  const match = /^([^.]+)\.github\.io$/i.exec(window.location.hostname)
  if (match) {
    const repo = window.location.pathname.split('/').filter(Boolean)[0]
    if (repo) return `https://github.com/${match[1]}/${repo}`
  }
  return 'https://github.com/hxuxingy1212/astorm-desktop-electron'
}

const repo = repoUrl()
const github = document.getElementById('github-link') as HTMLAnchorElement | null
if (github) github.href = repo

const address = document.getElementById('github-address')
if (address) address.textContent = repo.replace(/^https?:\/\//, '').replace(/\/$/, '')

const download = document.getElementById('download-link') as HTMLAnchorElement | null
if (download) {
  download.href = /github\.com/i.test(repo) ? `${repo}/releases/latest` : `${repo}/releases`
}

/* 导航滚动态：只有一条细边，不改变首屏布局 */
const nav = document.querySelector('.site-nav')
function onScroll(): void {
  nav?.classList.toggle('is-scrolled', window.scrollY > 12)
}
window.addEventListener('scroll', onScroll, { passive: true })
onScroll()

const full = document.getElementById('app-fullscreen')
if (full) full.setAttribute('href', './app/index.html#/chat/preview-auth')
