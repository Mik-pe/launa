import { ref, shallowRef } from 'vue'
import mqtt from 'mqtt'
import type { IClientOptions, MqttClient } from 'mqtt'
import type { MqttSettings } from '../types'

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

export function useMqttConnection() {
  const client = shallowRef<MqttClient | null>(null)
  const connected = ref(false)
  const connecting = ref(false)
  const connectionError = ref<string | null>(null)
  const initialConnect = ref(true)
  const retryCount = ref(0)
  const settings = ref<MqttSettings>(loadSettings())

  function createClient(): MqttClient {
    if (client.value) destroyClient()

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
    })

    c.on('offline', () => {
      connected.value = false
    })

    c.on('reconnect', () => {
      connecting.value = true
      retryCount.value++
    })

    client.value = c
    return c
  }

  function destroyClient() {
    if (client.value) {
      client.value.end(true)
      client.value = null
      connected.value = false
      connecting.value = false
    }
  }

  function persistSettings(s: MqttSettings) {
    settings.value = { ...s }
    localStorage.setItem('launa-settings', JSON.stringify(s))
    connectionError.value = null
  }

  return {
    client,
    connected,
    connecting,
    connectionError,
    initialConnect,
    retryCount,
    settings,
    createClient,
    destroyClient,
    persistSettings,
  }
}
