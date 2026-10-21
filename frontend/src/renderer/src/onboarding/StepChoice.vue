<script setup lang="ts">
/**
 * 引导第二步：选择连接方式——「本机」或「云容器」。
 *
 * 两张卡（整卡可点选，选中态用主题色描边 + 淡底）；选云容器时**就地展开**端点 / 令牌
 * 表单（高度+淡入过渡），带「测试连接」；点「继续」时云端必须先连通（没测过就现测）。
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  target: 'local' | 'cloud'
  endpoint: string
  token: string
  testing: boolean
  testResult: { ok: boolean; text: string } | null
  busy: boolean
}>()

const emit = defineEmits<{
  'update:target': ['local' | 'cloud']
  'update:endpoint': [string]
  'update:token': [string]
  test: []
  next: []
  back: []
}>()

const { t } = useI18n()

/** 云端：端点非空才让点「测试连接」（归一化交给主进程） */
const endpointFilled = computed(() => props.endpoint.trim().length > 0)
</script>

<template>
  <div class="ob-choice">
    <header class="ob-head">
      <h2 class="ob-title">{{ t('onboarding.choiceTitle') }}</h2>
      <p class="ob-sub">{{ t('onboarding.choiceSub') }}</p>
    </header>

    <div class="ob-cards">
      <!-- 本机 -->
      <div
        class="ob-opt"
        :class="{ active: target === 'local' }"
        role="radio"
        :aria-checked="target === 'local'"
        tabindex="0"
        data-ob="card-local"
        @click="emit('update:target', 'local')"
        @keydown.enter="emit('update:target', 'local')"
      >
        <span class="ob-opt-icon"><icon-cpu :size="19" /></span>
        <span class="ob-opt-body">
          <span class="ob-opt-title">{{ t('onboarding.localTitle') }}</span>
          <span class="ob-opt-desc">{{ t('onboarding.localDesc') }}</span>
        </span>
        <span class="ob-radio" />
      </div>

      <!-- 云容器 -->
      <div
        class="ob-opt"
        :class="{ active: target === 'cloud' }"
        role="radio"
        :aria-checked="target === 'cloud'"
        tabindex="0"
        data-ob="card-cloud"
        @click="emit('update:target', 'cloud')"
        @keydown.enter="emit('update:target', 'cloud')"
      >
        <span class="ob-opt-icon"><icon-globe :size="19" /></span>
        <span class="ob-opt-body">
          <span class="ob-opt-title">{{ t('onboarding.cloudTitle') }}</span>
          <span class="ob-opt-desc">{{ t('onboarding.cloudDesc') }}</span>
        </span>
        <span class="ob-radio" />
      </div>
    </div>

    <!-- 云端表单：选中云容器时展开（本机分支无需额外内容） -->
    <Transition name="ob-reveal">
      <div v-if="target === 'cloud'" class="ob-form" data-ob="cloud-form">
        <a-input
          :model-value="endpoint"
          :placeholder="t('onboarding.endpointPlaceholder')"
          allow-clear
          data-ob="endpoint"
          @update:model-value="emit('update:endpoint', $event)"
          @press-enter="emit('test')"
        >
          <template #prefix><icon-link :size="14" /></template>
        </a-input>
        <a-input-password
          :model-value="token"
          :placeholder="t('onboarding.tokenPlaceholder')"
          allow-clear
          data-ob="token"
          @update:model-value="emit('update:token', $event)"
          @press-enter="emit('test')"
        >
          <template #prefix><icon-safe :size="14" /></template>
        </a-input-password>
        <div class="ob-form-row">
          <a-button
            size="small"
            class="ob-test"
            :loading="testing"
            :disabled="!endpointFilled"
            data-ob="test"
            @click="emit('test')"
          >
            {{ t('onboarding.test') }}
          </a-button>
          <Transition name="ob-reveal">
            <span v-if="testResult" class="ob-test-line" :class="{ ok: testResult.ok }">
              <icon-check-circle v-if="testResult.ok" :size="14" />
              <icon-exclamation-circle v-else :size="14" />
              <span class="ob-test-text">{{ testResult.text }}</span>
            </span>
          </Transition>
        </div>
      </div>
    </Transition>

    <footer class="ob-foot">
      <a-button
        class="ob-ghost"
        type="text"
        :disabled="busy"
        data-ob="back"
        @click="emit('back')"
      >
        {{ t('onboarding.back') }}
      </a-button>
      <a-button
        class="ob-cta"
        type="primary"
        :loading="busy || testing"
        data-ob="next"
        @click="emit('next')"
      >
        {{ t('onboarding.next') }}
      </a-button>
    </footer>
  </div>
</template>

<style scoped>
.ob-choice {
  display: flex;
  flex-direction: column;
  gap: 18px;
  padding: var(--ob-step-pad, 32px 36px 28px);
  flex: 1;
}
.ob-head {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.ob-title {
  margin: 0;
  font-size: var(--font-size-lg);
  font-weight: 600;
  color: var(--text-primary);
}
.ob-sub {
  margin: 0;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

/* 两张卡：等高，选中态主题色描边 + 淡底 */
.ob-cards {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}
@media (max-width: 560px) {
  .ob-cards {
    grid-template-columns: 1fr;
  }
}
.ob-opt {
  display: flex;
  align-items: flex-start;
  gap: 11px;
  padding: 14px;
  border: 1px solid transparent;
  border-radius: 12px;
  background: var(--fill-1, rgba(0, 0, 0, 0.03));
  cursor: pointer;
  transition:
    background-color 160ms ease,
    border-color 160ms ease;
}
.ob-opt:hover {
  background: var(--fill-2, rgba(0, 0, 0, 0.06));
}
.ob-opt.active {
  border-color: var(--accent-text);
  /* 强调色淡底：--primary-1 是浅色档（深色主题下会变成一块白），用半透明混合两态都成立 */
  background: color-mix(in srgb, var(--accent-text) 12%, transparent);
}
.ob-opt-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  flex: none;
  border-radius: 9px;
  background: color-mix(in srgb, var(--bg-base) 70%, transparent);
  color: var(--accent-text);
}
.ob-opt-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}
.ob-opt-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}
.ob-opt-desc {
  font-size: var(--font-size-xs);
  line-height: 1.5;
  color: var(--text-tertiary);
}
/* 选中圆点：未选=空心，选中=强调色实心（macOS 单选观感） */
.ob-radio {
  width: 15px;
  height: 15px;
  flex: none;
  margin-left: auto;
  border-radius: 50%;
  border: 1.5px solid color-mix(in srgb, var(--text-tertiary) 55%, transparent);
  transition:
    border-color 160ms ease,
    background-color 160ms ease,
    box-shadow 160ms ease;
}
.ob-opt.active .ob-radio {
  border-color: var(--accent-text);
  background: var(--accent-text);
  box-shadow: inset 0 0 0 3px var(--bg-base);
}

/* 云端表单：输入框自带填充底，这里不再套一层面板 */
.ob-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 0;
}
.ob-form-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 24px;
}
.ob-test {
  flex: none;
}
.ob-test-line {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-size: var(--font-size-xs);
  color: var(--danger, #f53f3f);
}
.ob-test-line.ok {
  color: var(--success, #00b42a);
}
.ob-test-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 展开过渡：高度由 max-height 近似（内容短，观感足够）+ 淡入 */
.ob-reveal-enter-active,
.ob-reveal-leave-active {
  transition:
    opacity 200ms ease,
    transform 220ms cubic-bezier(0.32, 0.72, 0, 1);
}
.ob-reveal-enter-from,
.ob-reveal-leave-to {
  opacity: 0;
  transform: translateY(-6px);
}

.ob-foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  margin-top: auto;
  padding-top: 4px;
}
.ob-ghost {
  margin-right: auto;
  border-color: transparent !important;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}
.ob-ghost:hover:not(:disabled) {
  border-color: transparent !important;
  background: var(--fill-1) !important;
  color: var(--text-primary) !important;
}
.ob-cta {
  min-width: 96px;
  height: 32px;
  border-radius: 8px;
}
@media (prefers-reduced-motion: reduce) {
  .ob-reveal-enter-active,
  .ob-reveal-leave-active {
    transition: none;
  }
}
</style>
