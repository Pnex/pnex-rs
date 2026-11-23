// Runs in the pages of the followed platforms (MAIN world, before their
// scripts). Copies the JSON the web app itself receives for an account's
// lists to the extension (window.postMessage → relay.js). Which responses:
// address parts the agent's plan names, handed over by relay.js. It makes
// no request of its own and changes nothing on the page. Data is
// untrusted: the agent parses it again, the server validates it.
(() => {
  let wanted = [];
  window.addEventListener("message", (e) => {
    if (e.source === window && Array.isArray(e.data?.pnexWanted)) {
      wanted = e.data.pnexWanted.filter((w) => typeof w === "string" && w);
    }
  });
  const isWanted = (url) => wanted.some((w) => url.includes(w));
  const MAX = 8 * 1024 * 1024;
  const relay = (url, body) => {
    if (typeof body === "string" && body.length <= MAX) {
      window.postMessage({ pnexResponse: true, url: String(url), body }, location.origin);
    }
  };
  const fetch0 = window.fetch;
  window.fetch = async function (...args) {
    const res = await fetch0.apply(this, args);
    try {
      const url = res.url || String(args[0]?.url ?? args[0]);
      if (isWanted(url)) res.clone().text().then((t) => relay(url, t), () => {});
    } catch {}
    return res;
  };
  const open0 = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    if (isWanted(String(url))) {
      this.addEventListener("load", () => {
        try {
          relay(url, typeof this.response === "string" ? this.response : this.responseText);
        } catch {}
      });
    }
    return open0.call(this, method, url, ...rest);
  };
})();
