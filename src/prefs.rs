//! The Preferences window: an ordinary titled window hosting a webview form
//! over the whole config. Saving restarts only the sources whose settings
//! changed (see `sources::fingerprint`).
//!
//! Secrets never reach the page. It learns only whether each one is set, can
//! replace or remove one, and asks the app to copy the API token for it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebView, WebViewBuilder};

use crate::UserEvent;
use crate::config::{AlarmRule, AwsAccount, LinearConfig};

/// Messages from the page.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PrefsMessage {
    Ready,
    /// The edited config (without secrets) and any secrets the user replaced
    /// or removed, keyed by Keychain account name; an empty value removes.
    Save {
        config: Value,
        #[serde(default)]
        secrets: HashMap<String, String>,
    },
    /// One poll as this account. `secret` is an access key's secret typed
    /// but not saved yet; the Keychain's is used otherwise.
    TestCloudwatch {
        id: String,
        account: AwsAccount,
        #[serde(default)]
        aws_cli: Option<String>,
        #[serde(default)]
        secret: Option<String>,
    },
    /// Every alarm in the account, any state, to pick from.
    DiscoverAlarms {
        id: String,
        account: AwsAccount,
        #[serde(default)]
        aws_cli: Option<String>,
        #[serde(default)]
        secret: Option<String>,
    },
    /// `aws sso login --profile …` in the browser.
    SsoLogin {
        id: String,
        profile: String,
        #[serde(default)]
        aws_cli: Option<String>,
    },
    TestLinear {
        id: String,
        linear: LinearConfig,
        /// A key typed but not saved yet; the stored one otherwise.
        #[serde(default)]
        api_key: Option<String>,
    },
    /// Which rule (if any) an alarm name would hit.
    TestRule {
        name: String,
        rules: Vec<AlarmRule>,
    },
    CopyToken,
    RegenerateToken,
    /// System Settings › Privacy & Security › Full Disk Access.
    OpenFullDiskAccess,
    OpenConfigFolder,
    Close,
}

/// Which secrets are set, for the page.
#[derive(Debug, Serialize)]
pub struct SecretFlags {
    pub api_token: bool,
    pub linear_api_key: bool,
    /// Access key id → whether its secret is in the Keychain.
    pub aws: HashMap<String, bool>,
}

pub struct Prefs {
    window: Window,
    webview: WebView,
}

impl Prefs {
    pub fn open(
        target: &EventLoopWindowTarget<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title("Houston · Mission settings")
            .with_inner_size(LogicalSize::new(820.0, 600.0))
            .with_min_inner_size(LogicalSize::new(640.0, 440.0))
            .build(target)
            .map_err(|e| format!("window: {e}"))?;
        let webview = WebViewBuilder::new()
            .with_html(PAGE)
            .with_ipc_handler(move |request| {
                match serde_json::from_str::<PrefsMessage>(request.body()) {
                    Ok(msg) => {
                        let _ = proxy.send_event(UserEvent::Prefs(msg));
                    }
                    Err(e) => eprintln!("prefs ipc: {e}"),
                }
            })
            .build(&window)
            .map_err(|e| format!("webview: {e}"))?;
        window.set_focus();
        Ok(Self { window, webview })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        self.window.set_visible(true);
        self.window.set_focus();
    }

    /// Hand the page a message (`window.__prefs(msg)`).
    pub fn send(&self, msg: &Value) {
        if let Err(e) = self
            .webview
            .evaluate_script(&format!("window.__prefs({msg})"))
        {
            eprintln!("prefs: {e}");
        }
    }
}

const PAGE: &str = r##"<!doctype html>
<html><head><meta charset="utf-8">
<style>
  :root { color-scheme: light dark;
    --bg: #f5f5f7; --panel: #ffffff; --side: #ececf0; --fg: #1d1d1f; --muted: #6e6e73;
    --line: rgba(0,0,0,.1); --input: #ffffff; --accent: #0a84ff; --danger: #ff3b30; --ok: #28a745;
    --chip: rgba(0,0,0,.06); }
  @media (prefers-color-scheme: dark) { :root {
    --bg: #1c1c1e; --panel: #232326; --side: #18181a; --fg: #f5f5f7; --muted: #98989f;
    --line: rgba(255,255,255,.1); --input: #2c2c2e; --ok: #30d158; --danger: #ff453a;
    --chip: rgba(255,255,255,.08); } }
  * { box-sizing: border-box; }
  html, body { margin: 0; height: 100%; }
  body { font: 13px -apple-system, system-ui, sans-serif; color: var(--fg); background: var(--bg);
    display: grid; grid-template-columns: 190px 1fr; grid-template-rows: 1fr auto; height: 100vh; }
  nav { grid-row: 1 / 3; background: var(--side); padding: 18px 10px; border-right: 1px solid var(--line);
    -webkit-user-select: none; user-select: none; }
  nav .brand { font-weight: 700; font-size: 15px; padding: 0 10px 2px; }
  nav .tag { color: var(--muted); font-size: 11px; padding: 0 10px 16px; font-style: italic; }
  nav a { display: block; padding: 6px 10px; border-radius: 7px; color: inherit; text-decoration: none;
    cursor: default; margin-bottom: 1px; }
  nav a.on { background: var(--accent); color: #fff; }
  nav a .dot { float: right; width: 7px; height: 7px; border-radius: 50%; margin-top: 5px; background: var(--muted); opacity: .4; }
  nav a .dot.live { background: var(--ok); opacity: 1; }
  main { overflow: auto; padding: 22px 28px 28px; }
  h1 { font-size: 20px; margin: 0 0 4px; }
  .lede { color: var(--muted); margin: 0 0 18px; max-width: 60ch; line-height: 1.45; }
  .card { background: var(--panel); border: 1px solid var(--line); border-radius: 10px; padding: 4px 16px; margin-bottom: 16px; }
  .row { display: grid; grid-template-columns: 180px 1fr; gap: 12px; align-items: center; padding: 10px 0;
    border-bottom: 1px solid var(--line); }
  .row:last-child { border-bottom: 0; }
  .row > label { color: var(--fg); }
  .row .hint { grid-column: 2; color: var(--muted); font-size: 11.5px; margin-top: -6px; line-height: 1.4; }
  input[type=text], input[type=number], input[type=password], select, textarea {
    font: inherit; color: inherit; background: var(--input); border: 1px solid var(--line); border-radius: 6px;
    padding: 5px 8px; width: 100%; min-width: 0; }
  textarea { min-height: 64px; font-family: ui-monospace, Menlo, monospace; font-size: 12px; }
  input:focus, select:focus, textarea:focus { outline: 2px solid var(--accent); outline-offset: -1px; }
  .switch { position: relative; display: inline-block; vertical-align: middle; width: 38px; height: 22px; }
  .switch input { opacity: 0; width: 0; height: 0; }
  .switch span { position: absolute; inset: 0; background: var(--chip); border-radius: 11px; transition: .2s; cursor: pointer; }
  .switch span::after { content: ""; position: absolute; width: 18px; height: 18px; left: 2px; top: 2px;
    background: #fff; border-radius: 50%; transition: .2s; box-shadow: 0 1px 2px rgba(0,0,0,.3); }
  .switch input:checked + span { background: var(--ok); }
  .switch input:checked + span::after { transform: translateX(16px); }
  button { font: inherit; color: inherit; background: var(--chip); border: 1px solid var(--line); border-radius: 6px;
    padding: 5px 12px; cursor: pointer; white-space: nowrap; }
  button:hover { filter: brightness(1.08); }
  button.primary { background: var(--accent); color: #fff; border-color: transparent; }
  button.primary:disabled { opacity: .45; cursor: default; }
  button.danger { color: var(--danger); }
  button.small { padding: 3px 8px; font-size: 12px; }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; font-weight: 500; color: var(--muted); font-size: 11.5px; padding: 8px 4px 4px; }
  td { padding: 4px; vertical-align: middle; }
  td.narrow { width: 1%; white-space: nowrap; }
  .result { font-size: 12px; margin: 2px 0 8px; padding: 6px 9px; border-radius: 6px; background: var(--chip); }
  .result.ok { color: var(--ok); } .result.err { color: var(--danger); } .result.busy { color: var(--muted); }
  .secret { display: flex; gap: 8px; align-items: center; }
  .secret input { flex: 1; min-width: 0; }
  .pill { display: inline-block; white-space: nowrap; font-size: 11px; padding: 2px 8px; border-radius: 10px; background: var(--chip); color: var(--muted); }
  .pill.ok { color: var(--ok); }
  .status { font-family: ui-monospace, Menlo, monospace; font-size: 11.5px; color: var(--muted); white-space: pre-wrap; }
  footer { grid-column: 2; display: flex; gap: 10px; align-items: center; padding: 12px 28px;
    border-top: 1px solid var(--line); background: var(--panel); }
  footer .msg { flex: 1; color: var(--muted); font-size: 12px; }
  footer .msg.err { color: var(--danger); } footer .msg.ok { color: var(--ok); }
  .tryrule { display: flex; gap: 8px; margin: 12px 0 4px; }
  tr.hit td { background: rgba(10,132,255,.12); }
  .cardhead { display: flex; align-items: center; gap: 8px; padding: 12px 0 10px; border-bottom: 1px solid var(--line); }
  .cardhead b { font-size: 14px; }
  .cardhead .meta { color: var(--muted); font-size: 11.5px; margin-top: 2px; }
  .grow { flex: 1; }
  .bar { display: flex; align-items: center; gap: 12px; padding: 10px 16px; }
  .inline { display: flex; align-items: center; gap: 8px; }
  .numbox input { width: 76px; }
  .acct { padding: 0 16px 14px; }
  .acct > .result { margin-top: 10px; }
  .grid3 { display: grid; grid-template-columns: 1.1fr .7fr 1.2fr; gap: 10px 12px; margin-top: 12px; }
  .grid2 { display: grid; grid-template-columns: 1fr 1.3fr; gap: 10px 12px; }
  .fld { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .fld > span { font-size: 11.5px; color: var(--muted); font-weight: 500; }
  .fld small, small.note { color: var(--muted); font-size: 11px; line-height: 1.35; }
  .creds, .disc, .summary { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--line); }
  .creds { display: flex; flex-direction: column; gap: 10px; }
  .summary { color: var(--muted); font-size: 12px; }
  .summary b { color: var(--fg); font-weight: 500; }
  .seg { display: inline-flex; background: var(--chip); border-radius: 7px; padding: 2px; gap: 2px; align-self: flex-start; }
  .seg button { border: 0; background: transparent; padding: 3px 10px; border-radius: 5px; font-size: 12px; }
  .seg button.on { background: var(--accent); color: #fff; box-shadow: 0 1px 2px rgba(0,0,0,.18); }
  .disc .tools { display: flex; gap: 8px; align-items: center; margin-bottom: 8px; }
  .disc .tools input[type=text] { max-width: 220px; }
  .disc .count { color: var(--muted); font-size: 12px; }
  .disc .scroll { max-height: 300px; overflow: auto; border: 1px solid var(--line); border-radius: 8px; margin-bottom: 6px; }
  .disc th { position: sticky; top: 0; background: var(--panel); z-index: 1; }
  .disc td { border-top: 1px solid var(--line); font-size: 12px; }
  .disc tr:hover td { background: var(--chip); }
  /* Fixed columns: long CDK alarm names get the room, the rest stay narrow. */
  .disc table { table-layout: fixed; }
  .disc col.w { width: 30px; } .disc col.now { width: 78px; } .disc col.cond { width: 30%; } .disc col.sev { width: 96px; }
  .disc td.name { font-size: 12px; font-weight: 500; line-height: 1.35; overflow-wrap: anywhere; }
  .disc td.cond { color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .disc td.narrow { width: auto; }
  .disc select { max-width: 100%; }
  .disc select { width: auto; padding: 2px 4px; font-size: 12px; }
  button.add { margin-bottom: 8px; }
  .st { font-size: 10.5px; font-weight: 600; padding: 1px 6px; border-radius: 5px; background: var(--chip); }
  .st.ALARM { color: var(--danger); } .st.OK { color: var(--ok); } .st.INSUFFICIENT_DATA { color: #ff9f0a; }
  code { font-family: ui-monospace, Menlo, monospace; font-size: 12px; background: var(--chip); padding: 1px 5px; border-radius: 4px; }
</style></head>
<body>
<nav id="nav"></nav>
<main id="main"></main>
<footer>
  <span class="msg" id="msg">All systems nominal.</span>
  <button id="revert">Revert</button>
  <button id="save" class="primary" disabled>Save</button>
</footer>
<script>
const post = (m) => window.ipc.postMessage(JSON.stringify(m));
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));

const SECTIONS = [
  ["general", "General"], ["cloudwatch", "CloudWatch"], ["sqs", "SQS"], ["linear", "Linear"],
  ["rules", "Alarm rules"], ["iot", "IoT Core"], ["api", "Local API"],
];
let tab = "general";
let saved = null;       // config as last loaded or saved
let cfg = null;         // the working copy
let secretFlags = {};   // which secrets are in the Keychain
let secretEdits = {};   // account -> new value ("" removes)
let statuses = [];
let results = {};       // test id -> {cls, text}
let profiles = [];      // from ~/.aws/config
let focusNow = "", focusUnreadable = false;
let discovered = {};    // "cw<i>" -> alarms found by Discover alarms
let ruleHit = null;

const get = (path) => path.split(".").reduce((o, k) => o?.[k], cfg);
const set = (path, v) => { const ks = path.split("."); const last = ks.pop();
  ks.reduce((o, k) => (o[k] ??= {}), cfg)[last] = v; dirty(); };

function dirty() {
  const changed = JSON.stringify(cfg) !== JSON.stringify(saved) || Object.keys(secretEdits).length > 0;
  $("save").disabled = !changed;
  if (changed) msg("Unsaved changes.", "");
}
function msg(text, cls) { const m = $("msg"); m.textContent = text; m.className = "msg " + (cls || ""); }

// ---- field helpers --------------------------------------------------------
function row(label, control, hint) {
  return `<div class="row"><label>${label}</label><div>${control}</div>${hint ? `<div class="hint">${hint}</div>` : ""}</div>`;
}
function toggle(path) {
  return `<label class="switch"><input type="checkbox" data-path="${path}" data-kind="bool" ${get(path) ? "checked" : ""}><span></span></label>`;
}
function text(path, placeholder = "") {
  return `<input type="text" data-path="${path}" data-kind="text" value="${esc(get(path) ?? "")}" placeholder="${esc(placeholder)}">`;
}
function optText(path, placeholder = "") {
  return `<input type="text" data-path="${path}" data-kind="opt" value="${esc(get(path) ?? "")}" placeholder="${esc(placeholder)}">`;
}
function num(path, min, max) {
  return `<input type="number" data-path="${path}" data-kind="num" min="${min}" max="${max}" value="${esc(get(path))}">`;
}
function list(path, placeholder) {
  return `<input type="text" data-path="${path}" data-kind="list" value="${esc((get(path) || []).join(", "))}" placeholder="${esc(placeholder)}">`;
}
function lines(path, placeholder) {
  return `<textarea data-path="${path}" data-kind="lines" placeholder="${esc(placeholder)}">${esc((get(path) || []).join("\n"))}</textarea>`;
}
function result(id) {
  const r = results[id];
  return r ? `<div class="result ${r.cls}">${esc(r.text)}</div>` : "";
}
function secretField(account, flag, placeholder) {
  const edit = secretEdits[account];
  const state = edit === "" ? `<span class="pill">will be removed</span>`
    : edit ? `<span class="pill">replaced, not saved</span>`
    : flag ? `<span class="pill ok">🔒 in Keychain</span>` : `<span class="pill">not set</span>`;
  return `<div class="secret"><input type="password" data-secret="${account}" placeholder="${esc(flag ? "Type to replace" : placeholder)}" value="${esc(edit || "")}" autocomplete="off">
    ${state}${flag && edit === undefined ? `<button class="small danger" data-act="remove-secret" data-account="${account}">Remove</button>` : ""}</div>`;
}

// A segmented control; each button carries `attrs` and data-value.
function seg(attrs, options, value) {
  return `<div class="seg">${options.map(([v, l]) =>
    `<button type="button" ${attrs} data-value="${v}" class="${v === value ? "on" : ""}">${l}</button>`).join("")}</div>`;
}
// A label stacked over its control.
function fld(label, control, hint) {
  return `<div class="fld"><span>${label}</span>${control}${hint ? `<small>${hint}</small>` : ""}</div>`;
}

// AWS credentials for an account or the SQS queue at `base`.
function credsBlock(base) {
  const auth = get(base + ".auth") || { type: "profile" };
  const isKey = auth.type === "access_key";
  const picker = seg(`data-act="auth" data-base="${base}"`, [["profile", "Profile or SSO"], ["access_key", "IAM access key"]], auth.type);
  if (isKey) {
    const akid = auth.access_key_id || "";
    return `<div class="creds">${fld("Credentials", picker)}
      <div class="grid2">
        ${fld("Access key ID", `<input type="text" data-path="${base}.auth.access_key_id" data-kind="text" data-rerender value="${esc(akid)}" placeholder="AKIA…" autocomplete="off">`)}
        ${fld("Secret access key", akid ? secretField("aws-secret:" + akid, secretFlags.aws?.[akid], "wJalrXUtnFEMI…") : `<span class="pill">enter the key ID first</span>`)}
      </div>
      <small class="note">The secret goes in the Keychain. A user that can only read alarms is best.</small></div>`;
  }
  const prof = get(base + ".profile") || "";
  const known = profiles.find((p) => p.name === (prof || "default"));
  const hint = known ? `${known.sso ? "SSO profile" : "Profile"} from ~/.aws/config${known.region ? ` · ${esc(known.region)}` : ""}`
    : "Any profile in ~/.aws/config";
  return `<div class="creds">${fld("Credentials", picker)}
    ${fld("Profile", `<div class="inline"><input type="text" list="aws-profiles" data-path="${base}.profile" data-kind="opt" value="${esc(prof)}" placeholder="default">
      <button class="small" data-act="sso" data-base="${base}" title="aws sso login --profile …">Sign in with SSO</button></div>`, hint)}
    ${result("sso:" + base)}</div>`;
}

function secretFor(base) {
  const auth = get(base + ".auth");
  return auth?.type === "access_key" ? secretEdits["aws-secret:" + auth.access_key_id] || null : null;
}

// ---- CloudWatch accounts -------------------------------------------------
// Page-only state: whether an account shows its tick list, its filters, and
// ticks put aside while "everything" is chosen.
const onlyTicked = {}, discFilter = {}, stashed = {};

const isOnly = (i) => onlyTicked[i] ?? (cfg.cloudwatch.accounts[i].include || []).length > 0;

function accountName(a) {
  return a.label || (a.auth?.type === "access_key" ? "Access key account" : a.profile) || "New account";
}
function accountMeta(i) {
  const a = cfg.cloudwatch.accounts[i], n = (a.include || []).length;
  const who = a.auth?.type === "access_key" ? "access key" : (a.profile || "default profile");
  const what = n ? `${n} chosen alarm${n === 1 ? "" : "s"}` : a.alarm_prefix ? `alarms starting ${esc(a.alarm_prefix)}` : "every alarm";
  return `${esc(who)} · ${esc(a.region || "no region")} · watching ${what}`;
}
function countText(i) {
  const found = discovered["cw" + i] || [], n = (cfg.cloudwatch.accounts[i].include || []).length;
  if (!isOnly(i)) return `${found.length} found · all watched`;
  return n ? `${n} of ${found.length} ticked` : `none ticked yet, so all ${found.length} are watched`;
}
// Refresh the counts without re-rendering, so the list stays put.
function refreshCounts(i) {
  const c = $("count-" + i); if (c) c.textContent = countText(i);
  const m = $("meta-" + i); if (m) m.innerHTML = accountMeta(i);
}
function applyFilter(i) {
  const f = discFilter[i] || {}, q = (f.q || "").toLowerCase();
  document.querySelectorAll(`.disc[data-i="${i}"] tbody tr`).forEach((tr) => {
    tr.hidden = !(tr.dataset.name.includes(q) && (!f.state || tr.dataset.state === f.state));
  });
}

function watchPanel(i) {
  const found = discovered["cw" + i];
  if (!found) {
    return `<div class="summary">Watching ${(cfg.cloudwatch.accounts[i].include || []).length ? "the alarms you chose" : "everything that matches the prefix"}.
      <b>Discover alarms</b> to pick them and set severities.</div>`;
  }
  const include = cfg.cloudwatch.accounts[i].include || [];
  const only = isOnly(i), f = discFilter[i] || {};
  const sevOf = (name) => (cfg.alarm_rules.find((r) => r.pattern === name) || {}).severity || "";
  const inAlarm = found.filter((a) => a.state === "ALARM").length;
  return `<div class="disc ${only ? "" : "all"}" data-i="${i}">
    <div class="tools">
      ${seg(`data-act="watchmode" data-i="${i}"`, [["all", "Everything matching"], ["only", "Only ticked"]], only ? "only" : "all")}
      <span class="grow"></span><span class="count" id="count-${i}">${countText(i)}</span>
    </div>
    <div class="tools">
      <input type="text" data-filter="${i}" value="${esc(f.q || "")}" placeholder="Filter by name">
      ${seg(`data-act="statefilter" data-i="${i}"`, [["", "All"], ["ALARM", `In alarm${inAlarm ? ` (${inAlarm})` : ""}`], ["OK", "OK"], ["INSUFFICIENT_DATA", "No data"]], f.state || "")}
      <span class="grow"></span>
      ${only ? `<button class="small" data-act="tick-shown" data-i="${i}">Tick shown</button>
        <button class="small" data-act="untick-all" data-i="${i}">Untick all</button>` : ""}
    </div>
    <div class="scroll" data-scroll="disc-${i}"><table>
      <colgroup>${only ? `<col class="w">` : ""}<col><col class="now"><col class="cond"><col class="sev"></colgroup><thead><tr>
      ${only ? `<th></th>` : ""}<th>Alarm</th><th>Now</th><th>Condition</th><th>Severity</th></tr></thead><tbody>
    ${found.map((a) => `<tr data-name="${esc(a.name.toLowerCase())}" data-state="${esc(a.state)}">
      ${only ? `<td><input type="checkbox" data-act="watch" data-i="${i}" data-name="${esc(a.name)}" ${include.includes(a.name) ? "checked" : ""}></td>` : ""}
      <td class="name" title="${esc(a.description || a.name)}">${esc(a.name).replace(/-/g, "-<wbr>")}</td>
      <td class="narrow"><span class="st ${esc(a.state)}">${esc(a.state === "INSUFFICIENT_DATA" ? "NO DATA" : a.state)}</span></td>
      <td class="cond" title="${esc(a.metric || "")}">${esc(a.metric || "")}</td>
      <td class="narrow"><select data-sev="${esc(a.name)}">${["", "critical", "warning", "info"].map((v) =>
        `<option value="${v}" ${sevOf(a.name) === v ? "selected" : ""}>${v || "default"}</option>`).join("")}</select></td></tr>`).join("")}
    </tbody></table></div>
    <small class="note">Severity saves an exact-name alarm rule, placed first so it wins.</small></div>`;
}

// ---- sections -------------------------------------------------------------
const VIEWS = {
  general: () => `
    <h1>General</h1>
    <p class="lede">How loud Houston is allowed to be. Secrets live in your login Keychain, never in <code>config.json</code>.</p>
    <div class="card">
      ${row("Sounds", toggle("sounds"), "Sosumi for critical, Tink for warnings.")}
      ${row("Radio silence", toggle("do_not_disturb"), "No peeks and no sounds. Islands still collect in the notch.")}
      ${row("Follow macOS Focus", toggle("follow_focus"), `Go quiet while Do Not Disturb or any Focus is on. ${focusNow ? `<b>${esc(focusNow)}.</b>` : ""}
        ${focusUnreadable ? `<br><button class="small" data-act="fda">Open Full Disk Access</button> then add Houston (or your terminal, for <code>cargo run</code>) and restart it.` : "Focus modes started by a schedule aren't seen, only ones switched on by hand or by a Shortcut."}`)}
      ${row("Drills", toggle("demo.autoplay"), "Play the demo a second after Houston starts, and show drill buttons when you hover the notch. Off keeps the hover view to real notifications; drills stay in the menu.")}
      ${row("AWS CLI path", optText("aws_cli", "auto: /opt/homebrew/bin/aws"), "Only if <code>aws</code> lives somewhere unusual.")}
    </div>
    <div class="card">${row("Config folder", `<button data-act="open-folder">Open in Finder</button>`)}</div>
    <div class="card"><div class="row"><label>Source status</label><div class="status">${esc(statuses.join("\n") || "No reports yet.")}</div></div></div>`,

  cloudwatch: () => `
    <h1>CloudWatch</h1>
    <p class="lede">Polls <code>describe-alarms</code> for each account. Needs only <code>cloudwatch:DescribeAlarms</code>.</p>
    <div class="card bar">
      <label class="inline">${toggle("cloudwatch.enabled")} <span>Enabled</span></label>
      <span class="grow"></span>
      <label class="inline"><span>Poll every</span><span class="numbox">${num("cloudwatch.interval_secs", 15, 3600)}</span><span>seconds</span></label>
    </div>
    ${(cfg.cloudwatch.accounts || []).map((a, i) => {
      const base = `cloudwatch.accounts.${i}`;
      return `<section class="card acct">
        <header class="cardhead">
          <div class="who"><b>${esc(accountName(a))}</b><div class="meta" id="meta-${i}">${accountMeta(i)}</div></div>
          <span class="grow"></span>
          <button class="small" data-act="test-cw" data-i="${i}">Test</button>
          <button class="small" data-act="discover" data-i="${i}">${discovered["cw" + i] ? "Rescan" : "Discover alarms"}</button>
          <button class="small danger" data-act="del" data-list="cloudwatch.accounts" data-i="${i}" title="Remove this account">Remove</button>
        </header>
        ${result("cw" + i)}
        <div class="grid3">
          ${fld("Label", optText(`${base}.label`, "e.g. apollo prod"))}
          ${fld("Region", optText(`${base}.region`, "eu-west-2"))}
          ${fld("Alarm prefix", optText(`${base}.alarm_prefix`, "e.g. apollo-prod-"))}
        </div>
        ${credsBlock(base)}
        ${watchPanel(i)}
      </section>`;
    }).join("")}
    <button class="add" data-act="add-account">+ Add an account</button>`,

  sqs: () => `
    <h1>SQS</h1>
    <p class="lede">Alarms within seconds: long-poll a queue fed by EventBridge or an SNS alarm topic. Bodies can be alarms, Security Hub findings or island JSON. Needs <code>sqs:ReceiveMessage</code> and <code>sqs:DeleteMessage</code>.</p>
    <div class="card">
      ${row("Enabled", toggle("sqs.enabled"))}
      ${row("Queue URL", text("sqs.queue_url", "https://sqs.eu-west-2.amazonaws.com/123456789012/houston"))}
      ${row("Region", optText("sqs.region", "eu-west-2"))}
      ${credsBlock("sqs")}
      ${row("Long-poll wait (s)", num("sqs.wait_secs", 1, 20))}
    </div>`,

  linear: () => `
    <h1>Linear</h1>
    <p class="lede">Your issues, and the urgent ones in teams you watch. <b>Take it</b> on a card assigns the issue to you.</p>
    <div class="card">
      ${row("Enabled", toggle("linear.enabled"))}
      ${row("API key", secretField("linear-api-key", secretFlags.linear_api_key, "lin_api_…"),
        "Linear › Settings › Security & access › Personal API keys. Read access, plus write for <b>Take it</b>. Falls back to <code>LINEAR_API_KEY</code>.")}
      ${row("", `<button data-act="test-linear">Test connection</button>${result("linear")}`)}
      ${row("My issues", toggle("linear.assigned_to_me"), "Peek when one is assigned to you, or changes state or priority.")}
      ${row("Watch teams", list("linear.teams", "APO, OPS"), "Team keys, comma separated. Test connection lists the ones your key can see.")}
      ${row("Watch up to", `<select data-path="linear.max_priority" data-kind="num">
        ${[[1, "Urgent"], [2, "High"], [3, "Medium"], [4, "Low"]].map(([v, l]) => `<option value="${v}" ${get("linear.max_priority") == v ? "selected" : ""}>${l}</option>`).join("")}
        </select>`, "Watched issues stay pinned until done, cancelled or downgraded.")}
      ${row("Poll every (s)", num("linear.interval_secs", 30, 3600))}
    </div>`,

  rules: () => `
    <h1>Alarm rules</h1>
    <p class="lede">Reshape alarms by name, for every source. <code>*</code> is a wildcard and the <b>first match wins</b>, so put specific patterns first. Unmatched alarms stay sticky criticals.</p>
    <div class="card">
      <table><thead><tr><th>Pattern</th><th>Severity</th><th>Sticky</th><th>Silent</th><th>Mute</th><th>Runbook URL</th><th></th></tr></thead><tbody>
      ${(cfg.alarm_rules || []).map((r, i) => `<tr class="${ruleHit === i ? "hit" : ""}">
        <td>${text(`alarm_rules.${i}.pattern`, "apollo-prod-eks-*")}</td>
        <td class="narrow"><select data-path="alarm_rules.${i}.severity" data-kind="opt">
          ${[["", "default"], ["critical", "critical"], ["warning", "warning"], ["info", "info"], ["success", "success"]].map(([v, l]) => `<option value="${v}" ${(r.severity || "") === v ? "selected" : ""}>${l}</option>`).join("")}
        </select></td>
        <td class="narrow"><select data-path="alarm_rules.${i}.sticky" data-kind="tri">
          ${[["", "auto"], ["true", "yes"], ["false", "no"]].map(([v, l]) => `<option value="${v}" ${String(r.sticky ?? "") === v ? "selected" : ""}>${l}</option>`).join("")}
        </select></td>
        <td class="narrow"><input type="checkbox" data-path="alarm_rules.${i}.silent" data-kind="bool" ${r.silent ? "checked" : ""}></td>
        <td class="narrow"><input type="checkbox" data-path="alarm_rules.${i}.mute" data-kind="bool" ${r.mute ? "checked" : ""}></td>
        <td>${optText(`alarm_rules.${i}.runbook_url`, "https://…")}</td>
        <td class="narrow"><button class="small" data-act="up" data-i="${i}" title="Move up">↑</button>
          <button class="small danger" data-act="del" data-list="alarm_rules" data-i="${i}">✕</button></td></tr>`).join("")}
      </tbody></table>
      <p><button data-act="add-rule">+ Add rule</button></p>
    </div>
    <div class="card"><div class="tryrule">
      <input type="text" id="tryname" placeholder="Try an alarm name, e.g. apollo-prod-eks-pods-pending">
      <button data-act="try-rule">Which rule?</button></div>${result("rule")}</div>`,

  iot: () => `
    <h1>IoT Core <span class="pill">stub</span></h1>
    <p class="lede">Live call events over MQTT, one timer island per call. The mapping is ready; the MQTT client isn't written yet, so enabling this only reports "not implemented yet".</p>
    <div class="card">
      ${row("Enabled", toggle("iot.enabled"))}
      ${row("Endpoint", text("iot.endpoint", "abc123-ats.iot.eu-west-2.amazonaws.com"), "Apollo keeps it in SSM at <code>/apollo/&lt;env&gt;/iot/data-endpoint</code>.")}
      ${row("Profile", optText("iot.profile", "default"))}
      ${row("Region", optText("iot.region", "eu-west-2"))}
      ${row("Topic filters", lines("iot.topics", "apollo/prod/t/+/call/+/events"), "One per line.")}
    </div>`,

  api: () => `
    <h1>Local API</h1>
    <p class="lede">Radio Houston from scripts and CI on <code>127.0.0.1</code>. Every call but <code>/v1/health</code> needs the bearer token.</p>
    <div class="card">
      ${row("Enabled", toggle("api.enabled"))}
      ${row("Port", num("api.port", 1024, 65535))}
      ${row("Token", `<div class="secret"><span class="pill ${secretFlags.api_token ? "ok" : ""}">${secretFlags.api_token ? "🔒 in Keychain" : "not set"}</span>
        <button data-act="copy-token">Copy</button><button data-act="regen-token" class="danger">Regenerate</button></div>${result("token")}`,
        "Regenerating takes effect at once; anything using the old token stops working. <code>scripts/houston</code> reads it from the Keychain.")}
    </div>`,
};

// Re-draw the page, keeping scroll positions, filters and the focused field,
// so editing never makes the page jump. `fresh` starts at the top (new tab).
function render(fresh = false) {
  const main = $("main");
  const keep = { top: main.scrollTop, scrolls: {} };
  main.querySelectorAll("[data-scroll]").forEach((el) => { keep.scrolls[el.dataset.scroll] = el.scrollTop; });
  const a = document.activeElement;
  if (a && main.contains(a)) {
    for (const k of ["path", "filter", "secret"]) {
      if (a.dataset[k] !== undefined) {
        keep.focus = { sel: `[data-${k}="${CSS.escape(a.dataset[k])}"]`, start: a.selectionStart, end: a.selectionEnd };
        break;
      }
    }
  }
  $("nav").innerHTML = `<div class="brand">🛰️ Houston</div><div class="tag">we have a notch</div>` +
    SECTIONS.map(([id, label]) => {
      const live = { cloudwatch: cfg.cloudwatch.enabled, sqs: cfg.sqs.enabled, linear: cfg.linear.enabled,
        iot: cfg.iot.enabled, api: cfg.api.enabled }[id];
      return `<a data-tab="${id}" class="${tab === id ? "on" : ""}">${label}${live === undefined ? "" : `<span class="dot ${live ? "live" : ""}"></span>`}</a>`;
    }).join("");
  main.innerHTML = VIEWS[tab]() +
    `<datalist id="aws-profiles">${profiles.map((p) => `<option value="${esc(p.name)}">${p.sso ? "SSO" : ""}</option>`).join("")}</datalist>`;
  Object.keys(discFilter).forEach(applyFilter);
  if (fresh) return;
  main.scrollTop = keep.top;
  main.querySelectorAll("[data-scroll]").forEach((el) => { el.scrollTop = keep.scrolls[el.dataset.scroll] || 0; });
  const f = keep.focus && main.querySelector(keep.focus.sel);
  if (f) {
    f.focus({ preventScroll: true });
    try { f.setSelectionRange(keep.focus.start, keep.focus.end); } catch (_) { /* not a text field */ }
  }
}

// ---- events -----------------------------------------------------------------
document.addEventListener("input", (e) => {
  const el = e.target;
  if (el.dataset.secret) { secretEdits[el.dataset.secret] = el.value; if (!el.value) delete secretEdits[el.dataset.secret]; dirty(); return; }
  const path = el.dataset.path; if (!path) return;
  const kind = el.dataset.kind;
  const v = kind === "bool" ? el.checked
    : kind === "num" ? (el.value === "" ? 0 : Number(el.value))
    : kind === "opt" ? (el.value.trim() === "" ? null : el.value.trim())
    : kind === "tri" ? (el.value === "" ? null : el.value === "true")
    : kind === "list" ? el.value.split(",").map((s) => s.trim()).filter(Boolean)
    : kind === "lines" ? el.value.split("\n").map((s) => s.trim()).filter(Boolean)
    : el.value;
  set(path, v);
  if (kind === "bool" && /\.enabled$/.test(path)) render();
});
document.addEventListener("change", (e) => {
  const el = e.target;
  if (el.dataset.sev !== undefined) {
    const name = el.dataset.sev, at = cfg.alarm_rules.findIndex((r) => r.pattern === name);
    if (!el.value) { if (at >= 0) cfg.alarm_rules.splice(at, 1); }
    else if (at >= 0) cfg.alarm_rules[at].severity = el.value;
    // Exact names go first: the first matching rule wins.
    else cfg.alarm_rules.unshift({ pattern: name, severity: el.value, sticky: null, silent: false, mute: false, runbook_url: null });
    dirty(); return;
  }
  if (el.hasAttribute("data-rerender")) { render(); return; }
  if (el.tagName === "SELECT") el.dispatchEvent(new Event("input", { bubbles: true }));
});
document.addEventListener("input", (e) => {
  const i = e.target.dataset.filter; if (i === undefined) return;
  (discFilter[i] ??= {}).q = e.target.value.trim();
  applyFilter(i);
});

document.addEventListener("click", (e) => {
  const t = e.target.closest("[data-tab]");
  if (t) { tab = t.dataset.tab; ruleHit = null; render(true); return; }
  const b = e.target.closest("[data-act]"); if (!b) return;
  const i = Number(b.dataset.i);
  switch (b.dataset.act) {
    case "add-account": cfg.cloudwatch.accounts.push({ auth: { type: "profile" }, profile: null, region: "eu-west-2", label: null, alarm_prefix: null, include: [] }); dirty(); render(); break;
    case "sso": { const base = b.dataset.base, id = "sso:" + base;
      results[id] = { cls: "busy", text: "Opening your browser. Approve the sign-in there…" }; render();
      post({ type: "sso_login", id, profile: get(base + ".profile") || "default", aws_cli: cfg.aws_cli }); break; }
    case "discover": results["cw" + i] = { cls: "busy", text: "Scanning for alarms…" }; render();
      post({ type: "discover_alarms", id: "cw" + i, account: cfg.cloudwatch.accounts[i], aws_cli: cfg.aws_cli, secret: secretFor(`cloudwatch.accounts.${i}`) }); break;
    case "auth": {
      const base = b.dataset.base, now = get(base + ".auth")?.type || "profile";
      if (b.dataset.value !== now) {
        set(base + ".auth", b.dataset.value === "access_key" ? { type: "access_key", access_key_id: "" } : { type: "profile" });
        render();
      }
      break; }
    // Ticks change the data and the counts in place; no re-render, no jump.
    case "watch": { const inc = (cfg.cloudwatch.accounts[i].include ??= []), n = b.dataset.name;
      const at = inc.indexOf(n); if (b.checked && at < 0) inc.push(n); if (!b.checked && at >= 0) inc.splice(at, 1);
      dirty(); refreshCounts(i); break; }
    case "tick-shown": { const inc = (cfg.cloudwatch.accounts[i].include ??= []);
      b.closest(".disc").querySelectorAll("tbody tr:not([hidden]) input[data-act=watch]").forEach((box) => {
        box.checked = true; if (!inc.includes(box.dataset.name)) inc.push(box.dataset.name); });
      dirty(); refreshCounts(i); break; }
    case "untick-all":
      cfg.cloudwatch.accounts[i].include = [];
      b.closest(".disc").querySelectorAll("input[data-act=watch]").forEach((box) => { box.checked = false; });
      dirty(); refreshCounts(i); break;
    case "watchmode": {
      const acct = cfg.cloudwatch.accounts[i], only = b.dataset.value === "only";
      if (only === isOnly(i)) break;
      if (only) { acct.include = stashed[i] || acct.include || []; delete stashed[i]; }
      else { if ((acct.include || []).length) stashed[i] = acct.include; acct.include = []; }
      onlyTicked[i] = only; dirty(); render(); break; }
    case "statefilter": (discFilter[i] ??= {}).state = b.dataset.value; render(); break;
    case "add-rule": cfg.alarm_rules.push({ pattern: "", severity: null, sticky: null, silent: false, mute: false, runbook_url: null }); dirty(); render(); break;
    case "del": get(b.dataset.list).splice(i, 1); dirty(); render(); break;
    case "up": if (i > 0) { const r = cfg.alarm_rules; [r[i - 1], r[i]] = [r[i], r[i - 1]]; dirty(); render(); } break;
    case "test-cw": results["cw" + i] = { cls: "busy", text: "Calling Houston… (describe-alarms)" }; render();
      post({ type: "test_cloudwatch", id: "cw" + i, account: cfg.cloudwatch.accounts[i], aws_cli: cfg.aws_cli, secret: secretFor(`cloudwatch.accounts.${i}`) }); break;
    case "test-linear": results.linear = { cls: "busy", text: "Hailing Linear…" }; render();
      post({ type: "test_linear", id: "linear", linear: cfg.linear, api_key: secretEdits["linear-api-key"] || null }); break;
    case "try-rule": post({ type: "test_rule", name: $("tryname").value, rules: cfg.alarm_rules }); break;
    case "remove-secret": secretEdits[b.dataset.account] = ""; dirty(); render(); break;
    case "copy-token": post({ type: "copy_token" }); break;
    case "regen-token": post({ type: "regenerate_token" }); break;
    case "open-folder": post({ type: "open_config_folder" }); break;
    case "fda": post({ type: "open_full_disk_access" }); break;
  }
});
$("save").onclick = () => { $("save").disabled = true; msg("Transmitting…", ""); post({ type: "save", config: cfg, secrets: secretEdits }); };
$("revert").onclick = () => { cfg = structuredClone(saved); secretEdits = {}; results = {}; ruleHit = null; dirty(); render(); msg("Reverted.", ""); };
document.addEventListener("keydown", (e) => {
  if (e.metaKey && e.key === "s") { e.preventDefault(); if (!$("save").disabled) $("save").click(); }
  if (e.metaKey && e.key === "w") { e.preventDefault(); post({ type: "close" }); }
});

window.__prefs = (m) => {
  switch (m.type) {
    case "state":
      statuses = m.statuses; secretFlags = m.secrets; profiles = m.profiles || [];
      focusNow = m.focus || ""; focusUnreadable = !!m.focus_unreadable;
      if (!saved || m.reset) { saved = m.config; cfg = structuredClone(saved); secretEdits = {}; dirty(); }
      render(); break;
    case "status": statuses = m.statuses; if (tab === "general") render(); break;
    case "saved":
      if (m.error) { msg(m.error, "err"); $("save").disabled = false; break; }
      msg(m.restarted.length ? `Saved. Restarted ${m.restarted.join(", ")}. Roger that.` : "Saved. Roger that.", "ok"); break;
    case "test": results[m.id] = { cls: m.ok ? "ok" : "err", text: m.text }; render(); break;
    case "discovered":
      if (m.error) { results[m.id] = { cls: "err", text: m.error }; delete discovered[m.id]; }
      else { discovered[m.id] = m.alarms; results[m.id] = { cls: "ok", text: `Found ${m.alarms.length} alarm(s), ${m.alarms.filter((a) => a.state === "ALARM").length} in ALARM now.` }; }
      render(); break;
    case "rule":
      ruleHit = m.index;
      results.rule = m.index === null ? { cls: "", text: "No rule matches: it stays a sticky critical." }
        : { cls: "ok", text: `Rule ${m.index + 1} (${cfg.alarm_rules[m.index].pattern}) wins.` };
      render(); break;
    case "token": results.token = { cls: m.ok ? "ok" : "err", text: m.text }; secretFlags.api_token = true; render(); break;
  }
};
post({ type: "ready" });
</script></body></html>
"##;
