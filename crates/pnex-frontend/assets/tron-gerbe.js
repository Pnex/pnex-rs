/*!
 * Fond animé « gerbe de faisceaux » (style Tron) — page de login PNeX.
 *
 * Script statique SANS dépendance npm : pas de passage par js:build/esbuild
 * (réservé au glue esptool-js), servi tel quel via asset!().
 *
 * Expose deux globales consommées par le pont `src/tron.rs` :
 *   - window.pnexTronGerbe.mount(hostId) → crée le canvas WebGL2 dans le div
 *     hôte, démarre la boucle RAF ; false si WebGL2 indisponible (le dégradé
 *     CSS de secours reste alors visible).
 *   - window.pnexTronGerbe.unmount() → stoppe le RAF, retire le listener de
 *     resize, supprime canvas et contexte GL (appelé au unmount Dioxus).
 */
(function () {
  'use strict';

  // Paramètres figés (ex-curseurs de la démo) — ajustables ici.
  var CONFIG = {
    spread: 1.35,  // écartement en bas
    topw: 0.05,    // serrage en haut
    curve: 2.25,   // courbure (1 = droit)
    count: 24,     // nombre de faisceaux
    lw: 0.16,      // épaisseur
    top: 0.82,     // hauteur du foyer
    len: 1.75,     // longueur
    speed: 0.15,   // vitesse de défilement des segments (bas → foyer)
    seg: 3.0,      // segments (dash)
    sway: 0.025,   // souffle (sway)
    glow: 1.1,     // éclat / glow
    seed: 12.7,    // graine de palette (fixe → branding déterministe)
  };

  var mounted = null; // état courant { canvas, gl, raf, onResize, reduced }

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

  // Fragment shader : gerbe de faisceaux — foyer serré en haut, éventail
  // large en bas, segments colorés qui coulent le long des faisceaux.
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
    '  // v : profondeur le long de la gerbe, 0 au foyer (haut), 1 en bas.\n' +
    '  float v    = (u_top - p.y) / u_len;\n' +
    '  float band = step(0.0, v) * step(v, 1.0);\n' +
    '  float vp   = clamp(v, 0.0, 1.0);\n' +
    '\n' +
    '  // souffle latéral façon saule au vent\n' +
    '  float sway = sin(u_time*0.6 + vp*3.2) * u_sway * vp;\n' +
    '  float x = p.x - sway;\n' +
    '\n' +
    '  // largeur de la gerbe : serrée au foyer, large en bas\n' +
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
    '  // nombre fini de faisceaux + fondu au foyer et au bord bas\n' +
    '  float present = step(abs(lineId), N + 0.5) * band;\n' +
    '  float topFade = smoothstep(0.0, 0.06, v);\n' +
    '  float botFade = smoothstep(1.0, 0.86, v);\n' +
    '  present *= topFade * botFade;\n' +
    '\n' +
    '  // segments qui coulent le long des faisceaux\n' +
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
    '  // double lueur de foyer\n' +
    '  float g1 = smoothstep(0.10, 0.0, length(vec2(p.x-0.05, p.y-u_top)));\n' +
    '  float g2 = smoothstep(0.10, 0.0, length(vec2(p.x+0.05, p.y-u_top)));\n' +
    '  outCol += vec3(0.4, 0.9, 0.95) * (g1+g2) * 0.10;\n' +
    '\n' +
    '  outCol *= 1.0 - 0.24*pow(length(uv-0.5), 2.2);\n' +
    '  fragColor = vec4(outCol, 1.0);\n' +
    '}\n';

  var FRAG_SRC = FRAG + MAIN;

  // Monte l'animation dans le div hôte. Idempotent : un mount sur une gerbe
  // déjà montée ne fait rien. Renvoie false sans rien casser si WebGL2
  // manque — le dégradé CSS de secours reste alors visible.
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
      console.warn('tron-gerbe: WebGL2 indisponible, fond statique conservé');
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

    // Taille = celle du hôte (pas du viewport : la page login peut défiler
    // sur petit écran), DPR plafonné à 2 comme la démo.
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

    // Passe d'uniformes + draw — sans avancer le temps.
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

    // Resize : redraw immédiat (obligatoire en reduced motion, sinon la
    // frame statique ne suit pas la nouvelle taille).
    function onResize() {
      resize();
      if (mounted && mounted.reduced) render();
    }

    mounted = { canvas: canvas, gl: gl, raf: null, onResize: onResize, reduced: reduced };
    window.addEventListener('resize', onResize);
    if (reduced) {
      render(); // une frame statique, pas de boucle RAF
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
})();
