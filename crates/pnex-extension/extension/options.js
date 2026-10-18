// Options page: sign-in, pairing, state of the agent. UI glue only; the
// work is done by the wasm module (src/wasm.rs). Texts come from _locales.
import init, { login, pair, server_hint, unpair } from "./pkg/pnex_extension.js";

const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);
const msg = (key, ...subs) => api.i18n.getMessage(key, subs) || key;
let session = null; // { server, access_token } — memory only

for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = msg(el.dataset.i18n);

function fail(code) {
  const known = msg(`error_${String(code).replace(/-/g, "_")}`);
  $("error").textContent = known.startsWith("error_") ? msg("error_generic", String(code)) : known;
  $("error").hidden = false;
}

async function render() {
  const creds = (await api.storage.local.get("pnex.creds"))["pnex.creds"];
  const status = JSON.parse((await api.storage.session.get("pnex.status"))["pnex.status"] ?? "{}");
  $("setup").hidden = !!creds;
  $("paired").hidden = !creds;
  if (!creds) return;
  const c = JSON.parse(creds);
  $("name").textContent = c.name;
  $("paired-server").textContent = c.server;
  $("link").textContent = msg(status.connected ? "link_up" : "link_down");
  const missions = status.missions ?? [];
  $("no-missions").hidden = missions.length > 0;
  $("missions").replaceChildren(
    ...missions.map((m) => {
      const li = document.createElement("li");
      // An agent state code reads as a sentence; a value stays as read.
      const outcome = m.outcome?.startsWith("ext-") ? msg(`state_${m.outcome.replace(/-/g, "_")}`) : m.outcome;
      li.textContent = outcome ? `${m.target} — ${outcome}` : m.target;
      return li;
    }),
  );
  const origins = status.missing_origins ?? [];
  $("permissions").hidden = origins.length === 0;
  $("origins").replaceChildren(
    ...origins.map((origin) => {
      const li = document.createElement("li");
      li.textContent = `${origin} `;
      const allow = document.createElement("button");
      allow.textContent = msg("allow");
      allow.onclick = async () => {
        if (await api.permissions.request({ origins: [`${origin}/*`] })) api.runtime.sendMessage({ t: "wake" });
      };
      li.append(allow);
      return li;
    }),
  );
}

async function busy(button, run) {
  $("error").hidden = true;
  button.disabled = true;
  try {
    await run();
  } catch (e) {
    fail(e?.message ?? e);
  } finally {
    button.disabled = false;
  }
}

$("login").onclick = () =>
  busy($("login"), async () => {
    const server = $("server").value.trim().replace(/\/+$/, "");
    let origin;
    try {
      origin = new URL(server).origin;
    } catch {
      throw "server-invalid";
    }
    // The agent talks to its server from the extension: allow that origin.
    if (!(await api.permissions.request({ origins: [`${origin}/*`] }))) throw "permission-denied";
    const result = JSON.parse(await login(server));
    session = { server, access_token: result.access_token };
    $("org").replaceChildren(
      ...result.orgs.map((o) => new Option(o.name, o.id)),
    );
    $("orgs").hidden = false;
  });

$("pair").onclick = () =>
  busy($("pair"), async () => {
    await pair(session.server, session.access_token, Number($("org").value));
    session = null;
    api.runtime.sendMessage({ t: "wake" });
    await render();
  });

$("unpair").onclick = () =>
  busy($("unpair"), async () => {
    await unpair();
    await render();
  });

api.storage.onChanged.addListener(() => render());
await init();
$("server").value = (await server_hint()) ?? "";
await render();
