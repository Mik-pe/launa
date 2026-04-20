import { ref, shallowRef, computed, onMounted, onUnmounted } from 'vue'
import mqtt from 'mqtt'
import type { IClientOptions, MqttClient } from 'mqtt'
import type { SpaState, MqttSettings, AccessoryConfig } from '../types'

const RECONNECTING_TIMEOUT_MS = 30_000

const _stateKeyMap: Record<string, string> = {
  pump1: 'pump1_on', pump2: 'pump2_on', pump3: 'pump3_on',
  pump4: 'pump4_on', pump5: 'pump5_on', pump6: 'pump6_on',
  light1: 'light1', light2: 'light2', light3: 'light3', light4: 'light4',
  blower: 'blower', circulation_pump: 'circ_pump', mister: 'mister',
  hold_mode: 'hold_mode',
  heat_mode: 'heating_mode',
  temp_range: 'temp_range',
}

function loadSettings(): MqttSettings {
  try {
    const saved = localStorage.getItem('launa-settings')
    if (saved) return JSON.parse(saved) as MqttSettings
  } catch { /* ignore */ }
  return {
    brokerUrl: `ws://${window.location.hostname}:9001`,
    deviceId: 'launa_spa',
    username: '',
    password: '',
  }
}

export function useMqtt() {
  const client = shallowRef<MqttClient | null>(null)
  const connected = ref(false)
  const connecting = ref(false)
  const connectionError = ref<string | null>(null)
  const initialConnect = ref(true)
  const spaState = ref<SpaState | null>(null)
  const availability = ref('offline')
  const diagnostics = ref<Record<string, unknown> | string | null>(null)
  const alert_ = ref<string | null>(null)
  const retryCount = ref(0)
  const selfTestEnabled = ref(false)
  const sniffEnabled = ref(false)
  let reconnectingTimer: ReturnType<typeof setTimeout> | null = null

  const settings = ref<MqttSettings>(loadSettings())
  const serverConfig = ref<AccessoryConfig | null>(null)

  const pendingKeys = ref(new Set<string>())

  function saveSettings(s: MqttSettings) {
    settings.value = { ...s }
    localStorage.setItem('launa-settings', JSON.stringify(s))
    connectionError.value = null
    connect()
  }

  function connect() {
    if (client.value) disconnect()

    connecting.value = true
    connectionError.value = null
    initialConnect.value = true

    const opts: IClientOptions = {
      clean: true,
      reconnectPeriod: 5000,
    }
    if (settings.value.username) opts.username = settings.value.username
    if (settings.value.password) opts.password = settings.value.password

    const c = mqtt.connect(settings.value.brokerUrl, opts)

    c.on('connect', () => {
      connected.value = true
      connecting.value = false
      connectionError.value = null
      initialConnect.value = false
      retryCount.value = 0
      availability.value = 'reconnecting'
      clearTimeout(reconnectingTimer!)
      reconnectingTimer = setTimeout(() => {
        if (availability.value === 'reconnecting') {
          availability.value = 'offline'
        }
      }, RECONNECTING_TIMEOUT_MS)
      const base = `launa/${settings.value.deviceId}`
      c.subscribe(`${base}/state`)
      c.subscribe(`${base}/availability`)
      c.subscribe(`${base}/diagnostics`)
      c.subscribe(`${base}/alert`)
    })

    c.on('message', (topic: string, message: Buffer) => {
      const payload = message.toString()
      const base = `launa/${settings.value.deviceId}`

      if (topic === `${base}/state`) {
        try {
          const state = JSON.parse(payload) as SpaState
          spaState.value = state
          if (typeof state.self_test === 'boolean') {
            selfTestEnabled.value = state.self_test
          }
          if (typeof state.sniff_mode === 'boolean') {
            sniffEnabled.value = state.sniff_mode
          }
          if (pendingKeys.value.size > 0) {
            pendingKeys.value = new Set()
          }
        } catch { /* ignore */ }
      } else if (topic === `${base}/availability`) {
        clearTimeout(reconnectingTimer!)
        availability.value = payload
      } else if (topic === `${base}/diagnostics`) {
        try {
          diagnostics.value = JSON.parse(payload)
        } catch {
          diagnostics.value = payload
        }
      } else if (topic === `${base}/alert`) {
        alert_.value = payload
      }
    })

    c.on('error', (err: Error) => {
      connectionError.value = err.message
      if (!initialConnect.value) {
        connecting.value = false
      }
    })

    c.on('close', () => {
      connected.value = false
      connecting.value = false
      clearTimeout(reconnectingTimer!)
      availability.value = 'offline'
    })

    c.on('offline', () => {
      connected.value = false
    })

    c.on('reconnect', () => {
      connecting.value = true
      retryCount.value++
    })

    client.value = c
  }

  function disconnect() {
    clearTimeout(reconnectingTimer!)
    if (client.value) {
      client.value.end(true)
      client.value = null
      connected.value = false
      connecting.value = false
    }
  }

  function publish(subtopic: string, payload: string | number | boolean) {
    if (!client.value || !connected.value) return
    const topic = `launa/${settings.value.deviceId}/command/${subtopic}`
    client.value.publish(topic, String(payload), { qos: 1 })
  }

  function toggle(subtopic: string) {
    const key = _stateKeyMap[subtopic]
    if (key) {
      const next = new Set(pendingKeys.value)
      next.add(key)
      pendingKeys.value = next
    }
    publish(subtopic, true)
  }

  function setTemperature(temp: number) {
    const next = new Set(pendingKeys.value)
    next.add('set_temp')
    pendingKeys.value = next
    publish('set_temperature', String(temp))
  }

  function isPending(key: string): boolean {
    return pendingKeys.value.has(key)
  }

  function setSelfTest(enabled: boolean) {
    selfTestEnabled.value = enabled
    publish('self_test', enabled ? 'ON' : 'OFF')
  }

  function setSniff(enabled: boolean) {
    sniffEnabled.value = enabled
    publish('sniff', enabled ? 'ON' : 'OFF')
  }

  onMounted(() => {
    fetchServerConfig()
    connect()
  })

  onUnmounted(() => {
    clearTimeout(reconnectingTimer!)
    disconnect()
  })

  async function fetchServerConfig() {
    try {
      const res = await fetch('/api/config')
      if (res.ok) {
        serverConfig.value = await res.json() as AccessoryConfig
      }
    } catch { /* ignore */ }
  }

  async function saveAccessoryConfig(newCfg: AccessoryConfig) {
    try {
      const res = await fetch('/api/config', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(newCfg),
      })
      if (res.ok) {
        serverConfig.value = await res.json() as AccessoryConfig
      }
    } catch { /* ignore */ }
  }

  const visibleControls = computed<Record<string, boolean>>(() => {
    const cfg = serverConfig.value
    if (!cfg) return {}
    const vc: Record<string, boolean> = {}
    for (let i = 1; i <= 6; i++) vc['pump' + i] = i <= cfg.pumps
    for (let i = 1; i <= 4; i++) vc['light' + i] = i <= cfg.lights
    vc.blower = cfg.blower
    vc.mister = cfg.mister
    return vc
  })

  return {
    connected,
    connecting,
    connectionError,
    initialConnect,
    retryCount,
    spaState,
    availability,
    diagnostics,
    alert: alert_,
    settings,
    saveSettings,
    connect,
    disconnect,
    publish,
    toggle,
    setTemperature,
    isPending,
    visibleControls,
    serverConfig,
    saveAccessoryConfig,
    selfTestEnabled,
    setSelfTest,
    sniffEnabled,
    setSniff,
  }
}
