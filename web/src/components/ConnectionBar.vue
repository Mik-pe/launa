<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  connected: boolean
  connecting: boolean
  availability: string
  brokerUrl: string
  deviceId: string
  connectionError: string | null
  initialConnect: boolean
}>()

const emit = defineEmits<{
  'open-settings': []
}>()

const statusColor = computed(() => {
  if (props.connecting) {
    return props.initialConnect ? 'bg-amber-400' : 'bg-blue-400'
  }
  if (!props.connected) return 'bg-neutral-600'
  if (props.availability === 'online') return 'bg-emerald-400'
  if (props.availability === 'reconnecting') return 'bg-amber-400'
  return 'bg-amber-400'
})

const statusText = computed(() => {
  if (props.connecting) {
    return props.initialConnect ? 'Reconnecting...' : 'Connecting...'
  }
  if (!props.connected) return 'Disconnected'
  if (props.availability === 'online') return 'Online'
  if (props.availability === 'reconnecting') return 'Reconnecting...'
  return 'Offline'
})
</script>

<template>
  <header class="bg-neutral-900 text-white px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between gap-3">
    <div class="flex items-center gap-3 min-w-0">
      <div class="w-9 h-9 sm:w-10 sm:h-10 rounded-xl bg-gradient-to-br from-blue-500 to-cyan-400 flex items-center justify-center text-base sm:text-lg font-bold shadow-lg shadow-blue-500/20 shrink-0">
        <svg v-if="connecting" class="animate-spin h-5 w-5 text-white" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
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
      <div v-if="connectionError" class="flex items-center gap-1 text-xs text-red-400 max-w-[200px]">
        <svg class="w-4 h-4 shrink-0 sm:hidden" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 3.75h.008v.008H12v-.008z" /></svg>
        <span class="truncate hidden sm:inline max-w-[200px]" :title="connectionError">{{ connectionError }}</span>
      </div>
      <div class="flex items-center gap-2 text-sm">
        <span class="relative flex h-2.5 w-2.5">
          <span v-if="connected && availability === 'online'"
            class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75" />
          <span :class="['relative inline-flex rounded-full h-2.5 w-2.5', statusColor]" />
        </span>
        <span class="text-neutral-400 text-xs hidden sm:inline">{{ statusText }}</span>
      </div>
      <button @click="emit('open-settings')"
        class="w-8 h-8 flex items-center justify-center rounded-lg text-neutral-500 hover:text-neutral-300 hover:bg-neutral-800 transition-colors cursor-pointer"
        title="Settings">
        ⚙️
      </button>
    </div>
  </header>
</template>
