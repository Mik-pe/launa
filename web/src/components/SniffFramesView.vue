<script setup lang="ts">
import { ref } from 'vue'
import { useSniffFrames } from '../composables/useApi'
import { timeAgo, parsePayload } from '../utils/format'
import LoadingSpinner from './LoadingSpinner.vue'

const { data: frames, loading, error, refresh } = useSniffFrames(100, 8000)

const props = defineProps<{
  sniffEnabled?: boolean
  sniffPending?: boolean
}>()

const emit = defineEmits<{
  'capture': [frameCount: number]
  'stop': []
}>()

const frameCountInput = ref(10)
const captureInProgress = ref(false)

function startCapture(): void {
  const n = Math.max(1, Math.min(200, frameCountInput.value || 10))
  frameCountInput.value = n
  captureInProgress.value = true
  emit('capture', n)
  // Auto-clear in-progress and refresh after a reasonable wait
  setTimeout(() => {
    captureInProgress.value = false
    refresh()
  }, 8000)
}

function stopCapture(): void {
  captureInProgress.value = false
  emit('stop')
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Sniff Frames</h2>
      <span class="text-xs text-neutral-500">{{ frames?.length || 0 }} captures</span>
    </div>

    <!-- Capture controls -->
    <div class="bg-neutral-900 rounded-xl ring-1 ring-neutral-800 p-4">
      <div class="flex items-center gap-3">
        <div class="flex items-center gap-2">
          <label class="text-sm text-neutral-400 whitespace-nowrap">Capture</label>
          <input
            v-model.number="frameCountInput"
            type="number"
            min="1"
            max="200"
            :disabled="captureInProgress || sniffPending"
            class="w-20 px-2.5 py-1.5 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white text-center focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none disabled:opacity-40"
          />
          <span class="text-sm text-neutral-500">frames</span>
        </div>
        <button
          v-if="!captureInProgress && !sniffEnabled"
          @click="startCapture"
          :disabled="sniffPending"
          class="px-4 py-1.5 bg-blue-600 text-white text-sm font-medium rounded-lg hover:bg-blue-500 transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed whitespace-nowrap"
        >
          Start Capture
        </button>
        <button
          v-else
          @click="stopCapture"
          class="px-4 py-1.5 bg-red-600/80 text-white text-sm font-medium rounded-lg hover:bg-red-500 transition-colors cursor-pointer whitespace-nowrap"
        >
          Stop
        </button>
        <span v-if="captureInProgress || sniffEnabled" class="flex items-center gap-1.5 text-xs text-amber-400">
          <span class="w-2 h-2 bg-amber-400 rounded-full animate-pulse"></span>
          Capturing...
        </span>
      </div>
      <p class="text-xs text-neutral-600 mt-2">Captures raw RS-485 frames for diagnostics. Each frame includes timestamp and decoded payload.</p>
    </div>

    <div v-if="loading && !frames?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <LoadingSpinner class="h-8 w-8 mb-4" />
      <p class="text-sm">Loading frames...</p>
    </div>
    <div v-else-if="error" class="bg-red-500/10 border border-red-500/20 rounded-2xl px-5 py-4 text-sm text-red-400">Error: {{ error }}</div>
    <div v-else-if="!frames?.length" class="flex flex-col items-center justify-center py-20 text-neutral-500">
      <svg class="w-10 h-10 mb-3 text-neutral-700" fill="none" stroke="currentColor" stroke-width="1" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M9.348 14.652a3.75 3.75 0 0 1 0-5.304m5.304 0a3.75 3.75 0 0 1 0 5.304m-7.425 2.121a6.75 6.75 0 0 1 0-9.546m9.546 0a6.75 6.75 0 0 1 0 9.546M5.106 18.894c-3.808-3.807-3.808-9.98 0-13.788m13.788 0c3.808 3.807 3.808 9.98 0 13.788M12 12h.008v.008H12V12Zm.375 0a.375.375 0 1 1-.75 0 .375.375 0 0 1 .75 0Z" /></svg>
      <p class="text-sm">No sniff frames captured</p>
      <p class="text-xs text-neutral-600 mt-1">Use the capture control above to start</p>
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
