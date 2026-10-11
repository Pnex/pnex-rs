// Pages editor (pages.md #21, spike L0: yrs + Yjs) — esbuild IIFE bundle
// `assets/doc.js` → `window.pnexDoc`, driven by `src/doc_editor.rs`.
//
// ProseMirror holds the text and the selection; Yjs (y-prosemirror) holds
// the shared state, synced with the server over `/ws/doc/{id}` by the
// small provider below (y-protocols sync + awareness). The server is the
// authority: it re-checks every update against the same schema
// (`pnex_core::pages`), so this schema and its allowlists must stay in step
// with it (guarded by a Rust test reading this file).
//
// Events back to Rust by polling (tron.rs school, works in wasm and
// native): `window.__pnexDoc[hostId] = {status, closeCode, needTicket,
// peers, request, commentsSeq, activeComment}`. The websocket ticket is single-use: on every
// disconnect `needTicket` is bumped and Rust answers with `reconnect()`.

import * as Y from 'yjs';
import { ySyncPlugin, ySyncPluginKey, yCursorPlugin, yUndoPlugin, undo, redo, yXmlFragmentToProseMirrorRootNode, absolutePositionToRelativePosition, relativePositionToAbsolutePosition } from 'y-prosemirror';
import * as syncProtocol from 'y-protocols/sync';
import * as awarenessProtocol from 'y-protocols/awareness';
import * as encoding from 'lib0/encoding';
import * as decoding from 'lib0/decoding';
import { EditorState, Plugin, PluginKey, TextSelection } from 'prosemirror-state';
import { EditorView, Decoration, DecorationSet } from 'prosemirror-view';
import { Schema } from 'prosemirror-model';
import { schema as basic } from 'prosemirror-schema-basic';
import { addListNodes, wrapInList, splitListItem, liftListItem, sinkListItem } from 'prosemirror-schema-list';
import { keymap } from 'prosemirror-keymap';
import { baseKeymap, toggleMark, setBlockType, wrapIn } from 'prosemirror-commands';
import { inputRules, wrappingInputRule, textblockTypeInputRule } from 'prosemirror-inputrules';
import { MarkdownParser, defaultMarkdownSerializer } from 'prosemirror-markdown';
import MarkdownIt from 'markdown-it';

// ── Schema (mirror of pnex_core::pages::NODES / MARKS) ──────────────────
// Nodes: paragraph heading blockquote horizontal_rule code_block hard_break
// bullet_list ordered_list list_item (image removed: media blocks come in L4).
const LINK_SCHEMES = ['http:', 'https:', 'mailto:'];
function linkAllowed(href) {
  const h = String(href || '').trim();
  if (h.startsWith('/') && !h.startsWith('//')) return true;
  return LINK_SCHEMES.some((s) => h.length > s.length && h.slice(0, s.length).toLowerCase() === s);
}

const marks = basic.spec.marks.update('link', {
  attrs: { href: {}, title: { default: null } },
  inclusive: false,
  parseDOM: [{ tag: 'a[href]', getAttrs: (dom) => ({ href: dom.getAttribute('href'), title: dom.getAttribute('title') }) }],
  toDOM: (node) => ['a', { href: node.attrs.href, title: node.attrs.title, rel: 'noopener noreferrer nofollow', target: '_blank' }, 0],
});
const schema = new Schema({
  nodes: addListNodes(basic.spec.nodes.remove('image'), 'paragraph block*', 'block'),
  marks,
});

// ── Markdown ─────────────────────────────────────────────────────────────
const mdParser = new MarkdownParser(schema, MarkdownIt('commonmark', { html: false }), {
  blockquote: { block: 'blockquote' },
  paragraph: { block: 'paragraph' },
  list_item: { block: 'list_item' },
  bullet_list: { block: 'bullet_list' },
  ordered_list: { block: 'ordered_list', getAttrs: (tok) => ({ order: +tok.attrGet('start') || 1 }) },
  heading: { block: 'heading', getAttrs: (tok) => ({ level: +tok.tag.slice(1) }) },
  code_block: { block: 'code_block', noCloseToken: true },
  fence: { block: 'code_block', noCloseToken: true },
  hr: { node: 'horizontal_rule' },
  image: { ignore: true, noCloseToken: true },
  hardbreak: { node: 'hard_break' },
  em: { mark: 'em' },
  strong: { mark: 'strong' },
  link: { mark: 'link', getAttrs: (tok) => ({ href: tok.attrGet('href'), title: tok.attrGet('title') || null }) },
  code_inline: { mark: 'code', noCloseToken: true },
});

// Drops links outside the allowlist, whatever brought them (paste, import,
// typing): the server would refuse the whole update otherwise.
const linkGuard = new Plugin({
  appendTransaction(trs, _old, state) {
    if (!trs.some((tr) => tr.docChanged)) return null;
    let tr = null;
    state.doc.descendants((node, pos) => {
      for (const m of node.marks) {
        if (m.type === schema.marks.link && !linkAllowed(m.attrs.href)) {
          tr = (tr || state.tr).removeMark(pos, pos + node.nodeSize, m);
        }
      }
    });
    return tr;
  },
});

// ── Commands ("/" menu, bubble, header)  ─────────────────────────────────
const S = schema.nodes;
const COMMANDS = {
  bold: () => toggleMark(schema.marks.strong),
  italic: () => toggleMark(schema.marks.em),
  code: () => toggleMark(schema.marks.code),
  paragraph: () => setBlockType(S.paragraph),
  h1: () => setBlockType(S.heading, { level: 1 }),
  h2: () => setBlockType(S.heading, { level: 2 }),
  h3: () => setBlockType(S.heading, { level: 3 }),
  bullet: () => wrapInList(S.bullet_list),
  ordered: () => wrapInList(S.ordered_list),
  quote: () => wrapIn(S.blockquote),
  codeblock: () => setBlockType(S.code_block),
  rule: () => (state, dispatch) => {
    if (dispatch) dispatch(state.tr.replaceSelectionWith(S.horizontal_rule.create()).scrollIntoView());
    return true;
  },
  undo: () => undo,
  redo: () => redo,
};

function rules() {
  return inputRules({
    rules: [
      textblockTypeInputRule(/^(#{1,3})\s$/, S.heading, (m) => ({ level: m[1].length })),
      wrappingInputRule(/^\s*>\s$/, S.blockquote),
      wrappingInputRule(/^\s*([-+*])\s$/, S.bullet_list),
      wrappingInputRule(/^(\d+)\.\s$/, S.ordered_list, (m) => ({ order: +m[1] }), (m, n) => n.childCount + n.attrs.order === +m[1]),
      textblockTypeInputRule(/^```$/, S.code_block),
    ],
  });
}

// ── State shared with Rust ───────────────────────────────────────────────
const editors = {};
window.__pnexDoc = window.__pnexDoc || {};
let seq = 0;
function report(host, patch) {
  window.__pnexDoc[host] = Object.assign(window.__pnexDoc[host] || {}, patch);
}

// ── Notion-like chrome: "/" menu, selection bubble, "+" handle, hint ─────
// Drawn here, not in Rust, so arrow keys answer without the polling delay;
// every label comes from Rust at mount (`opts.labels`, fluent).
// [command, icon, Markdown shortcut]
const BLOCK_ITEMS = [
  ['paragraph', 'T', ''],
  ['h1', 'H1', '#'],
  ['h2', 'H2', '##'],
  ['h3', 'H3', '###'],
  ['bullet', '\u2022', '-'],
  ['ordered', '1.', '1.'],
  ['quote', '\u275D', '>'],
  ['codeblock', '</>', '```'],
  ['rule', '\u2014', '---'],
];

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

// "/query" right before the cursor, the "/" at the start of a block or after a space.
function slashMatch(state) {
  const { $from, empty } = state.selection;
  if (!empty || !$from.parent.isTextblock || $from.parent.type === S.code_block) return null;
  const before = $from.parent.textBetween(0, $from.parentOffset, null, '\ufffc');
  const m = /(?:^|\s)\/([^\s/]{0,24})$/.exec(before);
  if (!m) return null;
  return { from: $from.pos - m[1].length - 1, to: $from.pos, query: m[1].toLowerCase() };
}

function slashPlugin(labels) {
  let menu = null;
  let items = [];
  let index = 0;
  let match = null;
  let dismissed = null;
  const close = () => {
    if (menu) menu.remove();
    menu = null;
    match = null;
  };
  const pick = (view, cmd) => {
    const m = match;
    close();
    if (!m) return;
    view.dispatch(view.state.tr.delete(m.from, m.to));
    COMMANDS[cmd]()(view.state, view.dispatch, view);
    view.focus();
  };
  const highlight = () => {
    menu.querySelectorAll('.pnex-doc-menu-item').forEach((b, i) => {
      b.classList.toggle('is-active', i === index);
      b.setAttribute('aria-selected', String(i === index));
      if (i === index) b.scrollIntoView({ block: 'nearest' });
    });
  };
  const render = (view) => {
    if (!menu) {
      menu = el('div', 'pnex-doc-menu');
      menu.setAttribute('role', 'listbox');
      document.body.appendChild(menu);
    }
    menu.replaceChildren(el('div', 'pnex-doc-menu-section', labels.basicBlocks || ''));
    if (!items.length) menu.append(el('div', 'pnex-doc-menu-empty', labels.noResults || ''));
    items.forEach(([cmd, icon, hint], i) => {
      const b = el('button', 'pnex-doc-menu-item');
      b.type = 'button';
      b.setAttribute('role', 'option');
      b.append(el('span', 'pnex-doc-menu-icon', icon), el('span', 'pnex-doc-menu-label', labels[cmd] || cmd), el('span', 'pnex-doc-menu-hint', hint));
      b.onmousedown = (e) => e.preventDefault();
      b.onclick = () => pick(view, cmd);
      b.onmouseenter = () => {
        index = i;
        highlight();
      };
      menu.append(b);
    });
    highlight();
    // Below the "/", or above it when the viewport is too short.
    const c = view.coordsAtPos(match.from);
    const h = menu.offsetHeight;
    menu.style.left = Math.max(8, Math.min(c.left, window.innerWidth - menu.offsetWidth - 8)) + 'px';
    menu.style.top = (window.innerHeight - c.bottom > h + 8 ? c.bottom + 4 : Math.max(8, c.top - h - 4)) + 'px';
  };
  return new Plugin({
    view: () => ({
      update(view) {
        const m = slashMatch(view.state);
        if (!m) dismissed = null;
        if (!m || dismissed === m.from) return close();
        const changed = !match || match.query !== m.query;
        match = m;
        items = BLOCK_ITEMS.filter(([cmd, , hint]) => {
          const q = m.query;
          return !q || (labels[cmd] || '').toLowerCase().includes(q) || cmd.includes(q) || hint === q;
        });
        if (changed) index = 0;
        render(view);
      },
      destroy: close,
    }),
    props: {
      handleKeyDown(view, ev) {
        if (!menu) return false;
        if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
          if (items.length) {
            index = (index + (ev.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length;
            highlight();
          }
          return true;
        }
        if (ev.key === 'Enter' || ev.key === 'Tab') {
          if (!items.length) {
            close();
            return false;
          }
          pick(view, items[index][0]);
          return true;
        }
        if (ev.key === 'Escape') {
          dismissed = match.from;
          close();
          return true;
        }
        return false;
      },
      handleDOMEvents: {
        blur: () => {
          close();
          return false;
        },
      },
    },
  });
}

// Floating bar over a text selection: marks here, link and comment asked
// to Rust (`request`), which owns the link bar and the comments panel.
function bubblePlugin(host, labels) {
  let bar = null;
  const hide = () => {
    if (bar) bar.remove();
    bar = null;
  };
  const button = (label, text, cls, active, onclick) => {
    const b = el('button', 'pnex-doc-bubble-btn ' + cls + (active ? ' is-active' : ''), text);
    b.type = 'button';
    b.title = label || '';
    if (text !== label) b.setAttribute('aria-label', label || '');
    b.onmousedown = (e) => e.preventDefault();
    b.onclick = onclick;
    return b;
  };
  return new Plugin({
    view: () => ({
      update(view) {
        const { state } = view;
        const { from, to, empty } = state.selection;
        if (empty || !(state.selection instanceof TextSelection) || !view.hasFocus()) return hide();
        if (!bar) {
          bar = el('div', 'pnex-doc-bubble');
          bar.setAttribute('role', 'toolbar');
          document.body.appendChild(bar);
        }
        const mark = (name, text, cls) =>
          button(labels[name === 'strong' ? 'bold' : name === 'em' ? 'italic' : 'code'], text, cls, state.doc.rangeHasMark(from, to, schema.marks[name]), () => {
            toggleMark(schema.marks[name])(view.state, view.dispatch);
            view.focus();
          });
        const ask = (kind) => button(labels[kind], labels[kind], '', false, () => report(host, { request: { seq: ++seq, kind } }));
        bar.replaceChildren(mark('strong', 'B', 'is-bold'), mark('em', 'i', 'is-italic'), mark('code', '</>', 'is-code'), el('span', 'pnex-doc-bubble-sep'), ask('link'), ask('comment'));
        const a = view.coordsAtPos(from);
        const z = view.coordsAtPos(to);
        const left = Math.max(8, Math.min((a.left + z.right) / 2 - bar.offsetWidth / 2, window.innerWidth - bar.offsetWidth - 8));
        bar.style.left = left + 'px';
        bar.style.top = (a.top > bar.offsetHeight + 12 ? a.top - bar.offsetHeight - 8 : z.bottom + 8) + 'px';
      },
      destroy: hide,
    }),
    props: {
      handleDOMEvents: {
        blur: () => {
          hide();
          return false;
        },
      },
    },
  });
}

// "+" in the left gutter of the hovered block: a new line below that opens
// the "/" menu (or the menu in place when that block is an empty line).
function handlePlugin(labels) {
  return new Plugin({
    view(view) {
      const host = view.dom.parentNode;
      const h = el('button', 'pnex-doc-handle', '+');
      h.type = 'button';
      h.title = labels.addBlock || '';
      h.setAttribute('aria-label', labels.addBlock || '');
      host.appendChild(h);
      let target = null;
      const onMove = (e) => {
        const block = Array.from(view.dom.children).find((c) => {
          const r = c.getBoundingClientRect();
          return e.clientY >= r.top - 4 && e.clientY <= r.bottom + 4;
        });
        if (!block) return;
        const pos = view.posAtDOM(block, 0);
        const $p = view.state.doc.resolve(pos);
        target = $p.depth ? $p.before(1) : pos;
        const hr = host.getBoundingClientRect();
        const br = block.getBoundingClientRect();
        const line = parseFloat(getComputedStyle(block).lineHeight) || 24;
        h.style.top = br.top - hr.top + Math.max(0, (Math.min(line, br.height) - 24) / 2) + 'px';
        h.style.left = view.dom.getBoundingClientRect().left - hr.left - 30 + 'px';
        h.classList.add('is-visible');
      };
      const onLeave = () => h.classList.remove('is-visible');
      h.onmousedown = (e) => e.preventDefault();
      h.onclick = () => {
        const node = target === null ? null : view.state.doc.nodeAt(target);
        if (!node) return;
        let tr = view.state.tr;
        if (node.type === S.paragraph && node.content.size === 0) {
          tr = tr.insertText('/', target + 1).setSelection(TextSelection.create(tr.doc, target + 2));
        } else {
          const at = target + node.nodeSize;
          tr = tr.insert(at, S.paragraph.create(null, schema.text('/')));
          tr.setSelection(TextSelection.create(tr.doc, at + 2));
        }
        view.focus();
        view.dispatch(tr.scrollIntoView());
      };
      host.addEventListener('mousemove', onMove);
      host.addEventListener('mouseleave', onLeave);
      return {
        destroy() {
          host.removeEventListener('mousemove', onMove);
          host.removeEventListener('mouseleave', onLeave);
          h.remove();
        },
      };
    },
  });
}

// Grey hint on the empty line holding the cursor, and on an empty page.
function placeholderPlugin(text) {
  return new Plugin({
    props: {
      decorations(state) {
        const { doc } = state;
        const { $from, empty } = state.selection;
        let pos = null;
        if (doc.childCount === 1 && doc.firstChild.type === S.paragraph && doc.firstChild.content.size === 0) pos = 0;
        else if (empty && $from.parent.type === S.paragraph && $from.parent.content.size === 0) pos = $from.before();
        if (pos === null) return null;
        const node = doc.nodeAt(pos);
        return DecorationSet.create(doc, [Decoration.node(pos, pos + node.nodeSize, { class: 'is-empty', 'data-placeholder': text || '' })]);
      },
    },
  });
}

// Remote cursor. Name and color come from other clients' presence, relayed
// unchecked by the server: the name is text, the color must be #rrggbb
// (it lands in a style attribute).
function cursorBuilder(user) {
  const color = /^#[0-9a-fA-F]{6}$/.test(user && user.color) ? user.color : '#6b7280';
  const cursor = document.createElement('span');
  cursor.classList.add('ProseMirror-yjs-cursor');
  cursor.style.borderColor = color;
  const label = document.createElement('div');
  label.style.backgroundColor = color;
  label.textContent = String((user && user.name) || '').slice(0, 64);
  cursor.append('\u2060', label, '\u2060');
  return cursor;
}

// ── Comment anchors (P12) ────────────────────────────────────────────────
// An anchor is two Yjs relative positions ("<start b64>.<end b64>"): it
// follows the text through every edit, local or remote.
const b64 = (u8) => btoa(String.fromCharCode(...u8));
const unb64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

function binding(state) {
  const s = ySyncPluginKey.getState(state);
  return s && s.binding;
}

function anchorRange(state, anchor) {
  const b = binding(state);
  if (!b || !anchor) return null;
  try {
    const [a, z] = anchor.split('.');
    const frag = b.type;
    const from = relativePositionToAbsolutePosition(b.doc, frag, Y.decodeRelativePosition(unb64(a)), b.mapping);
    const to = relativePositionToAbsolutePosition(b.doc, frag, Y.decodeRelativePosition(unb64(z)), b.mapping);
    if (from === null || to === null || to <= from) return null;
    return { from, to };
  } catch (e) {
    return null;
  }
}

const commentsKey = new PluginKey('pnexComments');
function commentsPlugin(host) {
  return new Plugin({
    key: commentsKey,
    state: {
      init: () => [],
      apply: (tr, list) => tr.getMeta(commentsKey) || list,
    },
    props: {
      decorations(state) {
        const decos = [];
        for (const c of commentsKey.getState(state)) {
          if (c.resolved) continue;
          const r = anchorRange(state, c.anchor);
          if (r) decos.push(Decoration.inline(r.from, r.to, { class: 'pnex-doc-comment', 'data-comment': String(c.id) }));
        }
        return DecorationSet.create(state.doc, decos);
      },
      handleClick(view, pos) {
        for (const c of commentsKey.getState(view.state)) {
          if (c.resolved) continue;
          const r = anchorRange(view.state, c.anchor);
          if (r && pos >= r.from && pos <= r.to) {
            report(host, { activeComment: { seq: ++seq, id: c.id } });
            return false;
          }
        }
        return false;
      },
    },
  });
}

// ── Websocket provider (y-protocols) ─────────────────────────────────────
const MSG_SYNC = 0;
const MSG_AWARENESS = 1;
const MSG_QUERY_AWARENESS = 3;
// Custom message of the server: the comments of the page changed (room.rs).
const MSG_COMMENTS_CHANGED = 100;
// Close codes after which retrying is pointless (see controllers/ws_doc.rs).
const FINAL = new Set([4004, 4006, 4422, 4413]);

class Provider {
  constructor(host, doc, awareness) {
    this.host = host;
    this.doc = doc;
    this.awareness = awareness;
    this.ws = null;
    this.synced = false;
    // Pending flush() calls, answered in order by the server's SyncStep2.
    this.flushes = [];
    this.onUpdate = (update, origin) => {
      if (origin !== this) this.send(MSG_SYNC, (e) => syncProtocol.writeUpdate(e, update));
    };
    this.onAwareness = ({ added, updated, removed }, origin) => {
      if (origin === this) return;
      const changed = added.concat(updated, removed);
      this.send(MSG_AWARENESS, (e) => encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(awareness, changed)));
    };
    doc.on('update', this.onUpdate);
    awareness.on('update', this.onAwareness);
  }

  send(type, write) {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    const e = encoding.createEncoder();
    encoding.writeVarUint(e, type);
    write(e);
    this.ws.send(encoding.toUint8Array(e));
  }

  // Resolves true once the server has persisted every edit sent before the
  // call: it handles a socket's messages in order and persists an update
  // before reading the next one, so the reply to this SyncStep1 comes last.
  flush() {
    if (!this.synced || !this.ws || this.ws.readyState !== WebSocket.OPEN) return Promise.resolve(false);
    return new Promise((resolve) => {
      const w = { resolve };
      w.timer = setTimeout(() => {
        this.flushes = this.flushes.filter((x) => x !== w);
        resolve(false);
      }, 10000);
      this.flushes.push(w);
      this.send(MSG_SYNC, (e) => syncProtocol.writeSyncStep1(e, this.doc));
    });
  }

  settleFlushes(ok, all) {
    const done = all ? this.flushes.splice(0) : this.flushes.splice(0, 1);
    for (const w of done) {
      clearTimeout(w.timer);
      w.resolve(ok);
    }
  }

  connect(url) {
    if (this.ws) this.ws.close();
    this.synced = false;
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    report(this.host, { status: 'connecting' });
    ws.onopen = () => {
      this.send(MSG_SYNC, (e) => syncProtocol.writeSyncStep1(e, this.doc));
      if (this.awareness.getLocalState() !== null) {
        this.send(MSG_AWARENESS, (e) => encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(this.awareness, [this.doc.clientID])));
      }
    };
    ws.onmessage = (ev) => {
      const dec = decoding.createDecoder(new Uint8Array(ev.data));
      const type = decoding.readVarUint(dec);
      if (type === MSG_SYNC) {
        const reply = encoding.createEncoder();
        encoding.writeVarUint(reply, MSG_SYNC);
        const kind = syncProtocol.readSyncMessage(dec, reply, this.doc, this);
        if (kind === syncProtocol.messageYjsSyncStep2) {
          if (this.synced) this.settleFlushes(true, false);
          this.synced = true;
          report(this.host, { status: 'synced', closeCode: null });
        }
        if (encoding.length(reply) > 1) ws.send(encoding.toUint8Array(reply));
      } else if (type === MSG_AWARENESS) {
        awarenessProtocol.applyAwarenessUpdate(this.awareness, decoding.readVarUint8Array(dec), this);
        report(this.host, { peers: this.awareness.getStates().size - 1 });
      } else if (type === MSG_COMMENTS_CHANGED) {
        report(this.host, { commentsSeq: ++seq });
      } else if (type === MSG_QUERY_AWARENESS) {
        this.send(MSG_AWARENESS, (e) => encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(this.awareness, [this.doc.clientID])));
      }
    };
    ws.onclose = (ev) => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.synced = false;
      this.settleFlushes(false, true);
      const others = Array.from(this.awareness.getStates().keys()).filter((id) => id !== this.doc.clientID);
      awarenessProtocol.removeAwarenessStates(this.awareness, others, this);
      const final = FINAL.has(ev.code);
      // Nothing typed from now on would reach the server: stop accepting it.
      if (final && this.onFinal) this.onFinal();
      report(this.host, {
        status: ev.code === 4422 ? 'refused' : ev.code === 4413 ? 'toolarge' : final ? 'closed' : 'offline',
        closeCode: ev.code,
        peers: 0,
        needTicket: final || this.destroyed ? (window.__pnexDoc[this.host] || {}).needTicket : ++seq,
      });
    };
  }

  destroy() {
    this.destroyed = true;
    // Clear our presence for the others while the socket is still open.
    awarenessProtocol.removeAwarenessStates(this.awareness, [this.doc.clientID], 'local');
    this.doc.off('update', this.onUpdate);
    this.awareness.off('update', this.onAwareness);
    if (this.ws) this.ws.close();
  }
}

// ── Editor ───────────────────────────────────────────────────────────────
function makeView(host, doc, opts, extra) {
  const el = document.getElementById(host);
  if (!el) return null;
  el.innerHTML = '';
  const fragment = doc.getXmlFragment('prosemirror');
  const plugins = [ySyncPlugin(fragment), ...extra, linkGuard];
  const labels = opts.labels || {};
  if (opts.editable) {
    plugins.push(
      slashPlugin(labels),
      yUndoPlugin(),
      rules(),
      keymap({
        'Mod-z': undo,
        'Mod-y': redo,
        'Mod-Shift-z': redo,
        'Mod-b': toggleMark(schema.marks.strong),
        'Mod-i': toggleMark(schema.marks.em),
        'Mod-`': toggleMark(schema.marks.code),
        Enter: splitListItem(S.list_item),
        Tab: sinkListItem(S.list_item),
        'Shift-Tab': liftListItem(S.list_item),
      }),
      keymap(baseKeymap),
      placeholderPlugin(labels.placeholder),
      bubblePlugin(host, labels),
      handlePlugin(labels),
    );
  }
  return new EditorView(el, {
    state: EditorState.create({ schema, plugins }),
    editable: () => !!opts.editable,
    attributes: { class: 'pnex-doc-content', spellcheck: 'true' },
  });
}

function docOfState(b64) {
  const doc = new Y.Doc();
  const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  Y.applyUpdate(doc, bin);
  return doc;
}

function pmDocOf(ydoc) {
  return yXmlFragmentToProseMirrorRootNode(ydoc.getXmlFragment('prosemirror'), schema);
}

// Leaf blocks (paragraphs, headings, list item lines, rules), so a change in
// one list item shows as that item, not as the whole list.
function blocksOf(pm) {
  const out = [];
  pm.descendants((n) => {
    if (n.isTextblock || n.isLeaf) {
      out.push(n.type.name + '\u0000' + n.textContent);
      return false;
    }
    return true;
  });
  return out;
}

// Block-level LCS diff: [{op: 'eq'|'add'|'del', kind, text}].
function diffBlocks(a, b) {
  const n = a.length, m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Int32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const out = [];
  const push = (op, s) => { const [kind, text] = s.split('\u0000'); out.push({ op, kind, text }); };
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) { push('eq', a[i]); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) push('del', a[i++]);
    else push('add', b[j++]);
  }
  while (i < n) push('del', a[i++]);
  while (j < m) push('add', b[j++]);
  return out;
}

window.pnexDoc = {
  // Live page: {wsUrl, editable, user: {name, color}, labels} → bool.
  mount(host, opts) {
    try {
      this.unmount(host);
      const doc = new Y.Doc();
      const awareness = new awarenessProtocol.Awareness(doc);
      if (opts.user) awareness.setLocalStateField('user', opts.user);
      const view = makeView(host, doc, opts, [yCursorPlugin(awareness, { cursorBuilder }), commentsPlugin(host)]);
      if (!view) return false;
      const provider = new Provider(host, doc, awareness);
      provider.onFinal = () => view.setProps({ editable: () => false });
      editors[host] = { doc, awareness, view, provider };
      report(host, { status: 'connecting', closeCode: null, peers: 0, request: null, needTicket: 0 });
      provider.connect(opts.wsUrl);
      return true;
    } catch (e) {
      console.error('[pnexDoc] mount', e);
      return false;
    }
  },
  // Frozen version, read-only, no websocket.
  mountStatic(host, b64) {
    try {
      this.unmount(host);
      const doc = docOfState(b64);
      const view = makeView(host, doc, { editable: false }, []);
      if (!view) return false;
      editors[host] = { doc, view };
      return true;
    } catch (e) {
      console.error('[pnexDoc] mountStatic', e);
      return false;
    }
  },
  reconnect(host, wsUrl) {
    const ed = editors[host];
    if (ed && ed.provider) ed.provider.connect(wsUrl);
  },
  unmount(host) {
    const ed = editors[host];
    if (!ed) return;
    if (ed.provider) ed.provider.destroy();
    if (ed.awareness) ed.awareness.destroy();
    ed.view.destroy();
    ed.doc.destroy();
    delete editors[host];
    delete window.__pnexDoc[host];
  },
  // Header command (undo, redo).
  command(host, name) {
    const ed = editors[host];
    const make = COMMANDS[name];
    if (!ed || !make) return false;
    const view = ed.view;
    const ok = make()(view.state, view.dispatch, view);
    view.focus();
    return ok;
  },
  // Link on the selection; refused (false) outside the allowlist.
  link(host, href) {
    const ed = editors[host];
    if (!ed || !linkAllowed(href)) return false;
    const { state, dispatch } = ed.view;
    if (state.selection.empty) return false;
    dispatch(state.tr.addMark(state.selection.from, state.selection.to, schema.marks.link.create({ href: href.trim() })));
    return true;
  },
  // Anchor of the current selection: {anchor, quote}, or null.
  selectionAnchor(host) {
    const ed = editors[host];
    if (!ed) return null;
    const { state } = ed.view;
    const b = binding(state);
    const { from, to, empty } = state.selection;
    if (!b || empty) return null;
    const rel = (pos) => b64(Y.encodeRelativePosition(absolutePositionToRelativePosition(pos, b.type, b.mapping)));
    return { anchor: rel(from) + '.' + rel(to), quote: state.doc.textBetween(from, to, ' ').slice(0, 300) };
  },
  // Comments to highlight: [{id, anchor, resolved}].
  setComments(host, list) {
    const ed = editors[host];
    if (!ed) return false;
    ed.view.dispatch(ed.view.state.tr.setMeta(commentsKey, Array.isArray(list) ? list : []));
    return true;
  },
  // Selects and shows the text a comment is anchored on; false if it is gone.
  focusAnchor(host, anchor) {
    const ed = editors[host];
    if (!ed) return false;
    const r = anchorRange(ed.view.state, anchor);
    if (!r) return false;
    const { state, dispatch } = ed.view;
    dispatch(state.tr.setSelection(TextSelection.create(state.doc, r.from, r.to)).scrollIntoView());
    ed.view.focus();
    return true;
  },
  exportMarkdown(host) {
    const ed = editors[host];
    return ed ? defaultMarkdownSerializer.serialize(ed.view.state.doc) : '';
  },
  // Appends the parsed Markdown at the end of the page.
  importMarkdown(host, md) {
    const ed = editors[host];
    if (!ed) return false;
    try {
      const parsed = mdParser.parse(String(md || ''));
      const { state, dispatch } = ed.view;
      dispatch(state.tr.insert(state.doc.content.size, parsed.content).scrollIntoView());
      return true;
    } catch (e) {
      console.error('[pnexDoc] importMarkdown', e);
      return false;
    }
  },
  // Replaces the live content with a version (a normal, undoable edit).
  restore(host, b64) {
    const ed = editors[host];
    if (!ed || !ed.provider) return false;
    try {
      const pm = pmDocOf(docOfState(b64));
      const { state, dispatch } = ed.view;
      dispatch(state.tr.replaceWith(0, state.doc.content.size, pm.content));
      return true;
    } catch (e) {
      console.error('[pnexDoc] restore', e);
      return false;
    }
  },
  // Waits until the server has persisted the local edits → Promise<bool>.
  flush(host) {
    const ed = editors[host];
    return ed ? ed.provider.flush() : Promise.resolve(false);
  },
  // Diff of two frozen versions (b64 states), block level.
  diff(b64a, b64b) {
    try {
      return diffBlocks(blocksOf(pmDocOf(docOfState(b64a))), blocksOf(pmDocOf(docOfState(b64b))));
    } catch (e) {
      console.error('[pnexDoc] diff', e);
      return null;
    }
  },
};

// Base editor styles (ProseMirror needs white-space rules; y-prosemirror
// draws remote cursors).
(function injectStyle() {
  if (document.getElementById('pnex-doc-style')) return;
  const s = document.createElement('style');
  s.id = 'pnex-doc-style';
  s.textContent = `
.ProseMirror{position:relative;word-wrap:break-word;white-space:pre-wrap;white-space:break-spaces;font-variant-ligatures:none;outline:none;min-height:12rem}
.ProseMirror pre{white-space:pre-wrap}
.ProseMirror li{position:relative}
.pnex-doc-comment{background:#fef08a;border-bottom:2px solid #eab308;cursor:pointer}
.ProseMirror-hideselection *::selection{background:transparent}
.ProseMirror-selectednode{outline:2px solid #8cf}
.ProseMirror-separator,.ProseMirror-gapcursor{display:none}
.ProseMirror-yjs-cursor{position:relative;margin-left:-1px;margin-right:-1px;border-left:1px solid;border-right:1px solid;word-break:normal;pointer-events:none}
.ProseMirror-yjs-cursor>div{position:absolute;top:-1.05em;left:-1px;font-size:11px;line-height:normal;font-weight:600;user-select:none;color:#fff;padding:0 3px;border-radius:3px 3px 3px 0;white-space:nowrap}
.pnex-doc-content{color:#37352f;font-size:16px;line-height:1.5;padding-bottom:30vh}
.pnex-doc-content>*{margin:0}
.pnex-doc-content p{margin:1px 0;padding:3px 2px}
.pnex-doc-content h1,.pnex-doc-content h2,.pnex-doc-content h3{font-weight:600;line-height:1.3;padding:3px 2px;margin:0}
.pnex-doc-content h1{font-size:1.875em;margin-top:1.4em}
.pnex-doc-content h2{font-size:1.5em;margin-top:1.1em}
.pnex-doc-content h3{font-size:1.25em;margin-top:1em}
.pnex-doc-content>:first-child{margin-top:0}
.pnex-doc-content ul,.pnex-doc-content ol{padding-left:1.7em;margin:1px 0}
.pnex-doc-content ul{list-style:disc}
.pnex-doc-content ol{list-style:decimal}
.pnex-doc-content li>p{padding:3px 2px}
.pnex-doc-content blockquote{border-left:3px solid currentColor;padding:3px 14px;margin:4px 0;font-size:1.1em}
.pnex-doc-content blockquote p{padding:0}
.pnex-doc-content pre{background:#f7f6f3;border-radius:4px;padding:24px 24px;margin:4px 0;font:14px/1.6 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;tab-size:2}
.pnex-doc-content code{background:rgba(135,131,120,.15);color:#eb5757;border-radius:4px;padding:.2em .4em;font:85% ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
.pnex-doc-content pre code{background:none;color:inherit;padding:0;font-size:inherit}
.pnex-doc-content hr{border:0;border-top:1px solid rgba(55,53,47,.16);margin:14px 2px}
.pnex-doc-content a{color:inherit;text-decoration:underline;text-decoration-color:rgba(55,53,47,.4);text-underline-offset:2px}
.pnex-doc-content strong{font-weight:600}
.pnex-doc-content .is-empty::before{content:attr(data-placeholder);color:rgba(55,53,47,.35);pointer-events:none;float:left;height:0}
.pnex-doc-handle,.pnex-doc-menu-item,.pnex-doc-bubble-btn{border:0;background:transparent;font-family:inherit;margin:0}
.pnex-doc-handle{position:absolute;width:24px;height:24px;display:flex;align-items:center;justify-content:center;border-radius:4px;color:rgba(55,53,47,.35);font-size:20px;line-height:1;opacity:0;transition:opacity .15s;cursor:pointer}
.pnex-doc-handle.is-visible{opacity:1}
.pnex-doc-handle:hover{background:rgba(55,53,47,.08);color:rgba(55,53,47,.6)}
.pnex-doc-menu{position:fixed;z-index:60;width:300px;max-height:min(340px,60vh);overflow-y:auto;background:#fff;border-radius:8px;padding:4px;box-shadow:0 0 0 1px rgba(15,15,15,.05),0 3px 6px rgba(15,15,15,.1),0 9px 24px rgba(15,15,15,.2)}
.pnex-doc-menu-section{padding:8px 10px 4px;font-size:12px;font-weight:500;color:rgba(55,53,47,.65)}
.pnex-doc-menu-empty{padding:6px 10px;font-size:14px;color:rgba(55,53,47,.5)}
.pnex-doc-menu-item{display:flex;align-items:center;gap:10px;width:100%;padding:4px 10px;border-radius:6px;text-align:left;font-size:14px;color:#37352f;cursor:pointer}
.pnex-doc-menu-item.is-active{background:rgba(55,53,47,.08)}
.pnex-doc-menu-icon{width:22px;height:22px;flex:none;display:flex;align-items:center;justify-content:center;border:1px solid rgba(55,53,47,.16);border-radius:4px;font-size:11px;font-weight:600;color:rgba(55,53,47,.75);background:#fff}
.pnex-doc-menu-label{flex:1}
.pnex-doc-menu-hint{font-size:12px;color:rgba(55,53,47,.45);font-family:ui-monospace,monospace}
.pnex-doc-bubble{position:fixed;z-index:60;display:flex;align-items:center;gap:1px;padding:3px;background:#fff;border-radius:6px;box-shadow:0 0 0 1px rgba(15,15,15,.05),0 3px 6px rgba(15,15,15,.1),0 9px 24px rgba(15,15,15,.2)}
.pnex-doc-bubble-btn{min-width:28px;height:28px;padding:0 8px;border-radius:4px;font-size:14px;color:#37352f;cursor:pointer}
.pnex-doc-bubble-btn:hover{background:rgba(55,53,47,.08)}
.pnex-doc-bubble-btn.is-active{color:#2383e2}
.pnex-doc-bubble-btn.is-bold{font-weight:700}
.pnex-doc-bubble-btn.is-italic{font-style:italic;font-family:Georgia,serif}
.pnex-doc-bubble-btn.is-code{font:12px ui-monospace,monospace}
.pnex-doc-bubble-sep{width:1px;height:18px;margin:0 3px;background:rgba(55,53,47,.16)}
`;
  document.head.appendChild(s);
})();
