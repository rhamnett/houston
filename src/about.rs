//! "About Houston": macOS's standard About panel, with the version, a line
//! on what Houston is, a link to the repository and the island as its icon.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSAboutPanelOptionApplicationName,
    NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits, NSAboutPanelOptionVersion,
    NSApplication, NSImage,
};
use objc2_foundation::{NSAttributedString, NSData, NSDictionary, NSString, ns_string};

const CREDITS: &str = "Mission control in your notch: CloudWatch alarms, SQS events, Linear \
                       issues and anything your agents post.\n\ngithub.com/rhamnett/houston";

/// `0.3.0+6.gc239974` → ("0.3.0", Some("6.gc239974")): the panel shows
/// "Version 0.3.0 (6.gc239974)".
fn split_version(v: &str) -> (&str, Option<&str>) {
    match v.split_once('+') {
        Some((release, build)) => (release, Some(build)),
        None => (v, None),
    }
}

fn object<T: objc2::Message>(o: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(o) }
}

pub fn show() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (release, build) = split_version(crate::VERSION);
    // SAFETY: AppKit's own constants, valid for the life of the process.
    let mut keys: Vec<&NSString> = unsafe {
        vec![
            NSAboutPanelOptionApplicationName,
            NSAboutPanelOptionApplicationVersion,
            NSAboutPanelOptionCredits,
        ]
    };
    let mut values = vec![
        object(NSString::from_str("Houston")),
        object(NSString::from_str(release)),
        object(NSAttributedString::from_nsstring(&NSString::from_str(
            CREDITS,
        ))),
    ];
    // Without one the panel repeats the application version in brackets.
    keys.push(unsafe { NSAboutPanelOptionVersion });
    values.push(object(NSString::from_str(build.unwrap_or(""))));
    keys.push(ns_string!("Copyright"));
    values.push(object(NSString::from_str("© 2026 Richard Hamnett")));
    let png = crate::icon::app_icon_png(256);
    if let Some(icon) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png)) {
        keys.push(unsafe { NSAboutPanelOptionApplicationIcon });
        values.push(object(icon));
    }
    let options = NSDictionary::from_retained_objects(&keys, &values);
    let app = NSApplication::sharedApplication(mtm);
    // Houston has no Dock icon and is never the active app, so without this
    // the panel opens behind whatever you were using.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    // SAFETY: the keys are About panel options and the values their types.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(split_version("0.3.0"), ("0.3.0", None));
        assert_eq!(
            split_version("0.3.0+6.gc239974"),
            ("0.3.0", Some("6.gc239974"))
        );
    }
}
