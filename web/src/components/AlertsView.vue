<script setup>
import { computed } from 'vue'
import { useAlerts } from '../composables/useApi.js'

const { data: alerts, loading, error } = useAlerts(100, 8000)

function fmtTime(iso) {
  if (!iso) return ''
  try {
    return new Date(iso).toLocaleString()
  } catch {
    return iso
  }
}

function parsePayload(entry) {
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
      <span class="text-xs text-neutral-500">{{ alerts?.length || 0 }} entries</span>
    </div>

    <div v-if="loading && !alerts?.length" class="text-center py-12 text-neutral-500">
      Loading alerts...
    </div>
    <div v-else-if="error" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <div v-else-if="!alerts?.length" class="text-center py-12 text-neutral-500">
      No alerts recorded.
    </div>

    <div v-else class="space-y-2">
      <div
        v-for="(entry, i) in alerts"
        :key="i"
        class="bg-amber-950/30 border border-amber-800/30 rounded-xl px-4 py-3"
      >
        <div class="flex items-center gap-2 mb-1">
          <span class="text-amber-400">⚠️</span>
          <span class="text-xs text-neutral-500">{{ fmtTime(entry.received_at) }}</span>
        </div>
        <pre class="text-xs text-amber-300/80 bg-neutral-950/50 rounded-lg p-2 overflow-x-auto whitespace-pre-wrap">{{ JSON.stringify(parsePayload(entry), null, 2) }}</pre>
      </div>
    </div>
  </div>
</template>
