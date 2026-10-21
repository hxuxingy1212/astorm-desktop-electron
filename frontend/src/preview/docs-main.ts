import '@/styles/themes/base.css'
import '@/styles/themes/color-scheme.css'
import '@/theme/tokens.css'
import './site.css'
import './docs.css'
import logoUrl from '@/assets/logo.png'

document.querySelectorAll<HTMLImageElement>('img[data-logo]').forEach((img) => {
  img.src = logoUrl
})

type Theme = 'light' | 'dark'
const SITE_THEME_KEY = 'astorm:preview:site-theme'
const root = document.documentElement

function storedTheme(): Theme {
  try {
    return localStorage.getItem(SITE_THEME_KEY) === 'dark' ? 'dark' : 'light'
  } catch {
    return 'light'
  }
}

let theme: Theme = storedTheme()
function applyTheme(next: Theme, persist = true): void {
  theme = next
  root.dataset.siteTheme = next
  root.style.colorScheme = next
  if (persist) {
    try {
      localStorage.setItem(SITE_THEME_KEY, next)
    } catch {
      /* 隐私模式忽略 */
    }
  }
}

applyTheme(theme, false)

document.getElementById('site-theme')?.addEventListener('click', () => {
  applyTheme(theme === 'dark' ? 'light' : 'dark')
})

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

const nav = document.querySelector('.site-nav')
function onScroll(): void {
  nav?.classList.toggle('is-scrolled', window.scrollY > 12)
}
window.addEventListener('scroll', onScroll, { passive: true })
onScroll()
