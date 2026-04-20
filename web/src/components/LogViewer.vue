<script setup>
import { computed } from 'vue'
import { useLogs } from '../composables/useApi.js'

const { data: logs, loading, error } = useLogs(200, 5000)

const levelColors = {
  error: { bg: 'bg-red-500/20', text: 'text-red-400', border: 'border-red-500/30' },
  warn: { bg: 'bg-amber-500/20', text: 'text-amber-400', border: 'border-amber-500/30' },
  warning: { bg: 'bg-amber-500/20', text: 'text-amber-400', border: 'border-amber-500/30' },
  info: { bg: 'bg-blue-500/20', text: 'text-blue-400', border: 'border-blue-500/30' },
  debug: { bg: 'bg-neutral-500/20', text: 'text-neutral-400', border: 'border-neutral-500/30' },
  trace: { bg: 'bg-neutral-600/20', text: 'text-neutral-500', border: 'border-neutral-600/30' },
}

function getLevelStyle(level) {
  return levelColors[level?.toLowerCase()] || levelColors.info
}

function fmtTime(iso) {
  if (!iso) return ''
  try {
    return new Date(iso).toLocaleTimeString()
  } catch {
    return iso
  }
}

function fmtTimestamp(ms) {
  if (!ms) return ''
  try {
    return new Date(ms).toLocaleTimeString()
  } catch {
    return String(ms)
  }
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
      <span class="text-xs text-neutral-500">{{ logs?.length || 0 }} entries</span>
    </div>

    <!-- Level badges summary -->
    <div v-if="Object.keys(countByLevel).length" class="flex gap-2 flex-wrap">
      <span
        v-for="(count, level) in countByLevel"
        :key="level"
        :class="[
          'inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium',
          getLevelStyle(level).bg, getLevelStyle(level).text
        ]"
      >
        {{ level }} <span class="opacity-70">{{ count }}</span>
      </span>
    </div>

    <div v-if="loading && !logs?.length" class="text-center py-12 text-neutral-500">
      Loading logs...
    </div>
    <div v-else-if="error" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <div v-else-if="!logs?.length" class="text-center py-12 text-neutral-500">
      No log entries yet.
    </div>

    <!-- Log list -->
    <div v-else class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 overflow-hidden divide-y divide-neutral-800/50 max-h-[600px] overflow-y-auto">
      <div
        v-for="(log, i) in logs"
        :key="i"
        class="px-4 py-3 hover:bg-neutral-800/50 transition-colors"
      >
        <div class="flex items-start gap-3">
          <span
            :class="[
              'shrink-0 inline-flex px-2 py-0.5 rounded text-xs font-semibold uppercase',
              getLevelStyle(log.level).bg, getLevelStyle(log.level).text
            ]"
          >
            {{ log.level }}
          </span>
          <div class="min-w-0 flex-1">
            <p class="text-sm text-neutral-300 break-all whitespace-pre-wrap">{{ log.message }}</p>
            <p class="text-xs text-neutral-600 mt-1">
              {{ fmtTime(log.received_at) }}
              <span v-if="log.timestamp_ms" class="ml-2">device: {{ fmtTimestamp(log.timestamp_ms) }}</span>
            </p>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
