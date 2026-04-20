<script setup lang="ts">
const props = withDefaults(defineProps<{
  label: string
  modelValue: boolean
  disabled: boolean
  pending?: boolean
  readOnly?: boolean
  icon?: string
}>(), {
  pending: false,
  readOnly: false,
  icon: '',
})

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

function toggle(): void {
  if (props.disabled || props.readOnly) return
  emit('update:modelValue', !props.modelValue)
}
</script>

<template>
  <button
    @click="toggle"
    :disabled="disabled"
    :class="[
      'flex items-center justify-between w-full px-4 py-3 rounded-xl transition-all duration-200',
      readOnly
        ? 'cursor-default bg-neutral-800/50 ring-1 ring-neutral-700/50'
        : disabled
          ? 'opacity-40 cursor-not-allowed bg-neutral-800/50'
          : modelValue
            ? 'bg-blue-500/10 hover:bg-blue-500/20 ring-1 ring-blue-500/30 cursor-pointer'
            : 'bg-neutral-800 hover:bg-neutral-700/50 ring-1 ring-neutral-700 cursor-pointer'
    ]"
  >
    <div class="flex items-center gap-3">
      <span v-if="icon" class="text-lg">{{ icon }}</span>
      <span :class="['text-sm font-medium', modelValue ? 'text-blue-400' : 'text-neutral-400']">
        {{ label }}
      </span>
      <span v-if="pending" class="relative flex h-2.5 w-2.5">
        <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75" />
        <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-blue-400" />
      </span>
    </div>
    <div
      v-if="!readOnly"
      :class="[
        'relative w-10 h-[22px] rounded-full transition-colors duration-200 shrink-0',
        modelValue ? 'bg-blue-500' : 'bg-slate-300'
      ]"
    >
      <div
        :class="[
          'absolute top-[2px] w-[18px] h-[18px] bg-white rounded-full shadow-sm transition-transform duration-200',
          modelValue ? 'translate-x-[20px]' : 'translate-x-[2px]'
        ]"
      />
    </div>
    <span v-else :class="['text-xs font-medium px-2 py-0.5 rounded-full', modelValue ? 'bg-emerald-500/20 text-emerald-400' : 'bg-neutral-700 text-neutral-500']">
      {{ modelValue ? 'ON' : 'OFF' }}
    </span>
  </button>
</template>
