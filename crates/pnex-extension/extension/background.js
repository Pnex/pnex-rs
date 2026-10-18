// Service worker of the PNeX browser agent. Listeners are registered
// synchronously (MV3 wakes the worker for them); each one makes sure the
// wasm agent loop runs. The open websocket and its pings keep the worker
// alive; the alarm restarts it after the browser suspended it.
import init, { start } from "./pkg/pnex_extension.js";

const api = globalThis.browser ?? globalThis.chrome;
const ready = init().then(() => start());
const wake = () => ready.then(start);

api.alarms.create("pnex-wake", { periodInMinutes: 1 });
api.alarms.onAlarm.addListener(wake);
api.runtime.onStartup.addListener(wake);
api.runtime.onInstalled.addListener(wake);
api.runtime.onMessage.addListener((msg) => {
  if (msg?.t === "wake") wake();
});
api.action.onClicked.addListener(() => api.runtime.openOptionsPage());
