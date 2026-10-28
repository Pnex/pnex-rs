/*!
 * Animated "beam sheaf" background (Tron style) — PNeX login page.
 *
 * Static script WITHOUT npm dependency: does not go through js:build/esbuild
 * (reserved for the esptool-js glue), served as is via asset!().
 *
 * Exposes two globals consumed by the bridge `src/tron.rs`:
 *   - window.pnexTronGerbe.mount(hostId) → creates the WebGL2 canvas in the
 *     host div, starts the RAF loop; false if WebGL2 is unavailable (the
 *     fallback CSS gradient then stays visible).
 *   - window.pnexTronGerbe.unmount() → stops the RAF, removes the resize
 *     listener, deletes canvas and GL context (called on Dioxus unmount).
 */
(function () {
  'use strict';

  // Fixed parameters (former demo sliders) — tunable here.
  var CONFIG = {
    spread: 1.35,  // spread at the bottom
    topw: 0.05,    // tightness at the top
    curve: 2.25,   // courbure (1 = droit)
    count: 24,     // number of beams
    lw: 0.16,      // thickness
    top: 0.82,     // height of the focus
    len: 1.75,     // longueur
    speed: 0.15,   // scroll speed of the segments (bottom → focus)
    seg: 3.0,      // segments (dash)
    sway: 0.025,   // souffle (sway)
    glow: 1.1,     // brightness / glow
    seed: 12.7,    // palette seed (fixed → deterministic branding)
  };

  var mounted = null; // current state { canvas, gl, raf, onResize, reduced }

  function compile(gl, type, src) {
    var s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      console.error('tron-gerbe: ' + gl.getShaderInfoLog(s));
      return null;
    }
    return s;
  }

  var VERT = '#version 300 es\n' +
    'in vec2 a_pos;\n' +
    'void main(){ gl_Position = vec4(a_pos, 0.0, 1.0); }\n';

  // Fragment shader: beam sheaf — tight focus at the top, wide fan at the
  // bottom, colored segments flowing along the beams.
  var FRAG = '#version 300 es\n' +
    'precision highp float;\n' +
    'out vec4 fragColor;\n' +
    'uniform vec2  u_res;\n' +
    'uniform float u_time;\n' +
    'uniform float u_spread;\n' +
    'uniform float u_topw;\n' +
    'uniform float u_curve;\n' +
    'uniform float u_count;\n' +
    'uniform float u_lw;\n' +
    'uniform float u_top;\n' +
    'uniform float u_len;\n' +
    'uniform float u_speed;\n' +
    'uniform float u_seg;\n' +
    'uniform float u_sway;\n' +
    'uniform float u_glow;\n' +
    'uniform float u_seed;\n' +
    'float hash(float n){ return fract(sin(n*127.1)*43758.5453123); }\n' +
    'vec3 stripeColor(float id){\n' +
    '  float r = hash(id*0.318 + u_seed);\n' +
    '  vec3 teal    = vec3(0.10, 0.74, 0.70);\n' +
    '  vec3 red     = vec3(0.83, 0.17, 0.28);\n' +
    '  vec3 orange  = vec3(0.93, 0.45, 0.15);\n' +
    '  vec3 purple  = vec3(0.52, 0.24, 0.58);\n' +
    '  vec3 indigo  = vec3(0.27, 0.34, 0.66);\n' +
    '  vec3 magenta = vec3(0.80, 0.15, 0.44);\n' +
    '  if(r < 0.18) return teal;\n' +
    '  else if(r < 0.42) return red;\n' +
    '  else if(r < 0.58) return orange;\n' +
    '  else if(r < 0.74) return purple;\n' +
    '  else if(r < 0.88) return indigo;\n' +
    '  else return magenta;\n' +
    '}\n';

  var MAIN = 'void main(){\n' +
    '  vec2 uv = gl_FragCoord.xy / u_res.xy;\n' +
    '  vec2 p  = uv*2.0 - 1.0;\n' +
    '  float aspect = u_res.x / u_res.y;\n' +
    '  p.x *= aspect;\n' +
    '\n' +
    '  // v: depth along the sheaf, 0 at the focus (top), 1 at the bottom.\n' +
    '  float v    = (u_top - p.y) / u_len;\n' +
    '  float band = step(0.0, v) * step(v, 1.0);\n' +
    '  float vp   = clamp(v, 0.0, 1.0);\n' +
    '\n' +
    '  // sideways sway, like a willow in the wind\n' +
    '  float sway = sin(u_time*0.6 + vp*3.2) * u_sway * vp;\n' +
    '  float x = p.x - sway;\n' +
    '\n' +
    '  // sheaf width: tight at the focus, wide at the bottom\n' +
    '  float width = u_topw + u_spread * pow(vp, u_curve);\n' +
    '\n' +
    '  float N     = u_count;\n' +
    '  float icont = (x / max(width, 1e-4)) * N;\n' +
    '  float lineId = floor(icont + 0.5);\n' +
    '  float local  = icont - lineId;\n' +
    '\n' +
    '  float aa = min(fwidth(icont)*1.1 + 0.003, 0.45);\n' +
    '  float hw = u_lw;\n' +
    '  float line = smoothstep(hw+aa, hw-aa, abs(local));\n' +
    '\n' +
    '  // finite number of beams + fade at the focus and the bottom edge\n' +
    '  float present = step(abs(lineId), N + 0.5) * band;\n' +
    '  float topFade = smoothstep(0.0, 0.06, v);\n' +
    '  float botFade = smoothstep(1.0, 0.86, v);\n' +
    '  present *= topFade * botFade;\n' +
    '\n' +
    '  // segments flowing along the beams\n' +
    '  float scroll  = v*u_seg + u_time*u_speed;\n' +
    '  float segId   = floor(scroll);\n' +
    '  float segF    = fract(scroll);\n' +
    '  float segRnd  = hash(segId*2.13 + lineId*7.31 + u_seed*1.7);\n' +
    '  float on      = step(0.24, segRnd);\n' +
    '  float segEdge = smoothstep(0.0,0.06,segF) * smoothstep(1.0,0.94,segF);\n' +
    '\n' +
    '  float intensity = line * present * on * segEdge;\n' +
    '\n' +
    '  vec3 col = stripeColor(lineId);\n' +
    '  col *= (0.72 + 0.5*hash(segId*3.7 + lineId));\n' +
    '\n' +
    '  float halo = smoothstep(hw*3.0+aa, hw-aa, abs(local)) * present * on * segEdge;\n' +
    '\n' +
    '  vec3 bg = vec3(0.045, 0.155, 0.185);\n' +
    '  vec3 outCol = bg + col*intensity*u_glow + col*halo*0.22*u_glow;\n' +
    '\n' +
    '  // double focus glow\n' +
    '  float g1 = smoothstep(0.10, 0.0, length(vec2(p.x-0.05, p.y-u_top)));\n' +
    '  float g2 = smoothstep(0.10, 0.0, length(vec2(p.x+0.05, p.y-u_top)));\n' +
    '  outCol += vec3(0.4, 0.9, 0.95) * (g1+g2) * 0.10;\n' +
    '\n' +
    '  outCol *= 1.0 - 0.24*pow(length(uv-0.5), 2.2);\n' +
    '  fragColor = vec4(outCol, 1.0);\n' +
    '}\n';

  var FRAG_SRC = FRAG + MAIN;

  // Mounts the animation in the host div. Idempotent: a mount on an already
  // mounted sheaf does nothing. Returns false without breaking anything if
  // WebGL2 is missing — the fallback CSS gradient then stays visible.
  function mount(hostId) {
    var host = document.getElementById(hostId);
    if (!host || mounted) return !!mounted;
    var canvas = document.createElement('canvas');
    canvas.style.position = 'absolute';
    canvas.style.inset = '0';
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    canvas.style.display = 'block';
    host.appendChild(canvas);

    var gl = canvas.getContext('webgl2', { antialias: true, premultipliedAlpha: false });
    if (!gl) {
      console.warn('tron-gerbe: WebGL2 unavailable, static background kept');
      canvas.remove();
      return false;
    }

    var vs = compile(gl, gl.VERTEX_SHADER, VERT);
    var fs = compile(gl, gl.FRAGMENT_SHADER, FRAG_SRC);
    var prog = gl.createProgram();
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      console.error('tron-gerbe: link ' + gl.getProgramInfoLog(prog));
      canvas.remove();
      return false;
    }
    gl.useProgram(prog);

    var buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    var loc = gl.getAttribLocation(prog, 'a_pos');
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    var U = {};
    ['res', 'time', 'spread', 'topw', 'curve', 'count', 'lw', 'top', 'len',
      'speed', 'seg', 'sway', 'glow', 'seed'].forEach(function (n) {
        U[n] = gl.getUniformLocation(prog, 'u_' + n);
      });

    // Size = the host's (not the viewport's: the login page may scroll on
    // a small screen), DPR capped at 2 like the demo.
    function resize() {
      var dpr = Math.min(window.devicePixelRatio || 1, 2);
      var w = Math.floor(host.clientWidth * dpr);
      var h = Math.floor(host.clientHeight * dpr);
      if (w > 0 && h > 0 && (canvas.width !== w || canvas.height !== h)) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
    }

    var st = { t: 0, last: 0 };
    var reduced = window.matchMedia &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    // Uniforms pass + draw — without advancing time.
    function render() {
      resize();
      gl.uniform2f(U.res, canvas.width, canvas.height);
      gl.uniform1f(U.time, st.t);
      gl.uniform1f(U.spread, CONFIG.spread);
      gl.uniform1f(U.topw, CONFIG.topw);
      gl.uniform1f(U.curve, CONFIG.curve);
      gl.uniform1f(U.count, CONFIG.count);
      gl.uniform1f(U.lw, CONFIG.lw);
      gl.uniform1f(U.top, CONFIG.top);
      gl.uniform1f(U.len, CONFIG.len);
      gl.uniform1f(U.speed, CONFIG.speed);
      gl.uniform1f(U.seg, CONFIG.seg);
      gl.uniform1f(U.sway, CONFIG.sway);
      gl.uniform1f(U.glow, CONFIG.glow);
      gl.uniform1f(U.seed, CONFIG.seed);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    }

    function frame(now) {
      if (!mounted || mounted.reduced) return;
      var dt = st.last ? Math.min((now - st.last) / 1000, 0.1) : 0;
      st.last = now;
      st.t += dt;
      render();
      mounted.raf = requestAnimationFrame(frame);
    }

    // Resize: immediate redraw (mandatory in reduced motion, otherwise the
    // static frame does not follow the new size).
    function onResize() {
      resize();
      if (mounted && mounted.reduced) render();
    }

    mounted = { canvas: canvas, gl: gl, raf: null, onResize: onResize, reduced: reduced };
    window.addEventListener('resize', onResize);
    if (reduced) {
      render(); // one static frame, no RAF loop
    } else {
      mounted.raf = requestAnimationFrame(frame);
    }
    return true;
  }

  function unmount() {
    if (!mounted) return;
    if (mounted.raf !== null) cancelAnimationFrame(mounted.raf);
    if (mounted.onResize) window.removeEventListener('resize', mounted.onResize);
    var lose = mounted.gl && mounted.gl.getExtension('WEBGL_lose_context');
    if (lose) lose.loseContext();
    mounted.canvas.remove();
    mounted = null;
  }

  window.pnexTronGerbe = { mount: mount, unmount: unmount };

  // ── Loading gerbe behind the sidebar logo (D224) ──────────────────────
  // The login shader on a transparent ground: fewer, thinner, faster beams.
  // Any element `[data-pnex-gerbe]` hosts one; it runs while the attribute
  // is "on" (the shell sets it while API requests are in flight). Its
  // colour strength comes from the CSS variable `--gerbe-glow` (lighter
  // on the light theme, stronger on the dark one). No Rust bridge: the
  // script watches the attribute itself.
  var HEADER = {
    spread: 1.6, topw: 0.05, curve: 2.0, count: 3, lw: 0.09, top: 0.95,
    len: 1.9, speed: 0.75, seg: 2.2, sway: 0.025, seed: 12.7,
  };
  var HEADER_MAIN = 'void main(){\n' +
    '  vec2 uv = gl_FragCoord.xy / u_res.xy;\n' +
    '  vec2 p  = uv*2.0 - 1.0;\n' +
    '  p.x *= (u_res.x / u_res.y) / 2.2;\n' +
    '  float v    = (u_top - p.y) / u_len;\n' +
    '  float band = step(0.0, v) * step(v, 1.0);\n' +
    '  float vp   = clamp(v, 0.0, 1.0);\n' +
    '  float x = p.x - sin(u_time*0.6 + vp*3.2) * u_sway * vp;\n' +
    '  float width = u_topw + u_spread * pow(vp, u_curve);\n' +
    '  float icont = (x / max(width, 1e-4)) * u_count;\n' +
    '  float lineId = floor(icont + 0.5);\n' +
    '  float local  = icont - lineId;\n' +
    '  float aa = min(fwidth(icont)*1.1 + 0.003, 0.45);\n' +
    '  float line = smoothstep(u_lw+aa, u_lw-aa, abs(local));\n' +
    '  float present = step(abs(lineId), u_count + 0.5) * band\n' +
    '    * smoothstep(0.0, 0.06, v) * smoothstep(1.0, 0.86, v);\n' +
    '  float scroll = v*u_seg + u_time*u_speed;\n' +
    '  float segId = floor(scroll);\n' +
    '  float segF  = fract(scroll);\n' +
    '  float on = step(0.24, hash(segId*2.13 + lineId*7.31 + u_seed*1.7));\n' +
    '  float segEdge = smoothstep(0.0,0.06,segF) * smoothstep(1.0,0.94,segF);\n' +
    '  float k = present * on * segEdge;\n' +
    '  float halo = smoothstep(u_lw*3.0+aa, u_lw-aa, abs(local)) * k;\n' +
    '  vec3 col = min(stripeColor(lineId) * (0.72 + 0.5*hash(segId*3.7 + lineId)), 1.0);\n' +
    '  float a = clamp((line*k + halo*0.22) * u_glow, 0.0, 1.0);\n' +
    '  fragColor = vec4(col * a, a);\n' +
    '}\n';

  var headers = new WeakMap(); // host → { canvas, gl, U } or null (no WebGL2)
  var headerRaf = null;
  var headerTime = 0;
  var headerLast = 0;

  function headerSetup(host) {
    var canvas = document.createElement('canvas');
    canvas.setAttribute('aria-hidden', 'true');
    host.appendChild(canvas);
    var gl = canvas.getContext('webgl2', { premultipliedAlpha: true, alpha: true });
    var prog = gl && gl.createProgram();
    var vs = gl && compile(gl, gl.VERTEX_SHADER, VERT);
    var fs = gl && compile(gl, gl.FRAGMENT_SHADER, FRAG + HEADER_MAIN);
    if (!vs || !fs) {
      canvas.remove();
      return null;
    }
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      canvas.remove();
      return null;
    }
    gl.useProgram(prog);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    var loc = gl.getAttribLocation(prog, 'a_pos');
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    var U = {};
    ['res', 'time', 'spread', 'topw', 'curve', 'count', 'lw', 'top', 'len',
      'speed', 'seg', 'sway', 'glow', 'seed'].forEach(function (n) {
        U[n] = gl.getUniformLocation(prog, 'u_' + n);
      });
    return { canvas: canvas, gl: gl, U: U };
  }

  function headerDraw(host) {
    if (!headers.has(host)) headers.set(host, headerSetup(host));
    var h = headers.get(host);
    if (!h) return;
    var dpr = Math.min(window.devicePixelRatio || 1, 2);
    var w = Math.floor(host.clientWidth * dpr);
    var hh = Math.floor(host.clientHeight * dpr);
    if (w === 0 || hh === 0) return; // hidden (rail, closed drawer)
    if (h.canvas.width !== w || h.canvas.height !== hh) {
      h.canvas.width = w;
      h.canvas.height = hh;
      h.gl.viewport(0, 0, w, hh);
    }
    var gl = h.gl;
    var glow = parseFloat(getComputedStyle(host).getPropertyValue('--gerbe-glow')) || 0.38;
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.uniform2f(h.U.res, w, hh);
    gl.uniform1f(h.U.time, headerTime);
    gl.uniform1f(h.U.glow, glow);
    Object.keys(HEADER).forEach(function (k) {
      gl.uniform1f(h.U[k], HEADER[k]);
    });
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  function headerFrame(now) {
    var active = document.querySelectorAll('[data-pnex-gerbe="on"]');
    if (active.length === 0) {
      headerRaf = null;
      headerLast = 0;
      return;
    }
    headerTime += headerLast ? Math.min((now - headerLast) / 1000, 0.1) : 0;
    headerLast = now;
    active.forEach(headerDraw);
    headerRaf = requestAnimationFrame(headerFrame);
  }

  function headerWake() {
    if (headerRaf === null && document.querySelector('[data-pnex-gerbe="on"]')) {
      headerRaf = requestAnimationFrame(headerFrame);
    }
  }

  // Reduced motion: the logo stays still, no gerbe at all.
  var headerReduced = window.matchMedia &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (!headerReduced && window.MutationObserver) {
    new MutationObserver(headerWake).observe(document.documentElement, {
      subtree: true, childList: true, attributes: true, attributeFilter: ['data-pnex-gerbe'],
    });
    headerWake();
  }
})();
