<script setup lang="ts">
/**
 * 启动引导（Electron 壳子专属）——三步：
 *   ① 欢迎 → ② 选择连接方式（本机 / 云容器）→ ③ 偏好（明暗 · 语言 · 电源，可跳过）
 *
 * 何时进入：首次启动（`connection.json` 的 `onboarded` 标记为 false）或从菜单「连接方式…」/
 * 设置页主动进来；已完成引导时启动直接进应用，引导页只能从「连接方式…」打开。
 * 完成时由主进程换壳导航：本机 → 本地 UI；云容器 → 远端服务端托管的 Web UI。
 *
 * 视觉语言走引导页独立约定（无卡片/无面板，内容落在动态背景上）；动作按钮按 §3.4，
 * 强调色用 `--accent-text`；明暗/语言/电源改动**即时生效并落盘**（引导阶段 App.vue
 * 没挂载，这里自己 watch）。
 *
 * 本组件刻意不碰任何后端：不启 store 的会话链路、不调会话/设置 IPC，只做表单与探测。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Message } from '@arco-design/web-vue'
import { useSettingsStore } from '@/stores/settings'
import { applyTheme } from '@/theme'
import { showWindowControls } from '@/utils/platform'
import StepWelcome from './StepWelcome.vue'
import StepChoice from './StepChoice.vue'
import StepModel from './StepModel.vue'
import StepPrefs from './StepPrefs.vue'
import WindowControls from './WindowControls.vue'

const { t, te } = useI18n()
const settings = useSettingsStore()

const info = window.api?.shellConnection?.info ?? {
  mode: 'unset' as const,
  endpoint: '',
  hasToken: false,
  welcome: true,
  onboarded: false
}

/** 当前步（0 欢迎 / 1 选择 / …）；已配置过从菜单进来时直接落在「选择」 */
const step = ref(info.mode === 'unset' ? 0 : 1)
const configured = computed(() => info.mode === 'local' || info.mode === 'cloud')

/* ---------------- 连接方式（第二步） ---------------- */

const target = ref<'local' | 'cloud'>(info.mode === 'cloud' ? 'cloud' : 'local')
const endpoint = ref(info.endpoint ?? '')
const token = ref('')
const testing = ref(false)
const tested = ref(false)
/** 探测结果**结构化**存着，展示文案在 computed 里拼——切换语言时这一行跟着变 */
type TestResult = {
  ok: boolean
  code?: string
  message?: string
  status?: number
  detail?: string
  model?: string
  dataDir?: string
}
const testResult = ref<TestResult | null>(null)
const testResultText = computed(() => (testResult.value ? testText(testResult.value) : ''))
const testResultView = computed(() =>
  testResult.value ? { ok: testResult.value.ok, text: testResultText.value } : null
)
const endpointOk = computed(() => /^(https?:\/\/)?[\w.-]+(:\d+)?/i.test(endpoint.value.trim()))
const busy = ref(false)

/** 探测端点（GET /api/v1/status，Bearer 令牌）；通过后记 tested，后续步骤不再重复探测 */
async function test(): Promise<void> {
  if (!endpointOk.value) {
    tested.value = false
    testResult.value = { ok: false, code: 'endpointInvalid' }
    return
  }
  testing.value = true
  testResult.value = null
  try {
    const r = (await window.api.shellConnection.test(endpoint.value, token.value)) as TestResult
    tested.value = r.ok
    testResult.value = { ...r }
  } catch (e) {
    tested.value = false
    testResult.value = { ok: false, message: (e as Error).message }
  } finally {
    testing.value = false
  }
}

/**
 * 测试结果文案：主进程只回 code / 结构化字段，文案在这里按当前语言拼
 * （主进程没有 i18n 实例，早先直接回中文，英文界面里会蹦中文）。
 */
function testText(r: TestResult): string {
  if (r.ok) {
    return [
      t('onboarding.testOk'),
      r.model && t('onboarding.testModel', { model: r.model }),
      r.dataDir && t('onboarding.testData', { dir: r.dataDir })
    ]
      .filter(Boolean)
      .join(' · ')
  }
  const key = r.code ? `onboarding.testErr.${r.code}` : ''
  if (key && te(key)) return t(key, { status: r.status ?? '', message: r.detail ?? '' })
  return r.message ?? t('onboarding.saveFailed')
}

/** 第二步「继续」：云端要先连得上（没测过就现测一次），本机直接过 */
async function nextFromChoice(): Promise<void> {
  if (target.value === 'local') {
    next()
    return
  }
  if (!tested.value) await test()
  if (tested.value) next()
}

/* ---------------- 步骤流转 ---------------- */

type StepId = 'welcome' | 'choice' | 'model' | 'prefs'

/**
 * 本机分支比云端多一步「配置模型」：本机后端要有一个模型才能干活，而全新安装是空的；
 * 云端分支不出现——容器的模型在容器自己的「设置 → 模型」里配，壳子不碰远端配置。
 */
const flow = computed<StepId[]>(() =>
  target.value === 'cloud' ? ['welcome', 'choice', 'prefs'] : ['welcome', 'choice', 'model', 'prefs']
)
const current = computed<StepId>(() => flow.value[Math.min(step.value, flow.value.length - 1)] ?? 'welcome')

function next(): void {
  step.value = Math.min(step.value + 1, flow.value.length - 1)
}
function back(): void {
  step.value = Math.max(step.value - 1, 0)
}

/** 落盘与导航：完成（或跳过偏好）→ 落连接方式，主进程随后换壳导航 */
async function finish(): Promise<void> {
  busy.value = true
  try {
    settings.persist()
    if (target.value === 'cloud') {
      await window.api.shellConnection.save({
        mode: 'cloud',
        endpoint: endpoint.value,
        token: token.value
      })
    } else {
      await window.api.shellConnection.save({ mode: 'local' })
    }
  } catch (e) {
    busy.value = false
    Message.error((e as Error).message || t('onboarding.saveFailed'))
  }
}

/** 已配置过、从「连接方式…」进来时的返回：按当前配置回到应用（不重新落盘） */
async function exitWelcome(): Promise<void> {
  busy.value = true
  try {
    await window.api.shellConnection.closeWelcome()
  } catch (e) {
    busy.value = false
    Message.error((e as Error).message || t('onboarding.saveFailed'))
  }
}

/** 点步骤点回看（只允许回退，不允许跳过没走过的步骤） */
function go(i: number): void {
  if (i < step.value) step.value = i
}

/* ---------------- 偏好即时生效（引导阶段没有 App.vue 的 watcher） ---------------- */

watch(
  () => [settings.themeMode, settings.themePreset] as const,
  ([mode, preset]) => {
    applyTheme(mode, preset)
    settings.persist()
  },
  { immediate: true }
)
watch(
  () => settings.locale,
  (locale) => {
    void import('@/i18n').then(({ i18n }) => {
      i18n.global.locale.value = locale
    })
    document.documentElement.lang = locale
    settings.persist()
  }
)
watch(
  () => settings.keepAwake,
  (enabled) => {
    void window.api?.power?.setKeepAwake(enabled)
    settings.persist()
  },
  { immediate: true }
)

onMounted(() => {
  // 欢迎步直接回车 = 下一步（键盘可达性）
  window.addEventListener('keydown', onKey)
})
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' && step.value === 0) step.value = 1
}
</script>

<template>
  <div class="ob-root">
    <!-- 动态背景：主题色光斑缓慢漂移（reduced-motion 下静止） -->
    <div class="ob-bg" aria-hidden="true">
      <span class="ob-blob ob-blob-1" />
      <span class="ob-blob ob-blob-2" />
      <span class="ob-blob ob-blob-3" />
      <span class="ob-veil" />
    </div>

    <!-- 非 macOS 无边框窗口：自绘窗口按钮（macOS 用系统红黄绿，已由系统提供） -->
    <WindowControls v-if="showWindowControls" />

    <!-- 已配置过时从「连接方式…」进来：左上角返回当前应用（首次引导不显示） -->
    <button
      v-if="configured"
      type="button"
      class="ob-exit"
      data-ob="exit"
      :disabled="busy"
      @click="exitWelcome"
    >
      <icon-left :size="16" />
      {{ t('onboarding.backToApp') }}
    </button>

    <main class="ob-stage">
      <section class="ob-card">
        <Transition name="ob-step" mode="out-in">
          <StepWelcome
            v-if="current === 'welcome'"
            key="welcome"
            :busy="busy"
            @next="next"
          />
          <StepChoice
            v-else-if="current === 'choice'"
            key="choice"
            v-model:target="target"
            v-model:endpoint="endpoint"
            v-model:token="token"
            :testing="testing"
            :test-result="testResultView"
            :busy="busy"
            @test="test"
            @next="nextFromChoice"
            @back="back"
          />
          <StepModel
            v-else-if="current === 'model'"
            key="model"
            :busy="busy"
            @next="next"
            @skip="next"
            @back="back"
          />
          <StepPrefs
            v-else
            key="prefs"
            :busy="busy"
            @finish="finish"
            @skip="finish"
            @back="back"
          />
        </Transition>
      </section>

      <!-- 步骤指示：走过的可点回看（步数随分支变：本机多一步「模型」） -->
      <nav class="ob-dots">
        <button
          v-for="(id, i) in flow"
          :key="id"
          type="button"
          class="ob-dot"
          :class="{ active: step === i, done: step > i }"
          :disabled="step <= i"
          :aria-label="t('onboarding.stepOf', { n: i + 1, total: flow.length })"
          @click="go(i)"
        />
      </nav>
    </main>
  </div>
</template>

<style scoped>
.ob-root {
  position: relative;
  min-height: 100vh;
  /* 纵向可滚（小窗口/最小尺寸 780×600 下模型步会超过一屏），横向裁掉溢出的光斑 */
  overflow: hidden auto;
  background: var(--bg-1);
  /* 无边框窗口（本机形态）下整屏可拖动，交互元素各自 no-drag */
  -webkit-app-region: drag;
  /* 内容留白：窗口矮的时候收紧（各步骤共用，改一处即可） */
  --ob-stage-pad: 56px 24px 40px;
  --ob-step-pad: 32px 36px 28px;
}
@media (max-height: 720px) {
  .ob-root {
    --ob-stage-pad: 30px 24px 22px;
    --ob-step-pad: 22px 32px 18px;
  }
}

/* ---------- 动态背景 ---------- */

.ob-bg {
  /* fixed：绝对定位的 inset:-12vmax 会撑出滚动区（能滚进一片空白），固定定位不参与滚动 */
  position: fixed;
  inset: -12vmax;
  pointer-events: none;
}
.ob-blob {
  position: absolute;
  display: block;
  border-radius: 50%;
  filter: blur(64px);
  opacity: 0.5;
  will-change: transform;
}
.ob-blob-1 {
  top: 4%;
  left: 6%;
  width: 46vmax;
  height: 46vmax;
  background: radial-gradient(circle at 35% 35%, rgb(var(--primary-6)) 0%, transparent 62%);
  animation: ob-drift-a 32s ease-in-out infinite;
}
.ob-blob-2 {
  right: 2%;
  top: 12%;
  width: 38vmax;
  height: 38vmax;
  background: radial-gradient(circle at 60% 40%, rgb(var(--primary-4)) 0%, transparent 60%);
  animation: ob-drift-b 41s ease-in-out infinite;
}
.ob-blob-3 {
  left: 28%;
  bottom: -6%;
  width: 42vmax;
  height: 42vmax;
  background: radial-gradient(circle at 50% 50%, rgb(var(--primary-3)) 0%, transparent 58%);
  animation: ob-drift-c 52s ease-in-out infinite;
}
/* 内容区薄纱：以内容为中心的一圈柔光（无边界），文字压得住、光斑仍在四周可见。
   引导页刻意**没有卡片/面板**——内容直接落在动态背景上，像系统开箱设置而不是弹窗表单。 */
.ob-veil {
  position: absolute;
  inset: 0;
  background: radial-gradient(
    118% 80% at 50% 46%,
    color-mix(in srgb, var(--bg-1) 64%, transparent) 0%,
    color-mix(in srgb, var(--bg-1) 38%, transparent) 52%,
    color-mix(in srgb, var(--bg-1) 14%, transparent) 100%
  );
}
[data-theme='dark'] .ob-blob {
  opacity: 0.34;
}
@keyframes ob-drift-a {
  0%,
  100% {
    transform: translate3d(0, 0, 0) scale(1);
  }
  50% {
    transform: translate3d(6vmax, 4vmax, 0) scale(1.08);
  }
}
@keyframes ob-drift-b {
  0%,
  100% {
    transform: translate3d(0, 0, 0) scale(1.04);
  }
  50% {
    transform: translate3d(-5vmax, 6vmax, 0) scale(0.96);
  }
}
@keyframes ob-drift-c {
  0%,
  100% {
    transform: translate3d(0, 0, 0) scale(0.98);
  }
  50% {
    transform: translate3d(4vmax, -5vmax, 0) scale(1.06);
  }
}
@media (prefers-reduced-motion: reduce) {
  .ob-blob {
    animation: none;
  }
}

/* ---------- 舞台与卡片 ---------- */

/* 已配置过、从「连接方式…」进来：左上角返回当前应用 */
.ob-exit {
  position: fixed;
  top: 48px;
  left: 16px;
  z-index: 3;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 32px;
  padding: 0 10px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  cursor: pointer;
  -webkit-app-region: no-drag;
}
.ob-exit:hover:not(:disabled) {
  background: var(--fill-1);
  color: var(--text-primary);
}

.ob-stage {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 22px;
  min-height: 100vh;
  padding: var(--ob-stage-pad);
  /* 顶部留给 macOS 红黄绿 / 非 mac 的自绘窗口按钮 */
}
/* 内容列：只是排版用的窄栏，**没有面板外观**（无底、无描边、无阴影、无固定高度） */
.ob-card {
  width: 100%;
  max-width: 620px;
  display: flex;
  flex-direction: column;
  -webkit-app-region: no-drag;
}

/* ---------- 步骤过渡（macOS 观感：轻微上浮 + 淡入） ---------- */

.ob-step-enter-active {
  transition:
    opacity 260ms cubic-bezier(0.32, 0.72, 0, 1),
    transform 260ms cubic-bezier(0.32, 0.72, 0, 1);
}
.ob-step-leave-active {
  transition:
    opacity 150ms ease,
    transform 150ms ease;
}
.ob-step-enter-from {
  opacity: 0;
  transform: translateY(14px) scale(0.988);
}
.ob-step-leave-to {
  opacity: 0;
  transform: translateY(-8px) scale(0.992);
}
@media (prefers-reduced-motion: reduce) {
  .ob-step-enter-active,
  .ob-step-leave-active {
    transition: none;
  }
  .ob-step-enter-from,
  .ob-step-leave-to {
    transform: none;
  }
}

/* ---------- 步骤指示 ---------- */

.ob-dots {
  display: flex;
  align-items: center;
  gap: 8px;
  /* 步骤点在无边框窗口的拖动区里（整屏可拖），不给 no-drag 的话真实鼠标点不到它们 */
  -webkit-app-region: no-drag;
}
.ob-dot {
  width: 7px;
  height: 7px;
  padding: 0;
  border: none;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-tertiary) 45%, transparent);
  cursor: pointer;
  transition:
    width 220ms cubic-bezier(0.32, 0.72, 0, 1),
    background-color 200ms ease;
}
.ob-dot.done {
  background: color-mix(in srgb, var(--text-tertiary) 70%, transparent);
}
/* 可点回看的点给个悬浮反馈（当前/未走过的点是禁用的，不给） */
.ob-dot:not(:disabled):hover {
  background: color-mix(in srgb, var(--text-tertiary) 85%, transparent);
}
.ob-dot.active {
  width: 20px;
  background: var(--accent-text);
}
.ob-dot:disabled {
  cursor: default;
}
</style>

<!-- 非 scoped：三步共用的主按钮（子组件 scoped 只管尺寸）。全局把 .arco-btn-primary 压成中性灰
     （见 styles/global.css），引导页按动作按钮规范压回强调色实底（§3.4，--accent-text）。 -->
<style>
.ob-cta.arco-btn-primary {
  background-color: var(--accent-text) !important;
  border-color: var(--accent-text) !important;
  color: #fff !important;
}
.ob-cta.arco-btn-primary:hover {
  background-color: color-mix(in srgb, var(--accent-text) 88%, #000) !important;
  border-color: color-mix(in srgb, var(--accent-text) 88%, #000) !important;
}
.ob-cta.arco-btn-primary.arco-btn-loading {
  opacity: 0.72;
}
</style>
