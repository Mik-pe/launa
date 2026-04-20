<script setup>
import { computed } from 'vue'
import { useSniffFrames } from '../composables/useApi.js'

const { data: frames, loading, error } = useSniffFrames(100, 8000)

function timeAgo(iso) {
  if (!iso) return ''
  const diff = (Date.now() - new Date(iso).getTime()) / 1000
  if (diff < 5) return 'just now'
  if (diff < 60) return `${Math.floor(diff)}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return new Date(iso).toLocaleDateString()
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

    <div v-if="loading && !frames?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="animate-spin h-8 w-8 mb-4 text-blue-400" fill="none" viewBox="0 0 24 24">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
      </svg>
      <p class="text-sm">Loading frames...</p>
    </div>
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">Error: {{ error }}</div>
    <div v-else-if="!frames?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-10 h-10 mb-3 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M9.348 14.652a3.75 3.75 0 0 1 0-5.304m5.304 0a3.75 3.75 0 0 1 0 5.304m-7.425 2.121a6.75 6.75 0 0 1 0-9.546m9.546 0a6.75 6.75 0 0 1 0 9.546M5.106 18.894c-3.808-3.807-3.808-9.98 0-13.788m13.788 0c3.808 3.807 3.808 9.98 0 13.788M12 12h.008v.008H12V12Zm.375 0a.375.375 0 1 1-.75 0 .375.375 0 0 1 .75 0Z" /></svg>
      <p class="text-sm">No sniff frames captured</p>
      <p class="text-xs text-neutral-600 mt-1">Enable sniff mode in Settings</p>
    </div>

    <div v-else class="space-y-1.5 max-h-[65vh] overflow-y-auto pr-1">
      <div
        v-for="(entry, i) in frames"
        :key="i"
        class="bg-neutral-900 rounded-xl ring-1 ring-neutral-800 px-4 py-3 hover:bg-neutral-800/60 transition-colors border-l-[3px] border-l-cyan-500/50"
      >
        <div class="flex items-center gap-3 mb-2">
          <span class="text-[10px] text-cyan-400/60 font-mono font-bold">#{{ String(frames.length - i).padStart(3, '0') }}</span>
          <span class="text-[11px] text-neutral-600">{{ timeAgo(entry.received_at) }}</span>
        </div>
        <pre class="text-xs text-cyan-300/70 font-mono bg-neutral-950/60 rounded-lg p-3 overflow-x-auto whitespace-pre leading-relaxed">{{ JSON.stringify(parsePayload(entry), null, 2) }}</pre>
      </div>
    </div>
  </div>
</template>
