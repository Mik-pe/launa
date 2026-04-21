import { onMounted, onUnmounted } from 'vue'
import { useMqttConnection } from './useMqttConnection'
import { useSpaState } from './useSpaState'
import { useAccessoryConfig } from './useAccessoryConfig'

/** Shared reactive toast state — consumed by App.vue */
export const connectionErrorToast = { value: '' }

let toastTimer: ReturnType<typeof setTimeout> | null = null

function showDisconnectedToast() {
  connectionErrorToast.value = 'Command dropped — not connected to broker'
  if (toastTimer) clearTimeout(toastTimer)
  toastTimer = setTimeout(() => { connectionErrorToast.value = '' }, 3000)
  console.warn('[MQTT] Command dropped — not connected')
}

export function useMqtt() {
  const conn = useMqttConnection()
  const spa = useSpaState()
  const accessory = useAccessoryConfig()

  function publish(subtopic: string, payload: string | number | boolean) {
    if (!conn.client.value || !conn.connected.value) {
      showDisconnectedToast()
      return
    }
    const topic = `launa/${conn.settings.value.deviceId}/command/${subtopic}`
    conn.client.value.publish(topic, String(payload), { qos: 1 })
  }

  function connect() {
    const c = conn.createClient()

    c.on('connect', () => {
      spa.onConnect()
      const base = `launa/${conn.settings.value.deviceId}`
      c.subscribe(`${base}/state`)
      c.subscribe(`${base}/availability`)
      c.subscribe(`${base}/diagnostics`)
      c.subscribe(`${base}/alert`)
    })

    c.on('message', (topic: string, message: Buffer) => {
      spa.handleMessage(topic, message.toString(), conn.settings.value.deviceId)
    })
  }

  function disconnect() {
    spa.cleanup()
    conn.destroyClient()
  }

  function saveSettings(s: Parameters<typeof conn.persistSettings>[0]) {
    spa.cleanup()
    conn.persistSettings(s)
    connect()
  }

  onMounted(() => {
    accessory.fetchServerConfig()
    connect()
  })

  onUnmounted(() => {
    spa.cleanup()
    disconnect()
  })

  return {
    connected: conn.connected,
    connecting: conn.connecting,
    connectionError: conn.connectionError,
    initialConnect: conn.initialConnect,
    retryCount: conn.retryCount,
    spaState: spa.spaState,
    availability: spa.availability,
    diagnostics: spa.diagnostics,
    alert: spa.alert,
    settings: conn.settings,
    saveSettings,
    connect,
    disconnect,
    publish,
    toggle: (subtopic: string) => spa.toggle(subtopic, publish),
    setTemperature: (temp: number) => spa.setTemperature(temp, publish),
    isPending: spa.isPending,
    visibleControls: accessory.visibleControls,
    serverConfig: accessory.serverConfig,
    saveAccessoryConfig: accessory.saveAccessoryConfig,
    selfTestEnabled: spa.selfTestEnabled,
    setSelfTest: (enabled: boolean) => spa.setSelfTest(enabled, publish),
    sniffEnabled: spa.sniffEnabled,
    setSniff: (enabled: boolean) => spa.setSniff(enabled, publish),
  }
}
