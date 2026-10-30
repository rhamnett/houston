//! The island: a transparent, click-through window over the notch hosting a
//! webview. The page draws the black shape growing out of the notch, sizes it
//! for the current mode and reports the shape's rectangle back so the app knows
//! where the pointer counts as "on the island".

use serde::{Deserialize, Serialize};
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder};
use wry::{WebView, WebViewBuilder};

use crate::UserEvent;
use crate::island::Island;
use crate::notch::{self, Geometry, Notch, Rect};

/// Room for the widest island plus its glow.
pub const WIDTH: f64 = 640.0;
pub const HEIGHT: f64 = 500.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Nothing to show: just the notch.
    Idle,
    /// Collapsed with icon and value either side of the notch.
    Compact,
    /// One island expanded for a few seconds after it arrived.
    Peek,
    /// Pointer on the island: every island, with buttons.
    Expanded,
}

/// Messages from the page.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverlayMessage {
    Ready,
    /// The shape's rectangle in window points, origin top-left.
    Layout {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
    Action {
        id: String,
        action: String,
    },
    Dismiss {
        id: String,
    },
    ClearAll,
    SetDnd {
        on: bool,
    },
    Demo {
        trigger: String,
    },
}

#[derive(Debug, Serialize)]
pub struct IslandView<'a> {
    pub id: &'a str,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(flatten)]
    pub island: &'a Island,
}

#[derive(Debug, Serialize)]
pub struct OverlayState<'a> {
    pub mode: Mode,
    pub islands: Vec<IslandView<'a>>,
    pub peek: Option<&'a str>,
    pub notch: Notch,
    pub dnd: bool,
    /// The macOS Focus keeping Houston quiet, if any.
    pub focus: Option<&'a str>,
    pub api: Option<&'a str>,
    pub sources: Vec<&'a str>,
    pub triggers: &'a [(&'a str, &'a str)],
    /// Show the drill (demo) controls in the expanded island.
    pub drills: bool,
}

pub struct Overlay {
    window: Window,
    webview: WebView,
    frame: Rect,
}

impl Overlay {
    pub fn new(
        target: &EventLoopWindowTarget<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
        geometry: &Geometry,
    ) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title("Houston")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_visible(false)
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .build(target)
            .map_err(|e| format!("window: {e}"))?;
        notch::configure(&window);
        let webview = WebViewBuilder::new()
            .with_transparent(true)
            .with_accept_first_mouse(true)
            .with_html(PAGE)
            .with_ipc_handler(move |request| {
                match serde_json::from_str::<OverlayMessage>(request.body()) {
                    Ok(msg) => {
                        let _ = proxy.send_event(UserEvent::Overlay(msg));
                    }
                    Err(e) => eprintln!("overlay ipc: {e}: {}", request.body()),
                }
            })
            .build(&window)
            .map_err(|e| format!("webview: {e}"))?;
        let frame = geometry.window_frame(WIDTH, HEIGHT);
        notch::set_frame(&window, frame);
        notch::order_front(&window);
        Ok(Self {
            window,
            webview,
            frame,
        })
    }

    /// Follow display changes.
    pub fn place(&mut self, geometry: &Geometry) {
        let frame = geometry.window_frame(WIDTH, HEIGHT);
        if frame != self.frame {
            self.frame = frame;
            notch::set_frame(&self.window, frame);
            notch::order_front(&self.window);
        }
    }

    /// A rectangle from the page (top-left origin) in screen coordinates.
    pub fn to_screen(&self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            x: self.frame.x + x,
            y: self.frame.top() - y - h,
            w,
            h,
        }
    }

    pub fn set_interactive(&self, on: bool) {
        notch::set_ignores_mouse(&self.window, !on);
    }

    pub fn render(&self, json: &str) {
        if let Err(e) = self
            .webview
            .evaluate_script(&format!("window.__render({json})"))
        {
            eprintln!("overlay render: {e}");
        }
    }
}

const PAGE: &str = r##"<!doctype html>
<html><head><meta charset="utf-8">
<style>
  :root { color-scheme: dark;
    --fg: #f5f5f7; --muted: rgba(235,235,245,0.58); --faint: rgba(235,235,245,0.32);
    --card: rgba(255,255,255,0.075); --card2: rgba(255,255,255,0.13); --line: rgba(255,255,255,0.09);
    --critical: #ff453a; --warning: #ff9f0a; --info: #0a84ff; --success: #30d158;
    --spring: cubic-bezier(.2, 1.18, .32, 1); --ease: cubic-bezier(.32, .72, 0, 1); }
  * { box-sizing: border-box; -webkit-user-select: none; user-select: none; }
  html, body { margin: 0; height: 100%; background: transparent; overflow: hidden; }
  body { font: 12px -apple-system, system-ui, sans-serif; color: var(--fg); cursor: default; }
  button { font: inherit; color: inherit; border: 0; background: var(--card2); border-radius: 8px;
    padding: 5px 10px; cursor: pointer; transition: background .15s, transform .1s; }
  button:hover { background: rgba(255,255,255,0.2); }
  button:active { transform: scale(.96); }

  .island { position: absolute; top: 0; left: 50%; transform: translateX(-50%);
    background: #000; width: 180px; height: 32px; border-radius: 0 0 12px 12px;
    transition: width .52s var(--spring), height .52s var(--spring), border-radius .4s var(--ease),
      box-shadow .4s var(--ease), opacity .3s; }
  .island.hidden { opacity: 0; }
  /* Concave ears so the shape flows out of the menu bar like the notch does. */
  .island::before, .island::after { content: ""; position: absolute; top: 0; width: var(--ear, 8px);
    height: var(--ear, 8px); transition: width .4s var(--ease), height .4s var(--ease); }
  .island::before { left: calc(-1 * var(--ear, 8px));
    background: radial-gradient(circle at 0 100%, transparent calc(var(--ear, 8px) - .5px), #000 var(--ear, 8px)); }
  .island::after { right: calc(-1 * var(--ear, 8px));
    background: radial-gradient(circle at 100% 100%, transparent calc(var(--ear, 8px) - .5px), #000 var(--ear, 8px)); }
  .island.alarm { animation: glow 2.4s ease-in-out infinite; }
  @keyframes glow {
    0%, 100% { box-shadow: 0 4px 18px rgba(255,69,58,.18), 0 0 0 1px rgba(255,69,58,.25); }
    50% { box-shadow: 0 6px 28px rgba(255,69,58,.45), 0 0 0 1px rgba(255,69,58,.55); } }
  .clip { position: absolute; inset: 0; overflow: hidden; border-radius: inherit; }

  /* Left wing | gap the hardware notch hides | right wing, centred on the notch. */
  .wings { position: absolute; top: 0; left: 50%; transform: translateX(-50%); display: flex;
    align-items: center; opacity: 0; transition: opacity .18s; }
  .mode-compact .wings { opacity: 1; transition: opacity .3s .22s; }
  .wl, .wr { display: flex; flex: none; align-items: center; gap: 6px; white-space: nowrap; }
  .wl { justify-content: flex-start; padding-left: 12px; }
  .wr { justify-content: flex-end; padding-right: 14px; }
  .gap { flex: none; }
  .wr { font-weight: 600; font-variant-numeric: tabular-nums; font-size: 12.5px; }
  .glyph { width: 20px; height: 20px; display: grid; place-items: center; font-size: 13px; position: relative; }
  .glyph svg { position: absolute; inset: 0; }
  .more { color: var(--muted); font-weight: 500; font-size: 11px; margin-left: 4px; }
  .pulse { width: 7px; height: 7px; border-radius: 50%; background: var(--critical);
    animation: beat 1.2s ease-in-out infinite; }
  @keyframes beat { 0%,100% { transform: scale(.8); opacity: .6 } 50% { transform: scale(1.15); opacity: 1 } }

  .content { position: absolute; top: 0; left: 50%; transform: translateX(-50%); opacity: 0;
    filter: blur(6px); transition: opacity .14s, filter .14s; pointer-events: none; }
  .mode-peek .content, .mode-expanded .content { opacity: 1; filter: none; pointer-events: auto;
    transition: opacity .32s .12s, filter .32s .12s; }
  .band { display: flex; align-items: center; padding: 0 14px; }
  .band .side { flex: 1 1 0; min-width: 0; display: flex; align-items: center; gap: 4px; color: var(--muted);
    font-size: 11.5px; white-space: nowrap; overflow: hidden; }
  .band .notchgap { flex: none; }
  .crit { color: var(--critical); font-weight: 600; }
  .band .side.right { justify-content: flex-end; }
  .band button { padding: 3px 8px; font-size: 11px; background: transparent; color: var(--muted); }
  .band button:hover { background: var(--card2); color: var(--fg); }
  .band button.on { color: var(--warning); }
  .count { color: var(--fg); font-weight: 600; }
  .list { padding: 4px 12px 2px; max-height: 360px; overflow-y: auto; }
  .list::-webkit-scrollbar { width: 0; }
  .foot { display: flex; justify-content: space-between; color: var(--faint); font-size: 10.5px;
    padding: 6px 18px 12px; }

  .card { background: var(--card); border-radius: 16px; padding: 11px 12px 12px; margin: 0 0 8px;
    position: relative; animation: rise .38s var(--spring); }
  @keyframes rise { from { opacity: 0; transform: translateY(-6px) scale(.98) } }
  .card .row { display: flex; gap: 10px; align-items: flex-start; }
  .badge { width: 30px; height: 30px; border-radius: 9px; display: grid; place-items: center; flex: none;
    font-size: 16px; background: color-mix(in srgb, var(--sev) 22%, transparent); }
  .titles { flex: 1; min-width: 0; }
  .title { font-size: 13px; font-weight: 600; line-height: 1.25; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sub { color: var(--muted); margin-top: 2px; line-height: 1.3; }
  .when { color: var(--faint); font-size: 11px; white-space: nowrap; padding-top: 1px; }
  .x { position: absolute; top: 8px; right: 8px; width: 20px; height: 20px; padding: 0; border-radius: 50%;
    font-size: 10px; opacity: 0; background: var(--card2); }
  .card:hover .x { opacity: 1; }
  .card:hover .when { opacity: 0; }
  .body { color: rgba(235,235,245,.78); margin: 8px 0 0 40px; line-height: 1.35; }
  .sender { font-weight: 600; color: var(--fg); }
  .sev-critical { --sev: var(--critical); } .sev-warning { --sev: var(--warning); }
  .sev-info { --sev: var(--info); } .sev-success { --sev: var(--success); }
  .card.sev-critical { background: linear-gradient(180deg, rgba(255,69,58,.16), rgba(255,69,58,.05)); }

  .chips { display: flex; align-items: center; gap: 6px; margin: 8px 0 0 40px; font-size: 11px; }
  .st { padding: 2px 7px; border-radius: 6px; font-weight: 600; letter-spacing: .3px; background: var(--card2); }
  .st-ALARM { background: rgba(255,69,58,.22); color: #ff6961; }
  .st-OK { background: rgba(48,209,88,.2); color: #4cd964; }
  .st-INSUFFICIENT_DATA { background: rgba(255,159,10,.2); color: #ffb340; }
  .arrow { color: var(--faint); }

  .prog { height: 6px; border-radius: 3px; background: var(--card2); overflow: hidden; flex: 1; }
  .prog i { display: block; height: 100%; border-radius: 3px; background: var(--sev);
    transition: width .3s linear; position: relative; overflow: hidden; }
  .prog i::after { content: ""; position: absolute; inset: 0;
    background: linear-gradient(90deg, transparent, rgba(255,255,255,.35), transparent);
    animation: sheen 1.4s linear infinite; }
  @keyframes sheen { from { transform: translateX(-100%) } to { transform: translateX(100%) } }
  .progrow { display: flex; align-items: center; gap: 10px; margin: 10px 0 0 40px; }
  .pct { font-variant-numeric: tabular-nums; font-weight: 600; width: 34px; text-align: right; }

  .metric { display: flex; align-items: flex-end; gap: 12px; margin: 8px 0 0 40px; }
  .val { font-size: 26px; font-weight: 300; letter-spacing: -.5px; font-variant-numeric: tabular-nums; line-height: 1; }
  .val small { font-size: 13px; color: var(--muted); margin-left: 2px; }
  .metric svg { flex: 1; height: 38px; }
  .thr { color: var(--faint); font-size: 10.5px; margin: 4px 0 0 40px; }

  .timer { font-size: 26px; font-weight: 300; font-variant-numeric: tabular-nums; margin: 6px 0 0 40px; letter-spacing: -.5px; }

  .fields { display: grid; grid-template-columns: auto 1fr; gap: 3px 10px; margin: 9px 0 0 40px; font-size: 11px; }
  .fields dt { color: var(--faint); } .fields dd { margin: 0; color: var(--muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .actions { display: flex; gap: 6px; margin: 10px 0 0 40px; flex-wrap: wrap; }
  .actions .primary { background: var(--sev); color: #fff; }
  .actions .primary:hover { filter: brightness(1.12); background: var(--sev); }
  .actions .destructive { color: var(--critical); }

  .empty { text-align: center; padding: 14px 10px 8px; }
  .empty .big { font-size: 15px; font-weight: 600; }
  .empty.quiet { padding: 10px 10px 2px; }
  .empty.quiet .big { font-size: 13px; color: var(--muted); font-weight: 500; }
  .empty .sub { margin: 3px 0 12px; }
  .demo { display: flex; flex-wrap: wrap; gap: 6px; justify-content: center; }
  .demo button { font-size: 11px; padding: 5px 9px; }
  .demo button.tour { background: var(--info); color: #fff; }
</style></head>
<body>
<div id="island" class="island mode-idle"><div class="clip">
  <div class="wings" id="wings"><div class="wl" id="wl"></div><div class="gap" id="gap"></div><div class="wr" id="wr"></div></div>
  <div class="content" id="content"></div>
</div></div>
<script>
const post = (m) => window.ipc.postMessage(JSON.stringify(m));
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
/* Minimum wing width, and clearance either side of the notch's reported edges. */
const WING = 56, NOTCH_CLEAR = 6, PEEK_W = 380, EXPANDED_W = 440, MAX_H = 470;
/* Room either side of the notch for the header band in peek and expanded. */
const PEEK_SIDE = 90, EXPANDED_SIDE = 150;
let state = null;

/* Update the DOM in place instead of replacing it, so cards that are already
   showing don't replay their entrance, progress bars glide, and pulses keep
   their rhythm. Children with data-key are matched by key, others by position. */
function patch(el, html) {
  if (el._html === html) return;
  el._html = html;
  const t = document.createElement("template");
  t.innerHTML = html;
  morphChildren(el, t.content);
}
function morphNode(o, n) {
  if (o.nodeType !== n.nodeType || o.nodeName !== n.nodeName) { o.replaceWith(n); return; }
  if (o.nodeType === Node.TEXT_NODE) { if (o.nodeValue !== n.nodeValue) o.nodeValue = n.nodeValue; return; }
  if (o.nodeType !== Node.ELEMENT_NODE) return;
  for (const a of [...o.attributes]) if (!n.hasAttribute(a.name)) o.removeAttribute(a.name);
  for (const a of [...n.attributes]) if (o.getAttribute(a.name) !== a.value) o.setAttribute(a.name, a.value);
  morphChildren(o, n);
}
function morphChildren(parent, next) {
  const incoming = [...next.childNodes];
  const keyOf = (c) => c.nodeType === Node.ELEMENT_NODE ? c.dataset.key : undefined;
  if (incoming.some(keyOf)) {
    const old = new Map();
    for (const c of [...parent.childNodes]) {
      const k = keyOf(c);
      if (k && !old.has(k)) old.set(k, c); else c.remove();
    }
    const wanted = new Set(incoming.map(keyOf));
    for (const [k, c] of old) if (!wanted.has(k)) { c.remove(); old.delete(k); }
    // Survivors stay attached (re-attaching would restart their animations);
    // only new or reordered nodes move.
    let cursor = parent.firstChild;
    for (const n of incoming) {
      const o = old.get(keyOf(n));
      if (o) {
        morphNode(o, n);
        if (o === cursor) { cursor = cursor.nextSibling; continue; }
        parent.insertBefore(o, cursor);
      } else {
        parent.insertBefore(n, cursor);
      }
    }
    return;
  }
  const current = [...parent.childNodes];
  incoming.forEach((n, i) => (current[i] ? morphNode(current[i], n) : parent.appendChild(n)));
  for (const c of current.slice(incoming.length)) c.remove();
}

function clock(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, sec = s % 60;
  const two = (n) => String(n).padStart(2, "0");
  return h ? `${h}:${two(m)}:${two(sec)}` : `${two(m)}:${two(sec)}`;
}
function ago(ms) {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 10) return "now";
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h`;
}
function num(v) { return Number.isInteger(v) ? String(v) : v.toFixed(1); }

/* Short text for the right wing. */
function compactText(i) {
  if (i.compact) return esc(i.compact);
  const k = i.kind;
  switch (k.type) {
    case "progress": return `${Math.round(k.value * 100)}%`;
    case "metric": return `${num(k.value)}${esc(k.unit)}`;
    case "timer": return `<span class="tick" data-start="${k.started_at}">${clock(Date.now() - k.started_at)}</span>`;
    case "alert": return esc(k.to);
    default: return "";
  }
}

function ring(value) {
  const r = 9, c = 2 * Math.PI * r;
  return `<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="${r}" fill="none" stroke="rgba(255,255,255,.15)" stroke-width="2"/>
    <circle cx="10" cy="10" r="${r}" fill="none" stroke="var(--sev)" stroke-width="2" stroke-linecap="round"
      stroke-dasharray="${c}" stroke-dashoffset="${c * (1 - value)}" transform="rotate(-90 10 10)"/></svg>`;
}

function spark(series, threshold, w = 200, h = 38) {
  if (!series || series.length < 2) return "";
  const lo = Math.min(...series, threshold ?? Infinity), hi = Math.max(...series, threshold ?? -Infinity);
  const span = hi - lo || 1;
  const x = (i) => (i / (series.length - 1)) * w;
  const y = (v) => h - 3 - ((v - lo) / span) * (h - 6);
  const pts = series.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  const last = series[series.length - 1];
  const thr = threshold == null ? "" :
    `<line x1="0" x2="${w}" y1="${y(threshold)}" y2="${y(threshold)}" stroke="rgba(255,255,255,.35)" stroke-dasharray="3 3" stroke-width="1"/>`;
  return `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none">
    <defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="var(--sev)" stop-opacity=".35"/><stop offset="1" stop-color="var(--sev)" stop-opacity="0"/></linearGradient></defs>
    <polygon points="0,${h} ${pts} ${w},${h}" fill="url(#g)"/>${thr}
    <polyline points="${pts}" fill="none" stroke="var(--sev)" stroke-width="1.8" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>
    <circle cx="${x(series.length - 1)}" cy="${y(last)}" r="2.6" fill="var(--sev)"/></svg>`;
}

function kindBody(i) {
  const k = i.kind;
  switch (k.type) {
    case "alert":
      return `<div class="chips">${k.from ? `<span class="st st-${esc(k.from)}">${esc(k.from)}</span><span class="arrow">→</span>` : ""}
        <span class="st st-${esc(k.to)}">${esc(k.to)}</span></div>`;
    case "progress":
      return `<div class="progrow"><div class="prog"><i style="width:${(k.value * 100).toFixed(1)}%"></i></div>
        <div class="pct">${Math.round(k.value * 100)}%</div></div>`;
    case "metric":
      return `<div class="metric"><div class="val">${num(k.value)}<small>${esc(k.unit)}</small></div>${spark(k.series, k.threshold)}</div>
        ${k.threshold != null ? `<div class="thr">threshold ${num(k.threshold)}${esc(k.unit)}</div>` : ""}`;
    case "timer":
      return `<div class="timer tick" data-start="${k.started_at}">${clock(Date.now() - k.started_at)}</div>`;
    default: return "";
  }
}

function card(i, interactive) {
  const sender = i.kind.type === "message" && i.kind.sender ? `<span class="sender">${esc(i.kind.sender)}</span> · ` : "";
  const body = i.body || sender ? `<div class="body">${sender}${esc(i.body)}</div>` : "";
  const fields = i.fields.length ? `<dl class="fields">${i.fields.map((f) => `<dt>${esc(f.label)}</dt><dd>${esc(f.value)}</dd>`).join("")}</dl>` : "";
  const actions = interactive && i.actions.length ? `<div class="actions">${i.actions.map((a) =>
    `<button class="${a.role}" data-id="${esc(i.id)}" data-action="${esc(a.id)}">${esc(a.label)}</button>`).join("")}</div>` : "";
  return `<div class="card sev-${i.severity}" data-key="${esc(i.id)}">
    <div class="row"><div class="badge">${esc(i.icon || "🔔")}</div>
      <div class="titles"><div class="title">${esc(i.title)}</div>${i.subtitle ? `<div class="sub">${esc(i.subtitle)}</div>` : ""}</div>
      <div class="when" data-ts="${i.updated_ms}">${ago(i.updated_ms)}</div>
      ${interactive ? `<button class="x" data-dismiss="${esc(i.id)}" title="Dismiss">✕</button>` : ""}</div>
    ${kindBody(i)}${body}${interactive ? fields : ""}${actions}</div>`;
}

function band(left, right, h) {
  return `<div class="band" style="height:${h}px"><div class="side">${left}</div>
    <div class="notchgap" style="width:${state.notch.width + 2 * NOTCH_CLEAR}px"></div><div class="side right">${right}</div></div>`;
}

function expanded(h) {
  const n = state.islands.length;
  const critical = state.islands.filter((i) => i.severity === "critical").length;
  const left = n ? `<span class="count">${n}</span> active${critical ? ` · <span class="crit">${critical} critical</span>` : ""}` : "🛰️ Houston";
  const right = `${state.drills ? `<button data-trigger="tour" title="Run the full drill">▶</button>` : ""}
    <button id="dnd" class="${state.dnd ? "on" : ""}" title="${state.focus ? `Quiet while ${esc(state.focus)} is on` : "Do not disturb"}">${state.focus ? `☾ ${esc(state.focus)}` : state.dnd ? "☾ DND" : "☾"}</button>
    ${n ? `<button id="clear">Clear</button>` : ""}`;
  const list = n ? state.islands.map((i) => card(i, true)).join("") :
    state.drills
      ? `<div class="empty"><div class="big">All systems nominal</div><div class="sub">Nothing needs you. Fancy a drill?</div>
          <div class="demo">${state.triggers.map(([id, label]) => `<button class="${id === "tour" ? "tour" : ""}" data-trigger="${id}">${esc(label)}</button>`).join("")}</div></div>`
      : `<div class="empty quiet"><div class="big">All systems nominal</div></div>`;
  const shown = state.sources.filter((s) => state.drills || s !== "demo");
  const foot = `<div class="foot"><span>${state.api ? `API ${esc(state.api)}` : "API off"}</span><span>${shown.map(esc).join(" · ")}</span></div>`;
  return band(left, right, h) + `<div class="list">${list}</div>` + foot;
}

function peek(h) {
  const i = state.islands.find((x) => x.id === state.peek) || state.islands[0];
  return band(esc(i.source), `<span>${ago(i.updated_ms)}</span>`, h) + `<div class="list">${card(i, false)}</div>`;
}

function render() {
  if (!state) return;
  const el = $("island"), content = $("content");
  const nh = state.notch.height, nw = state.notch.width;
  const lead = state.islands[0];
  let mode = state.mode, w = nw, h = nh, radius = Math.min(12, nh / 2.4), ear = 8;
  const alarm = state.islands.some((i) => i.severity === "critical") && mode !== "expanded";
  const hidden = mode === "idle" && !state.notch.has_notch;
  // Set the class once, unchanged when nothing changed, so the glow keeps going.
  const cls = ["island", `mode-${mode}`, lead && `sev-${lead.severity}`, alarm && "alarm", hidden && "hidden"]
    .filter(Boolean).join(" ");
  if (el.className !== cls) el.className = cls;

  if (mode === "compact") {
    const k = lead.kind;
    const glyph = k.type === "progress" ? `${ring(k.value)}<span style="font-size:9px">${esc(lead.icon || "")}</span>` : esc(lead.icon || "🔔");
    $("wl").className = `wl sev-${lead.severity}`;
    patch($("wl"), `<div class="glyph">${glyph}</div>${lead.severity === "critical" ? `<div class="pulse"></div>` : ""}`);
    $("wr").className = `wr sev-${lead.severity}`;
    $("wr").style.color = `var(--${lead.severity === "info" ? "fg" : lead.severity})`;
    patch($("wr"), compactText(lead) + (state.islands.length > 1 ? `<span class="more">+${state.islands.length - 1}</span>` : ""));
    // Size both wings to the wider content so nothing runs under the notch and
    // the island stays centred on it.
    $("wl").style.width = $("wr").style.width = "auto";
    const wing = Math.ceil(Math.max(WING, $("wl").scrollWidth, $("wr").scrollWidth));
    const gap = nw + 2 * NOTCH_CLEAR;
    $("wl").style.width = $("wr").style.width = `${wing}px`;
    $("gap").style.width = `${gap}px`;
    $("wings").style.height = `${nh}px`;
    w = gap + 2 * wing;
  }
  if (mode === "peek" || mode === "expanded") {
    const side = 2 * (NOTCH_CLEAR + 14) + nw;
    w = mode === "peek" ? Math.max(PEEK_W, side + 2 * PEEK_SIDE) : Math.max(EXPANDED_W, side + 2 * EXPANDED_SIDE);
    radius = 26; ear = 14;
    content.style.width = `${w}px`;
    patch(content, mode === "peek" ? peek(nh) : expanded(nh));
    h = Math.min(MAX_H, content.scrollHeight);
  }
  el.style.width = `${w}px`;
  el.style.height = `${h}px`;
  el.style.borderRadius = `0 0 ${radius}px ${radius}px`;
  el.style.setProperty("--ear", `${ear}px`);
  // Where the pointer counts as on the island (a strip at the top when hidden).
  post({ type: "layout", x: (window.innerWidth - w) / 2, y: 0, w, h: Math.max(h, nh) });
}

window.__render = (s) => { state = s; render(); };

setInterval(() => {
  document.querySelectorAll(".tick").forEach((e) => { e.textContent = clock(Date.now() - Number(e.dataset.start)); });
  document.querySelectorAll(".when").forEach((e) => { e.textContent = ago(Number(e.dataset.ts)); });
}, 1000);

document.addEventListener("click", (e) => {
  const t = e.target.closest("button");
  if (!t) return;
  if (t.dataset.action) post({ type: "action", id: t.dataset.id, action: t.dataset.action });
  else if (t.dataset.dismiss) post({ type: "dismiss", id: t.dataset.dismiss });
  else if (t.dataset.trigger) post({ type: "demo", trigger: t.dataset.trigger });
  else if (t.id === "dnd") post({ type: "set_dnd", on: !state.dnd });
  else if (t.id === "clear") post({ type: "clear_all" });
});
post({ type: "ready" });
</script>
</body></html>
"##;
