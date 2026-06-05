<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import ClockSetting from './ClockSetting.vue'
import type { MqttSettings, AccessoryConfig, NotificationConfig } from '../types'

const props = withDefaults(defineProps<{
  modelValue?: boolean
  settings: MqttSettings
  accessoryConfig: AccessoryConfig
  notificationConfig: NotificationConfig
  spaHour?: number
  spaMinute?: number
  spaTimeFormat?: '12h' | '24h'
}>(), {
  modelValue: false,
})

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'save': [settings: MqttSettings]
  'saveAccessoryConfig': [config: AccessoryConfig]
  'saveNotificationConfig': [config: NotificationConfig]
  'setTime': [hour: number, minute: number, is24h: boolean]
}>()

const form = ref<MqttSettings>({ ...props.settings })
const accForm = ref<AccessoryConfig>({ ...props.accessoryConfig })
const notifForm = ref<NotificationConfig>({ ...props.notificationConfig })

const knownDevices = ref<{device_id: string; status: string}[]>([])
const deviceListLoading = ref(false)

watch(() => props.settings, (s) => { form.value = { ...s } }, { deep: true })
watch(() => props.modelValue, (open: boolean) => {
  if (open) {
    form.value = { ...props.settings }
    accForm.value = { ...props.accessoryConfig }
    notifForm.value = { ...props.notificationConfig }
    fetchDevices()
  }
})

async function fetchDevices() {
  deviceListLoading.value = true
  try {
    const res = await fetch('/api/devices')
    if (res.ok) {
      knownDevices.value = await res.json()
    }
  } catch { /* ignore */ } finally {
    deviceListLoading.value = false
  }
}

// --- Validation ---
const DEVICE_ID_RE = /^[a-zA-Z0-9_-]+$/

const brokerUrlError = computed(() => {
  const v = form.value.brokerUrl?.trim() ?? ''
  if (!v) return 'Broker URL is required'
  try {
    const u = new URL(v)
    if (u.protocol !== 'ws:' && u.protocol !== 'wss:') return 'URL must start with ws:// or wss://'
    if (!u.hostname) return 'Invalid hostname'
  } catch {
    return 'Invalid URL'
  }
  return ''
})

const deviceIdError = computed(() => {
  const v = form.value.deviceId?.trim() ?? ''
  if (!v) return 'Device ID is required'
  if (!DEVICE_ID_RE.test(v)) return 'Only letters, numbers, hyphens, underscores'
  return ''
})

const hasErrors = computed(() => !!brokerUrlError.value || !!deviceIdError.value)

function save(): void {
  if (hasErrors.value) return
  emit('save', { ...form.value })
  emit('saveAccessoryConfig', { ...accForm.value })
  emit('saveNotificationConfig', { ...notifForm.value })
  emit('update:modelValue', false)
}

function close(): void {
  emit('update:modelValue', false)
}
</script>

<template>
  <Teleport to="body">
    <Transition name="modal">
      <div v-if="modelValue" class="fixed inset-0 z-50 flex items-center justify-center p-4">
        <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" @click="close" />
        <div class="relative bg-neutral-900 rounded-2xl shadow-2xl w-full max-w-md p-6 ring-1 ring-neutral-700">
          <div class="flex items-center justify-between mb-6">
            <h2 class="text-lg font-semibold text-white">Settings</h2>
            <button @click="close"
              class="w-8 h-8 flex items-center justify-center rounded-lg hover:bg-neutral-800 text-neutral-500 hover:text-neutral-300 transition-colors cursor-pointer">
              ✕
            </button>
          </div>

          <div class="space-y-4">
            <div>
              <label class="block text-sm font-medium text-neutral-400 mb-1">Broker WebSocket URL</label>
              <input v-model="form.brokerUrl"
                type="text"
                placeholder="ws://192.168.1.100:9001"
                :class="['w-full px-3 py-2 bg-neutral-800 border rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:border-blue-500 outline-none', brokerUrlError ? 'border-red-500 focus:ring-red-500' : 'border-neutral-700 focus:ring-blue-500']" />
              <p v-if="brokerUrlError" class="text-xs text-red-400 mt-1">{{ brokerUrlError }}</p>
            </div>

            <div>
              <label class="block text-sm font-medium text-neutral-400 mb-1">Device ID</label>
              <div class="flex gap-2">
                <select v-model="form.deviceId"
                  :class="['flex-1 px-3 py-2 bg-neutral-800 border rounded-lg text-sm text-white focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none appearance-none', deviceIdError ? 'border-red-500 focus:ring-red-500' : 'border-neutral-700']">
                  <option value="" disabled selected v-if="!knownDevices.length">No devices found</option>
                  <option v-for="d in knownDevices" :key="d.device_id" :value="d.device_id">
                    {{ d.device_id + (d.status === 'online' ? ' (online)' : '') }}
                  </option>
                </select>
                <input v-model="form.deviceId"
                  type="text"
                  placeholder="or type custom ID"
                  :class="['flex-1 px-3 py-2 bg-neutral-800 border rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:border-blue-500 outline-none', deviceIdError ? 'border-red-500 focus:ring-red-500' : 'border-neutral-700 focus:ring-blue-500']" />
              </div>
              <p v-if="deviceIdError" class="text-xs text-red-400 mt-1">{{ deviceIdError }}</p>
              <p v-if="deviceListLoading" class="text-xs text-neutral-600 mt-1">Loading devices...</p>
            </div>

            <div class="grid grid-cols-2 gap-3">
              <div>
                <label class="block text-sm font-medium text-neutral-400 mb-1">Username</label>
                <input v-model="form.username"
                  type="text"
                  placeholder="(optional)"
                  class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none" />
              </div>
              <div>
                <label class="block text-sm font-medium text-neutral-400 mb-1">Password</label>
                <input v-model="form.password"
                  type="password"
                  placeholder="(optional)"
                  class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none" />
              </div>
            </div>
          </div>

          <!-- Clock sync -->
          <div class="pt-2 border-t border-neutral-700">
            <ClockSetting
              :spa-hour="spaHour"
              :spa-minute="spaMinute"
              :spa-time-format="spaTimeFormat"
              :disabled="false"
              @set-time="(h, m, is24h) => emit('setTime', h, m, is24h)"
            />
          </div>

          <!-- Accessory Configuration -->
          <div class="pt-2 border-t border-neutral-700">
            <h3 class="text-sm font-semibold text-neutral-300 mb-3">Accessories</h3>
            <div class="space-y-3">
              <div class="grid grid-cols-2 gap-3">
                <div>
                  <label class="block text-sm font-medium text-neutral-400 mb-1">Pumps</label>
                  <select v-model.number="accForm.pumps"
                    class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none">
                    <option v-for="n in 6" :key="n" :value="n">{{ n }}</option>
                  </select>
                </div>
                <div>
                  <label class="block text-sm font-medium text-neutral-400 mb-1">Lights</label>
                  <select v-model.number="accForm.lights"
                    class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none">
                    <option v-for="n in 4" :key="n" :value="n">{{ n }}</option>
                  </select>
                </div>
              </div>
              <div class="flex gap-4">
                <label class="flex items-center gap-2 cursor-pointer">
                  <input type="checkbox" v-model="accForm.blower"
                    class="w-4 h-4 rounded bg-neutral-800 border-neutral-600 text-blue-500 focus:ring-blue-500" />
                  <span class="text-sm text-neutral-300">Blower</span>
                </label>
                <label class="flex items-center gap-2 cursor-pointer">
                  <input type="checkbox" v-model="accForm.mister"
                    class="w-4 h-4 rounded bg-neutral-800 border-neutral-600 text-blue-500 focus:ring-blue-500" />
                  <span class="text-sm text-neutral-300">Mister</span>
                </label>
              </div>
            </div>
          </div>

          <!-- Notifications -->
          <div class="pt-2 border-t border-neutral-700">
            <h3 class="text-sm font-semibold text-neutral-300 mb-3">Notifications</h3>
            <div class="space-y-3">
              <div>
                <label class="block text-sm font-medium text-neutral-400 mb-1">Discord Webhook URL</label>
                <input v-model="notifForm.discord_webhook_url"
                  type="text"
                  placeholder="https://discord.com/api/webhooks/..."
                  class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none" />
                <p class="text-xs text-neutral-600 mt-1">Leave empty to disable notifications</p>
              </div>
              <div>
                <label class="block text-sm font-medium text-neutral-400 mb-1">Offline threshold (hours)</label>
                <input v-model.number="notifForm.offline_threshold_hours"
                  type="number"
                  min="1"
                  max="72"
                  class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none" />
                <p class="text-xs text-neutral-600 mt-1">Alert after this many consecutive hours offline</p>
              </div>
              <div>
                <label class="block text-sm font-medium text-neutral-400 mb-1">Monitored devices</label>
                <div class="flex flex-wrap gap-2 mb-2">
                  <span v-for="(dev, idx) in notifForm.monitored_devices" :key="idx"
                    class="inline-flex items-center gap-1 px-2 py-1 bg-blue-500/15 text-blue-400 rounded-lg text-xs ring-1 ring-blue-500/25">
                    <span>{{ dev }}</span>
                    <button @click="notifForm.monitored_devices.splice(idx, 1)"
                      class="text-blue-400/60 hover:text-blue-300 cursor-pointer">&times;</button>
                  </span>
                  <span v-if="!notifForm.monitored_devices.length"
                    class="text-xs text-neutral-600">All devices monitored when empty</span>
                </div>
                <div class="flex gap-2">
                  <select @change="($event: Event) => { const v = ($event.target as HTMLSelectElement).value; if (v && !notifForm.monitored_devices.includes(v)) { notifForm.monitored_devices.push(v); } ($event.target as HTMLSelectElement).value = ''; }"
                    class="flex-1 px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none">
                    <option value="">Add device...</option>
                    <option v-for="d in knownDevices.filter(d => !notifForm.monitored_devices.includes(d.device_id))" :key="d.device_id" :value="d.device_id">
                      {{ d.device_id }}
                    </option>
                  </select>
                </div>
              </div>
            </div>
          </div>

          <div class="flex gap-3 mt-6">
            <button @click="close"
              class="flex-1 px-4 py-2.5 border border-neutral-700 rounded-lg text-sm font-medium text-neutral-400 hover:bg-neutral-800 transition-colors cursor-pointer">
              Cancel
            </button>
            <button @click="save"
              :disabled="hasErrors"
              class="flex-1 px-4 py-2.5 bg-blue-600 text-white rounded-lg text-sm font-medium hover:bg-blue-500 transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed">
              Save &amp; Reconnect
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.modal-enter-active,
.modal-leave-active {
  transition: opacity 0.2s ease;
}
.modal-enter-from,
.modal-leave-to {
  opacity: 0;
}
</style>
