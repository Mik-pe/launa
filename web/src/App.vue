<script setup lang="ts">
import { ref } from 'vue'
import { useMqtt } from './composables/useMqtt'
import type { MqttSettings, AccessoryConfig } from './types'
import ConnectionBar from './components/ConnectionBar.vue'
import TemperatureCard from './components/TemperatureCard.vue'
import SelectControl from './components/SelectControl.vue'
import ControlsPanel from './components/ControlsPanel.vue'
import SettingsModal from './components/SettingsModal.vue'
import StatusDashboard from './components/StatusDashboard.vue'
import TemperatureChart from './components/TemperatureChart.vue'
import LogViewer from './components/LogViewer.vue'
import AlertsView from './components/AlertsView.vue'
import DiagnosticsView from './components/DiagnosticsView.vue'
import SniffFramesView from './components/SniffFramesView.vue'

const {
  connected,
  connecting,
  connectionError,
  initialConnect,
  retryCount,
  spaState,
  availability,
  alert: alertMsg,
  settings,
  saveSettings,
  connect,
  disconnect,
  toggle,
  setTemperature,
  isPending,
  publish,
  visibleControls,
  serverConfig,
  saveAccessoryConfig,
  selfTestEnabled,
  setSelfTest,
  sniffEnabled,
  setSniff,
} = useMqtt()

const showSettings = ref(false)
const activeTab = ref('control')

const tabs: { id: string; label: string; icon: string }[] = [
  { id: 'control', label: 'Control', icon: '🎛️' },
  { id: 'status', label: 'Status', icon: '📊' },
  { id: 'temperature', label: 'History', icon: '🌡️' },
  { id: 'logs', label: 'Logs', icon: '📋' },
  { id: 'alerts', label: 'Alerts', icon: '⚠️' },
  { id: 'diagnostics', label: 'Diagnostics', icon: '🔧' },
  { id: 'sniff', label: 'Sniff', icon: '📡' },
]

const heatModeLabels: Record<string, string> = {
  ready: 'Ready',
  rest: 'Rest',
  ready_in_rest: 'Ready in Rest',
}

const tempRangeOptions: { value: string; label: string }[] = [
  { value: 'high', label: 'High' },
  { value: 'low', label: 'Low' },
]

function handleSave(s: MqttSettings): void {
  saveSettings(s)
}

function cycleHeatMode(): void {
  toggle('heat_mode')
}

function handleTempRange(val: string): void {
  publish('temp_range', val)
}
</script>

<template>
  <div class="min-h-screen bg-neutral-950 pb-[env(safe-area-inset-bottom)]">
    <ConnectionBar
      :connected="connected"
      :connecting="connecting"
      :availability="availability"
      :broker-url="settings.brokerUrl"
      :device-id="settings.deviceId"
      :connection-error="connectionError"
      :initial-connect="initialConnect"
      @open-settings="showSettings = true"
    />

    <!-- Tab bar (always visible) -->
    <div class="bg-neutral-900/80 backdrop-blur-xl border-b border-neutral-800/60 sticky top-0 z-30">
      <div class="max-w-3xl mx-auto px-2">
        <nav class="flex overflow-x-auto gap-1 py-2 scrollbar-hide">
          <button
            v-for="tab in tabs"
            :key="tab.id"
            @click="activeTab = tab.id"
            :class="[
              'flex items-center gap-2 px-4 py-2.5 rounded-xl text-sm font-medium whitespace-nowrap transition-all duration-200 cursor-pointer shrink-0',
              activeTab === tab.id
                ? 'bg-blue-500/15 text-blue-400 ring-1 ring-blue-500/30 shadow-sm shadow-blue-500/10'
                : 'text-neutral-400 hover:text-neutral-200 hover:bg-neutral-800'
            ]"
          >
            <span class="text-base">{{ tab.icon }}</span>
            <span>{{ tab.label }}</span>
          </button>
        </nav>
      </div>
    </div>

    <!-- Tab content -->
    <div class="max-w-3xl mx-auto px-4 py-4 sm:py-6 space-y-4 sm:space-y-6">

        <!-- Control tab (original dashboard) -->
        <template v-if="activeTab === 'control'">
          <!-- Not connected: show connecting state inline -->
          <template v-if="!connected">
            <div class="flex flex-col items-center justify-center py-20">
              <svg class="animate-spin h-10 w-10 text-blue-400 mb-4" fill="none" viewBox="0 0 24 24">
                <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
              </svg>
              <p class="text-neutral-400 text-sm">
                <template v-if="connecting">Connecting to MQTT broker{{ retryCount > 0 ? ` (retry ${retryCount})` : '' }}...</template>
                <template v-else-if="connectionError">Connection failed: {{ connectionError }}</template>
                <template v-else>Not connected</template>
              </p>
            </div>
          </template>

          <!-- Connected: show controls -->
          <template v-else>
          <!-- Alert banner -->
          <div v-if="alertMsg"
            class="bg-amber-950/50 border border-amber-800/50 text-amber-300 rounded-xl px-4 py-3 text-sm flex items-center gap-2">
            <span>⚠️</span>
            <span>{{ alertMsg }}</span>
          </div>

          <!-- Temperature -->
          <TemperatureCard
            :state="spaState"
            :pending="isPending('set_temp')"
            @set-temperature="setTemperature"
          />

          <!-- Selects: Heat Mode, Temp Range -->
          <div class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 overflow-hidden divide-y divide-neutral-800">
            <!-- Heat Mode: tristate cycle button -->
            <div :class="['flex items-center justify-between gap-2 px-4 py-3 rounded-xl transition-all', availability !== 'online' ? 'opacity-40' : '']">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium text-neutral-400">Heat Mode</span>
                <span v-if="isPending('heating_mode')" class="relative flex h-2.5 w-2.5">
                  <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75" />
                  <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-blue-400" />
                </span>
              </div>
              <button
                @click="availability === 'online' && cycleHeatMode()"
                :disabled="availability !== 'online'"
                :title="'Click to cycle: Ready → Rest → Ready in Rest → Ready'"
                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-neutral-800 text-white ring-1 ring-neutral-700 hover:bg-neutral-700 hover:ring-neutral-600 active:bg-neutral-800 transition-colors cursor-pointer select-none"
              >
                <svg class="w-3 h-3 text-neutral-400" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" /></svg>
                <span>{{ heatModeLabels[spaState?.heating_mode ?? ''] || '--' }}</span>
              </button>
            </div>
            <SelectControl
              label="Temperature Range"
              :model-value="spaState?.temp_range ?? ''"
              :options="tempRangeOptions"
              :pending="isPending('temp_range')"
              :disabled="availability !== 'online'"
              @update:model-value="handleTempRange"
            />
          </div>

          <!-- Controls -->
          <div class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 p-4">
            <ControlsPanel
              :state="spaState"
              :connected="connected && availability === 'online'"
              :is-pending="isPending"
              :visible-controls="visibleControls"
              @toggle="toggle"
            />
          </div>

          <!-- Last Fault -->
          <div v-if="spaState?.last_fault"
            class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400 flex items-center gap-2">
            <span>🔴</span>
            <span>Last Fault: {{ spaState.last_fault }}</span>
          </div>

          <!-- Firmware -->
          <div v-if="spaState?.firmware_version"
            class="text-center text-xs text-neutral-600 pb-4">
            Firmware {{ spaState.firmware_version }}
          </div>
          </template>
        </template>

        <!-- Status tab -->
        <StatusDashboard v-else-if="activeTab === 'status'" :spa-state="spaState" :connected="connected && availability === 'online'" />

        <!-- Temperature chart tab -->
        <TemperatureChart v-else-if="activeTab === 'temperature'" />

        <!-- Logs tab -->
        <LogViewer v-else-if="activeTab === 'logs'" />

        <!-- Alerts tab -->
        <AlertsView v-else-if="activeTab === 'alerts'" />

        <!-- Diagnostics tab -->
        <DiagnosticsView v-else-if="activeTab === 'diagnostics'" />

        <!-- Sniff frames tab -->
        <SniffFramesView v-else-if="activeTab === 'sniff'" />

      </div>

    <!-- Settings button (floating) -->
    <button
      @click="showSettings = true"
      class="fixed bottom-6 right-6 w-12 h-12 bg-neutral-800 rounded-full shadow-lg ring-1 ring-neutral-700 flex items-center justify-center text-neutral-400 hover:text-neutral-200 hover:bg-neutral-700 transition-colors cursor-pointer z-40"
      title="Settings"
    >
      ⚙️
    </button>

    <SettingsModal
      v-model="showSettings"
      :settings="settings"
      :accessory-config="serverConfig ?? { pumps: 2, lights: 1, blower: true, mister: false }"
      :self-test-enabled="selfTestEnabled"
      :sniff-enabled="sniffEnabled"
      @save="handleSave"
      @save-accessory-config="saveAccessoryConfig"
      @toggle-self-test="setSelfTest"
      @toggle-sniff="setSniff"
    />
  </div>
</template>

<style scoped>
.scrollbar-hide {
  -ms-overflow-style: none;
  scrollbar-width: none;
}
.scrollbar-hide::-webkit-scrollbar {
  display: none;
}
</style>
