//! Alert sounds, played by the app itself.

use objc2_app_kit::NSSound;
use objc2_foundation::NSString;

use crate::island::Severity;

pub fn sound(severity: Severity) {
    let name = match severity {
        Severity::Critical => "Sosumi",
        Severity::Warning => "Tink",
        Severity::Info | Severity::Success => return,
    };
    if let Some(s) = NSSound::soundNamed(&NSString::from_str(name)) {
        s.play();
    }
}
