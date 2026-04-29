<script setup lang="ts">
import PendingDot from './PendingDot.vue'

const props = withDefaults(defineProps<{
  label: string
  modelValue: string
  options?: { value: string; label: string }[]
  disabled: boolean
  pending: boolean
  disabledReason?: string
}>(), {
  options: () => [],
})

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()
</script>

<template>
  <div :class="['flex flex-col sm:flex-row sm:items-center justify-between gap-2 sm:gap-0 px-4 py-3 rounded-xl transition-all', disabled ? 'opacity-40' : '']"
    :data-tooltip="disabled && disabledReason ? disabledReason : undefined">
    <div class="flex items-center gap-2">
      <span class="text-sm font-medium text-neutral-400">{{ label }}</span>
      <PendingDot v-if="pending" />
    </div>
    <div class="flex bg-neutral-800 rounded-lg p-0.5">
      <button
        v-for="opt in options"
        :key="opt.value"
        @click="!disabled && emit('update:modelValue', opt.value)"
        :class="[
          'px-3 py-1.5 rounded-md text-xs font-medium transition-colors cursor-pointer whitespace-nowrap',
          modelValue === opt.value
            ? 'bg-neutral-600 text-white shadow-sm'
            : 'text-neutral-500 hover:text-neutral-300'
        ]"
        :disabled="disabled"
      >
        {{ opt.label }}
      </button>
    </div>
  </div>
</template>
