# kamimusuhi intercom relay

Cloudflare Worker + Durable Object mailbox for sister-to-sister envelopes
between the individuals hosted on peer nodes (see
`crates/kamimusuhi-resident/src/intercom.rs`).

Nodes normally deliver envelopes directly (`POST /v1/intercom`). The relay
is the fallback for peers that cannot be reached directly — and this
node's own inbound path when it sits somewhere unreachable. The sender
posts to `/v1/send`, the recipient polls `/v1/inbox` on `relay.poll_secs`
and confirms with `/v1/ack`. Both paths converge on the same deduplicated
accept path on the resident, so redelivery is always safe.

## Trust boundary

- One shared bearer token authenticates all resident↔relay calls —
  `RELAY_TOKEN` on the worker, `KAMIMUSUHI_RELAY_TOKEN` (the name is
  `relay.token_env`) in each resident's `secrets.env`.
- The token gates mailbox operations; it does not prove *which* node
  sent an envelope. Envelope `from`/`to` are claims inside a shared-trust
  fleet — the same model as `KAMIMUSUHI_NODE_TOKEN` between residents.
- Message bodies are private conversation. They pass through Cloudflare
  as opaque JSON but are not end-to-end encrypted; do not route traffic
  you would not put in front of this account.
- Each recipient node owns one Durable Object; messages are deleted on
  `ack`, expire after 7 days, and the mailbox is capped at 512 entries.

## Deploy

```sh
cd relay
npm install
npx wrangler login                 # once, or CLOUDFLARE_API_TOKEN
npx wrangler secret put RELAY_TOKEN # the shared bearer token
npm run typecheck                  # tsc --noEmit
npm run dry-run                    # wrangler deploy --dry-run (no deploy)
npm run deploy                     # actually publish (explicit approval)
```

Then give each resident a relay section:

```json
"intercom": {
  "relay": {
    "url": "https://kamimusuhi-intercom-relay.<account>.workers.dev",
    "token_env": "KAMIMUSUHI_RELAY_TOKEN",
    "poll_secs": 15
  }
}
```

`check-config` validates the URL; a resident logs relay poll failures in
`/status` under `intercom.relay.status`.

## Wire protocol

```
POST /v1/send   {"to": "<node>", "envelope": {…}} → {"seq": n}
POST /v1/inbox  {"node": "<node>", "after": n, "limit": m}
                               → {"messages": [{"seq": n, "envelope": {…}}]}
POST /v1/ack    {"node": "<node>", "upto": n}      → {"ok": true}
GET  /health                                       → {"ok": true}
```

`after`/`upto`/`seq` are per-mailbox monotonics — a resident persists its
cursor in `current_state/intercom.json` (`relay_cursor`).
