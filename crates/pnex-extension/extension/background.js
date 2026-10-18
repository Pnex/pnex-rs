// Service worker of the PNeX browser agent. Listeners are registered
// synchronously (MV3 wakes the worker for them); each one makes sure the
// wasm agent loop runs. The open websocket and its pings keep the worker
// alive; the alarm restarts it after the browser suspended it.
import init, { on_response, start } from "./pkg/pnex_extension.js";

const api = globalThis.browser ?? globalThis.chrome;
const ready = init().then(() => start());
const wake = () => ready.then(start);

api.alarms.create("pnex-wake", { periodInMinutes: 1 });
api.alarms.onAlarm.addListener(wake);
api.runtime.onStartup.addListener(wake);
api.runtime.onInstalled.addListener(wake);
api.runtime.onMessage.addListener((msg, sender) => {
  if (msg?.t === "wake") wake();
  // Platform JSON relayed by our content scripts; the agent reads only its own tab.
  if (msg?.t === "platform-response" && sender.tab?.id != null && sender.id === api.runtime.id) {
    ready.then(() => on_response(sender.tab.id, String(msg.url), String(msg.body)));
  }
});
api.action.onClicked.addListener(() => api.runtime.openOptionsPage());

// Recordings sent by recorder.js, chunk by chunk (base64), kept here for
// the agent (host.js reads the same registry).
globalThis.__pnexRecordings ??= new Map();
api.runtime.onConnect.addListener((port) => {
  if (port.name !== "pnex-recorder" || port.sender?.id !== api.runtime.id) return;
  port.onMessage.addListener((msg) => {
    const rec = globalThis.__pnexRecordings.get(msg?.id);
    if (!rec) return;
    if (msg.t === "chunk") {
      const bin = atob(msg.data);
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      rec.size += bytes.length;
      // Past the cap the recording is cut, never grown without bound.
      if (rec.size <= rec.max) rec.chunks.push(bytes);
      else rec.reason ??= "max-size";
    } else if (msg.t === "done") {
      rec.reason ??= String(msg.reason ?? "ended");
      rec.done = true;
    }
  });
});
