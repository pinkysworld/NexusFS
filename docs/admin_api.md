# Admin API (skeleton)

The admin server is embedded and serves:
- static UI (`/`, `/app.js`)
- JSON APIs under `/api/*`
- per-track APIs under `/api/research/<track_id>/*` (future)

---

## Endpoints (implemented)

### GET `/api/status`
Headers:
- `x-nexusfs-token: <token>` (optional in dev mode)

Response:
```json
{
  "head": "hexstring-or-null",
  "device_id": "hexstring",
  "now_ms": 1700000000000
}
```

### GET `/api/fs/head`
Response:
```json
{ "head": "hexstring-or-null" }
```

### GET `/api/oplog/summary`
Response:
```json
{
  "entries": [
    [{ "DeviceId": 123 }, 55]
  ]
}
```

> Note: This is a raw serialized structure in the skeleton. Later, expose a cleaner JSON schema.

---

## Planned endpoints (next)

- `/api/peers`
- `/api/replication/status`
- `/api/storage/stats`
- `/api/energy/telemetry`
- `/api/proofs/stats`
