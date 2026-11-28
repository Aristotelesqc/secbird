//! Device and profile handling for the tray, kept free of Win32 code.

use secbird_core::buttons::{self, Input, Layer};
use secbird_core::profile::DeviceProfiles;
use secbird_core::{config_dir, discover, hidapi::HidApi, Db, Device, Dimming, Result, Store};

/// The profile button's setting key in profiles.json.
const PROFILE_BUTTON: &str = "profile";
use crate::studio::Studio;
use std::cell::{Cell, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime};

pub struct Engine {
    api: HidApi,
    devices: Vec<Device>,
    unsupported: Vec<Unsupported>,
    /// Synapse processes seen at the last rescan.
    synapse: Vec<String>,
    /// Devices a rescan took out of the driver mode Synapse left them in, not yet applied.
    released: Vec<String>,
    /// The program in front, when some profile is tied to it.
    foreground: Option<String>,
    /// Profiles picked with the mouse's profile button while a program with its own profile
    /// was in front, by device key: they win over that program's profile until another
    /// program comes to the front. Never saved.
    manual: RefCell<HashMap<String, String>>,
    /// The tray holds the profile button's hotkey, so it may take profile buttons.
    profile_hotkey: Cell<bool>,
    /// Programs tied to any profile, and the profiles.json timestamp they were read at.
    apps: HashSet<String>,
    apps_read: Option<SystemTime>,
    /// Latest DPI each device reported through its event reports.
    live_dpi: HashMap<String, (u16, u16)>,
    live_seq: u64,
    /// No per-key refresh until then, while the editor's light finder is open.
    frames_paused_until: Option<Instant>,
    /// Brightness percent while the PC is locked or its screen is off ([`Engine::lights_away`]).
    away: Option<u8>,
    /// Devices that didn't answer [`Engine::lights_back`], and until when to keep trying.
    relight: HashMap<String, Instant>,
    /// Devices that didn't answer [`Engine::lights_away`], and until when to keep trying.
    dim: HashMap<String, Instant>,
    /// Layered lighting streamed while the tray runs. In a cell because applying a profile
    /// (`&self`) has to tell it to start its stream over.
    studio: RefCell<Studio>,
    /// Grows each time a rescan finds other devices than the one before it.
    list_seq: u64,
    /// Settings were just sent from here (a profile, the editor): a DPI report the device
    /// sends back until then is that, not its button, and gets no popup.
    dpi_quiet_until: Cell<Option<Instant>>,
}

/// How long after settings were sent a DPI report from the device is taken as its answer.
const DPI_ECHO: Duration = Duration::from_millis(1500);

/// What the DPI popup says about the device ([`Engine::dpi_popup`]).
pub struct DpiPopup {
    pub device: String,
    /// The device's highest DPI, when known.
    pub max: Option<u16>,
    /// The stage this DPI is, of how many, when it is one of the device's stages.
    pub stage: Option<(usize, usize)>,
}

const FINDER_PAUSE: Duration = Duration::from_secs(10 * 60);
/// A wireless keyboard can miss the "back" write and answer ~20 s later (#52).
pub const RELIGHT_FOR: Duration = Duration::from_secs(2 * 60);
/// A device can miss the "away" write too (a Basilisk V4 HyperSpeed often does, right as the
/// screen goes off) and then stays lit the whole time: try again for a while.
pub const DIM_FOR: Duration = Duration::from_secs(30);

/// Battery state of one wireless device.
pub struct Battery {
    pub key: String,
    pub name: String,
    pub percent: u8,
    pub charging: bool,
}

/// A connected Razer product that can't be controlled (yet), shown so it isn't silently missing.
/// Why a wireless device we know is quiet, most often: it sleeps until a keypress (#37).
const ASLEEP: &str = "Not answering. Asleep? Press a key or move it";
const BLUETOOTH: &str = "Connected over Bluetooth and not answering. Use the cable or the USB receiver";

pub struct Unsupported {
    pub pid: u16,
    pub name: String,
    pub reason: &'static str,
    /// What the database says it is: the kind of input that shows it woke up.
    pub kind: Option<secbird_core::Kind>,
}

/// What a retry of [`Engine::lights_away`] or [`Engine::lights_back`] did, by device key.
pub struct LightsRetry {
    /// Took the write this time.
    pub done: Vec<String>,
    /// Still not answering when the time ran out: no more tries.
    pub gave_up: Vec<String>,
}

/// What happened when a device's active profile was sent to it.
pub struct ApplyReport {
    pub device: String,
    pub profile: String,
    pub failures: Vec<String>,
}

impl Engine {
    /// The text of `occam-rzr report`, for the tray's "Make a device report" item. Only reads.
    pub fn report_text(&mut self) -> String {
        let _ = self.api.refresh_devices();
        let db = Db::load(Some(&config_dir().join("devices.json"))).unwrap_or_default();
        secbird_core::report::build(&self.api, &db).to_string()
    }

    pub fn new() -> Result<Self> {
        Ok(Self {
            api: HidApi::new()?,
            devices: Vec::new(),
            unsupported: Vec::new(),
            synapse: Vec::new(),
            released: Vec::new(),
            foreground: None,
            manual: RefCell::new(HashMap::new()),
            profile_hotkey: Cell::new(false),
            apps: HashSet::new(),
            apps_read: None,
            live_dpi: HashMap::new(),
            live_seq: 0,
            frames_paused_until: None,
            away: None,
            relight: HashMap::new(),
            dim: HashMap::new(),
            studio: RefCell::new(Studio::default()),
            list_seq: 0,
            dpi_quiet_until: Cell::new(None),
        })
    }

    pub fn foreground(&self) -> Option<&str> {
        self.foreground.as_deref()
    }

    /// The profile a device uses right now: one picked with its profile button while this
    /// program is in front, else the one tied to the program in front, else the active one.
    fn current(&self, p: &DeviceProfiles, dev: &Device) -> String {
        match self.manual.borrow().get(&dev.key()).filter(|n| p.profiles.contains_key(*n)) {
            Some(name) => name.clone(),
            None => p.effective(self.foreground()).to_string(),
        }
    }

    /// The tray holds the profile button's hotkey ([`Engine::set_profile_hotkey`]). Until it
    /// does (the editor alone with `--serve`, or another program holds the combo), profile
    /// buttons keep their own setting.
    pub fn set_profile_hotkey(&self, held: bool) {
        self.profile_hotkey.set(held);
    }

    /// The tray handles the profile button of a mouse whose profile `name` leaves that button
    /// alone: it sends "next Occam profile" instead of cycling on-board slots.
    fn takes_profile_button(&self, p: &DeviceProfiles, name: &str, dev: &Device) -> bool {
        let set_here = p.profiles.get(name).is_some_and(|pr| pr.buttons.contains_key(PROFILE_BUTTON));
        self.profile_hotkey.get() && !set_here && dev.has_profile_button()
    }

    /// The mouse's profile button to "next Occam profile", when profile `name`
    /// leaves it alone. `None` when the tray doesn't take that button.
    fn send_profile_button(&self, p: &DeviceProfiles, name: &str, dev: &Device) -> Option<Result<bool>> {
        self.takes_profile_button(p, name, dev).then(|| {
            dev.set_input(Input::Button(buttons::PROFILE_BUTTON_ID), Layer::Normal, buttons::PROFILE_NEXT)
        })
    }

    /// The editor put buttons back to what the mouse stores (a reset, "Restore"): the
    /// profile button goes back to "next Occam profile" when the tray takes it.
    pub fn retake_profile_button(&self, dev: &Device) -> Option<Result<bool>> {
        let mut store = Self::store().ok()?;
        let p = store.for_device(dev);
        let name = self.current(p, dev);
        self.send_profile_button(p, &name, dev)
    }

    /// The profile button was pressed (the tray got its hotkey): every mouse whose profile
    /// button sends it moves to its next profile, in the order of the tray menu. With a
    /// program in front that has its own profile, the pick holds until another program
    /// comes to the front, and the active profile stays; otherwise it becomes the active
    /// profile, as a pick in the tray menu does. Returns what was applied.
    pub fn next_profile(&self) -> Vec<ApplyReport> {
        let Ok(mut store) = Self::store() else { return Vec::new() };
        let mut mice: Vec<&Device> = self.devices.iter().filter(|d| d.profile_button_is_next()).collect();
        if mice.is_empty() {
            // Another button sends it: every mouse.
            mice = self.devices.iter().filter(|d| d.spec.as_ref().is_some_and(|s| s.kind == secbird_core::Kind::Mouse)).collect();
        }
        let mut keys = Vec::new();
        let mut saved = false;
        for dev in mice {
            let p = store.for_device(dev);
            let now = self.current(p, dev);
            let names: Vec<&String> = p.profiles.keys().collect();
            let at = names.iter().position(|n| **n == now).unwrap_or(0);
            let next = names[(at + 1) % names.len().max(1)].clone();
            let app_profile = self.foreground().and_then(|exe| p.for_app(exe)).is_some();
            if app_profile {
                self.manual.borrow_mut().insert(dev.key(), next);
            } else {
                self.manual.borrow_mut().remove(&dev.key());
                if p.switch(&next).is_ok() {
                    saved = true;
                }
            }
            keys.push(dev.key());
        }
        if saved {
            let _ = store.save();
        }
        self.apply(&keys)
    }

    /// Exit: profile buttons the tray took go back to what the mouse stores, so they don't
    /// send a combo nobody listens to.
    pub fn release_profile_buttons(&self) {
        for dev in self.devices.iter().filter(|d| d.profile_button_is_next()) {
            let _ = dev.reset_input(Input::Button(buttons::PROFILE_BUTTON_ID), Layer::Normal);
        }
    }

    /// Collections that carry a device's own event reports (DPI and profile changes made
    /// with its buttons), as (device key, path). On the Basilisk V3 that is interface 1,
    /// collection 5 (usage 0001:0000, 16-byte reports); its sibling with the same usage
    /// stays silent, so listening to both costs nothing.
    pub fn event_paths(&self) -> Vec<(String, std::ffi::CString)> {
        if secbird_core::sim::only() {
            return Vec::new();
        }
        self.devices
            .iter()
            .flat_map(|dev| {
                self.api
                    .device_list()
                    .filter(move |i| {
                        i.vendor_id() == secbird_core::db::RAZER_VID
                            && i.product_id() == dev.pid
                            && i.usage_page() == 0x0001
                            && i.usage() == 0x0000
                    })
                    .map(move |i| (dev.key(), i.path().to_owned()))
            })
            .collect()
    }

    pub fn open_events(&self, path: &std::ffi::CStr) -> Option<secbird_core::hidapi::HidDevice> {
        self.api.open_path(path).ok()
    }

    /// The device changed DPI by itself (its DPI button). Record it in the profile in
    /// use, so a replug or wake keeps it instead of going back.
    pub fn dpi_changed(&mut self, key: &str, x: u16, y: u16) {
        self.live_seq += 1;
        self.live_dpi.insert(key.to_string(), (x, y));
        let Some(dev) = self.devices.iter().find(|d| d.key() == key) else { return };
        if let Ok(mut store) = Self::store() {
            let p = store.for_device(dev);
            let name = self.current(p, dev);
            if let Some(profile) = p.profiles.get_mut(&name) {
                profile.dpi = Some((x, y));
                let _ = store.save();
            }
        }
    }

    /// Settings were just sent to devices from here: see [`Engine::dpi_quiet`].
    pub fn hush_dpi(&self) {
        self.dpi_quiet_until.set(Some(Instant::now() + DPI_ECHO));
    }

    /// Settings were sent from here a moment ago (a profile applied, a change in the editor):
    /// a DPI report now is the device answering them, not its DPI button, so no popup.
    pub fn dpi_quiet(&self) -> bool {
        self.dpi_quiet_until.get().is_some_and(|t| Instant::now() < t)
    }

    /// For the DPI popup: the device's name, its highest DPI, and which of its stages `(x, y)`
    /// is (one short read of the stages, on devices with a DPI range).
    pub fn dpi_popup(&self, key: &str, x: u16, y: u16) -> DpiPopup {
        let Some(dev) = self.devices.iter().find(|d| d.key() == key) else {
            return DpiPopup { device: "Mouse".into(), max: None, stage: None };
        };
        let max = dev.spec.as_ref().and_then(|s| s.dpi.as_ref()).map(|r| r.max);
        let stage = max.and_then(|_| dev.dpi_stages().ok()).and_then(|s| {
            // The active stage when it is this DPI, else the first stage that is.
            let active = (s.active as usize).checked_sub(1).filter(|&i| s.stages.get(i) == Some(&(x, y)));
            let at = active.or_else(|| s.stages.iter().position(|&st| st == (x, y)))?;
            Some((at + 1, s.stages.len()))
        });
        DpiPopup { device: self.label(dev), max, stage }
    }

    /// For the on-board profile popup: the device's name and how many on-board profiles it
    /// has (0 when unknown).
    pub fn onboard_popup(&self, key: &str) -> (String, u8) {
        match self.devices.iter().find(|d| d.key() == key) {
            Some(dev) => (self.label(dev), dev.spec.as_ref().map_or(0, |s| s.onboard_profiles)),
            None => ("Mouse".into(), 0),
        }
    }

    /// Latest DPI reported by each device, and a counter that grows with every report,
    /// so the editor follows new reports without undoing a DPI just set in the editor.
    pub fn live_dpi(&self) -> (u64, &HashMap<String, (u16, u16)>) {
        (self.live_seq, &self.live_dpi)
    }

    /// Called when another window comes to the front, with its exe name. Devices whose
    /// profile depends on the program in front get that profile; nothing is sent when
    /// neither the old nor the new program is tied to a profile.
    pub fn set_foreground(&mut self, exe: Option<String>) -> Vec<ApplyReport> {
        self.refresh_apps();
        let exe = exe.map(|e| secbird_core::profile::normalize_app(&e)).filter(|e| self.apps.contains(e));
        if exe == self.foreground {
            return Vec::new();
        }
        let Ok(mut store) = Self::store() else {
            self.foreground = exe;
            self.manual.borrow_mut().clear();
            return Vec::new();
        };
        // What each device used before (a profile-button pick included), then the new program.
        let before: Vec<String> = self.devices.iter().map(|d| self.current(store.for_device(d), d)).collect();
        self.foreground = exe;
        self.manual.borrow_mut().clear();
        let changed: Vec<String> = self
            .devices
            .iter()
            .zip(before)
            .filter(|(d, before)| *before != self.current(store.for_device(d), d))
            .map(|(d, _)| d.key())
            .collect();
        self.apply(&changed)
    }

    /// Re-read the tied programs only when profiles.json has changed: a foreground switch
    /// then costs one file-time lookup.
    fn refresh_apps(&mut self) {
        let stamp = std::fs::metadata(config_dir().join("profiles.json")).and_then(|m| m.modified()).ok();
        if stamp != self.apps_read || stamp.is_none() {
            self.apps = Self::store().map(|s| s.apps()).unwrap_or_default();
            self.apps_read = stamp;
        }
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn unsupported(&self) -> &[Unsupported] {
        &self.unsupported
    }

    pub fn synapse(&self) -> &[String] {
        &self.synapse
    }

    /// Devices taken out of driver mode since the last call: their remaps need sending again.
    pub fn take_released(&mut self) -> Vec<String> {
        std::mem::take(&mut self.released)
    }

    /// Battery of every wireless device that answers.
    pub fn batteries(&self) -> Vec<Battery> {
        self.devices
            .iter()
            .filter_map(|d| {
                let percent = d.battery().ok().flatten()?;
                Some(Battery { key: d.key(), name: d.name(), percent, charging: d.charging().unwrap_or(false) })
            })
            .collect()
    }

    /// Zones showing a wireless mouse's battery (a dock's light, #39) follow its level.
    /// Failures are left out: a mouse off its dock or asleep doesn't answer, and the
    /// colour stays until the next check.
    pub fn refresh_battery_lights(&self) {
        let Ok(mut store) = Self::store() else { return };
        for dev in self.devices.iter().filter(|d| d.spec.as_ref().is_some_and(|s| s.wireless)) {
            let p = store.for_device(dev);
            let name = self.current(p, dev);
            p.refresh_battery_lights(&name, dev);
        }
    }

    /// Per-key colors of wired keyboards and keypads, sent again. Leaving quick adjustment
    /// mode (#45) leaves them dark, and the keyboard doesn't say when; Synapse streams
    /// frames all the time, so this does the same at a slow pace. Wireless boards are left
    /// alone: the traffic would keep them awake. Failures are left out, as for batteries.
    /// Not while away: the lights are down then anyway.
    pub fn refresh_frames(&self) {
        if self.away.is_some() || self.frames_paused_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Ok(mut store) = Self::store() else { return };
        let wired_boards = self.devices.iter().filter(|d| {
            d.matrix().is_some()
                && d.spec.as_ref().is_some_and(|s| {
                    !s.wireless && matches!(s.kind, secbird_core::Kind::Keyboard | secbird_core::Kind::Keypad)
                })
        });
        let studio = self.studio.borrow();
        for dev in wired_boards.filter(|d| !studio.is_streaming(&d.key())) {
            let p = store.for_device(dev);
            let name = self.current(p, dev);
            p.refresh_frame(&name, dev);
        }
    }

    /// Laptops set to "stays lit" still fade by themselves (#34), but any read restarts
    /// that idle fade, so read the setting back now and then. Only while the tray runs.
    pub fn keep_laptops_lit(&self) {
        if self.away.is_some() {
            return;
        }
        let Ok(mut store) = Self::store() else { return };
        let laptops = self.devices.iter().filter(|d| d.spec.as_ref().is_some_and(|s| s.kind == secbird_core::Kind::Laptop));
        for dev in laptops {
            let p = store.for_device(dev);
            let name = self.current(p, dev);
            let stays_lit = Dimming::StaysLit.to_string();
            if p.profiles.get(&name).is_some_and(|p| p.dimming.as_deref() == Some(stays_lit.as_str())) {
                let _ = dev.dimming();
            }
        }
    }

    /// The PC was locked or its screen went off (#48): every zone this app controls goes
    /// to `percent` of its saved brightness, with writes that aren't saved, so a replug or
    /// power cycle brings the lights back. Also call it after a rescan while away, for
    /// devices that just arrived. Devices with the older lighting commands are left lit.
    /// Returns the keys of the devices that missed it: [`Engine::retry_lights_away`] tries
    /// them again for [`DIM_FOR`].
    pub fn lights_away(&mut self, percent: u8) -> Vec<String> {
        self.away = Some(percent);
        self.relight.clear();
        let missed = self.each_zone(|dev, led| dim_zone(dev, led, percent));
        let until = Instant::now() + DIM_FOR;
        self.dim = missed.iter().map(|key| (key.clone(), until)).collect();
        missed
    }

    /// [`Engine::lights_away`] again for the devices that missed it, while still away, until
    /// they answer or [`DIM_FOR`] runs out.
    pub fn retry_lights_away(&mut self) -> LightsRetry {
        let gave_up = expire(&mut self.dim, Instant::now());
        let Some(percent) = self.away.filter(|_| !self.dim.is_empty()) else {
            return LightsRetry { done: Vec::new(), gave_up };
        };
        let missed = self.each_zone(|dev, led| {
            if !self.dim.contains_key(&dev.key()) {
                return Ok(());
            }
            dim_zone(dev, led, percent)
        });
        let done: Vec<String> = self.dim.keys().filter(|k| !missed.contains(k) && self.has(k)).cloned().collect();
        for key in &done {
            self.dim.remove(key);
        }
        LightsRetry { done, gave_up }
    }

    /// Back from [`Engine::lights_away`]: the saved brightness again, read from the device
    /// (it answers the saved level, not the dimmed one), so it works after a tray restart too.
    /// Then the profiles, in case one changed while away. Returns the keys of the devices
    /// that missed it: [`Engine::retry_lights_back`] tries them again for [`RELIGHT_FOR`].
    pub fn lights_back(&mut self) -> Vec<String> {
        if self.away.take().is_none() {
            return Vec::new();
        }
        self.dim.clear();
        let missed = self.each_zone(|dev, led| dev.set_brightness_live(led, dev.brightness(led)?));
        let until = Instant::now() + RELIGHT_FOR;
        self.relight = missed.iter().map(|key| (key.clone(), until)).collect();
        self.apply(&self.all_keys());
        missed
    }

    /// [`Engine::lights_back`] again for the devices that missed it, until they answer or
    /// [`RELIGHT_FOR`] runs out. Those that answer get their profile again.
    pub fn retry_lights_back(&mut self) -> LightsRetry {
        let gave_up = expire(&mut self.relight, Instant::now());
        if self.relight.is_empty() || self.away.is_some() {
            return LightsRetry { done: Vec::new(), gave_up };
        }
        let missed = self.each_zone(|dev, led| {
            if !self.relight.contains_key(&dev.key()) {
                return Ok(());
            }
            dev.set_brightness_live(led, dev.brightness(led)?)
        });
        let done: Vec<String> = self.relight.keys().filter(|k| !missed.contains(k) && self.has(k)).cloned().collect();
        for key in &done {
            self.relight.remove(key);
        }
        if !done.is_empty() {
            self.apply(&done);
        }
        LightsRetry { done, gave_up }
    }

    /// Whether the device with this key is in the list.
    fn has(&self, key: &str) -> bool {
        self.devices.iter().any(|d| d.key() == key)
    }

    /// Device names for the log, without serial numbers.
    pub fn names(&self, keys: &[String]) -> String {
        let names: Vec<String> = keys
            .iter()
            .map(|key| match self.devices.iter().find(|d| d.key() == *key) {
                Some(dev) => self.label(dev),
                None => key.split(':').next().unwrap_or_default().to_string(),
            })
            .collect();
        names.join(", ")
    }

    pub fn is_away(&self) -> bool {
        self.away.is_some()
    }

    /// Run `f` on each lighting zone of each device whose lighting this app controls.
    /// Returns the keys of the devices where `f` failed: an asleep wireless device doesn't answer.
    fn each_zone(&self, f: impl Fn(&Device, u8) -> Result<()>) -> Vec<String> {
        let Ok(mut store) = Self::store() else { return Vec::new() };
        let mut failed = Vec::new();
        for dev in &self.devices {
            if store.for_device(dev).lighting_off {
                continue;
            }
            let leds: Vec<u8> = dev.spec.iter().flat_map(|s| s.leds.iter().map(|l| l.id)).collect();
            if leds.into_iter().map(|led| f(dev, led)).filter(Result::is_err).count() > 0 {
                failed.push(dev.key());
            }
        }
        failed
    }

    /// The editor's light finder shows one light at a time; don't paint over it. A finder
    /// left open (a closed browser never says) stops pausing after a while.
    pub fn pause_frames(&mut self, pause: bool) {
        self.frames_paused_until = pause.then(|| Instant::now() + FINDER_PAUSE);
    }

    /// Re-enumerate devices. Returns the keys of devices that were not present before.
    pub fn rescan(&mut self) -> Vec<String> {
        let before: HashSet<String> = self.devices.iter().map(|d| d.key()).collect();
        let listed_before = self.listed();
        self.devices.clear(); // close stale handles before reopening
        let _ = self.api.refresh_devices();
        let db = Db::load(Some(&config_dir().join("devices.json")))
            .or_else(|_| Db::builtin())
            .unwrap_or_default();
        self.devices = discover(&self.api, &db);
        self.unsupported = secbird_core::device::unanswered(&self.api, &self.devices)
            .into_iter()
            .map(|pid| {
                let spec = db.get(pid);
                let (name, reason) = match spec {
                    Some(spec) if secbird_core::headset::is_headset(pid) => {
                        (spec.name.clone(), "Not answering. Is it switched on? Is Synapse running?")
                    }
                    Some(spec) if spec.lighting == secbird_core::Lighting::Kraken => (spec.name.clone(), "Headset protocol not supported yet"),
                    Some(spec) if spec.wireless => (spec.name.clone(), ASLEEP),
                    Some(spec) => (spec.name.clone(), "Not answering. Is Synapse running?"),
                    None => match db.bluetooth_name(pid) {
                        Some(name) => (name, BLUETOOTH),
                        None => (format!("Razer device {pid:04X}"), "Unknown model. Run occam-rzr report"),
                    },
                };
                Unsupported { pid, name, reason, kind: spec.map(|s| s.kind) }
            })
            .collect();
        self.synapse = secbird_core::debloat::synapse_running();
        // Synapse puts devices in driver mode and leaves them there when it stops; a mouse
        // in driver mode ignores its own remaps. Only when Synapse isn't running, so the two
        // never fight over the mode. A device that doesn't answer the mode is left alone.
        if self.synapse.is_empty() {
            for dev in &self.devices {
                if dev.leave_driver_mode().unwrap_or(false) && !self.released.contains(&dev.key()) {
                    self.released.push(dev.key());
                }
            }
        }
        if self.listed() != listed_before {
            self.list_seq += 1;
        }
        self.devices.iter().map(|d| d.key()).filter(|k| !before.contains(k)).collect()
    }

    /// Everything the editor lists, by name: the devices that answer, then the ones that
    /// don't. A receiver keeps its key when it learns which mouse it carries, but not its name.
    fn listed(&self) -> Vec<String> {
        let answering = self.devices.iter().map(|d| format!("+{} {}", d.key(), d.name()));
        let silent = self.unsupported.iter().map(|u| format!("-{:04X} {} {}", u.pid, u.name, u.reason));
        answering.chain(silent).collect()
    }

    /// A counter that grows when a rescan changes that list, so an open editor shows a
    /// device the tray found by itself (one that was asleep) without a click on Rescan.
    pub fn list_seq(&self) -> u64 {
        self.list_seq
    }

    /// Known wireless devices that are plugged in (their receiver is) but not answering yet.
    pub fn sleeping(&self) -> impl Iterator<Item = &Unsupported> {
        self.unsupported.iter().filter(|u| u.reason == ASLEEP)
    }

    pub fn has_sleeping(&self) -> bool {
        self.sleeping().next().is_some()
    }

    /// Read profiles fresh from disk, so changes made with the CLI are picked up.
    fn store() -> Result<Store> {
        Store::load(&config_dir().join("profiles.json"))
    }

    /// Profile names for a device and which one is active.
    pub fn profiles(dev: &Device) -> (Vec<String>, String) {
        match Self::store() {
            Ok(mut store) => {
                let p = store.for_device(dev);
                (p.profiles.keys().cloned().collect(), p.active.clone())
            }
            Err(_) => (Vec::new(), String::new()),
        }
    }

    /// The profile a device shows right now (see [`Engine::set_foreground`]).
    pub fn effective_profile(&self, dev: &Device) -> String {
        Self::store().map(|mut s| self.current(s.for_device(dev), dev)).unwrap_or_default()
    }

    /// Apply the current profile of each listed device (by [`Device::key`]): the one tied
    /// to the program in front, else the active one.
    pub fn apply(&self, keys: &[String]) -> Vec<ApplyReport> {
        self.studio.borrow_mut().restart(keys);
        if !keys.is_empty() {
            self.hush_dpi();
        }
        let mut store = Self::store();
        self.devices
            .iter()
            .filter(|d| keys.contains(&d.key()))
            .map(|dev| match &mut store {
                Ok(store) => {
                    let p = store.for_device(dev);
                    // Lights stay down while away (a lock screen in front switches profiles);
                    // this copy of the store is never saved.
                    p.lighting_off |= self.away.is_some();
                    let name = self.current(p, dev);
                    let mut failures: Vec<String> = p
                        .apply(&name, dev)
                        .into_iter()
                        .filter_map(|a| a.result.err().map(|e| format!("{}: {e}", a.setting)))
                        .collect();
                    if let Some(Err(e)) = self.send_profile_button(p, &name, dev) {
                        failures.push(format!("profile button: {e}"));
                    }
                    ApplyReport { device: dev.name(), profile: name, failures }
                }
                Err(e) => ApplyReport {
                    device: dev.name(),
                    profile: String::new(),
                    failures: vec![format!("profiles.json: {e}")],
                },
            })
            .collect()
    }

    /// Wireless mice that dropped their button remaps while asleep (#45) get them back.
    /// `skip` holds keys re-sent moments ago, left alone so a mouse that keeps losing
    /// them isn't written over and over. Returns the keys re-sent, with their reports.
    pub fn restore_lost_buttons(&self, skip: &HashSet<String>) -> Vec<(String, ApplyReport)> {
        let Ok(mut store) = Self::store() else { return Vec::new() };
        self.devices
            .iter()
            .filter(|d| d.spec.as_ref().is_some_and(|s| s.wireless && s.kind == secbird_core::Kind::Mouse))
            .filter(|d| !skip.contains(&d.key()))
            .filter_map(|dev| {
                let p = store.for_device(dev);
                let name = self.current(p, dev);
                let button_lost = self.takes_profile_button(p, &name, dev) && !dev.profile_button_is_next();
                if !p.buttons_lost(&name, dev) && !button_lost {
                    return None;
                }
                let mut failures: Vec<String> = p
                    .apply_buttons(&name, dev)
                    .into_iter()
                    .filter_map(|a| a.result.err().map(|e| format!("{}: {e}", a.setting)))
                    .collect();
                if let Some(Err(e)) = self.send_profile_button(p, &name, dev) {
                    failures.push(format!("profile button: {e}"));
                }
                Some((dev.key(), ApplyReport { device: self.label(dev), profile: name, failures }))
            })
            .collect()
    }

    pub fn all_keys(&self) -> Vec<String> {
        self.devices.iter().map(|d| d.key()).collect()
    }

    /// Make `name` the active profile for a device and apply it.
    /// Every mouse's remapped buttons back to what the mouse stores. Returns (device, count).
    pub fn restore_buttons(&self) -> Vec<(String, Result<usize>)> {
        let mut store = match Self::store() {
            Ok(s) => s,
            Err(e) => return vec![("profiles".into(), Err(e))],
        };
        self.devices
            .iter()
            .filter(|d| d.spec.as_ref().is_some_and(|s| s.kind == secbird_core::Kind::Mouse))
            .map(|d| (self.label(d), secbird_core::actions::restore_all_buttons(d, &mut store)))
            .collect()
    }

    pub fn switch_profile(&self, key: &str, name: &str) -> Result<Vec<ApplyReport>> {
        let dev = self
            .devices
            .iter()
            .find(|d| d.key() == key)
            .ok_or_else(|| secbird_core::Error::NotFound(format!("device {key}")))?;
        let mut store = Self::store()?;
        store.for_device(dev).switch(name)?;
        store.save()?;
        // A pick in the menu or the editor ends a profile-button pick.
        self.manual.borrow_mut().remove(key);
        Ok(self.apply(&[key.to_string()]))
    }

    pub fn studio(&self) -> RefMut<'_, Studio> {
        self.studio.borrow_mut()
    }

    /// One frame of layered lighting for every device that has some ([`Studio::tick`]).
    /// Devices whose scene just ended get their profile again, so they show their own
    /// lighting; returned for the failure notes.
    pub fn studio_tick(&self) -> Vec<ApplyReport> {
        let paused = self.frames_paused_until.is_some_and(|t| Instant::now() < t);
        let path = config_dir().join("profiles.json");
        let ended = {
            let mut studio = self.studio.borrow_mut();
            studio.tick(&self.devices, self.foreground(), &self.manual.borrow(), &path, self.away.is_some(), paused);
            for key in std::mem::take(&mut studio.gave_up) {
                secbird_core::log::append(&format!("lighting: {} stopped taking frames, trying again every 5 s", self.names(&[key])));
            }
            std::mem::take(&mut studio.ended)
        };
        self.lights_after_stream(&ended)
    }

    /// Devices that stopped streaming: their saved effect shown again, then the profile on
    /// top, so none stays on the last frame.
    fn lights_after_stream(&self, keys: &[String]) -> Vec<ApplyReport> {
        if keys.is_empty() {
            return Vec::new();
        }
        for dev in self.devices.iter().filter(|d| keys.contains(&d.key())) {
            dev.show_saved_lighting();
        }
        self.apply(keys)
    }

    /// Exit: every device that streams gets its own lighting back, so none freezes on the
    /// last frame.
    pub fn studio_stop(&self) {
        let keys = self.studio.borrow_mut().stop_all();
        self.lights_after_stream(&keys);
    }

    /// Menu/tooltip label: the name, plus a serial tail when several units share it.
    pub fn label(&self, dev: &Device) -> String {
        let twins = self.devices.iter().filter(|d| d.pid == dev.pid).count();
        match dev.serial.len() {
            n if twins > 1 && n > 0 => format!("{} ({})", dev.name(), &dev.serial[n.saturating_sub(4)..]),
            _ => dev.name(),
        }
    }
}

/// One zone to `percent` of its saved brightness, not saved on the device.
fn dim_zone(dev: &Device, led: u8, percent: u8) -> Result<()> {
    let saved = dev.brightness(led)?;
    dev.set_brightness_live(led, (saved as u32 * percent as u32 / 100) as u8)
}

/// Drop the entries whose time ran out, and return their keys.
fn expire(until: &mut HashMap<String, Instant>, now: Instant) -> Vec<String> {
    let mut gone: Vec<String> = until.iter().filter(|(_, t)| now >= **t).map(|(key, _)| key.clone()).collect();
    gone.sort();
    for key in &gone {
        until.remove(key);
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::expire;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    #[test]
    fn retries_run_out_one_device_at_a_time() {
        let now = Instant::now();
        let mut until = HashMap::from([("00EA:A".to_string(), now + Duration::from_secs(30)), ("02C9:B".to_string(), now)]);
        assert_eq!(expire(&mut until, now), ["02C9:B"]);
        assert_eq!(until.len(), 1);
        assert!(expire(&mut until, now + Duration::from_secs(29)).is_empty());
        assert_eq!(expire(&mut until, now + Duration::from_secs(30)), ["00EA:A"]);
        assert!(until.is_empty());
    }
}
