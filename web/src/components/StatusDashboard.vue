<script setup lang="ts">
import { computed } from 'vue'
import type { SpaState } from '../types'
import LoadingSpinner from './LoadingSpinner.vue'

const props = withDefaults(defineProps<{
  spaState: SpaState | null
  connected?: boolean
  visibleControls?: Record<string, boolean>
}>(), {
  spaState: null,
  connected: false,
  visibleControls: () => ({}),
})

const status = computed(() => props.spaState)

const components = computed(() => {
  if (!status.value) return []
  const s = status.value
  const vc = props.visibleControls
  const items: { label: string; on: boolean; icon: string }[] = []
  for (let i = 1; i <= 6; i++) {
    if (vc['pump' + i] !== false) {
      items.push({ label: `Pump ${i}`, on: !!s[`pump${i}_on` as keyof SpaState], icon: 'pump' })
    }
  }
  if (vc['light1'] !== false) items.push({ label: 'Light 1', on: !!s.light1, icon: 'light' })
  if (vc['light2'] !== false) items.push({ label: 'Light 2', on: !!s.light2, icon: 'light' })
  if (vc['blower'] !== false) items.push({ label: 'Blower', on: !!s.blower, icon: 'blower' })
  items.push({ label: 'Circ Pump', on: !!s.circ_pump, icon: 'pump' })
  if (vc['mister'] !== false) items.push({ label: 'Mister', on: !!s.mister, icon: 'mister' })
  items.push({ label: 'Hold Mode', on: !!s.hold_mode, icon: 'hold' })
  return items
})

const infoRows = computed(() => {
  if (!status.value) return []
  const s = status.value
  return [
    { label: 'Heat Mode', value: (s.heating_mode || '--').replace(/_/g, ' '), capitalize: true },
    { label: 'Temp Range', value: s.temp_range || '--', capitalize: true },
    { label: 'Time', value: `${String(s.hour ?? '--').padStart(2, '0')}:${String(s.minute ?? '--').padStart(2, '0')}` },
    { label: 'Panel Lock', value: s.panel_locked ? 'Locked' : 'Unlocked' },
    { label: 'Firmware', value: s.firmware_version || '--' },
  ]
})
</script>

<template>
  <div class="space-y-5">
    <!-- Not connected -->
    <div v-if="!connected" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <LoadingSpinner class="h-8 w-8 mb-4" />
      <p class="text-sm">Connecting...</p>
    </div>

    <!-- No data -->
    <div v-else-if="!status" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-12 h-12 mb-4 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M8.288 15.038a5.25 5.25 0 0 1 7.424 0M5.106 11.856c3.807-3.808 9.98-3.808 13.788 0M1.924 8.674c5.565-5.565 14.587-5.565 20.152 0M12.53 18.22l-.53.53-.53-.53a.75.75 0 0 1 1.06 0Z" /></svg>
      <p class="text-sm">Waiting for device to publish status...</p>
    </div>

    <template v-else>
      <!-- Info grid -->
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
        <div
          v-for="row in infoRows"
          :key="row.label"
          class="bg-neutral-900 rounded-xl ring-1 ring-neutral-800 px-4 py-3"
        >
          <p class="text-[11px] text-neutral-500 uppercase tracking-wider font-medium mb-1">{{ row.label }}</p>
          <p class="text-sm text-white font-medium" :class="{ 'capitalize': row.capitalize }">{{ row.value }}</p>
        </div>
      </div>

      <!-- Component status -->
      <div>
        <h3 class="text-[11px] font-semibold text-neutral-500 uppercase tracking-[0.15em] mb-3 px-1">Components</h3>
        <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
          <div
            v-for="comp in components"
            :key="comp.label"
            :class="[
              'relative flex items-center gap-3 px-4 py-3 rounded-xl transition-all duration-300',
              comp.on
                ? 'bg-emerald-500/10 ring-1 ring-emerald-500/25 shadow-sm shadow-emerald-500/5'
                : 'bg-neutral-900 ring-1 ring-neutral-800'
            ]"
          >
            <span :class="[
              'w-2 h-2 rounded-full shrink-0 transition-colors duration-300',
              comp.on ? 'bg-emerald-400 shadow-sm shadow-emerald-400/50' : 'bg-neutral-700'
            ]" />
            <span :class="[
              'text-sm font-medium transition-colors duration-300',
              comp.on ? 'text-emerald-400' : 'text-neutral-500'
            ]">{{ comp.label }}</span>
          </div>
        </div>
      </div>

      <!-- Fault -->
      <div v-if="status.last_fault" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4">
        <div class="flex items-center gap-3">
          <div class="w-8 h-8 rounded-full bg-red-500/20 flex items-center justify-center shrink-0">
            <svg class="w-4 h-4 text-red-400" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" /></svg>
          </div>
          <div>
            <p class="text-xs font-semibold text-red-400 uppercase tracking-wider">Last Fault</p>
            <p class="text-sm text-red-300/80 mt-0.5">{{ status.last_fault }}</p>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
