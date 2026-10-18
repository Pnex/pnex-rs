// Records the video the agent's tab is playing (D260): captureStream on the
// page's <video> (mozCaptureStream on Firefox), MediaRecorder in WebM at a
// bounded bitrate, 4 s chunks sent to the service worker over a port. Runs
// in the isolated world; injected on demand by the agent, never by itself.
// Protected (EME/DRM) media is never recorded.
(() => {
  const api = globalThis.browser ?? globalThis.chrome;
  if (globalThis.__pnexRecorder) return;
  globalThis.__pnexRecorder = true;

  const b64 = (buffer) => {
    const bytes = new Uint8Array(buffer);
    let s = "";
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };

  api.runtime.onMessage.addListener((msg) => {
    if (msg?.t !== "pnex-record") return;
    const port = api.runtime.connect({ name: "pnex-recorder" });
    const done = (reason) => port.postMessage({ t: "done", id: msg.id, reason });
    const video = [...document.querySelectorAll("video")].sort(
      (a, b) => b.videoWidth * b.videoHeight - a.videoWidth * a.videoHeight,
    )[0];
    if (!video) return done("no-video");
    if (video.mediaKeys) return done("ext-drm-protected");
    const capture = video.captureStream ?? video.mozCaptureStream;
    if (!capture) return done("no-capture");
    video.muted = true;
    // A video is recorded from its start (or where its previous part
    // stopped); a live from now on.
    if (!msg.live) video.currentTime = (msg.fromMs ?? 0) / 1000;
    video.play().catch(() => {});
    const stream = capture.call(video);
    const mime = ["video/webm;codecs=vp9,opus", "video/webm;codecs=vp8,opus", "video/webm"].find((m) =>
      MediaRecorder.isTypeSupported(m),
    );
    const recorder = new MediaRecorder(stream, {
      mimeType: mime,
      videoBitsPerSecond: msg.bitrate,
      audioBitsPerSecond: 64000,
    });
    let pausedSince = 0;
    let reason = "ended";
    // Chunks and the end travel in order, whatever arrayBuffer() takes.
    let sent = Promise.resolve();
    recorder.ondataavailable = (e) => {
      if (!e.data.size) return;
      sent = sent.then(async () =>
        port.postMessage({ t: "chunk", id: msg.id, data: b64(await e.data.arrayBuffer()) }),
      );
    };
    recorder.onstop = () => {
      sent = sent.then(() => done(reason));
    };
    const stop = (why) => {
      reason = why;
      clearInterval(watch);
      if (recorder.state !== "inactive") recorder.stop();
    };
    const started = Date.now();
    const watch = setInterval(() => {
      if (video.ended) return stop("ended");
      if (Date.now() - started >= msg.maxMs) return stop("max-duration");
      if (video.paused) {
        pausedSince ||= Date.now();
        if (Date.now() - pausedSince > 60000) stop("paused");
      } else {
        pausedSince = 0;
      }
    }, 1000);
    recorder.start(4000);
  });
})();
