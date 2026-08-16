//! Documented VirtualBox scan-code injection sink used only by an explicit
//! developer prototype action.

use std::collections::BTreeMap;

use crate::keyboard_routing::{GuestKeyboardSink, NormalizedKeyEvent};

use super::{ProcessVBoxManageExecutor, VBoxManageExecutor};

pub struct VirtualBoxGuestKeyboardSink {
    vm_id: String,
    executor: Box<dyn VBoxManageExecutor>,
    pressed: BTreeMap<(u16, bool, bool), NormalizedKeyEvent>,
}

impl VirtualBoxGuestKeyboardSink {
    pub fn discover(vm_id: String) -> Result<Self, String> {
        Ok(Self::new(
            vm_id,
            Box::new(ProcessVBoxManageExecutor::discover()?),
        ))
    }

    pub fn new(vm_id: String, executor: Box<dyn VBoxManageExecutor>) -> Self {
        Self {
            vm_id,
            executor,
            pressed: BTreeMap::new(),
        }
    }

    fn send(&self, event: &NormalizedKeyEvent) -> Result<(), String> {
        let mut arguments = vec![
            "controlvm".to_owned(),
            self.vm_id.clone(),
            "keyboardputscancode".to_owned(),
        ];
        arguments.extend(
            event
                .virtual_box_scancodes()
                .into_iter()
                .map(|byte| format!("{byte:02x}")),
        );
        let output = self.executor.execute(&arguments)?;
        if output.success {
            Ok(())
        } else {
            Err(format!(
                "VBoxManage keyboardputscancode failed (exit {:?}): {}",
                output.exit_code,
                output.stderr.trim()
            ))
        }
    }

    fn key(event: &NormalizedKeyEvent) -> (u16, bool, bool) {
        (event.scan_code, event.extended_e0, event.extended_e1)
    }
}

impl GuestKeyboardSink for VirtualBoxGuestKeyboardSink {
    fn key_down(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
        self.send(event)?;
        self.pressed.insert(Self::key(event), event.clone());
        Ok(())
    }

    fn key_up(&mut self, event: &NormalizedKeyEvent) -> Result<(), String> {
        self.send(event)?;
        self.pressed.remove(&Self::key(event));
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), String> {
        let pressed: Vec<_> = self.pressed.values().cloned().collect();
        let mut errors = Vec::new();
        for mut event in pressed {
            event.transition = crate::keyboard_routing::KeyTransition::Up;
            match self.send(&event) {
                Ok(()) => {
                    self.pressed.remove(&Self::key(&event));
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::keyboard_routing::{normalize_raw_keyboard_event, ModifierKey};

    use super::*;
    use crate::virtualization::virtualbox::VBoxOutput;

    #[derive(Clone, Default)]
    struct FakeExecutor(Arc<Mutex<Vec<Vec<String>>>>);

    impl VBoxManageExecutor for FakeExecutor {
        fn execute(&self, arguments: &[String]) -> Result<VBoxOutput, String> {
            self.0.lock().unwrap().push(arguments.to_vec());
            Ok(VBoxOutput {
                success: true,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn forwards_make_break_e0_and_modifier_in_order() {
        let fake = FakeExecutor::default();
        let mut sink =
            VirtualBoxGuestKeyboardSink::new("managed-vm".into(), Box::new(fake.clone()));
        let down = normalize_raw_keyboard_event("barnen".into(), "raw".into(), 0x1d, 0x02, 0x11);
        assert_eq!(down.modifier, Some(ModifierKey::RightControl));
        let up = normalize_raw_keyboard_event("barnen".into(), "raw".into(), 0x1d, 0x03, 0x11);
        sink.key_down(&down).unwrap();
        sink.key_up(&up).unwrap();

        let calls = fake.0.lock().unwrap();
        assert_eq!(
            calls[0],
            ["controlvm", "managed-vm", "keyboardputscancode", "e0", "1d"]
        );
        assert_eq!(
            calls[1],
            ["controlvm", "managed-vm", "keyboardputscancode", "e0", "9d"]
        );
    }

    #[test]
    fn release_all_emits_break_for_every_held_key() {
        let fake = FakeExecutor::default();
        let mut sink =
            VirtualBoxGuestKeyboardSink::new("managed-vm".into(), Box::new(fake.clone()));
        sink.key_down(&normalize_raw_keyboard_event(
            "barnen".into(),
            "raw".into(),
            0x1e,
            0,
            0x41,
        ))
        .unwrap();
        sink.key_down(&normalize_raw_keyboard_event(
            "barnen".into(),
            "raw".into(),
            0x30,
            0,
            0x42,
        ))
        .unwrap();
        sink.release_all().unwrap();
        let calls = fake.0.lock().unwrap();
        let injected: Vec<_> = calls
            .iter()
            .map(|call| call.last().unwrap().as_str())
            .collect();
        assert_eq!(injected, ["1e", "30", "9e", "b0"]);
    }
}
