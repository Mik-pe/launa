<script setup lang="ts">
import { computed } from 'vue'
import type { ConnectionInfo } from '../composables/useMqtt'
import type { SpaState } from '../types'

const props = defineProps<{
  connectionInfo: ConnectionInfo
  brokerUrl: string
  deviceId: string
  spaState: SpaState | null
}>()

const emit = defineEmits<{
  'open-settings': []
}>()

// RSSI signal strength: 0-4 bars, or -1 when connected but no RSSI data yet
// Excellent: >= -50, Good: >= -60, Fair: >= -70, Weak: >= -80, None: < -80
const wifiBars = computed(() => {
  const rssi = props.spaState?.wifi_rssi
  if (rssi == null) {
    return props.connectionInfo.status === 'online' ? -1 : null
  }
  if (rssi >= -50) return 4
  if (rssi >= -60) return 3
  if (rssi >= -70) return 2
  if (rssi >= -80) return 1
  return 0
})

const wifiColor = computed(() => {
  if (wifiBars.value === null) return 'text-neutral-600'
  if (wifiBars.value === -1) return 'text-neutral-500'
  if (wifiBars.value >= 3) return 'text-emerald-400'
  if (wifiBars.value >= 2) return 'text-amber-400'
  return 'text-red-400'
})

const wifiTooltip = computed(() => {
  if (wifiBars.value === null) return ''
  if (wifiBars.value === -1) return 'WiFi signal unknown'
  const rssi = props.spaState?.wifi_rssi
  const label = wifiBars.value >= 4 ? 'Excellent' : wifiBars.value >= 3 ? 'Good' : wifiBars.value >= 2 ? 'Fair' : wifiBars.value >= 1 ? 'Weak' : 'No signal'
  return `WiFi: ${label} (${rssi} dBm)`
})
</script>

<template>
  <header class="bg-neutral-900 text-white px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between gap-3">
    <div class="flex items-center gap-3 min-w-0">
      <div class="w-9 h-9 sm:w-10 sm:h-10 rounded-xl bg-gradient-to-br from-blue-500 to-cyan-400 flex items-center justify-center text-base sm:text-lg font-bold shadow-lg shadow-blue-500/20 shrink-0">
        <svg v-if="connectionInfo.status === 'connecting' || connectionInfo.status === 'reconnecting'" class="animate-spin h-5 w-5 text-white" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
        </svg>
        <span v-else>L</span>
      </div>
      <div class="min-w-0">
        <h1 class="text-base sm:text-lg font-semibold tracking-tight truncate">Launa Spa</h1>
        <p class="text-xs text-neutral-500 truncate">{{ deviceId }}</p>
      </div>
    </div>

    <div class="flex items-center gap-3 sm:gap-4 shrink-0">
      <div v-if="connectionInfo.error" class="flex items-center gap-1 text-xs text-red-400 max-w-[200px]">
        <svg class="w-4 h-4 shrink-0 sm:hidden" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 3.75h.008v.008H12v-.008z" /></svg>
        <span class="truncate hidden sm:inline max-w-[200px]" :title="connectionInfo.error">{{ connectionInfo.error }}</span>
      </div>
      <div class="flex items-center gap-2 text-sm"
        :title="connectionInfo.tooltip">
        <!-- WiFi signal icon -->
        <svg v-if="wifiBars !== null"
          :class="['w-4 h-4', wifiColor]"
          :title="wifiTooltip"
          viewBox="0 0 24 24" fill="currentColor">
          <circle cx="12" cy="19" r="1.5" />
          <path v-if="wifiBars >= 1" d="M8.46 14.54a5 5 0 017.08 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          <path v-if="wifiBars >= 2" d="M5.64 11.64a9 9 0 0112.72 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          <path v-if="wifiBars >= 3" d="M2.81 8.81a13 13 0 0118.38 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          <path v-if="wifiBars >= 4" d="M.34 5.66a17 17 0 0123.32 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          <template v-if="wifiBars === -1">
            <path d="M8.46 14.54a5 5 0 017.08 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
            <path d="M5.64 11.64a9 9 0 0112.72 0" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          </template>
        </svg>
        <span class="relative flex h-2.5 w-2.5">
          <span v-if="connectionInfo.status === 'online'"
            class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75" />
          <span :class="['relative inline-flex rounded-full h-2.5 w-2.5', connectionInfo.color]" />
        </span>
        <span class="text-neutral-400 text-xs hidden sm:inline">{{ connectionInfo.label }}</span>
      </div>
      <button @click="emit('open-settings')"
        class="w-8 h-8 flex items-center justify-center rounded-lg text-neutral-500 hover:text-neutral-300 hover:bg-neutral-800 transition-colors cursor-pointer"
        title="Settings">
        ⚙️
      </button>
    </div>
  </header>
</template>
