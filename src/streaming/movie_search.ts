// Cinejoy's frontend uses this public TMDB catalog credential. No user account
// or browser session is read. Keep this integration separate from playback.
const url = new URL("https://api.themoviedb.org/3/search/movie");
url.search = new URLSearchParams({
  api_key: "8476a7ab80ad76f0936744df0430e67c",
  query: Deno.args[0],
  include_adult: "false",
  language: "en-US",
  page: "1",
}).toString();
const response = await fetch(url, {
  signal: AbortSignal.timeout(10000),
  redirect: "error",
  headers: { Referer: "https://cinejoy.pk/" },
});
if (!response.ok) throw Error(`Movie search returned HTTP ${response.status}`);
const reader = response.body.getReader();
const chunks = [];
let length = 0;
while (true) {
  const { value, done } = await reader.read();
  if (done) break;
  length += value.length;
  if (length > 1024 * 1024) {
    await reader.cancel();
    throw Error("Movie search response exceeds limit");
  }
  chunks.push(value);
}
const body = new Uint8Array(length);
let offset = 0;
for (const chunk of chunks) {
  body.set(chunk, offset);
  offset += chunk.length;
}
const json = JSON.parse(new TextDecoder().decode(body));
if (!Array.isArray(json.results)) {
  throw Error("Movie search returned invalid results");
}
console.log(
  JSON.stringify(
    json.results.slice(0, 20).filter((m) =>
      Number.isSafeInteger(m.id) && m.id > 0 && typeof m.title === "string" &&
      !m.adult
    ).map((m) => ({
      id: m.id,
      title: m.title.slice(0, 160),
      year: String(m.release_date || "").slice(0, 4),
      poster_path: typeof m.poster_path === "string"
        ? m.poster_path.slice(0, 180)
        : null,
    })),
  ),
);
