//! Adapters for the services Insomnia drives. Everything here talks to the
//! outside world and returns domain values; no policy lives here.

use crate::model::{Lid, Power};
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use zbus::zvariant::{OwnedFd, Value};

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
pub trait Login1 {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;
    #[zbus(property)]
    fn lid_closed(&self) -> zbus::Result<bool>;
    #[zbus(property(emits_changed_signal = "false"))]
    fn docked(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.User",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/user/self"
)]
pub trait User {
    #[zbus(property)]
    fn display(&self) -> zbus::Result<(String, zbus::zvariant::OwnedObjectPath)>;
}

#[zbus::proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1")]
pub trait Session {
    fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.UPower", default_service = "org.freedesktop.UPower", default_path = "/org/freedesktop/UPower")]
pub trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice"
)]
pub trait DisplayDevice {
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.PowerProfiles",
    default_service = "org.freedesktop.UPower.PowerProfiles",
    default_path = "/org/freedesktop/UPower/PowerProfiles"
)]
pub trait PowerProfiles {
    fn hold_profile(&self, profile: &str, reason: &str, application_id: &str) -> zbus::Result<u32>;
    fn release_profile(&self, cookie: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
pub trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

pub const APP_ID: &str = "app.getinsomnia.Insomnia";
const DISPLAY_DEVICE_BATTERY: u32 = 2;

pub struct System {
    pub login1: Login1Proxy<'static>,
    pub upower: UPowerProxy<'static>,
    pub device: DisplayDeviceProxy<'static>,
    pub profiles: PowerProfilesProxy<'static>,
    notifications: NotificationsProxy<'static>,
    user: UserProxy<'static>,
    system_bus: zbus::Connection,
}

impl System {
    pub async fn connect(session_bus: &zbus::Connection) -> zbus::Result<Self> {
        let system_bus = zbus::Connection::system().await?;
        Ok(System {
            login1: Login1Proxy::new(&system_bus).await?,
            upower: UPowerProxy::new(&system_bus).await?,
            device: DisplayDeviceProxy::new(&system_bus).await?,
            profiles: PowerProfilesProxy::new(&system_bus).await?,
            notifications: NotificationsProxy::new(session_bus).await?,
            user: UserProxy::new(&system_bus).await?,
            system_bus,
        })
    }

    /// Blocks suspend from any source and logind's own lid handling. The lock
    /// lasts as long as the returned descriptor stays open, so it also ends
    /// when this process dies.
    pub async fn inhibit_sleep(&self, why: &str) -> zbus::Result<OwnedFd> {
        self.login1.inhibit("sleep:handle-lid-switch", "Insomnia", why, "block").await
    }

    pub async fn lid(&self) -> zbus::Result<Lid> {
        Ok(match (self.login1.lid_closed().await?, self.login1.docked().await?) {
            (false, _) => Lid::Open,
            (true, false) => Lid::Closed,
            (true, true) => Lid::ClosedDocked,
        })
    }

    /// Goes through the graphical session, since a user service is in no
    /// session of its own and logind lets a session's owner set its backlight.
    pub async fn set_backlight(&self, device: &str, level: u32) -> zbus::Result<()> {
        let (_, path) = self.user.display().await?;
        SessionProxy::builder(&self.system_bus).path(path)?.build().await?.set_brightness("backlight", device, level).await
    }

    /// Without UPower, a machine with no battery in sysfs is a desktop and
    /// has no floors. A laptop without UPower reads as an error.
    pub async fn power(&self) -> zbus::Result<Power> {
        match self.upower_power().await {
            Err(zbus::Error::MethodError(name, ..)) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" && !has_battery() => {
                Ok(Power::NoBattery)
            }
            other => other,
        }
    }

    async fn upower_power(&self) -> zbus::Result<Power> {
        let device = DisplayDeviceProxy::builder(&self.system_bus).cache_properties(zbus::proxy::CacheProperties::No).build().await?;
        let upower = UPowerProxy::builder(&self.system_bus).cache_properties(zbus::proxy::CacheProperties::No).build().await?;
        if !device.is_present().await? || device.kind().await? != DISPLAY_DEVICE_BATTERY {
            return Ok(Power::NoBattery);
        }
        Ok(Power::Battery { on_battery: upower.on_battery().await?, percent: device.percentage().await? })
    }

    /// power-profiles-daemon drops the hold if this connection closes.
    pub async fn hold_power_saver(&self, reason: &str) -> zbus::Result<u32> {
        self.profiles.hold_profile("power-saver", reason, APP_ID).await
    }

    pub async fn release_power_saver(&self, cookie: u32) -> zbus::Result<()> {
        self.profiles.release_profile(cookie).await
    }

    pub async fn notify(&self, summary: &str, body: &str) {
        let hints = HashMap::from([("desktop-entry", Value::from("insomnia"))]);
        if let Err(e) = self.notifications.notify("Insomnia", 0, "", summary, body, &[], hints, -1).await {
            eprintln!("insomnia: notification failed: {e}");
        }
    }
}

fn has_battery() -> bool {
    std::fs::read_dir("/sys/class/power_supply").into_iter().flatten().flatten().any(|e| {
        std::fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "Battery")
    })
}

/// The built-in panel's backlight. Reads come from sysfs; writes go through
/// logind, which needs no root.
pub mod backlight {
    use std::fs;
    use std::path::Path;

    const ROOT: &str = "/sys/class/backlight";

    /// The device a desktop's brightness keys would pick: firmware (ACPI)
    /// over platform over raw, as gnome-settings-daemon and powerdevil do.
    pub fn device() -> Option<String> {
        let rank = |kind: &str| ["firmware", "platform", "raw"].iter().position(|k| *k == kind);
        fs::read_dir(ROOT)
            .ok()?
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let kind = fs::read_to_string(Path::new(ROOT).join(&name).join("type")).ok()?;
                Some((rank(kind.trim())?, name))
            })
            .min()
            .map(|(_, name)| name)
    }

    pub fn level(device: &str) -> Result<u32, String> {
        let path = Path::new(ROOT).join(device).join("brightness");
        let text = fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        text.trim().parse().map_err(|_| format!("{} holds {:?}, not a number", path.display(), text.trim()))
    }
}

/// Output sinks through pipewire-pulse. Names stay stable across reconnects,
/// so a journaled name still finds its device after a restart.
pub mod audio {
    use super::*;

    async fn pactl(args: &[&str]) -> Result<String, String> {
        let run = Command::new("pactl").args(args).kill_on_drop(true).output();
        let out = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .map_err(|_| format!("pactl {} timed out", args.join(" ")))?
            .map_err(|e| format!("could not run pactl: {e}"))?;
        if !out.status.success() {
            return Err(format!("pactl {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub async fn sinks() -> Result<Vec<String>, String> {
        Ok(pactl(&["list", "short", "sinks"]).await?.lines().filter_map(|l| l.split('\t').nth(1)).map(str::to_owned).collect())
    }

    pub async fn is_muted(sink: &str) -> Result<bool, String> {
        let out = pactl(&["get-sink-mute", sink]).await?;
        match out.trim() {
            "Mute: yes" => Ok(true),
            "Mute: no" => Ok(false),
            other => Err(format!("unexpected pactl answer for {sink}: {other:?}")),
        }
    }

    pub async fn set_muted(sink: &str, muted: bool) -> Result<(), String> {
        pactl(&["set-sink-mute", sink, if muted { "1" } else { "0" }]).await.map(drop)
    }
}

/// "18:30" for today, "Tue 18:30" within a week, "Oct 21 18:30" beyond.
pub fn local_clock(unix: u64, now: u64) -> String {
    fn tm(t: u64) -> libc::tm {
        let t = t as libc::time_t;
        // SAFETY: localtime_r only writes the zeroed struct we pass it.
        unsafe {
            let mut out: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut out);
            out
        }
    }
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (at, today) = (tm(unix), tm(now));
    let clock = format!("{:02}:{:02}", at.tm_hour, at.tm_min);
    if (at.tm_year, at.tm_yday) == (today.tm_year, today.tm_yday) {
        clock
    } else if unix < now + 6 * 86_400 {
        format!("{} {clock}", DAYS[at.tm_wday as usize])
    } else {
        format!("{} {} {clock}", MONTHS[at.tm_mon as usize], at.tm_mday)
    }
}
