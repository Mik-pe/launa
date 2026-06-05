# Google Home Integration Plan

Direct Google Home support for launa_server without Home Assistant, using the
Google Smart Home Cloud-to-cloud API.

## Overview

launa_server already runs an axum HTTP server with a `MemoryStore` that holds
all spa state (temperatures, pump/light/blower states, device status). We add
a Google Smart Home fulfillment endpoint that maps this state to Google device
types/traits, and publishes commands to the MQTT broker that the ESP32 firmware
already subscribes to.

```
┌──────────┐         ┌──────────────┐         ┌───────────────┐
│  Google   │  HTTPS  │ Cloudflare   │  HTTP   │ launa_server  │
│  Home     │────────►│ Tunnel (free)│────────►│ (axum)        │
│  Cloud    │◄────────│ *.yourdomain │◄────────│               │
└──────────┘         └──────────────┘         │  /smarthome   │
                                                │  /auth/*      │
                                                │               │
                                                │  MemoryStore  │
                                                │  MQTT bridge  │
                                                └───────┬───────┘
                                                        │ MQTT
                                                        ▼
                                                ┌──────────────┐
                                                │  ESP32       │
                                                │  (launa app) │
                                                └──────────────┘
```

## Estimated Effort

| Component | Time | Notes |
|---|---|---|
| Cloudflare Tunnel + DNS | 30 min | Free tier, no port forwarding |
| Actions on Google + Cloud Console | 1 hr | Project + OAuth credentials |
| OAuth 2.0 server (minimal, single user) | 2-3 hr | Login page + code/token endpoints |
| SYNC intent handler | 1-2 hr | JSON mapping of device types |
| QUERY intent handler | 1 hr | Read from MemoryStore |
| EXECUTE intent handler | 2-3 hr | Publish to MQTT command topics |
| Report State (optional) | 1-2 hr | Push state to Home Graph API |
| Testing + debugging | 2-4 hr | Google's error messages are vague |
| **Total** | **~10-16 hr** | |

Cost: $0 (Cloudflare free, Actions on Google free for personal use).

---

## Phase 1: Infrastructure Setup

### 1.1 Cloudflare Tunnel

**Option A: Free quick tunnel (no domain needed)**

```bash
# Install cloudflared
brew install cloudflared

# One-liner — generates a random public URL on every start
cloudflared tunnel --url http://localhost:8080
```

This gives you a URL like `https://random-words.trycloudflare.com`. It changes
every time you restart, so you'll need to update the Actions on Google
fulfillment URL each time. Fine for testing, annoying for production.

**Option B: Named tunnel (requires a domain, or free Cloudflare domain)**

```bash
# Authenticate (opens browser)
cloudflared tunnel login

# Create tunnel
cloudflared tunnel create launa

# Configure ~/.cloudflared/config.yml
# tunnel: <tunnel-id>
# credentials-file: ~/.cloudflared/<tunnel-id>.json
# ingress:
#   - hostname: spa.<yourdomain>.com
#     service: http://localhost:8080
#   - service: http_status:404

# Create DNS record
cloudflared tunnel route dns launa spa.<yourdomain>.com

# Run (or install as launchd service)
cloudflared tunnel run launa
```

Stable URL. If you don't have a domain, register a cheap one (~$10/yr) through
Cloudflare Registrar, or use any domain you already own.

The tunnel gives you a public HTTPS URL (e.g. `https://spa.yourdomain.com`) that
routes to launa_server's HTTP port. No port forwarding, no SSL certs to manage.

**WAF note:** If you enable Cloudflare WAF/firewall rules, whitelist Google's
ASN (AS15169) for the fulfillment and auth endpoints. The HA community thread
has a working IP list:
https://community.home-assistant.io/t/how-to-connect-google-assistant-using-the-cloudflare-tunnel/545574

### 1.2 Actions on Google Project

1. Go to `console.actions.google.com` > New Project
2. Name it "Launa Spa" (or whatever)
3. Choose **Smart Home** action type
4. Set fulfillment URL to `https://spa.<yourdomain>.com/smarthome`
5. Enable account linking:
   - Authorization URL: `https://spa.<yourdomain>.com/auth`
   - Token URL: `https://spa.<yourdomain>.com/auth/token`
   - OAuth client ID/secret: from Google Cloud Console (below)
6. Before clicking Test, fill in "Actions directory information":
   - Description: "Connect Google Assistant to a private Launa spa server"
   - Privacy policy: public Google Doc saying "personal use only"
   - Logo: any image
7. Click **Test** to create a draft version (stays in testing mode indefinitely)

### 1.3 Google Cloud Console

1. Go to `console.cloud.google.com` > APIs & Services > Credentials
2. Create OAuth 2.0 Client ID (web application)
3. Add authorized redirect URI: `https://oauth-redirect.googleusercontent.com/r/<your-project-id>`
4. Note the **Client ID** and **Client Secret** — launa_server needs these
5. Configure OAuth consent screen (do NOT upload logo or it triggers verification):
   - User type: External
   - Add your Gmail as a test user
   - Only fill required fields

### 1.4 Service Account (for Report State)

1. In Google Cloud Console > IAM > Service Accounts
2. Create a service account with role "Smart Home Developer"
3. Generate a JSON key file — launa_server uses this to call Home Graph API
4. Enable the **HomeGraph API** in Google Cloud Console

---

## Phase 2: OAuth 2.0 Server

Google requires OAuth account linking even for personal use. We implement a
minimal version with a single hardcoded user.

### Endpoints

| Endpoint | Method | Purpose |
|---|---|---|
| `/auth` | GET | Login page (HTML form) |
| `/auth/login` | POST | Validate credentials, redirect with auth code |
| `/auth/token` | POST | Exchange auth code for JWT access token |

### Configuration

Add to `launa.toml` and `Config`:

```toml
[google_home]
enabled = true
oauth_client_id = "xxx.apps.googleusercontent.com"
oauth_client_secret = "GOCSPX-xxx"
username = "admin"
password_hash = "bcrypt-hash-here"
jwt_secret = "random-32-byte-hex-string"
service_account_key_path = "/path/to/service-account.json"
```

### Implementation

- **`/auth`**: Serves a minimal HTML login form with username/password fields.
  The `redirect_uri` and `state` query params from Google are passed through.
- **`/auth/login`**: Validates credentials against config. On success, generates
  a random authorization code, stores it (in MemoryStore or a HashMap with TTL),
  and redirects to `redirect_uri?code=<code>&state=<state>`.
- **/auth/token`**: Accepts `grant_type=authorization_code` with the code from
  the login step. Validates the code, checks `client_id`/`client_secret` match
  config. Returns a JWT access token.

Use `jsonwebtoken` crate for JWTs. Tokens include the username as the `sub`
claim and expire after 24 hours.

### Dependencies to add

```toml
jsonwebtoken = "9"
bcrypt = "0.16"
rand = "0.8"
```

---

## Phase 3: Smart Home Fulfillment

### Endpoint

`POST /smarthome` — single handler that dispatches based on intent.

Google sends requests like:
```json
{
  "requestId": "ff36a3cc-ec34-11e6-b1a0-64510650abcf",
  "inputs": [{
    "intent": "action.devices.SYNC"
  }]
}
```

The handler validates the Bearer token from the `Authorization` header, then
dispatches to the appropriate intent handler.

### 3.1 SYNC Intent

Returns the list of devices with their types, traits, and attributes.

#### Device Mapping

| Google Device | Type | Traits | HA Equivalent |
|---|---|---|---|
| Spa Thermostat | `action.devices.types.THERMOSTAT` | `TemperatureSetting` | number.set_temperature + sensor.temperature |
| Pump 1-6 | `action.devices.types.SWITCH` | `OnOff` | switch.pump1-6 |
| Light 1-4 | `action.devices.types.LIGHT` | `OnOff` | light.light1-4 |
| Blower | `action.devices.types.FAN` | `OnOff` | fan.blower |
| Mister | `action.devices.types.SWITCH` | `OnOff` | switch.mister |
| Circulation Pump | `action.devices.types.SWITCH` | `OnOff` | switch.circulation_pump |

#### SYNC Response Structure

```json
{
  "requestId": "...",
  "payload": {
    "agentUserId": "launa-user",
    "devices": [
      {
        "id": "spa_001_thermostat",
        "type": "action.devices.types.THERMOSTAT",
        "traits": ["action.devices.traits.TemperatureSetting"],
        "name": { "name": "Spa Temperature" },
        "attributes": {
          "availableThermostatModes": ["heat", "off"],
          "thermostatTemperatureUnit": "F",
          "thermostatTemperatureRange": {
            "minThresholdCelsius": 10,
            "maxThresholdCelsius": 40
          }
        },
        "willReportState": true
      },
      {
        "id": "spa_001_pump1",
        "type": "action.devices.types.SWITCH",
        "traits": ["action.devices.traits.OnOff"],
        "name": { "name": "Spa Pump 1" },
        "willReportState": true
      }
    ]
  }
}
```

For Celsius users, set `thermostatTemperatureUnit: "C"` and adjust ranges.

**Note:** The thermostat device uses `heat`/`off` modes because the Balboa
controller only heats — there's no cooling. `heat` = heating enabled, `off` =
heating disabled (spa in rest mode or powered off).

#### Device count

A typical spa with 2 pumps, 1 light, blower, mister = **7 Google devices**.
With all accessories (6 pumps, 4 lights) = **13 Google devices**. Keep it under
the Google limit (no issue — they support hundreds).

### 3.2 QUERY Intent

Returns current state for requested devices. All data is already in `MemoryStore`.

```json
// Request
{
  "requestId": "...",
  "inputs": [{
    "intent": "action.devices.QUERY",
    "payload": {
      "devices": [
        { "id": "spa_001_thermostat" },
        { "id": "spa_001_pump1" }
      ]
    }
  }]
}

// Response
{
  "requestId": "...",
  "payload": {
    "devices": {
      "spa_001_thermostat": {
        "status": "SUCCESS",
        "thermostatMode": "heat",
        "thermostatTemperatureAmbient": 37.8,
        "thermostatTemperatureSetpoint": 40.0
      },
      "spa_001_pump1": {
        "status": "SUCCESS",
        "on": true
      }
    }
  }
}
```

Implementation reads from the latest `TemperatureSample` and `ComponentEvent`
data in `MemoryStore`. Need to add a method to `MemoryStore` that returns the
latest state snapshot (current temp, set temp, all component states).

### 3.3 EXECUTE Intent

Handles commands by publishing to MQTT command topics that the ESP32 already
subscribes to.

```json
// Request: "Set spa to 104 degrees"
{
  "inputs": [{
    "intent": "action.devices.EXECUTE",
    "payload": {
      "commands": [{
        "devices": [{ "id": "spa_001_thermostat" }],
        "execution": [{
          "command": "action.devices.commands.ThermostatTemperatureSetpoint",
          "params": { "thermostatTemperatureSetpoint": 40.0 }
        }]
      }]
    }
  }]
}

// Request: "Turn on pump 1"
{
  "inputs": [{
    "intent": "action.devices.EXECUTE",
    "payload": {
      "commands": [{
        "devices": [{ "id": "spa_001_pump1" }],
        "execution": [{
          "command": "action.devices.commands.OnOff",
          "params": { "on": true }
        }]
      }]
    }
  }]
}
```

#### Command Mapping

| Google Command | MQTT Topic | Payload |
|---|---|---|
| `ThermostatTemperatureSetpoint` | `launa/{device_id}/command/set_temperature` | `104` (Fahrenheit integer) |
| `ThermostatSetMode` heat | `launa/{device_id}/command/heat_mode` | `ready` |
| `ThermostatSetMode` off | `launa/{device_id}/command/heat_mode` | `rest` |
| `OnOff` (pump N) | `launa/{device_id}/command/pumpN` | `true` / `false` |
| `OnOff` (light N) | `launa/{device_id}/command/lightN` | `true` / `false` |
| `OnOff` (blower) | `launa/{device_id}/command/blower` | `true` / `false` |
| `OnOff` (mister) | `launa/{device_id}/command/mister` | `true` / `false` |
| `OnOff` (circ pump) | `launa/{device_id}/command/circulation_pump` | `true` / `false` |

These are the exact command topics the ESP32 firmware already subscribes to
(via `launa-mqtt` command parser). The payloads (`true`/`false` for toggles,
integer for temperature) match the existing command format.

### 3.4 DISCONNECT Intent

Google sends this when the user unlinks their account. Just return an empty
response and clean up any stored tokens.

---

## Phase 4: Report State (Optional but Recommended)

Without Report State, Google polls your fulfillment endpoint via QUERY every
time the user asks "is the spa heating?". With Report State, you proactively
push state changes to Google's Home Graph API, enabling:

- Real-time state in the Google Home app (no stale data)
- Faster voice responses (Google doesn't need to call QUERY)
- Visual state tiles in the Google Home app

### Implementation

When `handle_state()` in `mqtt_bridge.rs` processes a state update, also call
Google's Home Graph API:

```
POST https://homegraph.googleapis.com/v1/devices:reportStateAndNotification
Authorization: Bearer <service-account-access-token>

{
  "requestId": "...",
  "agentUserId": "launa-user",
  "payload": {
    "devices": {
      "states": {
        "spa_001_thermostat": {
          "thermostatMode": "heat",
          "thermostatTemperatureAmbient": 37.8,
          "thermostatTemperatureSetpoint": 40.0
        },
        "spa_001_pump1": { "on": true }
      }
    }
  }
}
```

Use the service account JSON key to obtain an access token via Google's OAuth2
(`reqwest` is already a dependency). Cache the token (it lasts ~1 hour).

Can be deferred to a later phase — the integration works without it, just with
more QUERY polling latency.

---

## Phase 5: Code Structure

### New files in `crates/launa-server/src/`

```
google_home/
├── mod.rs          — Module root, public API
├── oauth.rs        — OAuth 2.0 endpoints (/auth, /auth/login, /auth/token)
├── fulfillment.rs  — POST /smarthome handler (SYNC/QUERY/EXECUTE/DISCONNECT)
├── sync.rs         — SYNC intent: device definitions and attributes
├── query.rs        — QUERY intent: state snapshot from MemoryStore
├── execute.rs      — EXECUTE intent: command dispatch to MQTT
└── report_state.rs — Home Graph API state reporting (Phase 4)
```

### Changes to existing files

| File | Change |
|---|---|
| `web.rs` | Add `/smarthome`, `/auth/*` routes to `build_router()` |
| `memory.rs` | Add `get_latest_state()` method for QUERY |
| `lib.rs` | Add `google_home` module, extend `Config` with Google Home fields |
| `mqtt_bridge.rs` | Hook Report State into `handle_state()` (Phase 4) |
| `Cargo.toml` | Add `jsonwebtoken`, `bcrypt`, `rand` dependencies |

### New dependencies

```toml
jsonwebtoken = "9"      # JWT token generation/validation
bcrypt = "0.16"         # Password hashing for OAuth login
rand = "0.8"            # Random authorization code generation
```

`reqwest` is already included (used for Discord webhooks). `serde` and
`serde_json` are already included.

---

## Voice Commands (after setup)

Once linked, you'll be able to say:

- "Hey Google, set the spa to 104 degrees"
- "Hey Google, turn on pump 1"
- "Hey Google, turn on the spa lights"
- "Hey Google, turn on the blower"
- "Hey Google, is the spa heating?"
- "Hey Google, what's the spa temperature?"
- "Hey Google, turn off the spa" (sets heat mode to rest)

---

## Gotchas and Lessons Learned

1. **OAuth consent screen:** Do NOT upload a logo. It triggers Google's
   verification process which takes 2-3 days and requires a privacy policy
   review. Just leave it blank.

2. **Actions directory info:** You must fill in the description, privacy policy,
   and contact email before the Test button will work.

3. **Project language:** Set the Actions project language to English regardless
   of your location. Other languages can cause SYNC failures.

4. **No HTTPS, no Google:** Google will not call HTTP endpoints. Cloudflare
   Tunnel handles this for free.

5. **Draft/testing mode:** Your Actions project stays in "testing" mode
   indefinitely. No certification needed for personal use. Only your Google
   account can link to it.

6. **Temperature units:** The Balboa protocol uses Fahrenheit by default. The
   Google `TemperatureSetting` trait requires temperatures in Celsius. Convert
   in the QUERY/EXECUTE handlers: `°C = (°F - 32) × 5/9`.

7. **Command topic payloads:** The ESP32 command parser expects `true`/`false`
   for toggle commands and an integer for set_temperature. Google sends
   different formats — the EXECUTE handler must translate.

8. **Google Home app testing:** Use "Try in Google Assistant" in the Actions
   console for initial testing, then link via the Google Home app on your phone
   (Add > Set up device > Works with Google > search for your action name).

## Reference Links

- Google Smart Home API docs: https://developers.home.google.com/cloud-to-cloud/get-started
- TemperatureSetting trait: https://developers.home.google.com/cloud-to-cloud/traits/temperaturesetting
- OnOff trait: https://developers.home.google.com/cloud-to-cloud/traits/onoff
- Cloudflare Tunnel + Google Assistant (HA community): https://community.home-assistant.io/t/how-to-connect-google-assistant-using-the-cloudflare-tunnel/545574
- Smart Home codelab: https://developers.home.google.com/codelabs/smarthome-washer
