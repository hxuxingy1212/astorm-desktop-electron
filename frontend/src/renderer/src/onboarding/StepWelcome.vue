<script setup lang="ts">
/**
 * 引导第一步：欢迎。品牌 + 一句话价值主张 + 「开始使用」。
 *
 * 入场动效：logo 缩放浮现 → 文案依次上浮（纯 CSS 动画，reduced-motion 下不播）。
 */
import { useI18n } from 'vue-i18n'

defineProps<{
  busy: boolean
}>()

const emit = defineEmits<{ next: [] }>()

const { t } = useI18n()
</script>

<template>
  <div class="ob-welcome">
    <div class="ob-hero">
      <img src="@/assets/logo.png" alt="Astorm" class="ob-logo" />
      <h1 class="ob-title">{{ t('onboarding.welcomeTitle') }}</h1>
      <p class="ob-sub">{{ t('onboarding.welcomeSub') }}</p>
    </div>

    <footer class="ob-foot">
      <a-button
        class="ob-cta"
        type="primary"
        :loading="busy"
        data-ob="start"
        @click="emit('next')"
      >
        {{ t('onboarding.start') }}
      </a-button>
    </footer>
  </div>
</template>

<style scoped>
.ob-welcome {
  display: flex;
  flex-direction: column;
  gap: 22px;
  padding: var(--ob-step-pad, 32px 36px 28px);
  flex: 1;
}
.ob-hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
  text-align: center;
}
.ob-logo {
  width: 76px;
  height: 76px;
  border-radius: 19px;
  object-fit: cover;
  /* 深色主题下图标是深底白标：补一圈发丝描边，边缘才立得住（否则像浮在空中） */
  box-shadow:
    0 0 0 0.5px color-mix(in srgb, var(--border-strong) 65%, transparent),
    var(--shadow-md);
  animation: ob-pop 520ms cubic-bezier(0.32, 0.72, 0, 1) both;
}
.ob-title {
  margin: 0;
  font-size: 25px;
  font-weight: 600;
  letter-spacing: 0.01em;
  color: var(--text-primary);
  animation: ob-rise 460ms cubic-bezier(0.32, 0.72, 0, 1) 60ms both;
}
.ob-sub {
  margin: 0;
  max-width: 400px;
  font-size: 13.5px;
  line-height: 1.6;
  color: var(--text-tertiary);
  animation: ob-rise 460ms cubic-bezier(0.32, 0.72, 0, 1) 130ms both;
}
/* 页脚：居中一列，与居中的 hero 同一轴线 */
.ob-foot {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding-top: 6px;
  animation: ob-rise 460ms cubic-bezier(0.32, 0.72, 0, 1) 200ms both;
}
.ob-cta {
  min-width: 148px;
  height: 34px;
  border-radius: 8px;
}
@keyframes ob-pop {
  from {
    opacity: 0;
    transform: scale(0.9);
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}
@keyframes ob-rise {
  from {
    opacity: 0;
    transform: translateY(10px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@media (prefers-reduced-motion: reduce) {
  .ob-logo,
  .ob-title,
  .ob-sub,
  .ob-points,
  .ob-foot {
    animation: none;
  }
}
</style>
