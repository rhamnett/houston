//! Screen geometry and the AppKit settings that make the overlay sit over the
//! notch: above the menu bar, on every Space and over full-screen apps, and
//! click-through except while the pointer is on the island.
//!
//! Rectangles here are in Cocoa screen coordinates (origin bottom-left of the
//! primary display, y up), which is what `NSEvent::mouseLocation` reports.

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSEvent, NSEventMask, NSMainMenuWindowLevel, NSScreen, NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use serde::Serialize;
use tao::event_loop::EventLoopProxy;
use tao::platform::macos::WindowExtMacOS;
use tao::window::Window;

use crate::UserEvent;

/// Width the island pretends the notch has on screens without one.
const VIRTUAL_NOTCH_WIDTH: f64 = 180.0;
const FALLBACK_MENU_BAR: f64 = 24.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn from_ns(r: NSRect) -> Self {
        Self {
            x: r.origin.x,
            y: r.origin.y,
            w: r.size.width,
            h: r.size.height,
        }
    }

    fn to_ns(self) -> NSRect {
        NSRect::new(NSPoint::new(self.x, self.y), NSSize::new(self.w, self.h))
    }

    pub fn top(&self) -> f64 {
        self.y + self.h
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.top()
    }

    /// Grow by `d` on every side.
    pub fn grow(&self, d: f64) -> Self {
        Self {
            x: self.x - d,
            y: self.y - d,
            w: self.w + 2.0 * d,
            h: self.h + 2.0 * d,
        }
    }
}

/// Sent to the page so the island lines up with the hardware notch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Notch {
    pub width: f64,
    pub height: f64,
    pub has_notch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub screen: Rect,
    pub notch: Notch,
}

impl Geometry {
    /// The overlay window: `w` × `h`, centred at the top of the screen.
    pub fn window_frame(&self, w: f64, h: f64) -> Rect {
        Rect {
            x: self.screen.x + (self.screen.w - w) / 2.0,
            y: self.screen.top() - h,
            w,
            h,
        }
    }
}

/// The built-in display if it has a notch, otherwise the primary display.
pub fn detect(mtm: MainThreadMarker) -> Option<Geometry> {
    let screens = NSScreen::screens(mtm);
    let all: Vec<_> = (0..screens.count())
        .map(|i| screens.objectAtIndex(i))
        .collect();
    let screen = all
        .iter()
        .find(|s| s.safeAreaInsets().top > 0.0)
        .or_else(|| all.first())?;
    let frame = Rect::from_ns(screen.frame());
    let top_inset = screen.safeAreaInsets().top;
    let notch = if top_inset > 0.0 {
        let left = screen.auxiliaryTopLeftArea().size.width;
        let right = screen.auxiliaryTopRightArea().size.width;
        Notch {
            width: (frame.w - left - right).max(VIRTUAL_NOTCH_WIDTH / 2.0),
            height: top_inset,
            has_notch: true,
        }
    } else {
        let visible = Rect::from_ns(screen.visibleFrame());
        let bar = frame.top() - visible.top();
        Notch {
            width: VIRTUAL_NOTCH_WIDTH,
            height: if bar > 0.0 { bar } else { FALLBACK_MENU_BAR },
            has_notch: false,
        }
    };
    Some(Geometry {
        screen: frame,
        notch,
    })
}

fn ns_window(window: &Window) -> Option<&NSWindow> {
    // SAFETY: tao hands out its live NSWindow, owned by `window` for as long as
    // the returned reference (tied to `window`) is used.
    unsafe { (window.ns_window() as *const NSWindow).as_ref() }
}

/// Float above the menu bar, everywhere, without a shadow, ignoring the mouse.
pub fn configure(window: &Window) {
    let Some(ns) = ns_window(window) else { return };
    ns.setLevel(NSMainMenuWindowLevel + 3);
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    ns.setHasShadow(false);
    ns.setIgnoresMouseEvents(true);
}

/// Place the window directly; tao would keep it below the menu bar.
pub fn set_frame(window: &Window, frame: Rect) {
    if let Some(ns) = ns_window(window) {
        ns.setFrame_display(frame.to_ns(), true);
    }
}

/// Show without taking focus from the frontmost app.
pub fn order_front(window: &Window) {
    if let Some(ns) = ns_window(window) {
        ns.orderFrontRegardless();
    }
}

pub fn set_ignores_mouse(window: &Window, ignore: bool) {
    if let Some(ns) = ns_window(window) {
        ns.setIgnoresMouseEvents(ignore);
    }
}

pub fn mouse() -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    (p.x, p.y)
}

/// Report `HoverEnter` once when the pointer moves onto `hot` while
/// `hovering` is false. Global monitors see only events bound for other apps,
/// which is exactly while the overlay is click-through; leaving is detected by
/// polling in the app. Needs no Accessibility permission.
pub fn install_hover_monitor(
    hot: Rc<Cell<Rect>>,
    hovering: Rc<Cell<bool>>,
    proxy: EventLoopProxy<UserEvent>,
) {
    let handler = block2::RcBlock::new(move |_event: NonNull<NSEvent>| {
        if hovering.get() {
            return;
        }
        let (x, y) = mouse();
        if hot.get().contains(x, y) {
            hovering.set(true);
            let _ = proxy.send_event(UserEvent::HoverEnter);
        }
    });
    let mask = NSEventMask::MouseMoved | NSEventMask::LeftMouseDragged;
    // The monitor lives for the whole process, so its token is leaked.
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &handler);
    std::mem::forget(monitor);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_hit_testing() {
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 30.0,
        };
        assert!(r.contains(10.0, 50.0));
        assert!(!r.contains(9.0, 30.0));
        assert!(r.grow(2.0).contains(9.0, 51.0));
    }

    #[test]
    fn window_is_centred_at_the_top() {
        let g = Geometry {
            screen: Rect {
                x: 0.0,
                y: 0.0,
                w: 1512.0,
                h: 982.0,
            },
            notch: Notch {
                width: 185.0,
                height: 32.0,
                has_notch: true,
            },
        };
        let f = g.window_frame(512.0, 400.0);
        assert_eq!((f.x, f.y), (500.0, 582.0));
        assert_eq!(f.top(), 982.0);
    }
}
