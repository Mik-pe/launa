import { onMounted, onUnmounted, ref, watch, computed } from 'vue'
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

function formatLastSeen(isoStr: string): string {
  try {
    const d = new Date(isoStr)
    const now = new Date()
    const diffMs = now.getTime() - d.getTime()
    const diffMin = Math.floor(diffMs / 60000)
    if (diffMin < 1) return 'just now'
    if (diffMin < 60) return `${diffMin}m ago`
    const diffHrs = Math.floor(diffMin / 60)
    if (diffHrs < 24) return `${diffHrs}h ${diffMin % 60}m ago`
    const diffDays = Math.floor(diffHrs / 24)
    return `${diffDays}d ${diffHrs % 24}h ago`
  } catch {
    return isoStr
  }
}

export type ConnectionStatus =
  | 'connecting'        // Browser connecting to broker for the first time
  | 'reconnecting'      // Browser lost connection, retrying
  | 'online'            // Everything connected, device reporting online
  | 'offline'           // Browser connected to broker, but device is offline/pending

export interface ConnectionInfo {
  status: ConnectionStatus
  label: string
  color: string
  tooltip: string
  lastSeen: string | null
  error: string | null
}

export function useMqtt() {
  const conn = useMqttConnection()
  const spa = useSpaState()
  const accessory = useAccessoryConfig()
  const lastSeen = ref<string | null>(null)

  async function fetchLastSeen() {
    try {
      const deviceId = conn.settings.value.deviceId
      const apiUrl = `${window.location.origin}/api/devices/${encodeURIComponent(deviceId)}/availability`
      const res = await fetch(apiUrl)
      if (res.ok) {
        const data = await res.json()
        if (data?.updated_at) {
          lastSeen.value = formatLastSeen(data.updated_at)
        }
      }
    } catch { /* ignore */ }
  }

  // Fetch "last seen" from server whenever device is not online
  watch(spa.availability, async (status) => {
    if (status !== 'online') {
      await fetchLastSeen()
    } else {
      lastSeen.value = null
    }
  }, { immediate: true })

  // Single source of truth for connection status
  const connectionInfo = computed<ConnectionInfo>(() => {
    const connecting = conn.connecting.value
    const connected = conn.connected.value
    const initialConnect = conn.initialConnect.value
    const availability = spa.availability.value
    const ls = lastSeen.value
    const err = conn.connectionError.value

    // Browser is trying to connect to broker
    if (connecting) {
      if (initialConnect) {
        return {
          status: 'connecting',
          label: 'Connecting...',
          color: 'bg-amber-400',
          tooltip: 'Connecting to broker...',
          lastSeen: null,
          error: err,
        }
      }
      return {
        status: 'reconnecting',
        label: 'Reconnecting...',
        color: 'bg-blue-400',
        tooltip: ls ? `Lost connection with broker, reconnecting — Last seen ${ls}` : 'Lost connection with broker, reconnecting',
        lastSeen: ls,
        error: err,
      }
    }

    // Browser connected to broker — check device status
    if (connected) {
      if (availability === 'online') {
        return {
          status: 'online',
          label: 'Online',
          color: 'bg-emerald-400',
          tooltip: '',
          lastSeen: null,
          error: null,
        }
      }

      // Device offline or pending (availability = 'offline', 'pending', or unknown)
      return {
        status: 'offline',
        label: 'Offline',
        color: 'bg-red-400',
        tooltip: ls ? `Device offline — Last seen ${ls}` : 'Device offline',
        lastSeen: ls,
        error: null,
      }
    }

    // Not connecting and not connected — shouldn't normally happen since
    // mqtt.js auto-reconnects, but handle it gracefully
    return {
      status: 'connecting',
      label: 'Connecting...',
      color: 'bg-amber-400',
      tooltip: 'Connecting to broker...',
      lastSeen: ls,
      error: err,
    }
  })

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
      c.subscribe(`${base}/boot`)
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

  // Convenience: is the device fully online and usable?
  const isOnline = computed(() => connectionInfo.value.status === 'online')

  return {
    connectionInfo,
    isOnline,
    spaState: spa.spaState,
    alert: spa.alert,
    settings: conn.settings,
    retryCount: conn.retryCount,
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
