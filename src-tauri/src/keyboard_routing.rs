//! Safe, backend-neutral physical-keyboard routing foundation.
//!
//! This milestone intentionally supplies only a diagnostic sink. A future
//! VirtualBox sink may use documented scancode injection, but real injection
//! and host-input suppression are not enabled here.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyTransition {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ModifierKey {
    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftAlt,
    RightAlt,
    LeftWindows,
    RightWindows,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct PressedKey {
    scan_code: u16,
    extended_e0: bool,
    extended_e1: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedKeyEvent {
    pub source_physical_device_id: String,
    pub raw_input_device_path: String,
    pub scan_code: u16,
    pub virtual_key: u16,
    pub transition: KeyTransition,
    pub extended_e0: bool,
    pub extended_e1: bool,
    pub modifier: Option<ModifierKey>,
}

impl NormalizedKeyEvent {
    fn pressed_key(&self) -> PressedKey {
        PressedKey {
            scan_code: self.scan_code,
            extended_e0: self.extended_e0,
            extended_e1: self.extended_e1,
        }
    }

    /// Encodes documented PC set-1 style make/break bytes suitable for a
    /// future `VBoxManage controlvm ... keyboardputscancode` sink.
    pub fn virtual_box_scancodes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(2);
        if self.extended_e1 {
            bytes.push(0xE1);
        } else if self.extended_e0 {
            bytes.push(0xE0);
        }
        let code = u8::try_from(self.scan_code & 0x7f).unwrap_or_default();
        bytes.push(match self.transition {
            KeyTransition::Down => code,
            KeyTransition::Up => code | 0x80,
        });
        bytes
    }
}

pub fn normalize_raw_keyboard_event(
    source_physical_device_id: String,
    raw_input_device_path: String,
    make_code: u16,
    flags: u16,
    virtual_key: u16,
) -> NormalizedKeyEvent {
    const BREAK: u16 = 0x01;
    const E0: u16 = 0x02;
    const E1: u16 = 0x04;
    let extended_e0 = flags & E0 != 0;
    let extended_e1 = flags & E1 != 0;
    NormalizedKeyEvent {
        source_physical_device_id,
        raw_input_device_path,
        scan_code: make_code,
        virtual_key,
        transition: if flags & BREAK != 0 {
            KeyTransition::Up
        } else {
            KeyTransition::Down
        },
        extended_e0,
        extended_e1,
        modifier: modifier_key(virtual_key, make_code, extended_e0),
    }
}

fn modifier_key(virtual_key: u16, scan_code: u16, extended_e0: bool) -> Option<ModifierKey> {
    match virtual_key {
        0x10 => Some(if scan_code == 0x36 {
            ModifierKey::RightShift
        } else {
            ModifierKey::LeftShift
        }),
        0x11 => Some(if extended_e0 {
            ModifierKey::RightControl
        } else {
            ModifierKey::LeftControl
        }),
        0x12 => Some(if extended_e0 {
            ModifierKey::RightAlt
        } else {
            ModifierKey::LeftAlt
        }),
        0x5B => Some(ModifierKey::LeftWindows),
        0x5C => Some(ModifierKey::RightWindows),
        _ => None,
    }
}

pub trait GuestKeyboardSink {
    fn key_down(&mut self, event: &NormalizedKeyEvent) -> Result<(), String>;
    fn key_up(&mut self, event: &NormalizedKeyEvent) -> Result<(), String>;
    fn release_all(&mut self) -> Result<(), String>;
}

impl<T: GuestKeyboardSink + ?Sized> GuestKeyboardSink for Box<T> {
    fn key_down(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
        (**self).key_down(event)
    }

    fn key_up(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
        (**self).key_up(event)
    }

    fn release_all(&mut self) -> Result<(), String> {
        (**self).release_all()
    }
}

pub trait PhysicalKeyboardSource {
    fn next_event(&mut self) -> Result<Option<NormalizedKeyEvent>, String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteOutcome {
    Forwarded,
    IgnoredDifferentPhysicalDevice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyboardDiagnosticAction {
    KeyDown,
    KeyUp,
    ReleaseAll,
    BackendError,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardRoutingDiagnosticEvent {
    pub action: KeyboardDiagnosticAction,
    pub event: Option<NormalizedKeyEvent>,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardRoutingDiagnosticStatus {
    pub active: bool,
    pub target_physical_device_id: Option<String>,
    pub current_session_id: u32,
    pub real_guest_injection_enabled: bool,
    pub successful_guest_sends: u64,
    pub host_input_suppression: String,
    pub transport: KeyboardRoutingTransport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyboardRoutingTransport {
    DiagnosticOnly,
    VirtualBoxScancodePrototype,
}

pub struct KeyboardRouter<S: GuestKeyboardSink> {
    target_physical_device_id: String,
    sink: S,
    pressed: BTreeMap<PressedKey, NormalizedKeyEvent>,
}

impl<S: GuestKeyboardSink> KeyboardRouter<S> {
    pub fn new(target_physical_device_id: String, sink: S) -> Self {
        Self {
            target_physical_device_id,
            sink,
            pressed: BTreeMap::new(),
        }
    }

    pub fn route(&mut self, event: NormalizedKeyEvent) -> Result<RouteOutcome, String> {
        if !event
            .source_physical_device_id
            .eq_ignore_ascii_case(&self.target_physical_device_id)
        {
            return Ok(RouteOutcome::IgnoredDifferentPhysicalDevice);
        }
        let result = match event.transition {
            KeyTransition::Down => {
                let result = self.sink.key_down(&event);
                if result.is_ok() {
                    self.pressed.insert(event.pressed_key(), event);
                }
                result
            }
            KeyTransition::Up => {
                let result = self.sink.key_up(&event);
                if result.is_ok() {
                    self.pressed.remove(&event.pressed_key());
                }
                result
            }
        };
        if let Err(error) = result {
            let release_error = self.release_all().err();
            return Err(match release_error {
                Some(release_error) => format!(
                    "keyboard sink failed: {error}; release_all also failed: {release_error}"
                ),
                None => format!("keyboard sink failed: {error}; pressed keys were released"),
            });
        }
        Ok(RouteOutcome::Forwarded)
    }

    pub fn release_all(&mut self) -> Result<(), String> {
        let pressed: Vec<_> = self.pressed.values().cloned().collect();
        let mut errors = Vec::new();
        for mut event in pressed {
            event.transition = KeyTransition::Up;
            if let Err(error) = self.sink.key_up(&event) {
                errors.push(error);
            }
        }
        self.pressed.clear();
        if let Err(error) = self.sink.release_all() {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn pressed_key_count(&self) -> usize {
        self.pressed.len()
    }
}

impl<S: GuestKeyboardSink> Drop for KeyboardRouter<S> {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

pub fn route_available_events(
    source: &mut dyn PhysicalKeyboardSource,
    router: &mut KeyboardRouter<impl GuestKeyboardSink>,
) -> Result<usize, String> {
    let mut forwarded = 0;
    while let Some(event) = source.next_event()? {
        if router.route(event)? == RouteOutcome::Forwarded {
            forwarded += 1;
        }
    }
    Ok(forwarded)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[derive(Default)]
    struct MockSink {
        events: Vec<NormalizedKeyEvent>,
        release_all_calls: usize,
        fail_next: bool,
    }

    impl GuestKeyboardSink for MockSink {
        fn key_down(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
            if std::mem::take(&mut self.fail_next) {
                return Err("mock failure".into());
            }
            self.events.push(event.clone());
            Ok(())
        }

        fn key_up(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
            self.events.push(event.clone());
            Ok(())
        }

        fn release_all(&mut self) -> Result<(), String> {
            self.release_all_calls += 1;
            Ok(())
        }
    }

    struct VecSource(VecDeque<NormalizedKeyEvent>);

    impl PhysicalKeyboardSource for VecSource {
        fn next_event(&mut self) -> Result<Option<NormalizedKeyEvent>, String> {
            Ok(self.0.pop_front())
        }
    }

    fn event(source: &str, scan: u16, flags: u16, vkey: u16) -> NormalizedKeyEvent {
        normalize_raw_keyboard_event(source.into(), format!(r"\\?\{source}"), scan, flags, vkey)
    }

    #[test]
    fn dennis_is_ignored_and_barnen_is_forwarded_through_separate_traits() {
        let mut source = VecSource(VecDeque::from([
            event("dennis", 0x1e, 0, 0x41),
            event("barnen", 0x30, 0, 0x42),
        ]));
        let mut router = KeyboardRouter::new("barnen".into(), MockSink::default());
        assert_eq!(route_available_events(&mut source, &mut router).unwrap(), 1);
        assert_eq!(router.pressed_key_count(), 1);
    }

    #[test]
    fn tracks_make_break_modifiers_and_scancode_encoding() {
        let mut router = KeyboardRouter::new("barnen".into(), MockSink::default());
        let down = event("barnen", 0x1d, 0x02, 0x11);
        assert_eq!(down.modifier, Some(ModifierKey::RightControl));
        assert_eq!(down.virtual_box_scancodes(), vec![0xe0, 0x1d]);
        router.route(down).unwrap();
        assert_eq!(router.pressed_key_count(), 1);
        let up = event("barnen", 0x1d, 0x03, 0x11);
        assert_eq!(up.virtual_box_scancodes(), vec![0xe0, 0x9d]);
        router.route(up).unwrap();
        assert_eq!(router.pressed_key_count(), 0);
    }

    #[test]
    fn releases_all_on_shutdown_error_and_disconnect_boundary() {
        let mut router = KeyboardRouter::new("barnen".into(), MockSink::default());
        router.route(event("barnen", 0x2a, 0, 0x10)).unwrap();
        router.release_all().unwrap();
        assert_eq!(router.pressed_key_count(), 0);

        router.sink.fail_next = true;
        assert!(router.route(event("barnen", 0x1e, 0, 0x41)).is_err());
        assert_eq!(router.pressed_key_count(), 0);
        assert!(router.sink.release_all_calls >= 2);
    }
}
