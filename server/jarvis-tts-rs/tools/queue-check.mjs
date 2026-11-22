import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
const base = process.env.TTS_URL ?? "http://127.0.0.1:8181";
const headers = process.env.JARVIS_BEARER_TOKEN
  ? { Authorization: "Bearer " + process.env.JARVIS_BEARER_TOKEN }
  : {};
const clients = [];
try {
  for (let i = 0; i < 5; i++) {
    const q = http.request(
      base + "/tts/stream",
      {
        method: "POST",
        headers: {
          ...headers,
          "Content-Type": "text/plain",
          "X-Request-Id": "hold-" + i,
        },
      },
      (r) => r.resume(),
    );
    q.on("error", () => {});
    q.write("Жду");
    clients.push(q);
  }
  await delay(150);
  const r = await fetch(base + "/tts", {
    method: "POST",
    headers: { ...headers, "Content-Type": "application/json" },
    body: '{"text":"Тест"}',
  });
  assert.equal(r.status, 429);
  for (let i = 0; i < 5; i++) {
    assert.equal(
      (await fetch(base + "/tts/hold-" + i, { method: "DELETE", headers }))
        .status,
      202,
    );
  }
} finally {
  for (const req of clients) req.destroy();
}
await delay(100);
const r = await fetch(base + "/tts", {
  method: "POST",
  headers: { ...headers, "Content-Type": "application/json" },
  body: '{"text":"Свободно."}',
});
assert.equal(r.status, 200);
assert.equal(
  Buffer.from(await r.arrayBuffer()).toString("ascii", 0, 4),
  "RIFF",
);
console.log(
  "Five unfinished inputs admitted, sixth rejected, all cancelled, next call returned WAV.",
);

// An open utterance holds the model while we enqueue a file followed by a stream.
const held = http.request(base + "/tts/stream", {
  method: "POST",
  headers: { ...headers, "Content-Type": "text/plain" },
  signal: AbortSignal.timeout(15000),
});
held.on("error", () => {});
const started = once(held, "response");
held.write("Начинаем.\n");
try {
  const [response] = await started;
  assert.equal(response.statusCode, 200);
  response.resume();
  const order = [];
  async function enqueue(path, id) {
    const r = await fetch(base + path, {
      method: "POST",
      headers: { ...headers, "Content-Type": "application/json", "X-Request-Id": id },
      body: JSON.stringify({ text: "Проверка очереди." }),
      signal: AbortSignal.timeout(15000),
    });
    assert.equal(r.status, 200);
    order.push(id);
    assert.ok((await r.arrayBuffer()).byteLength > 44);
  }
  const batch = enqueue("/tts", "priority-file");
  await delay(100);
  const interactive = enqueue("/tts/stream", "priority-stream");
  await delay(100);
  held.end();
  await Promise.all([batch, interactive]);
  assert.deepEqual(order, ["priority-stream", "priority-file"]);
  console.log("Queued stream received audio before the earlier queued WAV.");
} finally {
  held.destroy();
}
