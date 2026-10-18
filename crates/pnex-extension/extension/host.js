// Browser glue of the PNeX browser agent: thin wrappers over the extension
// APIs, WebCrypto and IndexedDB. Every decision lives in the Rust code
// (src/), compiled to wasm; this file only reaches what wasm cannot.

const api = globalThis.browser ?? globalThis.chrome;

// ── HTTP ────────────────────────────────────────────────────────────────

/**
 * Returns JSON text `{"status": n, "body": "..."}`; throws on network errors.
 * `extra`: JSON object of more headers (signed uploads), or "".
 */
export async function http(method, url, bearer, org, body, extra) {
  const headers = extra ? JSON.parse(extra) : {};
  if (bearer) headers["Authorization"] = `Bearer ${bearer}`;
  if (org) headers["X-Org-Id"] = org;
  if (body) headers["Content-Type"] = "application/json";
  const res = await fetch(url, { method, headers, body: body || undefined, credentials: "omit" });
  return JSON.stringify({ status: res.status, body: await res.text() });
}

export function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function now_ms() {
  return Date.now();
}

/** `n` random bytes, base64url without padding. */
export function random_b64(n) {
  return b64url(crypto.getRandomValues(new Uint8Array(n)));
}

/** SHA-256 of a string, base64url without padding (PKCE S256). */
export async function sha256_b64(text) {
  return b64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))));
}

function b64url(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// ── Storage ─────────────────────────────────────────────────────────────

export async function store_get(key) {
  const got = await api.storage.local.get(key);
  return got[key] ?? null;
}

export async function store_set(key, value) {
  await api.storage.local.set({ [key]: value });
}

export async function store_remove(key) {
  await api.storage.local.remove(key);
}

/** Live state for the options page (session storage: gone with the browser). */
export function set_status(json) {
  api.storage.session?.set({ "pnex.status": json }).catch(() => {});
}

/** `server.json` shipped in the zip by the PNeX server it was downloaded from. */
export async function bundled_server() {
  try {
    const res = await fetch(api.runtime.getURL("server.json"));
    return res.ok ? (await res.json()).server ?? null : null;
  } catch {
    return null;
  }
}

// ── Install key (D266): ECDSA P-256, private key never extractable ──────

function keyStore() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open("pnex-agent", 1);
    req.onupgradeneeded = () => req.result.createObjectStore("keys");
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function idb(db, mode, run) {
  return new Promise((resolve, reject) => {
    const req = run(db.transaction("keys", mode).objectStore("keys"));
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function installPair() {
  const db = await keyStore();
  let pair = await idb(db, "readonly", (s) => s.get("install"));
  if (!pair) {
    pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
    await idb(db, "readwrite", (s) => s.put(pair, "install"));
  }
  return pair;
}

/** Raw public key (65 bytes), base64url. */
export async function install_key() {
  const pair = await installPair();
  return b64url(new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey)));
}

/** ECDSA P-256 SHA-256 signature (64 bytes r‖s), base64url. */
export async function sign(message) {
  const pair = await installPair();
  const sig = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    pair.privateKey,
    new TextEncoder().encode(message),
  );
  return b64url(new Uint8Array(sig));
}

/** What the browser tells about itself (JSON of `AgentFingerprint`). */
export function fingerprint() {
  const ua = navigator.userAgentData;
  const brands = (ua?.brands ?? []).filter((b) => !/Not.?A.?Brand/i.test(b.brand));
  // "Google Chrome", "Microsoft Edge"… before the "Chromium" every one lists.
  const brand = brands.find((b) => b.brand !== "Chromium") ?? brands[0];
  const firefox = /Firefox\/([\d.]+)/.exec(navigator.userAgent);
  return JSON.stringify({
    browser: brand?.brand ?? (firefox ? "Firefox" : ""),
    browser_version: brand?.version ?? firefox?.[1] ?? "",
    os: ua?.platform ?? navigator.platform ?? "",
    arch: "",
    language: navigator.language ?? "",
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone ?? "",
    screen: globalThis.screen ? `${screen.width}x${screen.height}` : "",
    extension_version: api.runtime.getManifest().version,
  });
}

// ── Tabs ────────────────────────────────────────────────────────────────

/** Foreground tab (sign-in). */
export async function open_tab(url) {
  return (await api.tabs.create({ url, active: true })).id;
}

export async function close_tab(id) {
  await api.tabs.remove(id).catch(() => {});
}

function loaded(id, timeoutMs) {
  return new Promise((resolve) => {
    const done = () => {
      api.tabs.onUpdated.removeListener(listener);
      clearTimeout(timer);
      resolve();
    };
    const listener = (tabId, info) => {
      if (tabId === id && info.status === "complete") done();
    };
    const timer = setTimeout(done, timeoutMs);
    api.tabs.onUpdated.addListener(listener);
  });
}

/**
 * The agent's tab of a platform (D255): navigates the existing one, or opens
 * a background tab in the "PNeX" group. Resolves once loaded (30 s cap).
 */
export async function agent_tab(id, url) {
  let tab = null;
  if (id >= 0) tab = await api.tabs.get(id).catch(() => null);
  const wait = (tabId) => loaded(tabId, 30000);
  if (tab) {
    const ready = wait(tab.id);
    await api.tabs.update(tab.id, { url });
    await ready;
    return tab.id;
  }
  tab = await api.tabs.create({ url, active: false });
  const ready = wait(tab.id);
  await groupTab(tab.id);
  await ready;
  return tab.id;
}

async function groupTab(tabId) {
  if (!api.tabs.group || !api.tabGroups) return; // Firefox: no tab groups
  try {
    const { "pnex.group": known } = await api.storage.session.get("pnex.group");
    const exists = known != null && (await api.tabGroups.get(known).catch(() => null));
    const groupId = await api.tabs.group(exists ? { groupId: known, tabIds: [tabId] } : { tabIds: [tabId] });
    if (!exists) {
      await api.tabGroups.update(groupId, { title: "PNeX", color: "blue", collapsed: true });
      await api.storage.session.set({ "pnex.group": groupId });
    }
  } catch {
    // Grouping is cosmetic.
  }
}

/** Reads one element of the tab: JSON `{"found": bool, "value": "..."}`. */
export async function read_value(tabId, selector, attr) {
  const [result] = await api.scripting.executeScript({
    target: { tabId },
    args: [selector, attr],
    func: (sel, how) => {
      const el = document.querySelector(sel);
      if (!el) return { found: false, value: "" };
      const value = how.startsWith("attr:") ? el.getAttribute(how.slice(5)) ?? "" : el.textContent ?? "";
      return { found: true, value: value.trim().slice(0, 256) };
    },
  });
  return JSON.stringify(result?.result ?? { found: false, value: "" });
}

/** Readable text of the tab: JSON `{"url", "title", "text"}` (1 MB cap). */
export async function read_page(tabId) {
  const [result] = await api.scripting.executeScript({
    target: { tabId },
    func: () => ({
      url: location.href,
      title: document.title ?? "",
      text: (document.body?.innerText ?? "").slice(0, 1000000),
    }),
  });
  return JSON.stringify(result?.result ?? { url: "", title: "", text: "" });
}

/** Scrolls the tab by most of a screen, like a reader would. */
export async function scroll(tabId) {
  await api.scripting.executeScript({
    target: { tabId },
    func: () => window.scrollBy({ top: Math.round(window.innerHeight * 0.8), behavior: "smooth" }),
  });
}

/** Data a page embeds before any request (YouTube's `ytInitialData`), JSON or "null". */
export async function read_initial_data(tabId) {
  const [result] = await api.scripting.executeScript({
    target: { tabId },
    world: "MAIN",
    func: () => {
      try {
        return JSON.stringify(window.ytInitialData ?? null);
      } catch {
        return "null";
      }
    },
  });
  return result?.result ?? "null";
}

// Downloaded files stay on this side: wasm only signs their digest.
const blobs = new Map();
let nextBlob = 1;

/** Downloads `url`: JSON `{"id", "sha256", "size"}` (hex digest); throws on HTTP errors. */
export async function fetch_blob(url, maxBytes) {
  const res = await fetch(url, { credentials: "omit" });
  if (!res.ok) throw new Error(`download ${res.status}`);
  const bytes = new Uint8Array(await res.arrayBuffer());
  if (bytes.length === 0 || bytes.length > maxBytes) throw new Error(`size ${bytes.length}`);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const id = nextBlob++;
  blobs.set(id, bytes);
  const sha256 = Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join("");
  return JSON.stringify({ id, sha256, size: bytes.length });
}

/** POSTs a downloaded file, then forgets it: JSON `{"status", "body"}`. */
export async function upload_blob(id, url, bearer, extra) {
  const bytes = blobs.get(id);
  blobs.delete(id);
  if (!bytes) throw new Error("no such download");
  const headers = { ...JSON.parse(extra), Authorization: `Bearer ${bearer}`, "Content-Type": "video/mp4" };
  const res = await fetch(url, { method: "POST", headers, body: bytes, credentials: "omit" });
  return JSON.stringify({ status: res.status, body: await res.text() });
}

/** Origin pattern of a page (`https://host/*`). */
function originOf(url) {
  return `${new URL(url).origin}/*`;
}

export async function has_origin(url) {
  return api.permissions.contains({ origins: [originOf(url)] });
}

// ── Websocket ───────────────────────────────────────────────────────────

const sockets = new Map();
let nextSocket = 1;

/** Opens a websocket; resolves with its handle once open, rejects on failure. */
export function ws_open(url, onText, onClose) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const id = nextSocket++;
    let open = false;
    ws.onopen = () => {
      open = true;
      sockets.set(id, ws);
      resolve(id);
    };
    ws.onmessage = (e) => {
      if (typeof e.data === "string") onText(e.data);
    };
    ws.onclose = (e) => {
      sockets.delete(id);
      if (open) onClose(e.code);
      else reject(new Error(`websocket closed (${e.code})`));
    };
  });
}

export function ws_send(id, text) {
  sockets.get(id)?.send(text);
}

/** Closes without calling back: the wasm closures may be gone by then. */
export function ws_close(id) {
  const ws = sockets.get(id);
  if (!ws) return;
  ws.onmessage = null;
  ws.onclose = null;
  sockets.delete(id);
  ws.close();
}
