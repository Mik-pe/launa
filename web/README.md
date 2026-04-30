# Launa Web UI

Single-page dashboard for controlling a Balboa BP6013G1 spa via the Launa ESP32 firmware. Built with **Vue 3**, **TypeScript**, **Tailwind CSS**, and **Vite**.

The app connects to the [launa-server](../crates/launa-server) MQTT broker over WebSocket and provides real-time spa state, temperature control, pump/light toggles, diagnostics, logs, and more.

## Features

- **Control** — set temperature, toggle pumps/lights/blower, switch heat mode and temp range
- **Status** — live spa state dashboard with sensor readings
- **History** — temperature chart with configurable time range
- **Logs / Alerts / Diagnostics** — server-backed log and alert viewers with polling
- **Sniff** — raw RS-485 frame viewer for protocol debugging
- **Settings** — MQTT broker URL, device ID, accessory config, sniff toggle

## Tech Stack

| Layer | Technology |
|---|---|
| Framework | Vue 3 (`<script setup>` SFCs) |
| Language | TypeScript |
| Styling | Tailwind CSS v4 |
| Build | Vite 8 |
| MQTT | mqtt.js (WebSocket transport) |
| Package manager | [Bun](https://bun.sh) or npm |

## Development

Install dependencies and start the dev server:

```bash
bun install
bun run dev
```

Or with npm:

```bash
npm install
npm run dev
```

The Vite dev server runs on `http://0.0.0.0:5173` and proxies `/api` requests to `http://localhost:8080` (the launa-server HTTP API). The app connects to the MQTT broker at `ws://<hostname>:9001` by default (configurable in Settings).

### Developing with launa-server

To run the web UI alongside launa-server with auto-rebuild/restart:

```bash
bun run dev:sim
```

This uses `concurrently` to watch both the Vite dev server and the server binary.

## Production Build

```bash
bun run build
```

This runs `vue-tsc --noEmit` for type checking, then `vite build`. The output is written to `dist/`.

To preview the production build locally:

```bash
bun run preview
```

## Server Connection

| Environment | API | MQTT |
|---|---|---|
| **Development** | Vite dev server proxies `/api` → `http://localhost:8080` | Direct WebSocket to `ws://<hostname>:9001` |
| **Production** | Same-origin `/api` served by launa-server | Direct WebSocket to `ws://<hostname>:9001` |

In production, launa-server serves the built `dist/` assets and the `/api` routes from the same host/port, so no proxy is needed.

## Project Structure

```
web/src/
├── App.vue                 # Main app shell, tab navigation
├── main.ts                 # Vue app entry point
├── types.ts                # TypeScript interfaces (SpaState, MqttSettings, etc.)
├── style.css               # Tailwind CSS import
├── env.d.ts                # Vite/Vue type shims
├── components/
│   ├── AlertsView.vue      # Alert log viewer
│   ├── ConnectionBar.vue   # Top connection status bar
│   ├── ControlsPanel.vue   # Pump/light/blower toggle grid
│   ├── DiagnosticsView.vue # Diagnostic log viewer
│   ├── LoadingSpinner.vue  # Loading indicator
│   ├── LogViewer.vue       # Server log viewer
│   ├── PendingDot.vue      # Pending-state indicator dot
│   ├── SelectControl.vue   # Dropdown select control
│   ├── SettingsModal.vue   # MQTT + accessory config modal
│   ├── SniffFramesView.vue # Raw frame viewer
│   ├── StatusDashboard.vue # Full status dashboard
│   ├── TemperatureCard.vue # Temperature display + setter
│   └── TemperatureChart.vue# Temperature history chart
├── composables/
│   ├── useAccessoryConfig.ts # Fetch/save accessory config from server API
│   ├── useApi.ts            # REST API helpers (logs, alerts, diagnostics, sniff)
│   ├── useMqtt.ts           # Main MQTT composable (connects all pieces)
│   ├── useMqttConnection.ts # MQTT client lifecycle + reconnection
│   └── useSpaState.ts       # Spa state tracking + optimistic pending keys
└── utils/
    └── format.ts            # Time-ago formatting, payload parsing
```

## Scripts

| Script | Description |
|---|---|
| `dev` | Start Vite dev server |
| `build` | Type-check and build for production |
| `preview` | Preview production build locally |
| `typecheck` | Run `vue-tsc --noEmit` |
| `serve` | Serve `dist/` on port 8080 |
| `sim` | Build web UI and run launa-server |
| `dev:sim` | Run Vite + launa-server concurrently with watch |
| `start` | Build web UI and run launa-server |
