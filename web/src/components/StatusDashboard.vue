<script setup>
import { computed } from 'vue'
import { useLatestStatus } from '../composables/useApi.js'

const { data: latest, loading, error, refresh } = useLatestStatus(5000)

const status = computed(() => {
  if (!latest.value?.payload) return null
  try {
    return typeof latest.value.payload === 'string'
      ? JSON.parse(latest.value.payload)
      : latest.value.payload
  } catch {
    return null
  }
})

const receivedAt = computed(() => latest.value?.received_at || null)

function fmtTime(iso) {
  if (!iso) return '--'
  try {
    return new Date(iso).toLocaleTimeString()
  } catch {
    return iso
  }
}

const boolFields = [
  { key: 'is_heating', label: 'Heating' },
  { key: 'pump1_on', label: 'Pump 1' },
  { key: 'pump2_on', label: 'Pump 2' },
  { key: 'pump3_on', label: 'Pump 3' },
  { key: 'pump4_on', label: 'Pump 4' },
  { key: 'pump5_on', label: 'Pump 5' },
  { key: 'pump6_on', label: 'Pump 6' },
  { key: 'light1', label: 'Light 1' },
  { key: 'light2', label: 'Light 2' },
  { key: 'light3', label: 'Light 3' },
  { key: 'light4', label: 'Light 4' },
  { key: 'blower', label: 'Blower' },
  { key: 'circ_pump', label: 'Circ Pump' },
  { key: 'mister', label: 'Mister' },
  { key: 'hold_mode', label: 'Hold Mode' },
]

const valueFields = [
  { key: 'current_temp', label: 'Current Temp', suffix: '' },
  { key: 'set_temp', label: 'Set Temp', suffix: '' },
  { key: 'heating_mode', label: 'Heat Mode' },
  { key: 'temp_range', label: 'Temp Range' },
  { key: 'temp_scale', label: 'Scale' },
]
</script>

<template>
  <div class="space-y-4">
    <!-- Header -->
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold text-white">Live Status</h2>
      <span class="text-xs text-neutral-500">
        {{ receivedAt ? 'Updated ' + fmtTime(receivedAt) : 'No data' }}
      </span>
    </div>

    <!-- Loading -->
    <div v-if="loading && !status" class="text-center py-12 text-neutral-500">
      Loading status...
    </div>

    <!-- Error -->
    <div v-else-if="error" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
      Error: {{ error }}
    </div>

    <!-- No data -->
    <div v-else-if="!status" class="text-center py-12 text-neutral-500">
      No status data available. Waiting for device to publish...
    </div>

    <!-- Status content -->
    <template v-else>
      <!-- Big temp display -->
      <div class="bg-neutral-900 rounded-2xl p-6 text-center ring-1 ring-neutral-800">
        <p class="text-xs text-neutral-500 uppercase tracking-widest mb-1">Water Temperature</p>
        <div class="flex items-baseline justify-center gap-1">
          <span class="text-5xl font-light text-white">{{ status.current_temp ?? '--' }}</span>
          <span class="text-xl text-neutral-500">°{{ status.temp_scale === 'celsius' ? 'C' : 'F' }}</span>
        </div>
        <div class="mt-3 flex items-center justify-center gap-4 text-sm">
          <span class="text-neutral-400">Target: <span class="text-white font-medium">{{ status.set_temp ?? '--' }}°</span></span>
          <span v-if="status.is_heating" class="flex items-center gap-1 text-orange-400">
            <span class="relative flex h-2 w-2">
              <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-orange-400 opacity-75" />
              <span class="relative inline-flex rounded-full h-2 w-2 bg-orange-400" />
            </span>
            Heating
          </span>
        </div>
      </div>

      <!-- Value fields -->
      <div class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 overflow-hidden divide-y divide-neutral-800">
        <div v-for="f in valueFields" :key="f.key" class="flex items-center justify-between px-4 py-3">
          <span class="text-sm text-neutral-400">{{ f.label }}</span>
          <span class="text-sm text-white font-medium">{{ status[f.key] ?? '--' }}</span>
        </div>
      </div>

      <!-- Boolean fields grid -->
      <div class="bg-neutral-900 rounded-2xl p-4 ring-1 ring-neutral-800">
        <h3 class="text-xs font-semibold text-neutral-500 uppercase tracking-widest mb-3">Components</h3>
        <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
          <div
            v-for="f in boolFields"
            :key="f.key"
            :class="[
              'flex items-center justify-between px-3 py-2.5 rounded-xl text-sm',
              status[f.key]
                ? 'bg-blue-500/10 ring-1 ring-blue-500/30'
                : 'bg-neutral-800 ring-1 ring-neutral-700'
            ]"
          >
            <span :class="status[f.key] ? 'text-blue-400' : 'text-neutral-500'">{{ f.label }}</span>
            <span :class="[
              'text-xs font-medium px-1.5 py-0.5 rounded-full',
              status[f.key] ? 'bg-blue-500/20 text-blue-400' : 'bg-neutral-700 text-neutral-500'
            ]">
              {{ status[f.key] ? 'ON' : 'OFF' }}
            </span>
          </div>
        </div>
      </div>

      <!-- Fault -->
      <div v-if="status.last_fault" class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400">
        🔴 Last Fault: {{ status.last_fault }}
      </div>
    </template>
  </div>
</template>
