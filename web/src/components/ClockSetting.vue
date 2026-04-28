<script setup lang="ts">
import { ref, computed, watch } from 'vue'

const props = defineProps<{
  spaHour?: number
  spaMinute?: number
  spaTimeFormat?: '12h' | '24h'
  disabled?: boolean
}>()

const emit = defineEmits<{
  'set-time': [hour: number, minute: number, is24h: boolean]
}>()

const now = computed(() => {
  const d = new Date()
  return {
    hour: d.getHours(),
    minute: d.getMinutes(),
    formatted: `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`,
  }
})

const spaTimeStr = computed(() => {
  if (props.spaHour == null || props.spaMinute == null) return '--:--'
  return `${String(props.spaHour).padStart(2, '0')}:${String(props.spaMinute).padStart(2, '0')}`
})

function syncToNow(): void {
  emit('set-time', now.value.hour, now.value.minute, true)
}
</script>

<template>
  <div class="space-y-3">
    <h3 class="text-sm font-semibold text-neutral-300">Clock</h3>
    <div class="flex items-center gap-4">
      <div class="flex-1">
        <p class="text-[11px] text-neutral-500 uppercase tracking-wider font-medium mb-1">Spa Time</p>
        <p class="text-lg font-mono text-white tabular-nums">{{ spaTimeStr }}</p>
      </div>
      <div class="flex-1">
        <p class="text-[11px] text-neutral-500 uppercase tracking-wider font-medium mb-1">Your Time</p>
        <p class="text-lg font-mono text-neutral-400 tabular-nums">{{ now.formatted }}</p>
      </div>
    </div>
    <button
      @click="syncToNow"
      :disabled="disabled"
      class="w-full px-4 py-2 bg-neutral-800 text-neutral-300 rounded-lg text-sm font-medium hover:bg-neutral-700 transition-colors disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer flex items-center justify-center gap-2"
    >
      <svg class="w-4 h-4" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
      Sync to local time
    </button>
  </div>
</template>
