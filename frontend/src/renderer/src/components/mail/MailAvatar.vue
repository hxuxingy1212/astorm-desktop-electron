<script setup lang="ts">
/**
 * 邮件头像：优先显示发件人「真实头像」——按顺序尝试多个来源，任一加载成功即采用：
 *   0. **内置品牌图标**（apple / github / 阿里云 / 微信 …）：按发件域名匹配，离线可用，
 *      优先于一切网络来源（见 `mailBrandIcons`，由 scripts/gen-mail-brands.mjs 生成）
 *   1. 域名 favicon（企业发件人 → 公司 Logo，如 github.com / apple.com）及二级域回退
 *   2. DuckDuckGo 图标服务（部分站点根目录无 favicon.ico）
 *   3. Gravatar（个人邮箱头像，`sha256(email)`，`d=404` 无则 404）
 * 全部失败 → 回退首字母彩色圆。
 *
 * 桌面端第 1、2 项合并为一条 `astorm-icon://mail/<domain>`：主进程按域名做 7 天磁盘缓存、
 * 超时与失败标记（见 `src/main/iconCache.ts`），避免每次挂载都重新请求第三方；Web 端没有
 * 该协议，`mailIconUrls` 仍返回原来的直连候选链。Gravatar 同理走 `astorm-icon://avatar`。
 */
import { computed, onMounted, ref } from 'vue'
import { gravatarIconUrl, mailIconUrls } from '@/utils/siteIcons'
import { BRAND_ICONS, DOMAIN_BRAND } from './mailBrandIcons'

const props = withDefaults(defineProps<{ name?: string; email?: string; size?: number }>(), {
  name: '',
  email: '',
  size: 34
})

/** 进程级缓存：邮箱哈希 / Gravatar 未命中的邮箱（避免重复计算与请求） */
const hashCache = new Map<string, string>()
const gravatarMiss = new Set<string>()

function extractEmail(input: string): string {
  if (!input) return ''
  const m = input.match(/<([^>]+)>/)
  return (m?.[1] ?? input).trim()
}

const email = computed(() => extractEmail(props.email || props.name).toLowerCase())

/** 发件域名 → 内置品牌图标（先整域匹配，再退到末两段，如 email.apple.com → apple.com）。 */
const brand = computed(() => {
  const at = email.value.indexOf('@')
  if (at < 0) return null
  const domain = email.value.slice(at + 1)
  const parts = domain.split('.')
  const candidates = [domain]
  if (parts.length > 2) candidates.push(parts.slice(-2).join('.'))
  for (const d of candidates) {
    const slug = DOMAIN_BRAND[d]
    if (slug && BRAND_ICONS[slug]) return BRAND_ICONS[slug]
  }
  return null
})

const letter = computed(() => {
  const n = (props.name || '').trim()
  if (n) return n[0].toUpperCase()
  return email.value ? email.value[0].toUpperCase() : '?'
})
const background = computed(() => {
  let h = 0
  for (const ch of email.value || props.name) h = (h * 31 + ch.charCodeAt(0)) % 360
  return `hsl(${h}, 45%, 55%)`
})

/** 候选图片 URL（按优先级）；逐一尝试，全部失败则空数组 → 显示首字母。 */
const urls = ref<string[]>([])
const idx = ref(0)
const photo = ref(false)
const current = computed(() => urls.value[idx.value] ?? '')

async function sha256Hex(text: string): Promise<string> {
  const buf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text))
  return Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('')
}

onMounted(async () => {
  // 命中内置品牌图标 → 直接用，不做任何网络请求。
  if (brand.value) return
  const e = email.value
  const list: string[] = []

  // 发件域 favicon（桌面端由主进程做 7 天缓存与域名/二级域/DuckDuckGo 回退；
  // Web 端 mailIconUrls 返回原来的直连候选链）
  const at = e.indexOf('@')
  const domain = at >= 0 ? e.slice(at + 1) : ''
  if (domain) list.push(...mailIconUrls(domain, props.size * 2))

  // Gravatar（个人头像）
  if (at >= 0 && !gravatarMiss.has(e) && crypto?.subtle) {
    let hash = hashCache.get(e)
    if (!hash) {
      try {
        hash = await sha256Hex(e)
        hashCache.set(e, hash)
      } catch {
        hash = undefined
      }
    }
    if (hash) {
      const url = gravatarIconUrl(hash, props.size * 2)
      if (url) list.push(url)
    }
  }

  urls.value = list
})

function onLoad(): void {
  photo.value = true
}

function onError(): void {
  // 尝试下一个来源；全部失败则回退首字母
  if (idx.value < urls.value.length - 1) {
    idx.value += 1
    return
  }
  const e = email.value
  if (e) gravatarMiss.add(e)
  photo.value = false
  urls.value = []
}
</script>

<template>
  <!-- 内置品牌图标：白底 + 品牌色 Logo（离线可用，与主流邮箱一致） -->
  <span
    v-if="brand"
    class="mail-avatar mail-avatar-brand"
    :style="{ width: size + 'px', height: size + 'px' }"
  >
    <svg
      class="mail-avatar-brand-svg"
      :style="{ color: brand.hex }"
      viewBox="0 0 24 24"
      aria-hidden="true"
      v-html="brand.body"
    />
  </span>
  <span
    v-else
    class="mail-avatar"
    :style="{
      background: photo ? 'var(--bg-base)' : background,
      width: size + 'px',
      height: size + 'px',
      fontSize: Math.round(size * 0.42) + 'px'
    }"
  >
    <span class="mail-avatar-letter">{{ letter }}</span>
    <img
      v-if="current"
      class="mail-avatar-photo"
      :class="{ visible: photo }"
      :src="current"
      alt=""
      loading="lazy"
      referrerpolicy="no-referrer"
      @load="onLoad"
      @error="onError"
    />
  </span>
</template>

<style scoped>
.mail-avatar {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  border-radius: 50%;
  overflow: hidden;
  color: #fff;
  font-weight: 600;
}
.mail-avatar-letter {
  line-height: 1;
}
/* 图片绝对覆盖在首字母之上；未加载完成前保持透明，露出底下的首字母/底色 */
.mail-avatar-photo {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
  opacity: 0;
}
.mail-avatar-photo.visible {
  opacity: 1;
}
/* 内置品牌图标：浅底圆 + 品牌色 Logo（明暗主题下都清晰，Logo 不随主题反色） */
.mail-avatar-brand {
  background: #fff;
  box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.06);
}
.mail-avatar-brand-svg {
  width: 62%;
  height: 62%;
  display: block;
}
</style>
