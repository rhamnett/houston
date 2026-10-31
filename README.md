# Houston 🛰️

> *Houston, we have a notch.*

Mission control that lives in your MacBook notch. When something breaks in
prod, Houston peeks out to tell you. When nothing's broken, it keeps quiet.

It's a Dynamic-Island style notification area for macOS, written in Rust with
tao, wry and tray-icon. It shows CloudWatch alarms, Security Hub findings,
Linear issues and anything else that can post JSON, and new sources plug in behind one small
trait.

![A CloudWatch alarm peeking out of the notch](docs/peek.png)

## Quick start

You need macOS 12 or later and a Rust toolchain (1.88 or later, for let
chains). The AWS CLI v2 is needed only for the AWS sources.

```bash
git clone https://github.com/rhamnett/houston.git
cd houston
cargo run --release
```

A drill (the demo) plays a second after launch. Hover the notch to expand it.
Then send your first transmission from another terminal:

```bash
scripts/houston "Houston, we have a problem" "it's only a test" warning
```

To point it at real alarms, open **Mission settings…** from the menu bar
icon, turn on a source and save. Only the sources whose settings changed are
restarted. See [Sources](#sources-the-plug-in-seam).

## Mission settings (preferences)

Sections: General, CloudWatch, SQS, Linear, Alarm rules, IoT Core and Local
API.

- **Test** buttons run one real call in the background: a `describe-alarms`
  per CloudWatch account, or a Linear sign-in check that also lists the team
  keys your key can see.
- **Which rule?** takes an alarm name and highlights the rule that would win.
- **Save** (⌘S) checks the settings, then restarts only the affected
  sources. **Revert** throws away unsaved edits. ⌘V and the other editing
  shortcuts work in every field.

Everything is also in `~/Library/Application Support/houston/config.json`,
apart from secrets. Hand edits there are picked up at the next launch.

## Secrets live in the Keychain

The local API token, the Linear API key and the GitHub token (for update
checks) are stored in your login Keychain under the service `local.houston`,
never in `config.json`. Mission settings can replace or remove a secret, but
never reads one back: it only shows
"🔒 in Keychain". A secret found in the config file (from an older version,
or a hand edit) is moved to the Keychain and blanked on the next launch.

```bash
security find-generic-password -s local.houston -a api-token -w   # what scripts/houston does
```

The Keychain ties each item to the app that created it. After a rebuild,
macOS asks once whether the new binary may read it: choose **Always Allow**.
Builds from `cargo run` prompt after every rebuild. An installed
`Houston.app` prompts only when you reinstall it.

For the same reason, add a secret through Houston rather than with
`security add-generic-password`. An item the `security` tool creates belongs
to that tool, so Houston must ask before it can read it, and that dialog can
open behind other windows and look like a source that never polls. From a
terminal, `scripts/houston` goes through the local API, so Houston creates
the item itself. Add the account first (Mission settings, or `PATCH
/v1/settings`); a secret for a key no account uses is refused.

```bash
pbpaste | scripts/houston --secret aws-secret:AKIA…   # the value comes from stdin
scripts/houston --forget-secret aws-secret:AKIA…
```

If you do use `security`, pass `-T` with the path of the Houston binary so the
item trusts it from the start.

## What it does

- A new notification **peeks**: it grows out of the notch as a card for 4–8 s
  (longer for higher severity), then collapses.
- **Compact**: the island shows the most urgent notification's icon on the
  left of the notch and a short value on the right (`ALARM`, `62%`, `87%`,
  `05:12`), plus `+N` for the rest. Critical alarms make it glow red.
- **Hover** the notch to expand every notification, with buttons. Move away
  and it collapses. The window is click-through at all other times.
- On Macs without a notch it draws a black pill in the same place.

The menu bar icon shows how many warnings and criticals are active. Its menu
has **Run a drill** (the demos), **Radio silence** (do not disturb: no peeks,
no sounds), **Clear the board**, **Copy API token**, **Open config folder**
and **Check for updates…**.

With nothing on the board, the expanded island says **All systems nominal**
and, under it, **📡 N watched**: open it for a compact list of every alarm
being checked, firing first, with a state dot each. Click one to open it in
the CloudWatch console.

### macOS Focus

With **Follow macOS Focus** on (the default), Houston goes quiet whenever Do
Not Disturb or any other Focus is on: no peeks and no sounds, the same as
Radio silence. The island's ☾ button shows the Focus by name, and the menu
says which Focus is keeping it quiet.

Apple's Focus Status API needs an entitlement that ad-hoc-signed apps can't
have, so Houston reads the Do Not Disturb store in
`~/Library/DoNotDisturb/DB` every 2 s instead. Two caveats:

- If macOS protects that folder, the menu shows "Can't read Focus: needs Full
  Disk Access". Mission settings > General has a button to open the right
  pane. Add `Houston.app`, or your terminal if you use `cargo run`.
- Only Focus modes switched on by hand (from Control Centre, a Shortcut or a
  synced device) are in the store. Scheduled ones aren't seen.

### Mission vocabulary

| You'll see             | It means                                    |
|------------------------|---------------------------------------------|
| All systems nominal    | nothing is active                           |
| Lost signal from …     | AWS credentials expired or the call failed  |
| Radio silence          | do not disturb                              |
| Drill                  | the demo                                    |
| Capcom                 | the sender of the demo on-call message      |
| Splashdown             | quit                                        |

The alarm cards themselves stay plain: an alarm should never need decoding.

## Notification types

Every source produces the same `Island` (`src/island.rs`). `kind` selects the
body:

| kind       | shows                                              | compact      |
|------------|----------------------------------------------------|--------------|
| `alert`    | state chips `OK → ALARM`, reason, fields           | `ALARM`      |
| `progress` | animated bar and percent (`value` 0–1)             | ring + `42%` |
| `metric`   | big value, sparkline of `series`, dashed threshold | `87%`        |
| `message`  | sender and body                                    | `compact`    |
| `timer`    | live clock since `started_at` (unix ms)            | `05:12`      |

Common fields: `key` (posting the same key again updates in place), `title`,
`subtitle`, `body`, `icon` (an emoji), `severity` (`success` `info` `warning`
`critical`), `compact`, `fields` (label/value rows), `actions` (up to 3
buttons), `sticky` (stays until dismissed), `ttl_secs` (default 15),
`revision` and `silent`.

- **Actions**: `dismiss` and `snooze` (with `snooze_mins`) are handled by the
  app, and an action with a `url` opens it. Any other id goes back to the
  source that posted the island.
- **`revision`**: a dismissed island stays hidden while its source keeps
  re-posting the same revision. Poll sources rely on this.
- **`silent`**: updates in place without a peek or a sound, for progress ticks
  and live metrics.

## Drills (the demo)

With **Drills** on (Mission settings > General, or `demo.autoplay`), the
full drill plays a second after launch. You can replay it from **Run a drill**
in the menu bar menu, or from the empty expanded island. With Drills off,
both go away and the hover view only shows real notifications. The drill
runs in this order:

1. A CloudWatch alarm, built through the same mapper the real sources use.
   **Acknowledge** is a round trip back to the source.
2. A deploy with progress, ending in a success.
3. A live RDS CPU metric with a sparkline and threshold.
4. An on-call message from Capcom.
5. A SEV-2 incident timer with Acknowledge and Resolve.
6. The alarm going back to OK.

## Sources (the plug-in seam)

`src/source.rs`:

```rust
pub trait Source: Send + 'static {
    fn id(&self) -> String;
    fn run(self: Box<Self>, sink: Sink, inbox: Receiver<Inbound>);
}
```

Each source runs on its own thread. It calls `sink.upsert(island)`,
`sink.remove(key)` and `sink.status(text)`, and receives `Inbound::Action`
(button presses) and `Inbound::Trigger` on its inbox. To add one, write a
module in `src/sources/` and add it to `sources::from_config`. The UI, store,
peek logic and sounds need no changes.

Built in:

| source       | what it does                                                       | default |
|--------------|--------------------------------------------------------------------|---------|
| `demo`       | sample islands of every kind                                       | on      |
| `api`        | HTTP on `127.0.0.1:7878`                                           | on      |
| `cloudwatch` | polls `describe-alarms --state-value ALARM` per profile and region | off     |
| `sqs`        | long-polls an SQS queue fed by EventBridge or SNS                  | off     |
| `linear`     | your Linear issues, and urgent ones in the teams you watch         | off     |
| `iot`        | **stub**: AWS IoT Core MQTT subscriber (live call events)          | off     |

The AWS sources shell out to the `aws` CLI, signed with an IAM access key
from the Keychain or a CLI profile. When credentials stop working, a "Lost
signal" island says why. `src/aws.rs` maps three payload shapes to the
same alarm island: `describe-alarms` rows, EventBridge
`CloudWatch Alarm State Change` events, and SNS alarm notifications (raw or in
an SNS envelope). It also maps Security Hub findings (`Security Hub Findings -
Imported`, raw or in an SNS envelope) to one island per finding.

### Local API: radio Houston

```bash
scripts/houston "Build finished" "web · main · 4m12s" success
scripts/houston --json '{"key":"deploy","title":"Deploying","kind":{"type":"progress","value":0.4},"silent":true}'
scripts/houston --remove deploy
```

The endpoints are below. Every one except health needs
`Authorization: Bearer <token>`. The token is generated on first run and kept
in the Keychain. **Copy API token** in the menu, or the Local API section of
Mission settings, puts it on the clipboard, and Regenerate replaces it.

- `GET /v1/health`
- `POST /v1/islands`: an island as JSON
- `DELETE /v1/islands/{key}`
- `POST /v1/aws/alarm`: an EventBridge or SNS CloudWatch alarm payload, or a
  Security Hub findings event

Settings and remote control, the same things Mission settings and the menu do:

- `GET /v1/settings` (secrets blanked, plus which are set); `PATCH /v1/settings`
  with a [JSON merge patch](https://www.rfc-editor.org/rfc/rfc7396): send only
  what changes, `null` resets a key, arrays are replaced whole. It is
  validated, saved and applied like a Save, restarting only the affected
  sources.
- `PUT /v1/secrets/{name}` with `{"value": "…"}`, `DELETE /v1/secrets/{name}`:
  `linear-api-key`, `github-token`, or `aws-secret:<access key id>`. Secrets go to the Keychain
  and are never returned. The API token itself can only be regenerated in
  Mission settings.
- `GET /v1/status`: the board, radio silence, Focus, sources and their status
- `PUT /v1/radio-silence` with `{"on": true}`; `POST /v1/board/clear`,
  `/v1/board/dismiss` `{"id"}`, `/v1/board/press` `{"id", "action"}`
- `POST /v1/drills/{trigger}`, `/v1/cloudwatch/accounts/{i}/test`,
  `/v1/cloudwatch/accounts/{i}/discover`, `/v1/linear/test`,
  `/v1/alarm-rules/test` `{"name"}`; `GET /v1/aws/profiles`
- `POST /v1/commands/{tool}`: any MCP tool below, with its arguments as the body

```bash
TOKEN=$(security find-generic-password -s local.houston -a api-token -w)
curl -X PATCH -H "Authorization: Bearer $TOKEN" -d '{"sounds": false}' http://127.0.0.1:7878/v1/settings
```

Requests from a web page (an `Origin` other than localhost) are refused, so a
site can't reach the API through your browser.

### MCP: let an agent fly it

Houston is also an [MCP](https://modelcontextprotocol.io) server, so Claude
or another agent can read and change every setting, store secrets, run checks
and drills, and post or clear islands. Tools: `get_settings`,
`update_settings`, `set_secret`, `list_aws_profiles`, `get_status`,
`set_radio_silence`, `clear_board`, `dismiss_island`, `press_button`,
`run_drill`, `test_cloudwatch`, `discover_alarms`, `test_linear`,
`test_alarm_rule`, `check_for_updates`, `post_island`, `remove_island`.

Over HTTP, at `/mcp` on the local API with the same token (Claude Code):

```bash
claude mcp add --transport http houston http://127.0.0.1:7878/mcp \
  --header "Authorization: Bearer $(security find-generic-password -s local.houston -a api-token -w)"
```

Or over stdio, for clients that launch a command (Claude Desktop's
`claude_desktop_config.json`). `houston mcp` reads the token from the Keychain
and relays to the running app:

```json
{ "mcpServers": { "houston": {
  "command": "/Applications/Houston.app/Contents/MacOS/houston", "args": ["mcp"] } } }
```

Both need Houston running with the local API on. Turning the API off, or
changing its port, through MCP cuts the agent off too.

### CloudWatch, polling

In `~/Library/Application Support/houston/config.json`:

```json
"cloudwatch": {
  "enabled": true,
  "interval_secs": 60,
  "accounts": [
    { "profile": "prod", "region": "eu-west-2", "label": "prod" },
    { "profile": "staging", "region": "eu-west-2", "alarm_prefix": "api-" }
  ]
}
```

Each account signs its calls in one of two ways, set under **Credentials**
in Mission settings:

- **IAM access key** (the default, and recommended): a long-lived key for a
  read-only user. The key ID goes in the config file and the secret key goes
  in the Keychain (as `aws-secret:<key id>`). Both reach the AWS CLI through
  environment variables, never as command-line arguments. Removing the
  account removes its secret too.
- **CLI profile**: any profile in `~/.aws/config`, offered as you type, with
  keys in `~/.aws/credentials` or a `credential_process`. SSO profiles work,
  but their sessions expire within hours and polling stops until you run
  `aws sso login` again, so Mission settings warns when you pick one.

```json
{ "auth": { "type": "access_key", "access_key_id": "AKIA…" }, "region": "eu-west-2", "label": "voip" }
```

**Discover alarms** lists every alarm in the account (in any state, under
the prefix) with its current state and condition. Tick the ones to watch,
which saves them to `include`, or tick none to watch everything that matches
the prefix. Giving an alarm a severity saves an exact-name alarm rule at the
top of the list.

- An alarm that enters ALARM appears as a sticky critical island, with
  **Console ↗** and **Snooze 1h**.
- An alarm that leaves ALARM gets a 20-second OK island.
- A dismissed alarm stays hidden until its state changes again.
- This needs only `cloudwatch:DescribeAlarms`.

### CloudWatch, pushed through SQS (multi-account)

For many accounts, or to get alarms within seconds without polling:

1. Create an SQS queue in one account.
2. In each account, add an EventBridge rule matching
   `{"source":["aws.cloudwatch"],"detail-type":["CloudWatch Alarm State Change"]}`.
   Target the queue directly, or forward to a central event bus first.
   Existing SNS alarm topics can subscribe the queue instead.
3. Set the queue in the config:

```json
"sqs": { "enabled": true, "queue_url": "https://sqs.eu-west-2.amazonaws.com/123456789012/houston", "profile": "ops", "region": "eu-west-2" }
```

A message body can also be an island JSON, so a Lambda, a pipeline or anything
else that can write to SQS can post custom notifications. Messages are deleted
after they are shown. This needs `sqs:ReceiveMessage` and `sqs:DeleteMessage`.

Change sources in Mission settings: only the affected ones restart. Hand
edits to the file are read at the next launch.

### Alarm rules

By default every alarm in ALARM is a sticky critical. `alarm_rules` changes
that per alarm name, for every source. `pattern` uses `*` as a wildcard, and
the first matching rule wins, so put specific patterns first:

```json
"alarm_rules": [
  { "pattern": "apollo-prod-concurrent-calls-capacity", "severity": "critical",
    "runbook_url": "https://github.com/…/docs/runbooks/capacity.md" },
  { "pattern": "apollo-prod-eks-*", "severity": "warning" },
  { "pattern": "apollo-prod-billing-*", "severity": "info", "silent": true },
  { "pattern": "*langfuse*", "mute": true }
]
```

- `severity`: `critical`, `warning`, `info` or `success`, while in ALARM.
- `sticky`: defaults to true for critical. Other alarms stay for 5 minutes.
- `silent`: update without a peek or a sound.
- `mute`: drop the alarm.
- `runbook_url`: adds a **Runbook ↗** button.

### Linear

```json
"linear": {
  "enabled": true,
  "interval_secs": 60,
  "assigned_to_me": true,
  "teams": ["APO"],
  "max_priority": 2
}
```

Create a personal API key under Linear > Settings > Security & access. It
needs read access, plus write access if you want **Take it** to work. Paste
it into the Linear section of Mission settings, which stores it in the
Keychain. Otherwise Houston falls back to the `LINEAR_API_KEY` environment
variable.

- **Your issues**: an island when an issue is assigned to you, or when its
  state, priority or assignee changes. Edits to the title or description stay
  quiet. Launch is quiet too: the first poll only records what's there.
- **Watched teams**: issues in `teams` at `max_priority` or above (1 urgent,
  2 high, 3 medium) stay sticky until they're completed, cancelled or
  downgraded. Urgent is critical and high is a warning.
- **Open in Linear ↗** opens the issue. **Take it** (on issues that aren't
  yours) assigns the issue to you and polls again straight away.
- A bad key or an outage shows as "Lost signal from Linear".

### AWS IoT Core (stub)

`src/sources/iot.rs` has the config and the mapping, but no MQTT client yet.
When enabled it only reports "not implemented yet" in the status line. The
mapping turns Apollo's live call events (`apollo/<env>/t/<tenant>/call/<id>/events`)
into one timer island per call, which goes away on `call.ended`:

```json
"iot": { "enabled": false, "endpoint": "abc123-ats.iot.eu-west-2.amazonaws.com",
         "profile": "apollo-prod", "region": "eu-west-2",
         "topics": ["apollo/prod/t/+/call/+/events"] }
```

### Apollo (sip) prod

`examples/apollo-prod.json` has every Apollo route set up but turned off, and
a first draft of `alarm_rules`. Copy the parts you want into your config.

- **Polling** works as soon as `cloudwatch.enabled` is true and the account
  has an IAM access key (or a working CLI profile).
- **SQS** needs a queue that doesn't exist yet. In sip, subscribe one to the
  `apollo-prod-alarms` topic (and `security-incidents` for findings) next to
  `AlarmSlackNotifyConstruct`, with `rawMessageDelivery` off.
- **IoT** needs the MQTT client in `iot.rs`, and an IAM role with
  `iot:Connect`, `iot:Subscribe` and `iot:Receive` on the topic filter.

## Install and launch at login

Download `Houston-<version>-macos.zip` from
[Releases](https://github.com/rhamnett/houston/releases) (universal: Apple
silicon and Intel), unzip it and move Houston.app to Applications. It's
ad-hoc signed, not notarised, so open it from the right-click menu the first
time, or run `xattr -dr com.apple.quarantine /Applications/Houston.app`.

Or build it:

```bash
./bundle.sh                # dist/Houston.app and a zip, for this Mac
UNIVERSAL=1 ./bundle.sh    # both architectures, as releases are built
open dist/Houston.app
```

Then add it under System Settings > General > Login Items.

### Updates

Houston looks for a newer release 30 seconds after launch and then daily;
**Check for updates…** in the menu (or the `check_for_updates` MCP tool)
checks now. A new version arrives as an island with **Download ↗**,
**What's new ↗** and **Skip this version**. Nothing installs by itself. Turn
it off under Mission settings > General.

While the repository is private, the check needs a GitHub token that can see
it: a fine-grained token with **Contents: read** on `rhamnett/houston`, pasted
into Mission settings > General (it goes to the Keychain). Without one, the
status line says so and nothing else happens.

### Releasing

Releases are automatic. Pull requests squash-merge with their title as the
commit message, titles must be [Conventional
Commits](https://www.conventionalcommits.org) (the `pr-title` check), and
every merge to `main` works out the next [semantic
version](https://semver.org) from the commits since the last `v*` tag:

| PR title | Release |
| --- | --- |
| `fix: …`, `perf: …`, `revert: …` | patch: 0.3.0 → 0.3.1 |
| `feat: …` | minor: 0.3.1 → 0.4.0 |
| `feat!: …` (any type with `!`) | major: 1.4.0 → 2.0.0 (minor before 1.0) |
| `docs`, `test`, `ci`, `build`, `chore`, `refactor`, `style` | none |

A scope is optional: `fix(mcp): answer pings`. A breaking change needs the
`!` in the title; a `BREAKING CHANGE:` footer in the description is welcome
for the explanation, but `pr-title` fails if it's there without the `!`, so
the title alone decides the version. When there's something to
release, `release.yml` tags the merge, builds the universal app and
publishes a GitHub release with notes grouped from those titles; otherwise
it stops after one quick Linux job. The version lives in the tag, not in
`Cargo.toml` (whose `0.0.0` is a placeholder): `build.rs` stamps
`HOUSTON_VERSION` from the release, or `git describe` for local builds,
so `houston --version` says `0.3.0` on a release and `0.3.0+2.gabc1234` two
commits past it. The rules live in `scripts/conventional.sh`
(tests: `scripts/conventional_test.sh`).

CI (`ci.yml`) runs fmt, clippy, the tests and the end-to-end tests on every
pull request and push to `main`.

## Logs

`~/Library/Logs/houston.log` when started from Finder or as a login item;
the terminal otherwise.

## Layout

- `src/main.rs`: event loop, tray menu, peek and hover state, action routing
- `src/island.rs`: the notification model and validation
- `src/store.rs`: active islands, expiry, snooze, dismissal, ordering
- `src/source.rs`: the `Source` trait, `Sink` and `Registry`
- `src/sources/`: `demo`, `local_api`, `cloudwatch`, `sqs`, `linear`, `updates`, `iot` (stub)
- `src/aws.rs`: the AWS CLI runner, the alarm and findings mappers, alarm rules
- `src/overlay.rs`: the overlay window and the island page (HTML/CSS/JS)
- `src/notch.rs`: notch geometry, window level and Spaces, the hover monitor
- `src/prefs.rs`: the Mission settings window and its page
- `src/control.rs`: remote-control commands for the API and MCP
- `src/mcp.rs`: the MCP tools, JSON-RPC handling and the stdio bridge
- `src/secrets.rs`: Keychain storage for the API token and the Linear key
- `src/focus.rs`: following macOS Focus via the Do Not Disturb store
- `src/config.rs`, `src/icon.rs`, `src/notify.rs`, `src/log.rs`

## Development

```bash
cargo test                    # mappers, store, rules, config, the API and MCP over TCP
cargo test --test e2e -- --ignored   # a sandboxed Houston, flown over MCP and REST
cargo test -- --ignored       # also hits live APIs (Linear auth check)
cargo clippy --all-targets    # kept warning-free
cargo fmt
```

The end-to-end tests launch a second Houston with its own data folder
(`HOUSTON_DATA_DIR`), Keychain service (`HOUSTON_KEYCHAIN_SERVICE`) and port,
so they never touch your settings or secrets. It shows a menu bar icon for
the few seconds it runs.

New source? Add a module in `src/sources/`, implement `Source`, register it in
`sources::from_config` and give it a config block in `src/config.rs`.
`sources/iot.rs` is a good small example of the shape.

## Security and contributing

`main` only changes through pull requests: squash-merged, with a
Conventional Commit title (`pr-title`), CI (`check`) and Trivy (`trivy`)
passing; nobody can push to it, force-push or delete it.
`v*` release tags can't be moved or deleted once pushed. Trivy scans for
vulnerable dependencies, committed secrets and misconfigurations on every
pull request, on `main` and weekly; accepted findings live in
`.trivyignore.yaml` with a reason and an expiry. Dependabot keeps Cargo
crates and the SHA-pinned actions current, waiting until a release is two
weeks old and titling its PRs `fix(deps)` (crates ship in the app) or
`ci(deps)`. To report a vulnerability, see
[SECURITY.md](SECURITY.md).

## Known limits

- Clicking a button brings the app forward, so the app you were in loses focus
  until you click back. A non-activating `NSPanel` would avoid this.
- Display changes are picked up within 3 s.
- The IoT source is a stub: the config and mapping are there, but there's no
  MQTT client yet.
- Not affiliated with Notchy; this is a from-scratch take on the idea.
  Not affiliated with NASA either, sadly.
