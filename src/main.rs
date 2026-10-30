//! A Dynamic-Island style notification area in the MacBook notch. Sources
//! (a demo, a local HTTP API, CloudWatch polling, an SQS queue) post islands;
//! new ones expand out of the notch for a few seconds, then sit either side of
//! it until they expire or are dismissed. Hover the notch to see them all.

mod aws;
mod config;
mod icon;
mod island;
mod log;
mod notch;
mod notify;
mod overlay;
mod source;
mod sources;
mod store;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{TrayIcon, TrayIconBuilder};

use config::Config;
use island::{Severity, is_web_url};
use notch::{Geometry, Rect};
use overlay::{IslandView, Mode, Overlay, OverlayMessage, OverlayState};
use source::{Inbound, Registry, SourceEvent};
use store::{Store, Upsert};

#[derive(Debug)]
pub enum UserEvent {
    Source {
        source: String,
        event: SourceEvent,
    },
    Overlay(OverlayMessage),
    Menu(MenuEvent),
    /// The pointer moved onto the island.
    HoverEnter,
}

/// How often to check the pointer while the island is open.
const HOVER_POLL: Duration = Duration::from_millis(60);
/// Pointer must stay off the island this long before it collapses.
const LEAVE_GRACE: Duration = Duration::from_millis(220);
/// Slack around the island before the pointer counts as gone.
const LEAVE_MARGIN: f64 = 12.0;
const GEOMETRY_CHECK: Duration = Duration::from_secs(3);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn peek_for(severity: Severity) -> Duration {
    Duration::from_secs(match severity {
        Severity::Critical => 8,
        Severity::Warning => 6,
        Severity::Info | Severity::Success => 4,
    })
}

fn open_url(url: &str) {
    if !is_web_url(url) {
        return;
    }
    if let Err(e) = std::process::Command::new("/usr/bin/open").arg(url).spawn() {
        log::line(&format!("open {url}: {e}"));
    }
}

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn();
    if let Ok(mut child) = child {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

struct MenuIds {
    triggers: Vec<(MenuId, &'static str)>,
    dnd: MenuId,
    clear: MenuId,
    copy_token: MenuId,
    config: MenuId,
}

struct App {
    config: Config,
    store: Store,
    registry: Registry,
    overlay: Overlay,
    geometry: Geometry,
    geometry_checked: Instant,
    hot: Rc<Cell<Rect>>,
    hovering: Rc<Cell<bool>>,
    /// The page has told us where the island is for the current mode.
    open_hover: bool,
    left_at: Option<Instant>,
    peek: Option<(String, Instant)>,
    api_url: Option<String>,
    statuses: Vec<String>,
    last_render: String,
    tray: TrayIcon,
    dnd_item: CheckMenuItem,
    menu: MenuIds,
}

impl App {
    fn mode(&self) -> Mode {
        if self.open_hover {
            Mode::Expanded
        } else if self.peek.is_some() && !self.store.is_empty() {
            Mode::Peek
        } else if self.store.is_empty() {
            Mode::Idle
        } else {
            Mode::Compact
        }
    }

    fn render(&mut self) {
        let entries = self.store.sorted();
        let state = OverlayState {
            mode: self.mode(),
            islands: entries
                .iter()
                .map(|e| IslandView {
                    id: &e.id,
                    created_ms: e.created_ms,
                    updated_ms: e.updated_ms,
                    island: &e.island,
                })
                .collect(),
            peek: self.peek.as_ref().map(|(id, _)| id.as_str()),
            notch: self.geometry.notch,
            dnd: self.config.do_not_disturb,
            api: self.api_url.as_deref(),
            sources: self.registry.ids(),
            triggers: sources::DEMO_TRIGGERS,
        };
        let json = match serde_json::to_string(&state) {
            Ok(j) => j,
            Err(e) => return eprintln!("overlay state: {e}"),
        };
        if json != self.last_render {
            self.overlay.render(&json);
            self.last_render = json;
        }
        self.update_tray();
    }

    fn update_tray(&self) {
        let urgent = self
            .store
            .sorted()
            .iter()
            .filter(|e| e.island.severity >= Severity::Warning)
            .count();
        let title = if urgent > 0 {
            format!("{urgent}")
        } else {
            String::new()
        };
        self.tray.set_title(Some(title));
        let mut tip: Vec<String> = self
            .store
            .sorted()
            .iter()
            .take(6)
            .map(|e| format!("{} · {}", e.island.source, e.island.title))
            .collect();
        if tip.is_empty() {
            tip.push("Houston · all systems nominal".into());
        }
        tip.extend(self.statuses.iter().cloned());
        let _ = self.tray.set_tooltip(Some(tip.join("\n")));
    }

    fn on_source(&mut self, source: String, event: SourceEvent) {
        match event {
            SourceEvent::Upsert(island) => {
                let severity = island.severity;
                let id = store::id_of(&source, &island.key);
                if self.store.upsert(*island, now_ms()) == Upsert::Peek
                    && !self.config.do_not_disturb
                {
                    // A louder island takes over the peek; a quieter one waits its turn.
                    let current = self
                        .peek
                        .as_ref()
                        .and_then(|(pid, _)| self.store.get(pid))
                        .map(|e| e.island.severity);
                    if current.is_none_or(|s| severity >= s) {
                        self.peek = Some((id, Instant::now() + peek_for(severity)));
                    }
                    if self.config.sounds {
                        notify::sound(severity);
                    }
                }
            }
            SourceEvent::Remove(key) => {
                self.store.remove(&store::id_of(&source, &key));
            }
            SourceEvent::Status(msg) => {
                log::line(&msg);
                if msg.starts_with("local API on http") {
                    self.api_url = msg.strip_prefix("local API on http://").map(String::from);
                }
                self.statuses
                    .retain(|s| !s.starts_with(&format!("{source}:")));
                self.statuses.push(format!("{source}: {msg}"));
            }
        }
        self.render();
    }

    fn on_overlay(&mut self, msg: OverlayMessage) {
        match msg {
            OverlayMessage::Ready => self.last_render.clear(),
            OverlayMessage::Layout { x, y, w, h } => {
                self.hot.set(self.overlay.to_screen(x, y, w, h));
                return;
            }
            OverlayMessage::Action { id, action } => self.act(&id, &action),
            OverlayMessage::Dismiss { id } => {
                self.store.dismiss(&id);
            }
            OverlayMessage::ClearAll => self.store.clear(),
            OverlayMessage::SetDnd { on } => self.set_dnd(on),
            OverlayMessage::Demo { trigger } => {
                self.registry
                    .send(sources::DEMO_ID, Inbound::Trigger(trigger));
            }
        }
        self.render();
    }

    fn act(&mut self, id: &str, action_id: &str) {
        let Some(entry) = self.store.get(id) else {
            return;
        };
        let Some(action) = entry.island.actions.iter().find(|a| a.id == action_id) else {
            return;
        };
        log::line(&format!("action {action_id} on {id}"));
        if let Some(url) = &action.url {
            open_url(url);
            return;
        }
        match action_id {
            "dismiss" => {
                self.store.dismiss(id);
            }
            "snooze" => {
                let mins = action.snooze_mins.unwrap_or(60);
                self.store.snooze(id, mins, now_ms());
            }
            _ => {
                let (source, key) = (entry.island.source.clone(), entry.island.key.clone());
                self.registry.send(
                    &source,
                    Inbound::Action {
                        key,
                        action: action_id.to_string(),
                    },
                );
            }
        }
    }

    fn set_dnd(&mut self, on: bool) {
        self.config.do_not_disturb = on;
        self.dnd_item.set_checked(on);
        if on {
            self.peek = None;
        }
        if let Err(e) = self.config.save() {
            log::line(&format!("config: {e}"));
        }
    }

    fn on_menu(&mut self, event: MenuEvent) {
        let id = event.id();
        if let Some((_, trigger)) = self.menu.triggers.iter().find(|(m, _)| m == id) {
            self.registry
                .send(sources::DEMO_ID, Inbound::Trigger((*trigger).into()));
        } else if *id == self.menu.dnd {
            self.set_dnd(self.dnd_item.is_checked());
        } else if *id == self.menu.clear {
            self.store.clear();
        } else if *id == self.menu.copy_token {
            copy_to_clipboard(&self.config.api.token);
        } else if *id == self.menu.config
            && let Some(dir) = Config::dir()
        {
            let _ = std::process::Command::new("/usr/bin/open").arg(dir).spawn();
        }
        self.render();
    }

    fn hover_enter(&mut self) {
        self.open_hover = true;
        self.left_at = None;
        self.peek = None;
        self.overlay.set_interactive(true);
        self.render();
    }

    fn hover_leave(&mut self) {
        self.open_hover = false;
        self.hovering.set(false);
        self.left_at = None;
        self.overlay.set_interactive(false);
        self.render();
    }

    /// Timers: expiry, end of peek, pointer leaving, display changes.
    /// Returns when to wake next.
    fn tick(&mut self, mtm: MainThreadMarker) -> Instant {
        let now = Instant::now();
        let mut changed = self.store.expire(now_ms());
        if self.peek.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.peek = None;
            changed = true;
        }
        if self.open_hover {
            let (x, y) = notch::mouse();
            if self.hot.get().grow(LEAVE_MARGIN).contains(x, y) {
                self.left_at = None;
            } else {
                let since = *self.left_at.get_or_insert(now);
                if now.duration_since(since) >= LEAVE_GRACE {
                    self.hover_leave();
                }
            }
        }
        if now.duration_since(self.geometry_checked) >= GEOMETRY_CHECK {
            self.geometry_checked = now;
            if let Some(g) = notch::detect(mtm)
                && g != self.geometry
            {
                self.geometry = g;
                self.overlay.place(&g);
                changed = true;
            }
        }
        if changed {
            self.render();
        }
        let mut next = now + GEOMETRY_CHECK;
        if self.open_hover {
            next = next.min(now + HOVER_POLL);
        }
        if let Some((_, until)) = &self.peek {
            next = next.min(*until);
        }
        if let Some(t) = self.store.next_expiry() {
            let ms = (t - now_ms()).max(0) as u64;
            next = next.min(now + Duration::from_millis(ms + 5));
        }
        next
    }
}

fn build_menu(config: &Config) -> (Menu, MenuIds, CheckMenuItem) {
    let demo = Submenu::new("Run a drill", true);
    let mut triggers = Vec::new();
    for (trigger, label) in sources::DEMO_TRIGGERS {
        let item = MenuItem::new(*label, true, None);
        triggers.push((item.id().clone(), *trigger));
        let _ = demo.append(&item);
    }
    let dnd = CheckMenuItem::new(
        "Radio silence (do not disturb)",
        true,
        config.do_not_disturb,
        None,
    );
    let clear = MenuItem::new("Clear the board", true, None);
    let api_label = if config.api.enabled {
        format!("API on 127.0.0.1:{}", config.api.port)
    } else {
        "API off".into()
    };
    let api_info = MenuItem::new(api_label, false, None);
    let copy_token = MenuItem::new("Copy API token", config.api.enabled, None);
    let open_config = MenuItem::new("Open config folder", true, None);
    let menu = Menu::with_items(&[
        &demo,
        &PredefinedMenuItem::separator(),
        &dnd,
        &clear,
        &PredefinedMenuItem::separator(),
        &api_info,
        &copy_token,
        &open_config,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(Some("Quit Houston (splashdown)")),
    ])
    .expect("menu");
    let ids = MenuIds {
        triggers,
        dnd: dnd.id().clone(),
        clear: clear.id().clone(),
        copy_token: copy_token.id().clone(),
        config: open_config.id().clone(),
    };
    (menu, ids, dnd)
}

fn main() {
    log::init();
    let config = Config::load();
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();

    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = p.send_event(UserEvent::Menu(e));
    }));

    let (menu, menu_ids, dnd_item) = build_menu(&config);
    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon::island().expect("icon"))
        .with_tooltip("Houston")
        .with_menu(Box::new(menu))
        .build()
        .expect("status item");

    let mut setup = Some((config, menu_ids, dnd_item));
    let mut app: Option<App> = None;

    event_loop.run(move |event, target, control_flow| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // The overlay needs the event loop target, so it is built on the first event.
        if app.is_none() {
            let Some((config, menu, dnd_item)) = setup.take() else {
                return;
            };
            let Some(geometry) = notch::detect(mtm) else {
                eprintln!("houston: no screen");
                *control_flow = ControlFlow::ExitWithCode(1);
                return;
            };
            let overlay = match Overlay::new(target, proxy.clone(), &geometry) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("houston: {e}");
                    *control_flow = ControlFlow::ExitWithCode(1);
                    return;
                }
            };
            let hot = Rc::new(Cell::new(overlay.to_screen(
                (overlay::WIDTH - geometry.notch.width) / 2.0,
                0.0,
                geometry.notch.width,
                geometry.notch.height,
            )));
            let hovering = Rc::new(Cell::new(false));
            notch::install_hover_monitor(hot.clone(), hovering.clone(), proxy.clone());
            let registry = Registry::start(sources::from_config(&config), &proxy);
            log::line(&format!(
                "notch {}×{} (hardware notch: {})",
                geometry.notch.width, geometry.notch.height, geometry.notch.has_notch
            ));
            app = Some(App {
                config,
                store: Store::default(),
                registry,
                overlay,
                geometry,
                geometry_checked: Instant::now(),
                hot,
                hovering,
                open_hover: false,
                left_at: None,
                peek: None,
                api_url: None,
                statuses: Vec::new(),
                last_render: String::new(),
                tray: tray.clone(),
                dnd_item,
                menu,
            });
        }
        let Some(app) = app.as_mut() else { return };

        match event {
            Event::UserEvent(UserEvent::Source { source, event }) => app.on_source(source, event),
            Event::UserEvent(UserEvent::Overlay(msg)) => app.on_overlay(msg),
            Event::UserEvent(UserEvent::Menu(e)) => app.on_menu(e),
            Event::UserEvent(UserEvent::HoverEnter) => app.hover_enter(),
            Event::NewEvents(StartCause::Init) => app.render(),
            _ => {}
        }
        *control_flow = ControlFlow::WaitUntil(app.tick(mtm));
    });
}
