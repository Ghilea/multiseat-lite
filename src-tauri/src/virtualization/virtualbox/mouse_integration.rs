//! VirtualBox mouse-integration control boundary.
//!
//! VirtualBox 7.2 documents the GUI toggle, but neither VBoxManage nor the
//! Main API's `IMouse` exposes a supported enable/disable operation. The
//! production controller therefore reports that a manual GUI action is
//! required instead of inventing an extradata key or automating GUI input.

use std::{collections::HashMap, sync::Mutex};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseIntegrationState {
    Enabled,
    Disabled,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseIntegrationControl {
    Supported,
    #[default]
    ManualRequired,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MouseIntegrationStatus {
    pub requested: MouseIntegrationState,
    pub observed: MouseIntegrationState,
    pub control: MouseIntegrationControl,
    pub owned_by_multiseat: bool,
    pub message: Option<String>,
}

pub trait MouseIntegrationController: Send + Sync {
    fn request(
        &self,
        vm_id: &str,
        desired: MouseIntegrationState,
    ) -> Result<MouseIntegrationStatus, String>;

    fn restore_if_owned(&self, vm_id: &str) -> Result<MouseIntegrationStatus, String>;
}

pub fn documented_backend() -> Box<dyn MouseIntegrationController> {
    Box::new(DocumentedVirtualBoxController)
}

struct DocumentedVirtualBoxController;

impl MouseIntegrationController for DocumentedVirtualBoxController {
    fn request(
        &self,
        _vm_id: &str,
        desired: MouseIntegrationState,
    ) -> Result<MouseIntegrationStatus, String> {
        Ok(MouseIntegrationStatus {
            requested: desired,
            observed: MouseIntegrationState::Unknown,
            control: MouseIntegrationControl::ManualRequired,
            owned_by_multiseat: false,
            message: Some(
                "VirtualBox 7.2 exposes Mouse Integration as a GUI action; no documented VBoxManage or Main API toggle is available. Set it manually in the VM Input menu"
                    .to_owned(),
            ),
        })
    }

    fn restore_if_owned(&self, _vm_id: &str) -> Result<MouseIntegrationStatus, String> {
        Ok(MouseIntegrationStatus {
            message: Some(
                "MultiSeat Lite did not own Mouse Integration; nothing was restored".into(),
            ),
            ..MouseIntegrationStatus::default()
        })
    }
}

#[derive(Default)]
pub(crate) struct SimulatedMouseIntegrationController {
    states: Mutex<HashMap<String, MouseIntegrationState>>,
    original: Mutex<HashMap<String, MouseIntegrationState>>,
}

impl MouseIntegrationController for SimulatedMouseIntegrationController {
    fn request(
        &self,
        vm_id: &str,
        desired: MouseIntegrationState,
    ) -> Result<MouseIntegrationStatus, String> {
        let mut states = self
            .states
            .lock()
            .map_err(|_| "simulated mouse integration state lock is poisoned".to_owned())?;
        let previous = states
            .get(vm_id)
            .copied()
            .unwrap_or(MouseIntegrationState::Enabled);
        self.original
            .lock()
            .map_err(|_| "simulated mouse integration ownership lock is poisoned".to_owned())?
            .entry(vm_id.to_owned())
            .or_insert(previous);
        states.insert(vm_id.to_owned(), desired);
        Ok(MouseIntegrationStatus {
            requested: desired,
            observed: desired,
            control: MouseIntegrationControl::Supported,
            owned_by_multiseat: true,
            message: None,
        })
    }

    fn restore_if_owned(&self, vm_id: &str) -> Result<MouseIntegrationStatus, String> {
        let original = self
            .original
            .lock()
            .map_err(|_| "simulated mouse integration ownership lock is poisoned".to_owned())?
            .remove(vm_id);
        let Some(original) = original else {
            return Ok(MouseIntegrationStatus::default());
        };
        self.states
            .lock()
            .map_err(|_| "simulated mouse integration state lock is poisoned".to_owned())?
            .insert(vm_id.to_owned(), original);
        Ok(MouseIntegrationStatus {
            requested: original,
            observed: original,
            control: MouseIntegrationControl::Supported,
            owned_by_multiseat: false,
            message: Some("previous Mouse Integration state restored".into()),
        })
    }
}

pub(crate) fn simulated_backend() -> Box<dyn MouseIntegrationController> {
    Box::new(SimulatedMouseIntegrationController::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_controller_reports_manual_unknown_without_claiming_ownership() {
        let status = DocumentedVirtualBoxController
            .request("managed-vm", MouseIntegrationState::Disabled)
            .unwrap();
        assert_eq!(status.requested, MouseIntegrationState::Disabled);
        assert_eq!(status.observed, MouseIntegrationState::Unknown);
        assert_eq!(status.control, MouseIntegrationControl::ManualRequired);
        assert!(!status.owned_by_multiseat);
    }

    #[test]
    fn simulated_controller_restores_only_the_vm_it_owned() {
        let controller = SimulatedMouseIntegrationController::default();
        let locked = controller
            .request("managed-vm", MouseIntegrationState::Disabled)
            .unwrap();
        assert!(locked.owned_by_multiseat);
        assert_eq!(locked.observed, MouseIntegrationState::Disabled);
        let unrelated = controller.restore_if_owned("unrelated-vm").unwrap();
        assert_eq!(unrelated.observed, MouseIntegrationState::Unknown);
        let restored = controller.restore_if_owned("managed-vm").unwrap();
        assert_eq!(restored.observed, MouseIntegrationState::Enabled);
    }
}
