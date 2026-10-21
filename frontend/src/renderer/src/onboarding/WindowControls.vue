<script setup lang="ts">
/**
 * 非 macOS 无边框窗口的自绘窗口按钮（最小化 / 最大化 / 关闭）。
 *
 * 引导阶段 StatusBar 没挂载，没人画这三个按钮——Windows/Linux 是无边框窗口，
 * 没有它们就关不掉窗口。观感与 StatusBar 的窗口按钮一致（同款尺寸/悬浮底色）。
 */
import { onBeforeUnmount, onMounted, ref } from 'vue'

const api = window.api
const maximized = ref(false)

let off: (() => void) | null = null
onMounted(async () => {
  maximized.value = (await api?.window?.isMaximized?.()) ?? false
  off = api?.window?.onMaximizedChange?.((v: boolean) => (maximized.value = v)) ?? null
})
onBeforeUnmount(() => off?.())
</script>

<template>
  <div class="ob-winctl">
    <button
      type="button"
      class="ob-winbtn"
      :title="'—'"
      aria-label="minimize"
      @click="api.window.minimize()"
    >
      <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
        <path d="M2.5 6h7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
      </svg>
    </button>
    <button
      type="button"
      class="ob-winbtn"
      :aria-label="maximized ? 'restore' : 'maximize'"
      @click="api.window.toggleMaximize()"
    >
      <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
        <rect
          v-if="!maximized"
          x="3"
          y="3"
          width="6"
          height="6"
          rx="1"
          fill="none"
          stroke="currentColor"
          stroke-width="1.2"
        />
        <g v-else fill="none" stroke="currentColor" stroke-width="1.2">
          <rect x="2.6" y="4.4" width="5" height="5" rx="1" />
          <path d="M4.6 4.4V3.2a1 1 0 0 1 1-1h3.6a1 1 0 0 1 1 1v3.6a1 1 0 0 1-1 1H8.6" />
        </g>
      </svg>
    </button>
    <button
      type="button"
      class="ob-winbtn ob-winbtn-close"
      aria-label="close"
      @click="api.window.close()"
    >
      <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
        <path
          d="M3.4 3.4l5.2 5.2M8.6 3.4l-5.2 5.2"
          stroke="currentColor"
          stroke-width="1.2"
          stroke-linecap="round"
        />
      </svg>
    </button>
  </div>
</template>

<style scoped>
.ob-winctl {
  position: fixed;
  top: 0;
  right: 0;
  z-index: 3;
  display: flex;
  align-items: center;
  height: 38px;
  padding-right: 6px;
  -webkit-app-region: no-drag;
}
.ob-winbtn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 28px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background-color 120ms ease;
}
.ob-winbtn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.ob-winbtn-close:hover {
  background: var(--danger, #f53f3f);
  color: #fff;
}
</style>
