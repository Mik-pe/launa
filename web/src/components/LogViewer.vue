<script setup>
import { computed, ref } from 'vue'
import { useLogs } from '../composables/useApi'

const { data: logs, loading, error } = useLogs(200, 5000)

const activeFilters = ref(new Set())

const levels = ['error', 'warn', 'info', 'debug']

const levelStyles = {
  error: { border: 'border-l-red-500', bg: 'bg-red-500/10', text: 'text-red-400', dot: 'bg-red-500' },
  warn: { border: 'border-l-amber-500', bg: 'bg-amber-500/10', text: 'text-amber-400', dot: 'bg-amber-500' },
  warning: { border: 'border-l-amber-500', bg: 'bg-amber-500/10', text: 'text-amber-400', dot: 'bg-amber-500' },
  info: { border: 'border-l-blue-500', bg: 'bg-blue-500/10', text: 'text-blue-400', dot: 'bg-blue-500' },
  debug: { border: 'border-l-neutral-500', bg: 'bg-neutral-500/10', text: 'text-neutral-400', dot: 'bg-neutral-500' },
}

function getStyle(level) {
  return levelStyles[level?.toLowerCase()] || levelStyles.info
}

function toggleFilter(level) {
  const s = new Set(activeFilters.value)
  if (s.has(level)) s.delete(level)
  else s.add(level)
  activeFilters.value = s
}

const filteredLogs = computed(() => {
  const filters = activeFilters.value
  if (filters.size === 0) return logs.value || []
  return (logs.value || []).filter(l => filters.has((l.level || 'info').toLowerCase()))
})

function timeAgo(iso) {
  if (!iso) return ''
  const diff = (Date.now() - new Date(iso).getTime()) / 1000
  if (diff < 5) return 'just now'
  if (diff < 60) return `${Math.floor(diff)}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return new Date(iso).toLocaleDateString()
}

function fmtTimestamp(ms) {
  if (!ms) return ''
  try { return new Date(ms).toLocaleTimeString() } catch { return String(ms) }
}

const countByLevel = computed(() => {
  const counts = {}
  for (const log of (logs.value || [])) {
    const l = (log.level || 'info').toLowerCase()
    counts[l] = (counts[l] || 0) + 1
  }
  return counts
})
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Device Logs</h2>
      <span class="text-xs text-neutral-500">{{ filteredLogs.length }} / {{ logs?.length || 0 }}</span>
    </div>

    <!-- Filter bar -->
    <div class="flex items-center gap-2 flex-wrap">
      <button
        v-for="level in levels"
        :key="level"
        @click="toggleFilter(level)"
        :class="[
          'inline-flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-medium transition-all duration-200 cursor-pointer ring-1',
          activeFilters.has(level)
            ? getStyle(level).bg + ' ' + getStyle(level).text + ' ring-current/30'
            : 'bg-neutral-900 text-neutral-500 ring-neutral-800 hover:text-neutral-300'
        ]"
      >
        <span :class="['w-1.5 h-1.5 rounded-full', getStyle(level).dot, activeFilters.has(level) ? 'opacity-100' : 'opacity-40']" />
        {{ level }}
        <span class="opacity-60">{{ countByLevel[level] || 0 }}</span>
      </button>
      <span v-if="activeFilters.size" class="text-xs text-neutral-600">filtered</span>
    </div>

    <div v-if="loading && !logs?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="animate-spin h-8 w-8 mb-4 text-blue-400" fill="none" viewBox="0 0 24 24">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
      </svg>
      <p class="text-sm">Loading logs...</p>
    </div>
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">Error: {{ error }}</div>
    <div v-else-if="!filteredLogs.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-10 h-10 mb-3 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="m6.75 7.5 3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0 0 21 18V6a2.25 2.25 0 0 0-2.25-2.25H5.25A2.25 2.25 0 0 0 3 6v12a2.25 2.25 0 0 0 2.25 2.25Z" /></svg>
      <p class="text-sm">{{ activeFilters.size ? 'No logs match the filter' : 'No log entries yet' }}</p>
    </div>

    <!-- Log list -->
    <div v-else class="space-y-1.5 max-h-[65vh] overflow-y-auto pr-1">
      <div
        v-for="(log, i) in filteredLogs"
        :key="i"
        :class="[
          'bg-neutral-900 rounded-xl px-4 py-3 border-l-[3px] transition-colors hover:bg-neutral-800/70',
          getStyle(log.level).border
        ]"
      >
        <div class="flex items-start justify-between gap-3">
          <p class="text-sm text-neutral-300 break-all whitespace-pre-wrap flex-1 leading-relaxed">{{ log.message }}</p>
          <span :class="[
            'shrink-0 text-[10px] font-bold uppercase tracking-wider px-2 py-0.5 rounded-md',
            getStyle(log.level).bg + ' ' + getStyle(log.level).text
          ]">{{ log.level }}</span>
        </div>
        <div class="flex items-center gap-3 mt-1.5 text-[11px] text-neutral-600">
          <span>{{ timeAgo(log.received_at) }}</span>
          <span v-if="log.timestamp_ms" class="text-neutral-700">device: {{ fmtTimestamp(log.timestamp_ms) }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
