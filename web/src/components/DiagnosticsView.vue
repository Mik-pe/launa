<script setup lang="ts">
import { computed } from 'vue'
import { useDiagnostics } from '../composables/useApi'
import type { TimestampedEntry } from '../types'

const { data: diagnostics, loading, error } = useDiagnostics(50, 8000)

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

const latest = computed<TimestampedEntry | null>(() => diagnostics.value?.[0] || null)
const latestParsed = computed(() => latest.value ? parsePayload(latest.value) : null)

function isObject(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}
</script>

<template>
  <div class="space-y-5">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Diagnostics</h2>
      <span class="text-xs text-neutral-500">{{ diagnostics?.length || 0 }} entries</span>
    </div>

    <div v-if="loading && !diagnostics?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="animate-spin h-8 w-8 mb-4 text-blue-400" fill="none" viewBox="0 0 24 24">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
      </svg>
      <p class="text-sm">Loading diagnostics...</p>
    </div>
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">Error: {{ error }}</div>
    <div v-else-if="!diagnostics?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-10 h-10 mb-3 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M8.25 3v1.5M4.5 8.25H3m18 0h-1.5M4.5 12H3m18 0h-1.5m-15 3.75H3m18 0h-1.5M8.25 19.5V21M12 3v1.5m0 15V21m3.75-18v1.5m0 15V21m-9-1.5h10.5a2.25 2.25 0 0 0 2.25-2.25V6.75a2.25 2.25 0 0 0-2.25-2.25H6.75A2.25 2.25 0 0 0 4.5 6.75v10.5a2.25 2.25 0 0 0 2.25 2.25Z" /></svg>
      <p class="text-sm">No diagnostic data available</p>
    </div>

    <template v-else>
      <!-- Latest diagnostic: key-value grid -->
      <div v-if="latestParsed && isObject(latestParsed)" class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 overflow-hidden">
        <div class="px-5 py-3 border-b border-neutral-800 flex items-center justify-between">
          <p class="text-[11px] text-neutral-500 uppercase tracking-wider font-semibold">Latest Snapshot</p>
          <p class="text-[11px] text-neutral-600">{{ timeAgo(latest?.received_at ?? '') }}</p>
        </div>
        <div class="divide-y divide-neutral-800/50">
          <div
            v-for="(value, key) in latestParsed"
            :key="key"
            class="flex items-center justify-between px-5 py-3"
          >
            <span class="text-sm text-neutral-400 font-medium">{{ key }}</span>
            <span :class="[
              'text-sm font-medium tabular-nums',
              typeof value === 'number' ? 'text-blue-400' : typeof value === 'boolean' ? (value ? 'text-emerald-400' : 'text-neutral-500') : 'text-white'
            ]">{{ typeof value === 'boolean' ? (value ? 'true' : 'false') : value }}</span>
          </div>
        </div>
      </div>

      <!-- Raw fallback for non-object payloads -->
      <div v-else-if="latestParsed" class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 p-5">
        <pre class="text-xs text-neutral-400 font-mono leading-relaxed whitespace-pre-wrap">{{ JSON.stringify(latestParsed, null, 2) }}</pre>
      </div>

      <!-- History list (collapsed) -->
      <details v-if="diagnostics.length > 1" class="group">
        <summary class="cursor-pointer text-xs text-neutral-600 hover:text-neutral-400 transition-colors px-1 py-2 flex items-center gap-1.5">
          <svg class="w-3 h-3 transition-transform group-open:rotate-90" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="m8.25 4.5 7.5 7.5-7.5 7.5" /></svg>
          Show {{ diagnostics.length - 1 }} older entries
        </summary>
        <div class="mt-2 space-y-1.5 max-h-[40vh] overflow-y-auto">
          <div
            v-for="(entry, i) in diagnostics.slice(1)"
            :key="i"
            class="bg-neutral-900 rounded-xl ring-1 ring-neutral-800 px-4 py-3"
          >
            <p class="text-[11px] text-neutral-600 mb-2">{{ timeAgo(entry.received_at) }}</p>
            <pre class="text-xs text-neutral-500 font-mono leading-relaxed whitespace-pre-wrap">{{ JSON.stringify(parsePayload(entry), null, 2) }}</pre>
          </div>
        </div>
      </details>
    </template>
  </div>
</template>
