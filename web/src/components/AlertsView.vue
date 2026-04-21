<script setup lang="ts">
import { computed, ref } from 'vue'
import { useAlerts, clearAlerts } from '../composables/useApi'
import type { TimestampedEntry } from '../types'
import LoadingSpinner from './LoadingSpinner.vue'

const { data: alerts, loading, error, refresh } = useAlerts(100, 8000)
const clearing = ref(false)

async function handleClear(): Promise<void> {
  if (clearing.value) return
  clearing.value = true
  try {
    await clearAlerts()
    await refresh()
  } catch { /* ignore */ } finally {
    clearing.value = false
  }
}

function timeAgo(iso: string): string {
  if (!iso) return ''
  const diff = (Date.now() - new Date(iso).getTime()) / 1000
  if (diff < 5) return 'just now'
  if (diff < 60) return `${Math.floor(diff)}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return new Date(iso).toLocaleDateString()
}

function parsePayload(entry: TimestampedEntry): any {
  try {
    return typeof entry.payload === 'string' ? JSON.parse(entry.payload) : entry.payload
  } catch {
    return { raw: entry.payload }
  }
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Alerts</h2>
      <div class="flex items-center gap-3">
        <span class="text-xs text-neutral-500">{{ alerts?.length || 0 }} entries</span>
        <button
          v-if="alerts?.length"
          @click="handleClear"
          :disabled="clearing"
          class="text-xs text-neutral-500 hover:text-red-400 transition-colors cursor-pointer disabled:opacity-50"
        >
          {{ clearing ? 'Clearing...' : 'Clear all' }}
        </button>
      </div>
    </div>

    <div v-if="loading && !alerts?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <LoadingSpinner class="h-8 w-8 mb-4" />
      <p class="text-sm">Loading alerts...</p>
    </div>
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">Error: {{ error }}</div>
    <div v-else-if="!alerts?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-10 h-10 mb-3 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M9 12.75L11.25 15 15 9.75M21 12a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
      <p class="text-sm">No alerts recorded</p>
      <p class="text-xs text-neutral-600 mt-1">All clear!</p>
    </div>

    <div v-else class="space-y-2 max-h-[65vh] overflow-y-auto pr-1">
      <div
        v-for="(entry, i) in alerts"
        :key="i"
        class="bg-amber-500/5 border border-amber-500/15 rounded-xl px-4 py-3 hover:bg-amber-500/10 transition-colors"
      >
        <div class="flex items-center gap-2 mb-2">
          <div class="w-6 h-6 rounded-full bg-amber-500/20 flex items-center justify-center shrink-0">
            <svg class="w-3.5 h-3.5 text-amber-400" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" /></svg>
          </div>
          <span class="text-[11px] text-amber-400/60 font-medium">{{ timeAgo(entry.received_at) }}</span>
        </div>
        <pre class="text-xs text-amber-200/70 bg-neutral-950/50 rounded-lg p-3 overflow-x-auto whitespace-pre-wrap font-mono leading-relaxed">{{ JSON.stringify(parsePayload(entry), null, 2) }}</pre>
      </div>
    </div>
  </div>
</template>
