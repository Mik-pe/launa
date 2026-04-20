import { ref, shallowRef, computed, onMounted, onUnmounted } from 'vue'
import mqtt from 'mqtt'

export function useMqtt() {
  const client = shallowRef(null)
  const connected = ref(false)
  const connecting = ref(false)
  const connectionError = ref(null)
  const spaState = ref(null)
  const availability = ref('offline')
  const diagnostics = ref(null)
  const alert_ = ref(null)
  const retryCount = ref(0)
  const selfTestEnabled = ref(false)
  const sniffEnabled = ref(false)

  const settings = ref(loadSettings())
  const serverConfig = ref(null)

  // Pending commands: Set<string> of state keys that have a pending command.
  // e.g. 'pump1_on', 'set_temp', 'heating_mode' — cleared when the real
  // state update arrives confirming the change.
  const pendingKeys = ref(new Set())

  function loadSettings() {
    try {
      const saved = localStorage.getItem('launa-settings')
      if (saved) return JSON.parse(saved)
    } catch {}
    return {
      brokerUrl: `ws://${window.location.hostname}:9001`,
      deviceId: 'launa_spa',
      username: '',
      password: '',
    }
  }

  function saveSettings(s) {
    settings.value = { ...s }
    localStorage.setItem('launa-settings', JSON.stringify(s))
    connectionError.value = null
    connect()
  }

  function connect() {
    if (client.value) disconnect()

    connecting.value = true
    connectionError.value = null

    const opts = {}
    if (settings.value.username) opts.username = settings.value.username
    if (settings.value.password) opts.password = settings.value.password
    opts.clean = true
    opts.reconnectPeriod = 5000

    const c = mqtt.connect(settings.value.brokerUrl, opts)

    c.on('connect', () => {
      connected.value = true
      connecting.value = false
      connectionError.value = null
      retryCount.value = 0
      const base = `launa/${settings.value.deviceId}`
      c.subscribe(`${base}/state`)
      c.subscribe(`${base}/availability`)
      c.subscribe(`${base}/diagnostics`)
      c.subscribe(`${base}/alert`)
    })

    c.on('message', (topic, message) => {
      const payload = message.toString()
      const base = `launa/${settings.value.deviceId}`

      if (topic === `${base}/state`) {
        try {
          spaState.value = JSON.parse(payload)
          // Sync mode flags from device state
          if (typeof spaState.value.self_test === 'boolean') {
            selfTestEnabled.value = spaState.value.self_test
          }
          if (typeof spaState.value.sniff_mode === 'boolean') {
            sniffEnabled.value = spaState.value.sniff_mode
          }
          // Clear all pending keys on every state update — the real state has arrived
          if (pendingKeys.value.size > 0) {
            pendingKeys.value = new Set()
          }
        } catch {}
      } else if (topic === `${base}/availability`) {
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

    c.on('error', (err) => {
      connectionError.value = err.message
      connecting.value = false
    })

    c.on('close', () => {
      connected.value = false
      connecting.value = false
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
    if (client.value) {
      client.value.end(true)
      client.value = null
      connected.value = false
      connecting.value = false
    }
  }

  function publish(subtopic, payload) {
    if (!client.value || !connected.value) return
    const topic = `launa/${settings.value.deviceId}/command/${subtopic}`
    client.value.publish(topic, String(payload), { qos: 1 })
  }

  // Map command subtopics to the state key they affect
  const _stateKeyMap = {
    pump1: 'pump1_on', pump2: 'pump2_on', pump3: 'pump3_on',
    pump4: 'pump4_on', pump5: 'pump5_on', pump6: 'pump6_on',
    light1: 'light1', light2: 'light2', light3: 'light3', light4: 'light4',
    blower: 'blower', circulation_pump: 'circ_pump', mister: 'mister',
    hold_mode: 'hold_mode',
    heat_mode: 'heating_mode',
    temp_range: 'temp_range',
  }

  function toggle(subtopic) {
    const key = _stateKeyMap[subtopic]
    if (key) {
      const next = new Set(pendingKeys.value)
      next.add(key)
      pendingKeys.value = next
    }
    publish(subtopic, true)
  }

  function setTemperature(temp) {
    const next = new Set(pendingKeys.value)
    next.add('set_temp')
    pendingKeys.value = next
    publish('set_temperature', String(temp))
  }

  function isPending(key) {
    return pendingKeys.value.has(key)
  }

  function setSelfTest(enabled) {
    selfTestEnabled.value = enabled
    publish('self_test', enabled ? 'ON' : 'OFF')
  }

  function setSniff(enabled) {
    sniffEnabled.value = enabled
    publish('sniff', enabled ? 'ON' : 'OFF')
  }

  onMounted(() => {
    fetchServerConfig()
    connect()
  })

  onUnmounted(() => {
    disconnect()
  })

  async function fetchServerConfig() {
    try {
      const res = await fetch('/api/config')
      if (res.ok) {
        serverConfig.value = await res.json()
      }
    } catch {}
  }

  async function saveAccessoryConfig(newCfg) {
    try {
      const res = await fetch('/api/config', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(newCfg),
      })
      if (res.ok) {
        serverConfig.value = await res.json()
      }
    } catch {}
  }

  const visibleControls = computed(() => {
    const cfg = serverConfig.value
    if (!cfg) return {}
    const vc = {}
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
