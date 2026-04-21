<script setup lang="ts">
import ToggleSwitch from './ToggleSwitch.vue'
import type { SpaState } from '../types'

const props = withDefaults(defineProps<{
  state: SpaState | null
  connected: boolean
  isPending: (key: string) => boolean
  visibleControls?: Record<string, boolean>
}>(), {
  visibleControls: () => ({}),
})

const emit = defineEmits<{
  toggle: [subtopic: string, value: boolean]
}>()

function toggleItem(subtopic: string, currentVal: unknown): void {
  emit('toggle', subtopic, !currentVal)
}

/** Type-safe lookup for dynamically indexed SpaState properties. */
function getSpaProp(s: SpaState | null | undefined, key: string): unknown {
  if (!s) return undefined
  return (s as Record<string, unknown>)[key]
}
</script>

<template>
  <div class="space-y-6">
    <!-- Pumps -->
    <section>
      <h3 class="text-xs font-semibold text-neutral-500 uppercase tracking-widest mb-3 px-1">Pumps</h3>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
        <ToggleSwitch
          v-for="i in 6" :key="'pump'+i"
          v-show="visibleControls['pump'+i] !== false"
          :label="'Pump ' + i"
          :model-value="!!getSpaProp(state, 'pump'+i+'_on')"
          :disabled="!connected"
          :pending="isPending('pump'+i+'_on')"
          @update:model-value="toggleItem('pump'+i, getSpaProp(state, 'pump'+i+'_on'))"
          :icon="i <= 2 ? '🌀' : '⚙️'"
        />
      </div>
    </section>

    <!-- Lights -->
    <section>
      <h3 class="text-xs font-semibold text-neutral-500 uppercase tracking-widest mb-3 px-1">Lights</h3>
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
        <ToggleSwitch
          v-for="i in 4" :key="'light'+i"
          v-show="visibleControls['light'+i] !== false"
          :label="'Light ' + i"
          :model-value="!!getSpaProp(state, 'light'+i)"
          :disabled="!connected"
          :pending="isPending('light'+i)"
          @update:model-value="toggleItem('light'+i, getSpaProp(state, 'light'+i))"
          icon="💡"
        />
      </div>
    </section>

    <!-- Accessories -->
    <section>
      <h3 class="text-xs font-semibold text-neutral-500 uppercase tracking-widest mb-3 px-1">Accessories</h3>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
        <ToggleSwitch
          v-show="visibleControls.blower !== false"
          label="Blower"
          :model-value="!!state?.blower"
          :disabled="!connected"
          :pending="isPending('blower')"
          @update:model-value="toggleItem('blower', state?.blower)"
          icon="💨"
        />
        <ToggleSwitch
          label="Circ Pump"
          :model-value="!!state?.circ_pump"
          :disabled="true"
          :pending="false"
          :read-only="true"
          icon="🔄"
        />
        <ToggleSwitch
          v-show="visibleControls.mister !== false"
          label="Mister"
          :model-value="!!state?.mister"
          :disabled="!connected"
          :pending="isPending('mister')"
          @update:model-value="toggleItem('mister', state?.mister)"
          icon="💨"
        />
      </div>
    </section>

    <!-- Mode Controls -->
    <section>
      <h3 class="text-xs font-semibold text-neutral-500 uppercase tracking-widest mb-3 px-1">Mode</h3>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
        <ToggleSwitch
          label="Hold Mode"
          :model-value="!!state?.hold_mode"
          :disabled="!connected"
          :pending="isPending('hold_mode')"
          @update:model-value="toggleItem('hold_mode', state?.hold_mode)"
          icon="⏸️"
        />
        <ToggleSwitch
          label="AUX 1"
          :model-value="false"
          :disabled="!connected"
          @update:model-value="toggleItem('aux1', false)"
        />
        <ToggleSwitch
          label="AUX 2"
          :model-value="false"
          :disabled="!connected"
          @update:model-value="toggleItem('aux2', false)"
        />
      </div>
    </section>
  </div>
</template>
