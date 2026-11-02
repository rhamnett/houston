//! "About Houston": macOS's standard About panel, with the version, a line
//! on what Houston is, a link to the repository and the island as its icon.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSAboutPanelOptionApplicationName,
    NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits, NSAboutPanelOptionVersion,
    NSApplication, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage,
    NSLinkAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextAlignment,
};
use objc2_foundation::{
    NSAttributedString, NSData, NSDictionary, NSMutableAttributedString, NSString, NSURL, ns_string,
};

const TAGLINE: &str = "Mission control in your notch.\nCloudWatch, SQS, Linear and your agents.";
const REPO: &str = "github.com/rhamnett/houston";

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

/// The credits in the panel's own style: centred, small and secondary, with
/// the repository as a link. Plain text would be left-aligned at 12 pt.
fn credits() -> Retained<NSAttributedString> {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    style.setParagraphSpacingBefore(4.0);
    // SAFETY: AppKit's attribute names, each given a value of its type.
    let (keys, values) = unsafe {
        (
            [
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            [
                object(NSFont::systemFontOfSize(11.0)),
                object(NSColor::secondaryLabelColor()),
                object(style),
            ],
        )
    };
    let plain = NSDictionary::from_retained_objects(&keys, &values);
    let url = NSURL::URLWithString(&NSString::from_str(&format!("https://{REPO}")));
    let mut link_keys = keys.to_vec();
    let mut link_values = values.to_vec();
    if let Some(url) = url {
        link_keys.push(unsafe { NSLinkAttributeName });
        link_values.push(object(url));
    }
    let linked = NSDictionary::from_retained_objects(&link_keys, &link_values);
    // SAFETY: both dictionaries map attribute names to valid values.
    unsafe {
        let text = NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(&format!("{TAGLINE}\n")),
            Some(&plain),
        );
        text.appendAttributedString(&NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(REPO),
            Some(&linked),
        ));
        Retained::into_super(text)
    }
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
        object(credits()),
    ];
    // Without one the panel repeats the application version in brackets.
    keys.push(unsafe { NSAboutPanelOptionVersion });
    values.push(object(NSString::from_str(build.unwrap_or(""))));
    keys.push(ns_string!("Copyright"));
    values.push(object(NSString::from_str("© 2026 Richard Hamnett")));
    let png = crate::icon::app_icon_png(512);
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
