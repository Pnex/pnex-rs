// Isolated world of the followed platforms: tells hook.js which responses
// the agent's plan reads, and forwards what it copied to the agent's
// service worker. Only messages of this window, with the hook's marker.
const api = globalThis.browser ?? globalThis.chrome;
const key = `pnexResponses:${location.hostname}`;
api.storage.local.get(key).then((got) => {
  let wanted = [];
  try {
    wanted = JSON.parse(got[key] ?? "[]");
  } catch {}
  window.postMessage({ pnexWanted: wanted }, location.origin);
}, () => {});
window.addEventListener("message", (e) => {
  if (e.source !== window || !e.data?.pnexResponse) return;
  api.runtime.sendMessage({ t: "platform-response", url: e.data.url, body: e.data.body }).catch(() => {});
});
