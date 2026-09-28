/**
 * kamimusuhi intercom relay — a mailbox for sister-to-sister envelopes.
 *
 * Nodes normally deliver `POST /v1/intercom` envelopes directly. When a
 * peer is unreachable, the sender's resident drops the envelope here and
 * the recipient's resident picks it up on its `poll_secs` timer. Both
 * paths converge on the same accept path, so relaying is idempotent.
 *
 * All operations are POSTs and authenticate with the shared bearer token
 * (`RELAY_TOKEN`, set with `wrangler secret put`). The relay is a dumb
 * mailbox: it validates envelope shape and bounds, not identity beyond
 * the token — node ids in envelopes are claims.
 *
 *   POST /v1/send   {"to": "<node>", "envelope": {...}}  → {"seq": n}
 *   POST /v1/inbox  {"node": "<node>", "after": n, "limit": m}
 *                                    → {"messages": [{"seq": n, "envelope": {...}}]}
 *   POST /v1/ack    {"node": "<node>", "upto": n}        → {"ok": true}
 *   GET  /health                                        → {"ok": true}
 *
 * Each recipient node owns one Durable Object mailbox; entries are
 * deleted on ack. A crash between ingest and ack just redelivers — the
 * resident deduplicates by envelope id.
 */

const MAX_BODY_BYTES = 64 * 1024;
const MAX_ENVELOPE_BODY = 32 * 1024;
const MAX_ID_CHARS = 128;
const MAX_NODE_CHARS = 64;
const MAILBOX_KEEP = 512;
const MAILBOX_TTL_MS = 7 * 24 * 3600 * 1000;
const INBOX_LIMIT = 100;

const ID_RE = /^[A-Za-z0-9\-_.@]{1,128}$/;
const NODE_RE = /^[A-Za-z0-9_-]{1,64}$/;

export interface Env {
  MAILBOX: DurableObjectNamespace;
  RELAY_TOKEN: string;
}

interface Envelope {
  id: string;
  from: string;
  to: string;
  body: string;
  conversation?: string;
  in_reply_to?: string;
  hop?: number;
  kind?: string;
  sent_at?: number;
  auto_left?: number;
  auto?: boolean;
}

interface Stored {
  seq: number;
  envelope: Envelope;
  stored_at: number;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function timingSafeEqual(a: string, b: string): boolean {
  const x = new TextEncoder().encode(a);
  const y = new TextEncoder().encode(b);
  if (x.length !== y.length) return false;
  let diff = 0;
  for (let i = 0; i < x.length; i++) diff |= x[i] ^ y[i];
  return diff === 0;
}

function authorized(request: Request, env: Env): boolean {
  const token = request.headers.get("Authorization") ?? "";
  return token.startsWith("Bearer ") && timingSafeEqual(token.slice(7), env.RELAY_TOKEN ?? "");
}

function validId(v: unknown): v is string {
  return typeof v === "string" && v.length <= MAX_ID_CHARS && ID_RE.test(v);
}

function validNode(v: unknown): v is string {
  return typeof v === "string" && NODE_RE.test(v) && v.length <= MAX_NODE_CHARS;
}

function validEnvelope(e: unknown): e is Envelope {
  if (typeof e !== "object" || e === null) return false;
  const env = e as Record<string, unknown>;
  return (
    validId(env.id) &&
    validNode(env.from) &&
    validNode(env.to) &&
    typeof env.body === "string" &&
    env.body.length > 0 &&
    env.body.length <= MAX_ENVELOPE_BODY &&
    (env.conversation === undefined || validId(env.conversation)) &&
    (env.in_reply_to === undefined || validId(env.in_reply_to))
  );
}

async function readJson(request: Request): Promise<Record<string, unknown> | Response> {
  const length = Number(request.headers.get("Content-Length") ?? "0");
  if (length > MAX_BODY_BYTES) return json({ error: "request too large" }, 413);
  try {
    const text = await request.text();
    if (text.length > MAX_BODY_BYTES) return json({ error: "request too large" }, 413);
    const body = JSON.parse(text);
    if (typeof body !== "object" || body === null) throw new Error("not an object");
    return body as Record<string, unknown>;
  } catch {
    return json({ error: "invalid JSON" }, 400);
  }
}

function mailbox(env: Env, node: string): DurableObjectStub {
  return env.MAILBOX.get(env.MAILBOX.idFromName(node));
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/health") {
      return json({ ok: true, service: "kamimusuhi-intercom-relay" });
    }
    if (!authorized(request, env)) {
      return json({ error: "bearer token required" }, 401);
    }
    if (request.method !== "POST") {
      return json({ error: "POST only" }, 405);
    }
    const body = await readJson(request);
    if (body instanceof Response) return body;
    switch (url.pathname) {
      case "/v1/send": {
        const to = body.to;
        const envelope = body.envelope;
        if (!validNode(to)) return json({ error: "bad `to`" }, 400);
        if (!validEnvelope(envelope)) return json({ error: "bad envelope" }, 400);
        if (envelope.to !== to) return json({ error: "envelope `to` mismatch" }, 400);
        const stub = mailbox(env, to);
        const res = await stub.fetch("https://mailbox/send", {
          method: "POST",
          body: JSON.stringify({ envelope }),
        });
        return new Response(res.body, res);
      }
      case "/v1/inbox": {
        const node = body.node;
        const after = body.after;
        const limit = body.limit;
        if (!validNode(node)) return json({ error: "bad `node`" }, 400);
        if (after !== undefined && (typeof after !== "number" || after < 0 || !Number.isInteger(after))) {
          return json({ error: "bad `after`" }, 400);
        }
        if (limit !== undefined && (typeof limit !== "number" || !Number.isInteger(limit))) {
          return json({ error: "bad `limit`" }, 400);
        }
        const stub = mailbox(env, node);
        const res = await stub.fetch("https://mailbox/inbox", {
          method: "POST",
          body: JSON.stringify({ after: after ?? 0, limit: Math.min(limit ?? INBOX_LIMIT, INBOX_LIMIT) }),
        });
        return new Response(res.body, res);
      }
      case "/v1/ack": {
        const node = body.node;
        const upto = body.upto;
        if (!validNode(node)) return json({ error: "bad `node`" }, 400);
        if (typeof upto !== "number" || upto < 0 || !Number.isInteger(upto)) {
          return json({ error: "bad `upto`" }, 400);
        }
        const stub = mailbox(env, node);
        const res = await stub.fetch("https://mailbox/ack", {
          method: "POST",
          body: JSON.stringify({ upto }),
        });
        return new Response(res.body, res);
      }
      default:
        return json({ error: "not found" }, 404);
    }
  },
} satisfies ExportedHandler<Env>;

/** Per-recipient-node mailbox: ordered, acked, bounded. */
export class Mailbox implements DurableObject {
  constructor(private state: DurableObjectState) {}

  private async nextSeq(): Promise<number> {
    const seq = ((await this.state.storage.get<number>("seq")) ?? 0) + 1;
    await this.state.storage.put("seq", seq);
    return seq;
  }

  private async entries(): Promise<Stored[]> {
    const map = await this.state.storage.list<Stored>({ prefix: "m/" });
    return [...map.values()];
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const body = (await request.json()) as Record<string, unknown>;
    switch (url.pathname) {
      case "/send": {
        const envelope = body.envelope as Envelope;
        // Idempotent on envelope.id: a resident retries when the relay
        // response was lost.
        const existing = await this.entries();
        const dup = existing.find((m) => m.envelope.id === envelope.id);
        if (dup) return json({ seq: dup.seq, dedup: true });
        const seq = await this.nextSeq();
        await this.state.storage.put(`m/${String(seq).padStart(12, "0")}`, {
          seq,
          envelope,
          stored_at: Date.now(),
        } satisfies Stored);
        // Bounded mailbox: evict expired, then oldest over the cap.
        const now = Date.now();
        let kept = await this.entries();
        const stale = kept.filter((m) => now - m.stored_at > MAILBOX_TTL_MS);
        for (const m of stale) {
          await this.state.storage.delete(`m/${String(m.seq).padStart(12, "0")}`);
        }
        kept = kept.filter((m) => now - m.stored_at <= MAILBOX_TTL_MS);
        if (kept.length > MAILBOX_KEEP) {
          const drop = kept.slice(0, kept.length - MAILBOX_KEEP);
          await this.state.storage.delete(drop.map((m) => `m/${String(m.seq).padStart(12, "0")}`));
        }
        return json({ seq });
      }
      case "/inbox": {
        const after = (body.after as number) ?? 0;
        const limit = (body.limit as number) ?? INBOX_LIMIT;
        const now = Date.now();
        const all = await this.entries();
        const stale = all.filter((m) => now - m.stored_at > MAILBOX_TTL_MS);
        for (const m of stale) {
          await this.state.storage.delete(`m/${String(m.seq).padStart(12, "0")}`);
        }
        const messages = all
          .filter((m) => m.seq > after && now - m.stored_at <= MAILBOX_TTL_MS)
          .slice(0, limit)
          .map((m) => ({ seq: m.seq, envelope: m.envelope }));
        return json({ messages });
      }
      case "/ack": {
        const upto = body.upto as number;
        const doomed = (await this.entries())
          .filter((m) => m.seq <= upto)
          .map((m) => `m/${String(m.seq).padStart(12, "0")}`);
        if (doomed.length > 0) await this.state.storage.delete(doomed);
        return json({ ok: true, deleted: doomed.length });
      }
      default:
        return json({ error: "not found" }, 404);
    }
  }
}
