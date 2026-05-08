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
  clearAlert,
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
  sniffEnabled,
  setSniff,
  retryCount,
} = useMqtt()

const showSettings = ref(false)
const activeTab = ref('control')
const devSubTab = ref('logs')
const showToast = ref(false)
const hasNewAlerts = ref(false)

// Mirror the connection error toast into a local ref so the template can react
watch(() => connectionErrorToast.value, (msg) => {
  showToast.value = !!msg
})

// Track new MQTT alerts for tab badge (show badge on Developer tab)
watch(alertMsg, (msg) => {
  if (msg && !(activeTab.value === 'developer' && devSubTab.value === 'alerts')) {
    hasNewAlerts.value = true
  }
})

watch(activeTab, (tab) => {
  if (tab === 'developer' && devSubTab.value === 'alerts') hasNewAlerts.value = false
})

watch(devSubTab, (sub) => {
  if (sub === 'alerts') hasNewAlerts.value = false
})

const tabs: { id: string; label: string }[] = [
  { id: 'control', label: 'Control' },
  { id: 'status', label: 'Status' },
  { id: 'temperature', label: 'History' },
  { id: 'developer', label: 'Developer' },
]

const devSubTabs: { id: string; label: string }[] = [
  { id: 'logs', label: 'Logs' },
  { id: 'alerts', label: 'Alerts' },
  { id: 'diagnostics', label: 'Diagnostics' },
  { id: 'sniff', label: 'Sniff' },
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
        <nav class="flex justify-center overflow-x-auto gap-1 py-2 scrollbar-hide">
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
            <svg v-if="tab.id === 'control'" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M10.5 6h9.75M10.5 6a1.5 1.5 0 1 1-3 0m3 0a1.5 1.5 0 1 0-3 0M3.75 6H7.5m3 12h9.75m-9.75 0a1.5 1.5 0 0 1-3 0m3 0a1.5 1.5 0 0 0-3 0m-3.75 0H7.5m9-6h3.75m-3.75 0a1.5 1.5 0 0 1-3 0m3 0a1.5 1.5 0 0 0-3 0m-9.75 0h9.75" /></svg>
            <svg v-else-if="tab.id === 'status'" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" /></svg>
            <svg v-else-if="tab.id === 'temperature'" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" /></svg>
            <svg v-else-if="tab.id === 'developer'" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M17.25 6.75 22.5 12l-5.25 5.25m-10.5 0L1.5 12l5.25-5.25m7.5-3-4.5 16.5" /></svg>
            <span>{{ tab.label }}</span>
            <span v-if="tab.id === 'developer' && hasNewAlerts"
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
          <div class="bg-neutral-900 rounded-2xl ring-1 ring-neutral-800 divide-y divide-neutral-800">
            <!-- Heat Mode: tristate cycle button -->
            <div :class="['flex items-center justify-between gap-2 px-4 py-3 rounded-xl transition-all', (!isOnline || !isRegistered) ? 'opacity-40' : '']">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium text-neutral-400">Heat Mode</span>
                <PendingDot v-if="isPending('heating_mode')" />
              </div>
              <button
                @click="isOnline && isRegistered && cycleHeatMode()"
                :disabled="!isOnline || !isRegistered"
                :data-tooltip="!isRegistered ? 'Waiting for spa registration...' : 'Tap to cycle: Ready → Rest → Ready in Rest'"
                class="flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium bg-blue-500/10 text-blue-300 ring-1 ring-blue-500/25 hover:bg-blue-500/20 hover:ring-blue-500/40 active:bg-blue-500/10 transition-colors cursor-pointer select-none"
              >
                <span>{{ heatModeLabels[spaState?.heating_mode ?? ''] || '--' }}</span>
                <svg class="w-3.5 h-3.5 text-blue-400/60" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" d="M8.25 4.5l7.5 7.5-7.5 7.5" /></svg>
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

        <!-- Developer tab (sub-nested: Logs, Alerts, Diagnostics, Sniff) -->
        <template v-else-if="activeTab === 'developer'">
          <div class="flex gap-1 mb-4">
            <button
              v-for="sub in devSubTabs"
              :key="sub.id"
              @click="devSubTab = sub.id"
              :class="[
                'relative flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium whitespace-nowrap transition-all duration-200 cursor-pointer shrink-0',
                devSubTab === sub.id
                  ? 'bg-blue-500/15 text-blue-400 ring-1 ring-blue-500/30'
                  : 'text-neutral-500 hover:text-neutral-300 hover:bg-neutral-800'
              ]"
            >
              <span>{{ sub.label }}</span>
              <span v-if="sub.id === 'alerts' && hasNewAlerts"
                class="w-2 h-2 bg-amber-400 rounded-full" />
            </button>
          </div>

          <!-- Developer sub-tab content -->
          <LogViewer v-if="devSubTab === 'logs'" />
          <AlertsView v-else-if="devSubTab === 'alerts'" @cleared="clearAlert" />
          <DiagnosticsView v-else-if="devSubTab === 'diagnostics'" />
          <SniffFramesView
            v-else-if="devSubTab === 'sniff'"
            :sniff-enabled="sniffEnabled"
            :sniff-pending="isPending('sniff_mode')"
            @capture="(n) => setSniff(n)"
            @stop="setSniff(false)"
          />
        </template>

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
      :spa-hour="spaState?.hour"
      :spa-minute="spaState?.minute"
      :spa-time-format="spaState?.time_format"
      @save="handleSave"
      @save-accessory-config="saveAccessoryConfig"
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
