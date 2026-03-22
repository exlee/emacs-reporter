// worker.js
// Requires: R2 bucket bound as DB_BUCKET, KV namespace bound as RATE_LIMIT

const MAX_SIZE_BYTES = 50 * 1024 * 1024;
const RATE_LIMIT = 3;
const RATE_WINDOW_MS = 24 * 60 * 60 * 1000;
const BZIP2_MAGIC = [0x42, 0x5a, 0x68]; // BZh

export default {
  async fetch(request, env) {
    if (request.method !== "PUT") {
      return response(405, "method not allowed");
    }

    // ── Filename validation ───────────────────────────────────────────────────

    const url = new URL(request.url);
    const filename = url.pathname.replace(/^\/+/, "");

    if (!filename.match(/^[0-9a-f-]{36}\.db\.bz2$/)) {
      return response(400, "invalid filename — expected <uuidv7>.db.bz2");
    }

    // ── Rate limiting ─────────────────────────────────────────────────────────

    const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
    const rateLimitKey = `rl:${ip}`;
    const now = Date.now();

    const existing = await env.RATE_LIMIT.get(rateLimitKey, { type: "json" });
    const record = existing ?? { count: 0, window_start: now };

    if (now - record.window_start > RATE_WINDOW_MS) {
      record.count = 0;
      record.window_start = now;
    }

    if (record.count >= RATE_LIMIT) {
      const reset_secs = Math.ceil(
        (record.window_start + RATE_WINDOW_MS - now) / 1000
      );
      return response(429, `rate limit exceeded — resets in ${reset_secs}s`);
    }

    // ── Size check ────────────────────────────────────────────────────────────

    const contentLength = parseInt(request.headers.get("content-length") ?? "0");
    if (contentLength > MAX_SIZE_BYTES) {
      return response(413, `file too large — limit is ${MAX_SIZE_BYTES / 1024 / 1024} MB`);
    }

    // ── Read body + bzip2 magic validation ────────────────────────────────────

    const body = await request.arrayBuffer();

    if (body.byteLength > MAX_SIZE_BYTES) {
      return response(413, `file too large — limit is ${MAX_SIZE_BYTES / 1024 / 1024} MB`);
    }

    if (body.byteLength < 3) {
      return response(400, "file too small");
    }

    const magic = new Uint8Array(body.slice(0, 3));
    if (magic[0] !== BZIP2_MAGIC[0] ||
        magic[1] !== BZIP2_MAGIC[1] ||
        magic[2] !== BZIP2_MAGIC[2]) {
      return response(400, "file does not appear to be bzip2 compressed");
    }

    // ── Store in R2 ───────────────────────────────────────────────────────────

    await env.DB_BUCKET.put(filename, body, {
      httpMetadata: { contentType: "application/x-bzip2" },
      customMetadata: {
        uploaded_at: new Date().toISOString(),
        uploader_ip: ip,
        size_bytes: body.byteLength.toString(),
      },
    });

    // ── Increment rate limit counter only after successful store ──────────────

    record.count += 1;
    const ttl_secs = Math.ceil((record.window_start + RATE_WINDOW_MS - now) / 1000);
    await env.RATE_LIMIT.put(rateLimitKey, JSON.stringify(record), {
      expirationTtl: ttl_secs,
    });

    return response(200, "ok");
  },
};

function response(status, message) {
  return new Response(JSON.stringify({ status, message }), {
    status,
    headers: { "content-type": "application/json" },
  });
}
