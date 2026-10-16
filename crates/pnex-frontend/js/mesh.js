// 3D model viewer (assisted-creation.md §8, D231): a GLB in an orbit view,
// its skeleton, bone rotation, named poses, embedded animations and a PNG
// thumbnail. Bundled into viewers.js; exposed as `window.pnexViewers.mesh`.
// Every entry point returns plain JSON-able values and never throws: the
// Rust side shows "preview unavailable" on false.

/* eslint-disable */

// Namespace import with static member access: esbuild still tree-shakes.
import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';

var DEG = Math.PI / 180;

function host(hostId) {
  return document.getElementById(hostId);
}

function state(hostId) {
  var h = host(hostId);
  return h && h._pnexMesh;
}

function mount(hostId, url) {
  var h = host(hostId);
  if (!h || !url) return false;
  stop(h);
  var renderer;
  try {
    // preserveDrawingBuffer: the thumbnail reads the canvas after a frame.
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
  } catch (e) {
    console.warn('[pnex-viewers] mesh: no WebGL', e);
    return false;
  }
  var w = Math.max(h.clientWidth, 300);
  var hgt = Math.max(h.clientHeight, 300);
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.setSize(w, hgt);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  h.innerHTML = '';
  h.appendChild(renderer.domElement);

  var scene = new THREE.Scene();
  var camera = new THREE.PerspectiveCamera(40, w / hgt, 0.01, 1000);
  scene.add(new THREE.HemisphereLight(0xffffff, 0x445566, 2.2));
  var sun = new THREE.DirectionalLight(0xffffff, 2);
  sun.position.set(3, 6, 4);
  scene.add(sun);
  var controls = new OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true;

  var st = {
    renderer: renderer,
    scene: scene,
    camera: camera,
    controls: controls,
    ready: false,
    error: false,
    bones: [],
    rest: {},
    clips: [],
    mixer: null,
    action: null,
    helper: null,
    clock: new THREE.Clock(),
    raf: null,
  };
  h._pnexMesh = st;

  new GLTFLoader().load(
    url,
    function (gltf) {
      if (h._pnexMesh !== st) return;
      var root = gltf.scene;
      scene.add(root);
      var seen = {};
      root.traverse(function (o) {
        if (o.isBone && o.name && !seen[o.name]) {
          seen[o.name] = true;
          st.bones.push(o);
          st.rest[o.name] = o.quaternion.clone();
        }
      });
      st.helper = new THREE.SkeletonHelper(root);
      st.helper.visible = false;
      scene.add(st.helper);
      st.clips = gltf.animations || [];
      st.mixer = new THREE.AnimationMixer(root);
      frame(st, root);
      st.ready = true;
    },
    undefined,
    function (err) {
      console.warn('[pnex-viewers] mesh: load failed', err);
      st.error = true;
    }
  );

  var loop = function () {
    if (!h.isConnected || h._pnexMesh !== st) return;
    var dt = st.clock.getDelta();
    if (st.mixer) st.mixer.update(dt);
    controls.update();
    var cw = h.clientWidth;
    var ch = h.clientHeight;
    if (cw > 0 && ch > 0 && (cw !== renderer.domElement.width / renderer.getPixelRatio() || ch !== renderer.domElement.height / renderer.getPixelRatio())) {
      renderer.setSize(cw, ch);
      camera.aspect = cw / ch;
      camera.updateProjectionMatrix();
    }
    renderer.render(scene, camera);
    st.raf = requestAnimationFrame(loop);
  };
  st.raf = requestAnimationFrame(loop);
  return true;
}

// Fits the camera on the model's bounding box.
function frame(st, root) {
  var box = new THREE.Box3().setFromObject(root);
  if (box.isEmpty()) return;
  var size = box.getSize(new THREE.Vector3()).length();
  var center = box.getCenter(new THREE.Vector3());
  // Distance at which the bounding sphere fits the vertical field of view.
  var dist = (size / 2 / Math.tan((st.camera.fov * DEG) / 2)) * 1.1;
  st.controls.target.copy(center);
  st.camera.position.copy(center).add(new THREE.Vector3(0.35, 0.25, 1).normalize().multiplyScalar(dist));
  st.camera.near = size / 100;
  st.camera.far = size * 100;
  st.camera.updateProjectionMatrix();
}

function stop(h) {
  var st = h._pnexMesh;
  if (!st) return;
  h._pnexMesh = null;
  if (st.raf) cancelAnimationFrame(st.raf);
  try {
    st.controls.dispose();
    st.scene.traverse(function (o) {
      if (o.geometry) o.geometry.dispose();
      var mats = o.material ? (Array.isArray(o.material) ? o.material : [o.material]) : [];
      mats.forEach(function (m) {
        Object.keys(m).forEach(function (k) {
          if (m[k] && m[k].isTexture) m[k].dispose();
        });
        m.dispose();
      });
    });
    st.renderer.dispose();
    st.renderer.forceContextLoss();
  } catch (e) {
    // already gone
  }
}

// Load state, bones (Euler degrees) and clip names.
function info(hostId) {
  var st = state(hostId);
  if (!st) return { ready: false, error: true, bones: [], clips: [] };
  return {
    ready: st.ready,
    error: st.error,
    playing: st.action ? st.action.getClip().name : null,
    skeleton: !!(st.helper && st.helper.visible),
    bones: st.bones.map(function (b) {
      return {
        name: b.name,
        x: Math.round(b.rotation.x / DEG),
        y: Math.round(b.rotation.y / DEG),
        z: Math.round(b.rotation.z / DEG),
      };
    }),
    clips: st.clips.map(function (c) {
      return c.name;
    }),
  };
}

function bone(st, name) {
  for (var i = 0; i < st.bones.length; i++) {
    if (st.bones[i].name === name) return st.bones[i];
  }
  return null;
}

// Stops any animation: a pose is edited on a still model.
function still(st) {
  if (st.mixer) st.mixer.stopAllAction();
  st.action = null;
}

function rotate(hostId, arg) {
  var st = state(hostId);
  var b = st && arg && bone(st, arg.bone);
  if (!b) return false;
  still(st);
  b.rotation.set((+arg.x || 0) * DEG, (+arg.y || 0) * DEG, (+arg.z || 0) * DEG);
  return true;
}

// Bones moved from the rest pose, as quaternions rounded to 4 decimals.
function pose(hostId) {
  var st = state(hostId);
  if (!st) return null;
  var out = {};
  st.bones.forEach(function (b) {
    var r = st.rest[b.name];
    if (r && r.angleTo(b.quaternion) < 1e-3) return;
    out[b.name] = b.quaternion.toArray().map(function (v) {
      return Math.round(v * 1e4) / 1e4;
    });
  });
  return out;
}

// Rest pose, then the given rotations; unknown bones are ignored.
function applyPose(hostId, p) {
  var st = state(hostId);
  if (!st) return false;
  still(st);
  st.bones.forEach(function (b) {
    var q = p && p[b.name];
    if (Array.isArray(q) && q.length === 4 && q.every(isFinite)) {
      b.quaternion.fromArray(q).normalize();
    } else if (st.rest[b.name]) {
      b.quaternion.copy(st.rest[b.name]);
    }
  });
  return true;
}

function play(hostId, name) {
  var st = state(hostId);
  if (!st || !st.mixer) return false;
  still(st);
  var clip = st.clips.find(function (c) {
    return c.name === name;
  });
  if (!clip) return true;
  st.action = st.mixer.clipAction(clip);
  st.action.play();
  return true;
}

function skeleton(hostId, on) {
  var st = state(hostId);
  if (!st || !st.helper) return false;
  st.helper.visible = !!on;
  return true;
}

// PNG data URL of the current view, skeleton hidden.
function snapshot(hostId) {
  var st = state(hostId);
  if (!st || !st.ready) return null;
  var shown = st.helper && st.helper.visible;
  if (shown) st.helper.visible = false;
  st.renderer.render(st.scene, st.camera);
  var url = st.renderer.domElement.toDataURL('image/png');
  if (shown) st.helper.visible = true;
  return url;
}

export { stop };
export var api = {
    mount: mount,
    info: info,
    rotate: rotate,
    pose: pose,
    applyPose: applyPose,
    play: play,
    skeleton: skeleton,
    snapshot: snapshot,
};
