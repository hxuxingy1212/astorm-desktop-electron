// 生成邮件头像的「品牌图标」模块：从 @iconify-json/simple-icons 里**只抽取**精选品牌，
// 落成一个紧凑的本地 TS 模块（避免把 4.6MB 的全量 icons.json 打进渲染包）。
//
// 用法：node scripts/gen-mail-brands.mjs
// 产物：frontend/src/renderer/src/components/mail/mailBrandIcons.ts（提交入库）
//
// 需要新增品牌时：改下面的 SLUGS / DOMAIN_BRAND / HEX，重跑本脚本即可。

import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const src = resolve(root, 'frontend/node_modules/@iconify-json/simple-icons/icons.json')
const out = resolve(root, 'frontend/src/renderer/src/components/mail/mailBrandIcons.ts')

const data = JSON.parse(readFileSync(src, 'utf8'))
const icons = { ...data.icons }
// 解析别名（形如 { parent: 'qq' }）：指向父图标的 body。
for (const [name, alias] of Object.entries(data.aliases ?? {})) {
  const parent = alias.parent && icons[alias.parent]
  if (parent) icons[name] = parent
}

/** 精选品牌 slug（simple-icons 名）。 */
const SLUGS = [
  'apple', 'icloud', 'alibabacloud', 'alibabadotcom', 'alipay', 'taobao', 'wechat', 'tencentqq',
  'bilibili', 'sinaweibo', 'zhihu', 'baidu', 'huawei', 'xiaomi', 'neteasecloudmusic',
  'github', 'gitlab', 'bitbucket', 'steam', 'termius', 'docker', 'kubernetes', 'postman',
  'google', 'gmail', 'googlechrome', 'youtube', 'android', 'microsoft', 'microsoftoutlook', 'windows',
  'amazon', 'amazonwebservices', 'paypal', 'netflix', 'adobe', 'dropbox', 'slack', 'notion',
  'figma', 'discord', 'linkedin', 'facebook', 'meta', 'x', 'reddit', 'medium', 'twitch', 'zoom',
  'atlassian', 'jira', 'confluence', 'trello', 'asana', 'linear', 'salesforce', 'oracle',
  'digitalocean', 'cloudflare', 'mozilla', 'firefox', 'spotify', 'telegram', 'whatsapp',
  'openai', 'anthropic', 'npm', 'python', 'rust', 'vuedotjs', 'svelte', 'tailwindcss',
  'vercel', 'netlify', 'stripe', 'shopify', 'nvidia', 'intel', 'airbnb', 'uber', 'ebay',
  'bookingdotcom', 'heroku', 'protonmail'
]

/** 品牌色（simple-icons 官方 hex）。缺省则用中性前景色。 */
const HEX = {
  apple: '#000000', icloud: '#3693F3', alibabacloud: '#FF6A00', alibabadotcom: '#FF6A00',
  alipay: '#1677FF', taobao: '#FF4400', wechat: '#07C160', tencentqq: '#1EBAFC',
  bilibili: '#00A1D6', sinaweibo: '#E6162D', zhihu: '#0084FF', baidu: '#2932E1',
  huawei: '#FF0000', xiaomi: '#FF6900', neteasecloudmusic: '#C20C0C',
  github: '#181717', gitlab: '#FC6D26', bitbucket: '#0052CC', steam: '#000000',
  termius: '#0A84FF', docker: '#2496ED', kubernetes: '#326CE5', postman: '#FF6C37',
  google: '#4285F4', gmail: '#EA4335', googlechrome: '#4285F4', youtube: '#FF0000',
  android: '#34A853', microsoft: '#5E5E5E', microsoftoutlook: '#0078D4', windows: '#0078D4',
  amazon: '#FF9900', amazonwebservices: '#FF9900', paypal: '#003087', netflix: '#E50914',
  adobe: '#FF0000', dropbox: '#0061FF', slack: '#4A154B', notion: '#000000', figma: '#F24E1E',
  discord: '#5865F2', linkedin: '#0A66C2', facebook: '#0866FF', meta: '#0866FF', x: '#000000',
  reddit: '#FF4500', medium: '#000000', twitch: '#9146FF', zoom: '#0B5CFF',
  atlassian: '#0052CC', jira: '#0052CC', confluence: '#0052CC', trello: '#0052CC', asana: '#F06A6A',
  linear: '#5E6AD2', salesforce: '#00A1E0', oracle: '#F80000', digitalocean: '#0080FF',
  cloudflare: '#F38020', mozilla: '#000000', firefox: '#FF7139', spotify: '#1DB954',
  telegram: '#26A5E4', whatsapp: '#25D366', openai: '#412991', anthropic: '#191919',
  npm: '#CB3837', python: '#3776AB', rust: '#000000', vuedotjs: '#4FC08D', svelte: '#FF3E00',
  tailwindcss: '#06B6D4', vercel: '#000000', netlify: '#00C7B7', stripe: '#635BFF',
  shopify: '#7AB55C', nvidia: '#76B900', intel: '#0071C5', airbnb: '#FF5A5F', uber: '#000000',
  ebay: '#E53238', bookingdotcom: '#003580', heroku: '#430098', protonmail: '#6D4AFF'
}

/** 域名 → 品牌 slug（匹配时先整域、再退到末两段）。 */
const DOMAIN_BRAND = {
  'apple.com': 'apple', 'icloud.com': 'icloud', 'me.com': 'icloud',
  'alibabacloud.com': 'alibabacloud', 'aliyun.com': 'alibabacloud', 'alibaba-inc.com': 'alibabadotcom',
  'alibaba.com': 'alibabadotcom', 'alipay.com': 'alipay', 'taobao.com': 'taobao', 'tmall.com': 'taobao',
  'wechat.com': 'wechat', 'weixin.qq.com': 'wechat', 'qq.com': 'tencentqq',
  'bilibili.com': 'bilibili', 'weibo.com': 'sinaweibo', 'sina.com': 'sinaweibo', 'sina.cn': 'sinaweibo',
  'zhihu.com': 'zhihu', 'baidu.com': 'baidu', 'huawei.com': 'huawei', 'xiaomi.com': 'xiaomi',
  '163.com': 'neteasecloudmusic', '126.com': 'neteasecloudmusic',
  'github.com': 'github', 'githubusercontent.com': 'github', 'gitlab.com': 'gitlab',
  'bitbucket.org': 'bitbucket', 'steampowered.com': 'steam', 'steamcommunity.com': 'steam',
  'valvesoftware.com': 'steam', 'termius.com': 'termius', 'docker.com': 'docker',
  'kubernetes.io': 'kubernetes', 'postman.com': 'postman',
  'google.com': 'google', 'gmail.com': 'gmail', 'googlemail.com': 'gmail', 'youtube.com': 'youtube',
  'outlook.com': 'microsoftoutlook', 'hotmail.com': 'microsoftoutlook', 'live.com': 'microsoftoutlook',
  'microsoft.com': 'microsoft', 'office.com': 'microsoft',
  'amazon.com': 'amazon', 'amazon.cn': 'amazon', 'amazonaws.com': 'amazonwebservices',
  'paypal.com': 'paypal', 'netflix.com': 'netflix', 'adobe.com': 'adobe', 'dropbox.com': 'dropbox',
  'slack.com': 'slack', 'notion.so': 'notion', 'notion.com': 'notion', 'figma.com': 'figma',
  'discord.com': 'discord', 'discordapp.com': 'discord', 'linkedin.com': 'linkedin',
  'facebook.com': 'facebook', 'fb.com': 'facebook', 'meta.com': 'meta',
  'x.com': 'x', 'twitter.com': 'x', 'reddit.com': 'reddit', 'medium.com': 'medium',
  'twitch.tv': 'twitch', 'zoom.us': 'zoom', 'atlassian.com': 'atlassian', 'jira.com': 'jira',
  'digitalocean.com': 'digitalocean', 'mozilla.org': 'mozilla', 'firefox.com': 'firefox',
  'spotify.com': 'spotify', 'telegram.org': 'telegram', 'whatsapp.com': 'whatsapp',
  'openai.com': 'openai', 'anthropic.com': 'anthropic', 'claude.ai': 'anthropic',
  'npmjs.com': 'npm', 'python.org': 'python', 'rust-lang.org': 'rust', 'vuejs.org': 'vuedotjs',
  'svelte.dev': 'svelte', 'tailwindcss.com': 'tailwindcss', 'vercel.com': 'vercel',
  'netlify.com': 'netlify', 'stripe.com': 'stripe', 'shopify.com': 'shopify', 'oracle.com': 'oracle',
  'salesforce.com': 'salesforce', 'nvidia.com': 'nvidia', 'intel.com': 'intel', 'airbnb.com': 'airbnb',
  'uber.com': 'uber', 'ebay.com': 'ebay', 'booking.com': 'bookingdotcom', 'linear.app': 'linear',
  'heroku.com': 'heroku', 'protonmail.com': 'protonmail', 'proton.me': 'protonmail',
  'asana.com': 'asana', 'trello.com': 'trello'
}

const picked = {}
const missing = []
for (const slug of SLUGS) {
  const icon = icons[slug]
  if (!icon || !icon.body) {
    missing.push(slug)
    continue
  }
  picked[slug] = { hex: HEX[slug] ?? '#000000', body: icon.body }
}

const esc = (s) => s.replace(/\\/g, '\\\\').replace(/'/g, "\\'")
const bodyLines = Object.entries(picked)
  .map(([k, v]) => `  '${k}': { hex: '${v.hex}', body: '${esc(v.body)}' }`)
  .join(',\n')
const domainLines = Object.entries(DOMAIN_BRAND)
  .filter(([, slug]) => picked[slug])
  .map(([d, slug]) => `  '${d}': '${slug}'`)
  .join(',\n')

const content = `/* eslint-disable */
// AUTO-GENERATED by scripts/gen-mail-brands.mjs — 请勿手改，改脚本后重跑。
// 数据来源：simple-icons（CC0）。仅抽取精选品牌，避免全量 icons.json 进包。

/** 单个品牌图标：品牌色 + 24x24 的 SVG body（fill=currentColor）。 */
export interface MailBrandIcon {
  hex: string
  body: string
}

/** slug → 图标。 */
export const BRAND_ICONS: Record<string, MailBrandIcon> = {
${bodyLines}
}

/** 发件域名 → 品牌 slug（匹配时先整域、再退到末两段）。 */
export const DOMAIN_BRAND: Record<string, string> = {
${domainLines}
}
`

writeFileSync(out, content)
console.log(`[gen-mail-brands] 写入 ${out}`)
console.log(`[gen-mail-brands] 品牌 ${Object.keys(picked).length} 个，域名映射 ${Object.keys(DOMAIN_BRAND).filter((d) => picked[DOMAIN_BRAND[d]]).length} 条`)
if (missing.length) console.warn(`[gen-mail-brands] 缺失（已跳过）: ${missing.join(', ')}`)
