<script setup>
import { computed } from 'vue'
import { useLatestStatus } from '../composables/useApi'

const { data: latest, loading, error, refresh } = useLatestStatus(5000)

const status = computed(() => {
  if (!latest.value?.payload) return null
  try {
    return typeof latest.value.payload === 'string'
      ? JSON.parse(latest.value.payload)
      : latest.value.payload
  } catch {
    return null
  }
})

const receivedAt = computed(() => latest.value?.received_at || null)

function fmtTime(iso) {
  if (!iso) return '--'
  try {
    return new Date(iso).toLocaleTimeString()
  } catch {
    return iso
  }
}

function timeAgo(iso) {
  if (!iso) return ''
  const diff = (Date.now() - new Date(iso).getTime()) / 1000
  if (diff < 5) return 'just now'
  if (diff < 60) return `${Math.floor(diff)}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  return `${Math.floor(diff / 3600)}h ago`
}

const scale = computed(() => status.value?.temp_scale === 'celsius' ? 'C' : 'F')

const components = computed(() => {
  if (!status.value) return []
  const s = status.value
  return [
    { label: 'Pump 1', on: !!s.pump1_on, icon: 'pump' },
    { label: 'Pump 2', on: !!s.pump2_on, icon: 'pump' },
    { label: 'Pump 3', on: !!s.pump3_on, icon: 'pump' },
    { label: 'Pump 4', on: !!s.pump4_on, icon: 'pump' },
    { label: 'Pump 5', on: !!s.pump5_on, icon: 'pump' },
    { label: 'Pump 6', on: !!s.pump6_on, icon: 'pump' },
    { label: 'Light 1', on: !!s.light1, icon: 'light' },
    { label: 'Light 2', on: !!s.light2, icon: 'light' },
    { label: 'Blower', on: !!s.blower, icon: 'blower' },
    { label: 'Circ Pump', on: !!s.circ_pump, icon: 'pump' },
    { label: 'Mister', on: !!s.mister, icon: 'mister' },
    { label: 'Hold Mode', on: !!s.hold_mode, icon: 'hold' },
  ]
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
    <!-- Loading -->
    <div v-if="loading && !status" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="animate-spin h-8 w-8 mb-4 text-blue-400" fill="none" viewBox="0 0 24 24">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
      </svg>
      <p class="text-sm">Loading status...</p>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">
      <p class="font-medium">Connection Error</p>
      <p class="text-red-400/70 mt-1">{{ error }}</p>
    </div>

    <!-- No data -->
    <div v-else-if="!status" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-12 h-12 mb-4 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M8.288 15.038a5.25 5.25 0 0 1 7.424 0M5.106 11.856c3.807-3.808 9.98-3.808 13.788 0M1.924 8.674c5.565-5.565 14.587-5.565 20.152 0M12.53 18.22l-.53.53-.53-.53a.75.75 0 0 1 1.06 0Z" /></svg>
      <p class="text-sm">Waiting for device to publish status...</p>
    </div>

    <template v-else>
      <!-- Temperature hero card (only shown when spa is reporting a temperature) -->
      <div v-if="status.current_temp != null" class="relative overflow-hidden rounded-2xl ring-1 ring-neutral-800">
        <div class="absolute inset-0 bg-gradient-to-br from-blue-600/10 via-transparent to-orange-500/10" />
        <div class="relative bg-neutral-900/80 backdrop-blur px-6 py-8 text-center">
          <p class="text-[11px] text-neutral-500 uppercase tracking-[0.2em] font-semibold mb-3">Water Temperature</p>
          <div class="flex items-baseline justify-center gap-1.5">
            <span class="text-6xl sm:text-7xl font-extralight text-white tabular-nums tracking-tight">
              {{ status.current_temp }}
            </span>
            <span class="text-2xl text-neutral-500 font-light">°{{ scale }}</span>
          </div>
          <div class="mt-4 flex items-center justify-center gap-6">
            <div class="flex items-center gap-2 text-sm text-neutral-400">
              <svg class="w-4 h-4 text-blue-400" fill="none" stroke="currentColor" stroke-width="1.5" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M15 11.25l-3-3m0 0l-3 3m3-3v7.5M21 12a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
              <span>Target <span class="text-white font-medium">{{ status.set_temp ?? '--' }}°</span></span>
            </div>
            <div v-if="status.is_heating" class="flex items-center gap-2 text-sm">
              <span class="relative flex h-2.5 w-2.5">
                <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-orange-400 opacity-75" />
                <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-orange-400" />
              </span>
              <span class="text-orange-400 font-medium">Heating</span>
            </div>
            <div v-else class="flex items-center gap-2 text-sm text-neutral-500">
              <span class="w-2.5 h-2.5 rounded-full bg-neutral-600" />
              <span>Idle</span>
            </div>
          </div>
          <p class="text-[11px] text-neutral-600 mt-4">
            Updated {{ timeAgo(receivedAt) }}
          </p>
        </div>
      </div>

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
