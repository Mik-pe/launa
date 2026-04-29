<script setup lang="ts">
import { ref, watch } from 'vue'
import { useMqtt, connectionErrorToast } from './composables/useMqtt'
import PendingDot from './components/PendingDot.vue'
import type { MqttSettings, AccessoryConfig } from './types'
import LoadingSpinner from './components/LoadingSpinner.vue'
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
  connectionInfo,
  isOnline,
  isRegistered,
  spaState,
  alert: alertMsg,
  settings,
  saveSettings,
  connect,
  disconnect,
  toggle,
  setTemperature,
  setTime,
  isPending,
  publish,
  visibleControls,
  serverConfig,
  saveAccessoryConfig,
  selfTestEnabled,
  setSelfTest,
  sniffEnabled,
  setSniff,
  retryCount,
} = useMqtt()

const showSettings = ref(false)
const activeTab = ref('control')
const showToast = ref(false)
const hasNewAlerts = ref(false)

// Mirror the connection error toast into a local ref so the template can react
watch(() => connectionErrorToast.value, (msg) => {
  showToast.value = !!msg
})

// Track new MQTT alerts for tab badge
watch(alertMsg, (msg) => {
  if (msg && activeTab.value !== 'alerts') {
    hasNewAlerts.value = true
  }
})

watch(activeTab, (tab) => {
  if (tab === 'alerts') hasNewAlerts.value = false
})

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
  <div class="min-h-screen bg-neutral-950 overflow-x-hidden pb-[env(safe-area-inset-bottom)]">
    <ConnectionBar
      :connection-info="connectionInfo"
      :broker-url="settings.brokerUrl"
      :device-id="settings.deviceId"
      :spa-state="spaState"
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
              'relative flex items-center gap-2 px-4 py-2.5 rounded-xl text-sm font-medium whitespace-nowrap transition-all duration-200 cursor-pointer shrink-0',
              activeTab === tab.id
                ? 'bg-blue-500/15 text-blue-400 ring-1 ring-blue-500/30 shadow-sm shadow-blue-500/10'
                : 'text-neutral-400 hover:text-neutral-200 hover:bg-neutral-800'
            ]"
          >
            <span class="text-base">{{ tab.icon }}</span>
            <span>{{ tab.label }}</span>
            <span v-if="tab.id === 'alerts' && hasNewAlerts"
              class="absolute -top-1 -right-1 w-2.5 h-2.5 bg-amber-400 rounded-full ring-2 ring-neutral-900/80" />
          </button>
        </nav>
      </div>
    </div>

    <!-- Tab content -->
    <div class="max-w-3xl mx-auto px-4 py-4 sm:py-6 space-y-4 sm:space-y-6">

        <!-- Control tab (original dashboard) -->
        <template v-if="activeTab === 'control'">
          <!-- Not connected: show connecting state inline -->
          <template v-if="!isOnline">
            <div class="flex flex-col items-center justify-center py-20">
              <LoadingSpinner class="h-10 w-10 mb-4" />
              <p class="text-neutral-400 text-sm">
                <template v-if="connectionInfo.status === 'connecting'">Connecting to MQTT broker...</template>
                <template v-else-if="connectionInfo.status === 'reconnecting'">Reconnecting to MQTT broker (retry {{ retryCount }})...</template>
                <template v-else-if="connectionInfo.status === 'offline'">Device offline</template>
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

          <!-- Registration banner -->
          <div v-if="isOnline && !isRegistered"
            class="bg-orange-950/50 border border-orange-800/50 text-orange-300 rounded-xl px-4 py-3 text-sm flex items-center gap-2">
            <span>🔌</span>
            <span>Not registered with spa controller — controls disabled. Attempting to connect...</span>
          </div>

          <!-- Temperature -->
          <TemperatureCard
            :state="spaState"
            :pending="isPending('set_temp')"
            :disabled="!isRegistered"
            @set-temperature="isRegistered && setTemperature($event)"
          />

          <!-- Selects: Heat Mode, Temp Range -->
          <div class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 overflow-hidden divide-y divide-neutral-800">
            <!-- Heat Mode: tristate cycle button -->
            <div :class="['flex items-center justify-between gap-2 px-4 py-3 rounded-xl transition-all', (!isOnline || !isRegistered) ? 'opacity-40' : '']">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium text-neutral-400">Heat Mode</span>
                <PendingDot v-if="isPending('heating_mode')" />
              </div>
              <button
                @click="isOnline && isRegistered && cycleHeatMode()"
                :disabled="!isOnline || !isRegistered"
                :data-tooltip="!isRegistered ? 'Waiting for spa registration...' : 'Click to cycle: Ready → Rest → Ready in Rest → Ready'"
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
              :disabled="!isOnline || !isRegistered"
              :disabled-reason="!isRegistered ? 'Waiting for spa registration...' : undefined"
              @update:model-value="handleTempRange"
            />
          </div>

          <!-- Controls -->
          <div class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 p-4">
            <ControlsPanel
              :state="spaState"
              :connected="isOnline && isRegistered"
              :is-pending="isPending"
              :visible-controls="visibleControls"
              :disabled-reason="!isRegistered ? 'Waiting for spa registration...' : undefined"
              @toggle="toggle"
            />
          </div>

          <!-- Last Fault -->
          <div v-if="spaState?.last_fault"
            class="bg-red-950/50 border border-red-900/50 rounded-xl px-4 py-3 text-sm text-red-400 flex items-center gap-2">
            <span>🔴</span>
            <span>Last Fault: {{ spaState.last_fault }}</span>
          </div>

          <!-- Firmware & Reboot -->
          <div class="flex items-center justify-center gap-3 pb-4 text-xs text-neutral-600">
            <span v-if="spaState?.firmware_version">Firmware {{ spaState.firmware_version }}</span>
            <button v-if="isOnline && isRegistered"
              @click="publish('reboot', 'ON')"
              class="px-2 py-0.5 rounded bg-neutral-700 hover:bg-red-700 text-neutral-300 text-xs transition-colors"
              data-tooltip="Reboot device">
              Reboot
            </button>
          </div>
          </template>
        </template>

        <!-- Status tab -->
        <StatusDashboard v-else-if="activeTab === 'status'" :spa-state="spaState" :connected="isOnline" :visible-controls="visibleControls" />

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

    <!-- Disconnect command toast -->
    <Transition name="toast">
      <div v-if="showToast"
        class="fixed bottom-20 left-1/2 -translate-x-1/2 z-50 bg-red-900/90 text-red-200 text-sm px-4 py-2.5 rounded-xl shadow-lg ring-1 ring-red-800/50 backdrop-blur-sm">
        {{ connectionErrorToast }}
      </div>
    </Transition>

    <SettingsModal
      v-model="showSettings"
      :settings="settings"
      :accessory-config="serverConfig ?? { pumps: 2, lights: 1, blower: true, mister: false }"
      :self-test-enabled="selfTestEnabled"
      :sniff-enabled="sniffEnabled"
      :self-test-pending="isPending('self_test')"
      :sniff-pending="isPending('sniff_mode')"
      :spa-hour="spaState?.hour"
      :spa-minute="spaState?.minute"
      :spa-time-format="spaState?.time_format"
      @save="handleSave"
      @save-accessory-config="saveAccessoryConfig"
      @toggle-self-test="setSelfTest"
      @toggle-sniff="setSniff"
      @set-time="setTime"
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
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.3s ease, transform 0.3s ease;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 10px);
}
</style>
