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
  .pill { display: inline-block; font-size: 11px; padding: 2px 8px; border-radius: 10px; background: var(--chip); color: var(--muted); }
  .pill.ok { color: var(--ok); }
  .status { font-family: ui-monospace, Menlo, monospace; font-size: 11.5px; color: var(--muted); white-space: pre-wrap; }
  footer { grid-column: 2; display: flex; gap: 10px; align-items: center; padding: 12px 28px;
    border-top: 1px solid var(--line); background: var(--panel); }
  footer .msg { flex: 1; color: var(--muted); font-size: 12px; }
  footer .msg.err { color: var(--danger); } footer .msg.ok { color: var(--ok); }
  .tryrule { display: flex; gap: 8px; margin: 12px 0 4px; }
  tr.hit td { background: rgba(10,132,255,.12); }
  .cardhead { display: flex; align-items: center; gap: 8px; padding: 12px 0 8px; border-bottom: 1px solid var(--line); }
  .cardhead b { font-size: 14px; }
  .grow { flex: 1; }
  .disc { margin: 8px 0 12px; }
  .disc .tools { display: flex; gap: 8px; align-items: center; margin-bottom: 6px; }
  .disc .scroll { max-height: 280px; overflow: auto; border: 1px solid var(--line); border-radius: 8px; }
  .disc th { position: sticky; top: 0; background: var(--panel); }
  .disc td { border-top: 1px solid var(--line); font-size: 12px; }
  .disc td.name { font-family: ui-monospace, Menlo, monospace; font-size: 11.5px; word-break: break-all; }
  .disc select { width: auto; padding: 2px 4px; font-size: 12px; }
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

// AWS credentials for an account or the SQS queue at `base`.
function credsBlock(base) {
  const auth = get(base + ".auth") || { type: "profile" };
  const isKey = auth.type === "access_key";
  const akid = isKey ? auth.access_key_id || "" : "";
  const prof = get(base + ".profile") || "";
  const known = profiles.find((p) => p.name === (prof || "default"));
  const picker = `<select data-auth="${base}">
      <option value="profile" ${isKey ? "" : "selected"}>CLI profile or SSO</option>
      <option value="access_key" ${isKey ? "selected" : ""}>IAM access key</option></select>`;
  if (isKey) {
    return row("Credentials", picker) +
      row("Access key ID", `<input type="text" data-path="${base}.auth.access_key_id" data-kind="text" data-rerender value="${esc(akid)}" placeholder="AKIA…" autocomplete="off">`) +
      row("Secret access key", akid ? secretField("aws-secret:" + akid, secretFlags.aws?.[akid], "wJalrXUtnFEMI…") : `<span class="pill">enter the key ID first</span>`,
        "Kept in the Keychain. Best as a user that can only read alarms (and the queue).");
  }
  const id = "sso:" + base;
  return row("Credentials", picker) +
    row("Profile", `<div class="secret"><input type="text" list="aws-profiles" data-path="${base}.profile" data-kind="opt" value="${esc(prof)}" placeholder="default">
        <button class="small" data-act="sso" data-base="${base}">Sign in with SSO</button></div>${result(id)}`,
      known ? `${known.sso ? "SSO profile" : "Profile"} from ~/.aws/config${known.region ? `, region ${esc(known.region)}` : ""}.`
        : "Any profile in ~/.aws/config. Sign in with SSO runs <code>aws sso login --profile …</code> and opens your browser.");
}

function secretFor(base) {
  const auth = get(base + ".auth");
  return auth?.type === "access_key" ? secretEdits["aws-secret:" + auth.access_key_id] || null : null;
}

function discoverTable(i) {
  const found = discovered["cw" + i];
  if (!found) return "";
  const acct = cfg.cloudwatch.accounts[i];
  const include = acct.include || [];
  const sevOf = (name) => (cfg.alarm_rules.find((r) => r.pattern === name) || {}).severity || "";
  return `<div class="disc">
    <div class="tools"><input type="text" data-filter="${i}" placeholder="Filter ${found.length} alarms">
      <button class="small" data-act="watch-shown" data-i="${i}">Tick shown</button>
      <button class="small" data-act="watch-all" data-i="${i}">Watch everything</button></div>
    <div class="hint" style="margin:0 0 6px">${include.length ? `Watching <b>${include.length}</b> ticked alarm(s).`
      : "Nothing ticked, so every alarm matching the prefix is watched. Tick some to watch only those."}
      Severity saves an exact-name alarm rule.</div>
    <div class="scroll"><table><thead><tr><th>Watch</th><th>Alarm</th><th>Now</th><th>Condition</th><th>Severity</th></tr></thead><tbody>
    ${found.map((a) => `<tr data-name="${esc(a.name.toLowerCase())}">
      <td class="narrow"><input type="checkbox" data-act="watch" data-i="${i}" data-name="${esc(a.name)}" ${include.includes(a.name) ? "checked" : ""}></td>
      <td class="name" title="${esc(a.description || "")}">${esc(a.name)}</td>
      <td class="narrow"><span class="st ${esc(a.state)}">${esc(a.state === "INSUFFICIENT_DATA" ? "NO DATA" : a.state)}</span></td>
      <td>${esc(a.metric || "")}</td>
      <td class="narrow"><select data-sev="${esc(a.name)}">${["", "critical", "warning", "info"].map((v) =>
        `<option value="${v}" ${sevOf(a.name) === v ? "selected" : ""}>${v || "default"}</option>`).join("")}</select></td></tr>`).join("")}
    </tbody></table></div></div>`;
}

// ---- sections -------------------------------------------------------------
const VIEWS = {
  general: () => `
    <h1>General</h1>
    <p class="lede">How loud Houston is allowed to be. Secrets live in your login Keychain, never in <code>config.json</code>.</p>
    <div class="card">
      ${row("Sounds", toggle("sounds"), "Sosumi for critical, Tink for warnings.")}
      ${row("Radio silence", toggle("do_not_disturb"), "No peeks and no sounds. Islands still collect in the notch.")}
      ${row("Drill on launch", toggle("demo.autoplay"), "Play the demo a second after Houston starts. Takes effect next launch.")}
      ${row("AWS CLI path", optText("aws_cli", "auto: /opt/homebrew/bin/aws"), "Only if <code>aws</code> lives somewhere unusual.")}
    </div>
    <div class="card">${row("Config folder", `<button data-act="open-folder">Open in Finder</button>`)}</div>
    <div class="card"><div class="row"><label>Source status</label><div class="status">${esc(statuses.join("\n") || "No reports yet.")}</div></div></div>`,

  cloudwatch: () => `
    <h1>CloudWatch</h1>
    <p class="lede">Poll <code>describe-alarms</code> for each profile and region. Uses your AWS CLI profiles, so SSO works; needs only <code>cloudwatch:DescribeAlarms</code>.</p>
    <div class="card">
      ${row("Enabled", toggle("cloudwatch.enabled"))}
      ${row("Poll every (s)", num("cloudwatch.interval_secs", 15, 3600), "15 s minimum.")}
    </div>
    ${(cfg.cloudwatch.accounts || []).map((a, i) => {
      const base = `cloudwatch.accounts.${i}`;
      const name = a.label || (a.auth?.type === "access_key" ? "Access key account" : a.profile) || "New account";
      const watching = (a.include || []).length ? `${a.include.length} alarm(s)` : (a.alarm_prefix ? `${esc(a.alarm_prefix)}*` : "all alarms");
      return `<div class="card">
        <div class="cardhead"><b>${esc(name)}</b><span class="pill">${watching}</span><span class="grow"></span>
          <button class="small" data-act="test-cw" data-i="${i}">Test</button>
          <button class="small" data-act="discover" data-i="${i}">Discover alarms</button>
          <button class="small danger" data-act="del" data-list="cloudwatch.accounts" data-i="${i}">Remove</button></div>
        ${result("cw" + i)}
        ${row("Label", optText(`${base}.label`, "shown on cards, e.g. apollo prod"))}
        ${row("Region", optText(`${base}.region`, "eu-west-2"))}
        ${row("Alarm prefix", optText(`${base}.alarm_prefix`, "apollo-prod-"), "Only alarms whose names start with this. Discover alarms uses it too.")}
        ${credsBlock(base)}
        ${discoverTable(i)}</div>`;
    }).join("")}
    <p><button data-act="add-account">+ Add account</button></p>`,

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

function render() {
  $("nav").innerHTML = `<div class="brand">🛰️ Houston</div><div class="tag">we have a notch</div>` +
    SECTIONS.map(([id, label]) => {
      const live = { cloudwatch: cfg.cloudwatch.enabled, sqs: cfg.sqs.enabled, linear: cfg.linear.enabled,
        iot: cfg.iot.enabled, api: cfg.api.enabled }[id];
      return `<a data-tab="${id}" class="${tab === id ? "on" : ""}">${label}${live === undefined ? "" : `<span class="dot ${live ? "live" : ""}"></span>`}</a>`;
    }).join("");
  $("main").innerHTML = VIEWS[tab]() +
    `<datalist id="aws-profiles">${profiles.map((p) => `<option value="${esc(p.name)}">${p.sso ? "SSO" : ""}</option>`).join("")}</datalist>`;
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
  if (el.dataset.auth) {
    set(el.dataset.auth + ".auth", el.value === "access_key" ? { type: "access_key", access_key_id: "" } : { type: "profile" });
    render(); return;
  }
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
  const q = e.target.value.trim().toLowerCase();
  e.target.closest(".disc").querySelectorAll("tbody tr").forEach((tr) => { tr.style.display = tr.dataset.name.includes(q) ? "" : "none"; });
});

document.addEventListener("click", (e) => {
  const t = e.target.closest("[data-tab]");
  if (t) { tab = t.dataset.tab; ruleHit = null; render(); return; }
  const b = e.target.closest("[data-act]"); if (!b) return;
  const i = Number(b.dataset.i);
  switch (b.dataset.act) {
    case "add-account": cfg.cloudwatch.accounts.push({ auth: { type: "profile" }, profile: null, region: "eu-west-2", label: null, alarm_prefix: null, include: [] }); dirty(); render(); break;
    case "sso": { const base = b.dataset.base, id = "sso:" + base;
      results[id] = { cls: "busy", text: "Opening your browser. Approve the sign-in there…" }; render();
      post({ type: "sso_login", id, profile: get(base + ".profile") || "default", aws_cli: cfg.aws_cli }); break; }
    case "discover": results["cw" + i] = { cls: "busy", text: "Scanning for alarms…" }; render();
      post({ type: "discover_alarms", id: "cw" + i, account: cfg.cloudwatch.accounts[i], aws_cli: cfg.aws_cli, secret: secretFor(`cloudwatch.accounts.${i}`) }); break;
    case "watch": { const inc = (cfg.cloudwatch.accounts[i].include ??= []), n = b.dataset.name;
      const at = inc.indexOf(n); if (b.checked && at < 0) inc.push(n); if (!b.checked && at >= 0) inc.splice(at, 1);
      dirty(); render(); break; }
    case "watch-shown": { const inc = (cfg.cloudwatch.accounts[i].include ??= []);
      b.closest(".disc").querySelectorAll("tbody tr").forEach((tr) => { if (tr.style.display !== "none") {
        const n = tr.querySelector("[data-name]").dataset.name; if (!inc.includes(n)) inc.push(n); } });
      dirty(); render(); break; }
    case "watch-all": cfg.cloudwatch.accounts[i].include = []; dirty(); render(); break;
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
