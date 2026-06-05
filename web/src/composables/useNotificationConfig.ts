import { ref } from 'vue'
import type { NotificationConfig } from '../types'

export function useNotificationConfig() {
  const notificationConfig = ref<NotificationConfig>({
    discord_webhook_url: '',
    offline_threshold_hours: 6,
    monitored_devices: [],
  })

  async function fetchNotificationConfig() {
    try {
      const res = await fetch('/api/notifications')
      if (res.ok) {
        notificationConfig.value = await res.json() as NotificationConfig
      }
    } catch { /* ignore */ }
  }

  async function saveNotificationConfig(newCfg: NotificationConfig) {
    try {
      const res = await fetch('/api/notifications', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(newCfg),
      })
      if (res.ok) {
        notificationConfig.value = await res.json() as NotificationConfig
      }
    } catch { /* ignore */ }
  }

  return {
    notificationConfig,
    fetchNotificationConfig,
    saveNotificationConfig,
  }
}
