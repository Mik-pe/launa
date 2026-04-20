<script setup>
import { ref, watch } from 'vue'

const props = defineProps({
  modelValue: { type: Boolean, default: false },
  settings: Object,
  accessoryConfig: Object,
  selfTestEnabled: { type: Boolean, default: false },
  sniffEnabled: { type: Boolean, default: false },
})

const emit = defineEmits(['update:modelValue', 'save', 'saveAccessoryConfig', 'toggleSelfTest', 'toggleSniff'])

const form = ref({ ...props.settings })
const accForm = ref({ ...props.accessoryConfig })

watch(() => props.settings, (s) => { form.value = { ...s } }, { deep: true })
watch(() => props.modelValue, (open) => {
  if (open) {
    form.value = { ...props.settings }
    accForm.value = { ...props.accessoryConfig }
  }
})

function save() {
  emit('save', { ...form.value })
  emit('saveAccessoryConfig', { ...accForm.value })
  emit('update:modelValue', false)
}

function close() {
  emit('update:modelValue', false)
}

function toggleSelfTest() {
  emit('toggleSelfTest', !props.selfTestEnabled)
}

function toggleSniff() {
  emit('toggleSniff', !props.sniffEnabled)
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
            <div class="flex items-center justify-between">
              <div>
                <h3 class="text-sm font-semibold text-neutral-300">Self-Test Mode</h3>
                <p class="text-xs text-neutral-500 mt-0.5">Simulate spa state without hardware</p>
              </div>
              <button @click="toggleSelfTest"
                :class="[
                  'relative inline-flex h-6 w-11 items-center rounded-full transition-colors cursor-pointer',
                  selfTestEnabled ? 'bg-amber-500' : 'bg-neutral-700'
                ]">
                <span :class="[
                  'inline-block h-4 w-4 transform rounded-full bg-white transition-transform shadow',
                  selfTestEnabled ? 'translate-x-6' : 'translate-x-1'
                ]" />
              </button>
            </div>
          </div>

          <!-- Sniff Mode -->
          <div class="pt-2 border-t border-neutral-700">
            <div class="flex items-center justify-between">
              <div>
                <h3 class="text-sm font-semibold text-neutral-300">Sniff Mode</h3>
                <p class="text-xs text-neutral-500 mt-0.5">Capture raw RS-485 frames to MQTT</p>
              </div>
              <button @click="toggleSniff"
                :class="[
                  'relative inline-flex h-6 w-11 items-center rounded-full transition-colors cursor-pointer',
                  sniffEnabled ? 'bg-cyan-500' : 'bg-neutral-700'
                ]">
                <span :class="[
                  'inline-block h-4 w-4 transform rounded-full bg-white transition-transform shadow',
                  sniffEnabled ? 'translate-x-6' : 'translate-x-1'
                ]" />
              </button>
            </div>
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
