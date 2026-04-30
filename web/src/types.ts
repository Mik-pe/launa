export interface SpaState {
  current_temp?: number | null
  set_temp?: number | null
  temp_scale?: 'celsius' | 'fahrenheit'
  time_format?: '12h' | '24h'
  temp_range?: 'high' | 'low'
  heating_mode?: 'ready' | 'rest' | 'ready_in_rest'
  is_heating?: boolean
  pump1_on?: boolean
  pump2_on?: boolean
  pump3_on?: boolean
  pump4_on?: boolean
  pump5_on?: boolean
  pump6_on?: boolean
  light1?: boolean
  light2?: boolean
  light3?: boolean
  light4?: boolean
  blower?: boolean
  circ_pump?: boolean
  mister?: boolean
  hold_mode?: boolean
  panel_locked?: boolean
  hour?: number
  minute?: number
  firmware_version?: string
  last_fault?: string
  sniff_mode?: boolean
  wifi_rssi?: number | null
  registration_state?: string
}

export interface MqttSettings {
  brokerUrl: string
  deviceId: string
  username: string
  password: string
}

export interface AccessoryConfig {
  pumps: number
  lights: number
  blower: boolean
  mister: boolean
}

export interface LogEntry {
  level: string
  message: string
  timestamp_ms: number
  received_at: string
}

export interface TimestampedEntry {
  payload: string
  received_at: string
}

export interface AvailabilityEntry {
  status: string
  received_at: string
}

export interface TemperatureSample {
  current_temp: number | null
  set_temp: number | null
  received_at: string
}

export interface ComponentEvent {
  component: string
  state: number
  received_at: string
}

export interface GraphData {
  temperatures: TemperatureSample[]
  components: ComponentEvent[]
}
