//! The Linux virtual-pad backends behind [`Pads`](super::Pads): UHID/usbip managers, one per
//! identity. Xbox360 over uinput is the common default and stays in `Pads`.

use super::*;

/// Linux UHID/usbip Triton backend.
type Sc2Manager = pf_inject::steam_controller2::Triton2Manager;

/// Managers are created lazily and own only the indices routed to them.
#[derive(Default)]
pub(super) struct PadBackends {
    xboxone: Option<crate::inject::gamepad::GamepadManager>,
    dualsense: Option<crate::inject::dualsense::DualSenseManager>,
    dualsense_edge: Option<crate::inject::dualsense::DualSenseEdgeManager>,
    dualshock4: Option<crate::inject::dualshock4::DualShock4Manager>,
    steamdeck: Option<crate::inject::steam_controller::SteamControllerManager>,
    switchpro: Option<crate::inject::switch_pro::SwitchProManager>,
    steamctrl: Option<crate::inject::steam_controller::SteamCtrlManager>,
    steamctrl2: Option<Sc2Manager>,
    steamctrl2_puck: Option<crate::inject::steam_controller2::Triton2Manager>,
}

impl PadBackends {
    /// Route a pad event to the manager for `kind`, building it on first use. `false` = no
    /// backend of this kind here; the caller's Xbox360 default takes it.
    pub(super) fn route_handle(
        &mut self,
        kind: GamepadPref,
        ev: &punktfunk_core::input::GamepadEvent,
    ) -> bool {
        match kind {
            GamepadPref::DualSense => self
                .dualsense
                .get_or_insert_with(crate::inject::dualsense::DualSenseManager::new)
                .handle(ev),
            GamepadPref::DualSenseEdge => self
                .dualsense_edge
                .get_or_insert_with(crate::inject::dualsense::DualSenseEdgeManager::new)
                .handle(ev),
            GamepadPref::DualShock4 => self
                .dualshock4
                .get_or_insert_with(crate::inject::dualshock4::DualShock4Manager::new)
                .handle(ev),
            GamepadPref::SteamDeck => self
                .steamdeck
                .get_or_insert_with(crate::inject::steam_controller::SteamControllerManager::new)
                .handle(ev),
            GamepadPref::SwitchPro => self
                .switchpro
                .get_or_insert_with(crate::inject::switch_pro::SwitchProManager::new)
                .handle(ev),
            GamepadPref::SteamController => self
                .steamctrl
                .get_or_insert_with(crate::inject::steam_controller::SteamCtrlManager::new)
                .handle(ev),
            GamepadPref::SteamController2 => self
                .steamctrl2
                .get_or_insert_with(Sc2Manager::new)
                .handle(ev),
            GamepadPref::SteamController2Puck => self
                .steamctrl2_puck
                .get_or_insert_with(|| {
                    crate::inject::steam_controller2::Triton2Manager::with_backend(
                        crate::inject::steam_controller2::TritonProto::puck(),
                    )
                })
                .handle(ev),
            GamepadPref::XboxOne => self
                .xboxone
                .get_or_insert_with(|| {
                    crate::inject::gamepad::GamepadManager::with_identity(
                        crate::inject::gamepad::PadIdentity::xbox_one(),
                    )
                })
                .handle(ev),
            _ => return false,
        }
        true
    }

    /// Touchpad / motion for the pad's manager. No device yet → no-op. Xbox has no rich plane.
    pub(super) fn apply_rich(&mut self, kind: GamepadPref, rich: punktfunk_core::quic::RichInput) {
        match kind {
            GamepadPref::DualSense => {
                if let Some(m) = &mut self.dualsense {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::DualSenseEdge => {
                if let Some(m) = &mut self.dualsense_edge {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::DualShock4 => {
                if let Some(m) = &mut self.dualshock4 {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::SteamDeck => {
                if let Some(m) = &mut self.steamdeck {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::SwitchPro => {
                if let Some(m) = &mut self.switchpro {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::SteamController => {
                if let Some(m) = &mut self.steamctrl {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::SteamController2 => {
                if let Some(m) = &mut self.steamctrl2 {
                    m.apply_rich(rich)
                }
            }
            GamepadPref::SteamController2Puck => {
                if let Some(m) = &mut self.steamctrl2_puck {
                    m.apply_rich(rich)
                }
            }
            _ => {}
        }
    }

    /// A Triton device (either shape) is live: its USB OUT is 1 kHz, so haptics poll at 1 ms.
    pub(super) fn sc2_active(&self) -> bool {
        self.steamctrl2.is_some() || self.steamctrl2_puck.is_some()
    }

    pub(super) fn pump(
        &mut self,
        rumble: &mut impl FnMut(u16, u16, u16, u16, u16),
        hidout: &mut impl FnMut(punktfunk_core::quic::HidOutput),
    ) {
        if let Some(m) = &mut self.xboxone {
            m.pump_rumble(&mut *rumble);
        }
        if let Some(m) = &mut self.dualsense {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.dualsense_edge {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.dualshock4 {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.steamdeck {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.switchpro {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.steamctrl {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.steamctrl2_puck {
            m.pump(&mut *rumble, &mut *hidout);
        }
        if let Some(m) = &mut self.steamctrl2 {
            m.pump(&mut *rumble, &mut *hidout);
        }
    }

    /// Re-emit HID reports so kernel/SDL do not drop a held-steady UHID pad.
    pub(super) fn heartbeat(&mut self) {
        let gap = std::time::Duration::from_millis(8);
        if let Some(m) = &mut self.dualsense {
            m.heartbeat(gap);
        }
        if let Some(m) = &mut self.dualsense_edge {
            m.heartbeat(gap);
        }
        if let Some(m) = &mut self.dualshock4 {
            m.heartbeat(gap);
        }
        if let Some(m) = &mut self.steamdeck {
            m.heartbeat(gap);
        }
        if let Some(m) = &mut self.switchpro {
            m.heartbeat(gap);
        }
        if let Some(m) = &mut self.steamctrl {
            m.heartbeat(gap);
        }
        if let Some(m) = &mut self.steamctrl2 {
            m.heartbeat(gap);
        }
    }
}
