<script setup>
import { computed } from 'vue'
import { useSniffFrames } from '../composables/useApi.js'

const { data: frames, loading, error } = useSniffFrames(100, 8000)

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
      <h2 class="text-lg font-semibold text-white">Sniff Frames</h2>
      <span class="text-xs text-neutral-500">{{ frames?.length || 0 }} frames</span>
    </div>

    <div v-if="loading && !frames?.length" class="text-center py-12 text-neutral-500">
      Loading frames...
    </div>
    <div v-else-if="error" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <div v-else-if="!frames?.length" class="text-center py-12 text-neutral-500">
      No sniff frames captured. Enable sniff mode in Settings.
    </div>

    <div v-else class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 overflow-hidden divide-y divide-neutral-800/50 max-h-[600px] overflow-y-auto">
      <div
        v-for="(entry, i) in frames"
        :key="i"
        class="px-4 py-3 hover:bg-neutral-800/50 transition-colors"
      >
        <div class="flex items-center gap-2 mb-1">
          <span class="text-neutral-500 text-xs">#{{ frames.length - i }}</span>
          <span class="text-xs text-neutral-600">{{ fmtTime(entry.received_at) }}</span>
        </div>
        <pre class="text-xs text-cyan-400/80 bg-neutral-950 rounded-lg p-2 overflow-x-auto whitespace-pre-wrap font-mono">{{ JSON.stringify(parsePayload(entry), null, 2) }}</pre>
      </div>
    </div>
  </div>
</template>
