<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import PendingDot from './PendingDot.vue'
import type { SpaState } from '../types'

const props = withDefaults(defineProps<{
  state: SpaState | null
  pending: boolean
  disabled?: boolean
}>(), {
  disabled: false,
})

const emit = defineEmits<{
  'set-temperature': [temp: number]
}>()

const hasTemp = computed(() => {
  const t = props.state?.current_temp
  return t !== null && t !== undefined
})

const currentTemp = computed(() => props.state?.current_temp)
const setTemp = computed(() => props.state?.set_temp)
const tempScale = computed(() => props.state?.temp_scale === 'celsius' ? 'C' : 'F')
const isHeating = computed(() => props.state?.is_heating === true)
const heatingMode = computed(() => {
  const m = props.state?.heating_mode
  if (m === 'ready') return 'Ready'
  if (m === 'rest') return 'Rest'
  if (m === 'ready_in_rest') return 'Ready in Rest'
  return m || '--'
})
const tempRange = computed(() => {
  const r = props.state?.temp_range
  if (r === 'high') return 'High'
  if (r === 'low') return 'Low'
  return r || '--'
})

// Track a local target so rapid +/- presses accumulate instead of
// re-reading the stale set_temp from the ESP (which hasn't acked yet).
const localTarget = ref<number | null>(null)

// Sync local target when the spa's reported set_temp changes (i.e. ack arrived).
watch(setTemp, (v) => {
  localTarget.value = v ?? null
})

// The displayed target: local if pending, otherwise the spa's reported value.
const displayTarget = computed(() => localTarget.value ?? setTemp.value)

function adjustTemp(delta: number): void {
  if (props.disabled) return
  const isCelsius = tempScale.value === 'C'
  const isLow = props.state?.temp_range === 'low'
  const min = isCelsius ? (isLow ? 10 : 26) : (isLow ? 50 : 80)
  const max = isCelsius ? (isLow ? 26 : 40) : (isLow ? 80 : 104)
  const step = isCelsius ? 0.5 : 1
  const base = localTarget.value ?? setTemp.value ?? min
  const newTemp = Math.min(max, Math.max(min, base + delta * step))
  localTarget.value = newTemp
  emit('set-temperature', isCelsius ? newTemp : Math.round(newTemp))
}
</script>

<template>
  <div v-if="hasTemp" class="bg-neutral-900 rounded-2xl p-4 sm:p-6 text-white ring-1 ring-neutral-800">
    <div class="flex items-start justify-between mb-4">
      <div>
        <p class="text-xs text-neutral-500 uppercase tracking-widest font-medium">Water Temperature</p>
        <div class="flex items-baseline gap-1 mt-1">
          <span class="text-4xl sm:text-6xl font-light tracking-tight">
            {{ currentTemp !== null && currentTemp !== undefined ? currentTemp : '--' }}
          </span>
          <span class="text-xl sm:text-2xl text-neutral-500">°{{ tempScale }}</span>
        </div>
      </div>
      <div v-if="isHeating"
        class="flex items-center gap-1.5 bg-orange-500/20 text-orange-300 px-3 py-1.5 rounded-full text-xs font-medium">
        <PendingDot size-class="h-2 w-2" color-class="bg-orange-400" />
        Heating
      </div>
    </div>

    <div class="bg-neutral-800/50 rounded-xl p-3 sm:p-4">
      <div class="flex items-center justify-between mb-3">
        <span class="text-sm text-neutral-400">Target</span>
        <div class="flex items-center gap-2">
          <span class="text-lg sm:text-xl font-semibold">{{ displayTarget ?? '--' }}°{{ tempScale }}</span>
          <PendingDot v-if="pending" size-class="h-3 w-3" />
        </div>
      </div>
      <div class="flex gap-2">
        <button @click="adjustTemp(-1)"
          :disabled="disabled"
          :class="[
            'flex-1 rounded-lg py-3 sm:py-2.5 text-lg font-medium transition-colors select-none',
            disabled ? 'bg-neutral-700/50 text-neutral-600 cursor-not-allowed' : 'bg-neutral-700 hover:bg-neutral-600 active:bg-neutral-800 cursor-pointer'
          ]">
          −
        </button>
        <button @click="adjustTemp(1)"
          :disabled="disabled"
          :class="[
            'flex-1 rounded-lg py-3 sm:py-2.5 text-lg font-medium transition-colors select-none',
            disabled ? 'bg-blue-600/50 text-blue-300/50 cursor-not-allowed' : 'bg-blue-600 hover:bg-blue-500 active:bg-blue-700 cursor-pointer'
          ]">
          +
        </button>
      </div>
      <div class="flex gap-4 mt-3 text-xs text-neutral-500">
        <span>Mode: <span class="text-neutral-300">{{ heatingMode }}</span></span>
        <span>Range: <span class="text-neutral-300">{{ tempRange }}</span></span>
      </div>
    </div>
  </div>
  <div v-else class="bg-neutral-900 rounded-2xl p-4 sm:p-6 text-white ring-1 ring-neutral-800 text-center">
    <p class="text-xs text-neutral-500 uppercase tracking-widest font-medium mb-2">Water Temperature</p>
    <p class="text-neutral-600 text-sm">Waiting for spa connection...</p>
  </div>
</template>
