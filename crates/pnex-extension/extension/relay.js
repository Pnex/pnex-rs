// Isolated world of x.com: forwards what hook_x.js copied to the agent's
// service worker. Only messages of this window, with the hook's marker.
const api = globalThis.browser ?? globalThis.chrome;
window.addEventListener("message", (e) => {
  if (e.source !== window || !e.data?.pnexResponse) return;
  api.runtime.sendMessage({ t: "platform-response", url: e.data.url, body: e.data.body }).catch(() => {});
});
