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
// peers, slash, dirtySeq}`. The websocket ticket is single-use: on every
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

// ── Commands (toolbar and slash palette) ─────────────────────────────────
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

// "/" typed in an empty block opens the palette (rendered by Rust).
function slashPlugin(host) {
  return new Plugin({
    view: () => ({
      update(view) {
        const { $from, empty } = view.state.selection;
        const open = empty && $from.parent.isTextblock && $from.parent.textContent === '/';
        const cur = (window.__pnexDoc[host] || {}).slash;
        if (open && !cur) {
          const c = view.coordsAtPos($from.pos);
          report(host, { slash: { seq: ++seq, x: c.left, y: c.bottom } });
        } else if (!open && cur) {
          report(host, { slash: null });
        }
      },
    }),
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

  connect(url) {
    if (this.ws) this.ws.close();
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
        if (kind === syncProtocol.messageYjsSyncStep2) report(this.host, { status: 'synced', closeCode: null });
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
  if (opts.editable) {
    plugins.push(
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
      slashPlugin(host),
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

function blocksOf(pm) {
  const out = [];
  pm.forEach((n) => out.push(n.type.name + '\u0000' + n.textContent));
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
  // Live page: {wsUrl, editable, user: {name, color}} → bool.
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
      report(host, { status: 'connecting', closeCode: null, peers: 0, slash: null, needTicket: 0 });
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
  // Toolbar / palette command; `dropSlash` removes the "/" that opened the palette.
  command(host, name, dropSlash) {
    const ed = editors[host];
    const make = COMMANDS[name];
    if (!ed || !make) return false;
    const view = ed.view;
    if (dropSlash) {
      const { $from } = view.state.selection;
      if ($from.parent.textContent === '/') view.dispatch(view.state.tr.delete($from.start(), $from.end()));
    }
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
`;
  document.head.appendChild(s);
})();
