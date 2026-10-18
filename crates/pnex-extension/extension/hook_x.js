// Runs in the page of x.com (MAIN world, before its scripts). Copies the
// JSON the X web app receives for an account's timeline to the extension
// (via window.postMessage → relay.js). It makes no request of its own and
// changes nothing on the page. Data is untrusted: the agent parses it, the
// server validates it.
(() => {
  const WANTED = /\/graphql\/[^/]+\/(UserTweets|UserMedia|UserTweetsAndReplies)\b/;
  const MAX = 4 * 1024 * 1024;
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
      if (WANTED.test(url)) res.clone().text().then((t) => relay(url, t), () => {});
    } catch {}
    return res;
  };
  const open0 = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    if (WANTED.test(String(url))) {
      this.addEventListener("load", () => {
        try {
          relay(url, typeof this.response === "string" ? this.response : this.responseText);
        } catch {}
      });
    }
    return open0.call(this, method, url, ...rest);
  };
})();
