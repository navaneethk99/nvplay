// Cinejoy's public request protocol, independently implemented using WebCrypto.
// Protocol reference: github.com/MADPANDA3D/pandaflix/core/providers/cinejoy.go (GPL-3.0).
// No browser, account cookies, ads, or remote JavaScript are loaded.
const API = "https://api.wing.st";
const ENCODER_SHA256 =
  "40c923580779e2a850fc5ab3f0046be565ed717883cf4547ed3a87de2450bdcf";
const source = new URL(Deno.args[0]);
const match = source.pathname.match(/^\/watch\/movie\/(\d+)\/?$/);
if (
  !["cinejoy.pk", "www.cinejoy.pk", "cinejoy.to", "www.cinejoy.to"].includes(
    source.hostname,
  ) || !match
) {
  throw Error("Expected a Cinejoy movie watch URL");
}
const origin = source.origin;
const headers = {
  Origin: origin,
  Referer: origin + "/",
  "User-Agent": "Mozilla/5.0",
};
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const deadline = AbortSignal.timeout(30000);
function httpsURL(value, base) {
  const url = new URL(value, base);
  if (
    url.protocol !== "https:" || url.username || url.password ||
    !url.hostname.includes(".") ||
    /^[\d.]+$/.test(url.hostname) || url.hostname.startsWith("[") ||
    /\.(local|internal)$/.test(url.hostname)
  ) {
    throw Error("Invalid stream address");
  }
  return url.href;
}
async function get(url, body) {
  try {
    return await getOnce(url, body);
  } catch (error) {
    // A transient fetch/timeout gets one retry within the same total deadline.
    if (
      deadline.aborted || !["TimeoutError", "TypeError"].includes(error.name)
    ) throw error;
    return await getOnce(url, body);
  }
}
async function getOnce(url, body) {
  const response = await fetch(httpsURL(url), {
    method: body ? "POST" : "GET",
    headers: body
      ? { ...headers, "Content-Type": "text/plain;charset=UTF-8" }
      : headers,
    body,
    signal: AbortSignal.any([deadline, AbortSignal.timeout(10000)]),
    redirect: "error",
  });
  if (!response.ok) {
    throw Error(`Cinejoy server returned HTTP ${response.status}`);
  }
  const reader = response.body.getReader();
  const parts = [];
  let size = 0;
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > 4 * 1024 * 1024) {
      await reader.cancel();
      throw Error("Cinejoy response exceeds limit");
    }
    parts.push(value);
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return bytes;
}
const wasm = await get(API + "/crush.wasm");
const hash = Array.from(
  new Uint8Array(await crypto.subtle.digest("SHA-256", wasm)),
  (n) => n.toString(16).padStart(2, "0"),
).join("");
if (hash !== ENCODER_SHA256) {
  throw Error(
    "Cinejoy request encoder changed; nvplay needs an adapter update",
  );
}
const module = await WebAssembly.compile(wasm);
if (WebAssembly.Module.imports(module).length) {
  throw Error("Cinejoy encoder has unexpected host imports");
}
async function resolve(path, payload) {
  const e = (await WebAssembly.instantiate(module)).exports;
  const body = encoder.encode(JSON.stringify({ path, payload }));
  const seed = crypto.getRandomValues(new Uint8Array(44));
  const capacity = body.length + 512;
  const p = e.alloc(body.length),
    s = e.alloc(seed.length),
    o = e.alloc(capacity);
  new Uint8Array(e.memory.buffer, p, body.length).set(body);
  new Uint8Array(e.memory.buffer, s, seed.length).set(seed);
  const n = e.seal_request(p, body.length, s, seed.length, o, capacity);
  if (n <= 98 || n > capacity) throw Error("Cinejoy request encoding failed");
  const sealed = new Uint8Array(e.memory.buffer, o, n).slice();
  const wire = await get(API + "/g", sealed.slice(98));
  if (wire.length < 28) throw Error("Invalid Cinejoy response");
  const key = await crypto.subtle.importKey(
    "raw",
    sealed.slice(0, 32),
    "AES-GCM",
    false,
    ["decrypt"],
  );
  const aad = new Uint8Array([
    ...encoder.encode("lumen-gate-v2"),
    0,
    2,
    sealed[32],
    ...sealed.slice(33, 98),
  ]);
  const plain = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: wire.slice(0, 12), additionalData: aad },
    key,
    wire.slice(12),
  );
  const envelope = JSON.parse(decoder.decode(plain));
  if (envelope.status && (envelope.status < 200 || envelope.status >= 300)) {
    throw Error(`Cinejoy resolver status ${envelope.status}`);
  }
  return envelope.data?.stream || [];
}
function attributes(line) {
  return Object.fromEntries(
    Array.from(
      line.matchAll(/([A-Z0-9-]+)=("[^"]*"|[^,]*)/g),
      (m) => [m[1], m[2].replace(/^"|"$/g, "")],
    ),
  );
}
// Select before opening with FFmpeg: probing every language can take minutes.
function selectPlaylist(text, base, maxHeight) {
  if (!text.startsWith("#EXTM3U")) {
    throw Error("Cinejoy did not return an HLS playlist");
  }
  const lines = text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  const variants = [];
  for (let i = 0; i < lines.length - 1; i++) {
    if (lines[i].startsWith("#EXT-X-STREAM-INF:")) {
      const a = attributes(lines[i]);
      variants.push({
        line: lines[i],
        url: httpsURL(lines[i + 1], base),
        a,
        height: Number(a.RESOLUTION?.split("x")[1] || 0),
      });
    }
  }
  if (!variants.length) throw Error("Cinejoy HLS master has no video variants");
  variants.sort((a, b) => b.height - a.height);
  const variant = variants.find((v) => v.height <= maxHeight) ||
    variants.at(-1);
  const audio = lines.filter((l) => l.startsWith("#EXT-X-MEDIA:")).map(
    (line) => ({ line, a: attributes(line) }),
  )
    .filter((x) => x.a.TYPE === "AUDIO" && x.a["GROUP-ID"] === variant.a.AUDIO);
  const selected = audio.find((x) => x.a.DEFAULT === "YES") || audio[0];
  const output = ["#EXTM3U", "#EXT-X-VERSION:7"];
  if (selected) {
    const url = httpsURL(selected.a.URI, base);
    if (/["\r\n]/.test(url)) throw Error("Invalid audio address");
    output.push(selected.line.replace(/URI="[^"]*"/, `URI="${url}"`));
  }
  output.push(variant.line, variant.url);
  const result = output.join("\n") + "\n";
  if (result.length > 12000) throw Error("Cinejoy playlist exceeds limit");
  return result;
}
// Availability means more than a nonempty resolver response: follow the
// selected video/audio playlists and prove their first media is reachable.
async function probeMedia(url, depth = 0) {
  if (depth > 2) throw Error("HLS playlist nesting exceeds limit");
  const text = decoder.decode(await get(url));
  if (!text.startsWith("#EXTM3U")) throw Error("Not an HLS media playlist");
  const lines = text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  const first = lines.find((l) => !l.startsWith("#"));
  if (!first) throw Error("HLS media playlist is empty");
  if (lines.some((l) => l.startsWith("#EXT-X-STREAM-INF:"))) {
    return await probeMedia(httpsURL(first, url), depth + 1);
  }
  if (
    lines.some((l) => l.startsWith("#EXT-X-KEY:") && !l.includes("METHOD=NONE"))
  ) {
    throw Error("Encrypted HLS availability cannot be verified");
  }
  const map = lines.find((l) => l.startsWith("#EXT-X-MAP:"));
  const targets = [first];
  if (map) {
    const init = attributes(map).URI;
    if (init) targets.unshift(init);
  }
  for (const target of targets) {
    const response = await fetch(httpsURL(target, url), {
      headers: { ...headers, Range: "bytes=0-1023" },
      signal: AbortSignal.any([deadline, AbortSignal.timeout(8000)]),
      redirect: "error",
    });
    if (!response.ok) {
      await response.body?.cancel();
      throw Error(`Media returned HTTP ${response.status}`);
    }
    const reader = response.body.getReader();
    let size = 0;
    const prefix = [];
    try {
      while (size < 1024) {
        const { value, done } = await reader.read();
        if (done) break;
        if (prefix.length < 100) {
          prefix.push(...value.slice(0, 100 - prefix.length));
        }
        size += value.length;
      }
    } finally {
      await reader.cancel();
    }
    const head = decoder.decode(new Uint8Array(prefix)).trimStart()
      .toLowerCase();
    if (size < 188 || head.startsWith("<") || head.startsWith("{")) {
      throw Error("Media is missing or returned an error page");
    }
  }
}
async function verifyPlaylist(playlist) {
  const lines = playlist.split("\n");
  const video = lines.find((l) => l.startsWith("https://"));
  if (!video) throw Error("No video playlist");
  const audio = lines.find((l) => l.startsWith("#EXT-X-MEDIA:"));
  await probeMedia(video);
  if (audio) await probeMedia(httpsURL(attributes(audio).URI));
}
const catalog = JSON.parse(decoder.decode(await get(API + "/servers")));
const ready = new Set(
  catalog.servers?.filter((s) => s.status === "ok").map((s) => s.name),
);
const errors = [];
for (const server of ["Nebula", "Lisbon", "Solara"]) {
  if (!ready.has(server)) continue;
  try {
    const payload = { tmdb: match[1] };
    let streams = await resolve(`/${server}/movie`, payload);
    let stream = streams.find((s) => /^https:\/\//.test(s.playlist));
    if (!stream) {
      const embedded = streams.find((s) =>
        typeof s.id === "string" || typeof s.id === "number"
      );
      if (embedded) {
        streams = await resolve(`/${server}/movie`, {
          ...payload,
          embed: embedded.id,
        });
      }
      stream = streams.find((s) => /^https:\/\//.test(s.playlist));
    }
    if (!stream) throw Error("No playable HLS stream");
    const url = httpsURL(stream.playlist);
    const playlist = selectPlaylist(
      decoder.decode(await get(url)),
      url,
      Number(Deno.args[1]) || 1080,
    );
    if (Deno.args[2] === "check") await verifyPlaylist(playlist);
    console.log(JSON.stringify({ playlist, referer: origin + "/", server }));
    Deno.exit(0);
  } catch (error) {
    errors.push(`${server}: ${String(error.message).slice(0, 500)}`);
  }
}
throw Error(errors.join("; ") || "No Cinejoy servers are available");
