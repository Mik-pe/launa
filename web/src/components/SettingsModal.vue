<script setup lang="ts">
import { ref, watch } from 'vue'
import ToggleSwitch from './ToggleSwitch.vue'
import type { MqttSettings, AccessoryConfig } from '../types'

const props = withDefaults(defineProps<{
  modelValue?: boolean
  settings: MqttSettings
  accessoryConfig: AccessoryConfig
  selfTestEnabled?: boolean
  sniffEnabled?: boolean
}>(), {
  modelValue: false,
  selfTestEnabled: false,
  sniffEnabled: false,
})

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'save': [settings: MqttSettings]
  'saveAccessoryConfig': [config: AccessoryConfig]
  'toggleSelfTest': [enabled: boolean]
  'toggleSniff': [enabled: boolean]
}>()

const form = ref<MqttSettings>({ ...props.settings })
const accForm = ref<AccessoryConfig>({ ...props.accessoryConfig })

watch(() => props.settings, (s) => { form.value = { ...s } }, { deep: true })
watch(() => props.modelValue, (open: boolean) => {
  if (open) {
    form.value = { ...props.settings }
    accForm.value = { ...props.accessoryConfig }
  }
})

function save(): void {
  emit('save', { ...form.value })
  emit('saveAccessoryConfig', { ...accForm.value })
  emit('update:modelValue', false)
}

function close(): void {
  emit('update:modelValue', false)
}

function toggleSelfTest(val: boolean): void {
  emit('toggleSelfTest', val)
}

function toggleSniff(val: boolean): void {
  emit('toggleSniff', val)
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
                class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none" />
            </div>

            <div>
              <label class="block text-sm font-medium text-neutral-400 mb-1">Device ID</label>
              <input v-model="form.deviceId"
                type="text"
                placeholder="launa_spa"
                class="w-full px-3 py-2 bg-neutral-800 border border-neutral-700 rounded-lg text-sm text-white placeholder-neutral-600 focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none" />
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

          <!-- Self-Test Mode -->
          <div class="pt-2 border-t border-neutral-700">
            <ToggleSwitch
              label="Self-Test Mode"
              :model-value="selfTestEnabled"
              :disabled="false"
              @update:model-value="toggleSelfTest"
            />
            <p class="text-xs text-neutral-500 mt-1 px-4">Simulate spa state without hardware</p>
          </div>

          <!-- Sniff Mode -->
          <div class="pt-2 border-t border-neutral-700">
            <ToggleSwitch
              label="Sniff Mode"
              :model-value="sniffEnabled"
              :disabled="false"
              @update:model-value="toggleSniff"
            />
            <p class="text-xs text-neutral-500 mt-1 px-4">Capture raw RS-485 frames to MQTT</p>
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

          <div class="flex gap-3 mt-6">
            <button @click="close"
              class="flex-1 px-4 py-2.5 border border-neutral-700 rounded-lg text-sm font-medium text-neutral-400 hover:bg-neutral-800 transition-colors cursor-pointer">
              Cancel
            </button>
            <button @click="save"
              class="flex-1 px-4 py-2.5 bg-blue-600 text-white rounded-lg text-sm font-medium hover:bg-blue-500 transition-colors cursor-pointer">
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
