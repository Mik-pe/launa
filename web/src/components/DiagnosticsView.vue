<script setup>
import { computed } from 'vue'
import { useDiagnostics } from '../composables/useApi.js'

const { data: diagnostics, loading, error } = useDiagnostics(100, 8000)

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
      <h2 class="text-lg font-semibold text-white">Diagnostics</h2>
      <span class="text-xs text-neutral-500">{{ diagnostics?.length || 0 }} entries</span>
    </div>

    <div v-if="loading && !diagnostics?.length" class="text-center py-12 text-neutral-500">
      Loading diagnostics...
    </div>
    <div v-else-if="error" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <div v-else-if="!diagnostics?.length" class="text-center py-12 text-neutral-500">
      No diagnostic data available.
    </div>

    <div v-else class="space-y-2">
      <div
        v-for="(entry, i) in diagnostics"
        :key="i"
        class="bg-neutral-900 rounded-xl ring-1 ring-neutral-800 px-4 py-3"
      >
        <div class="flex items-center gap-2 mb-2">
          <span class="text-neutral-500">🔧</span>
          <span class="text-xs text-neutral-500">{{ fmtTime(entry.received_at) }}</span>
        </div>
        <pre class="text-xs text-neutral-400 bg-neutral-950 rounded-lg p-3 overflow-x-auto whitespace-pre-wrap">{{ JSON.stringify(parsePayload(entry), null, 2) }}</pre>
      </div>
    </div>
  </div>
</template>
