//! The session domain with no I/O. `plan` turns what the daemon observes into
//! what it should hold; the daemon only converges the system onto that plan.

use serde::{Deserialize, Serialize};

pub const MAX_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const REMIND_BEFORE_END: u64 = 5 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub started_at: u64,
    pub ends_at: u64,
}

impl Session {
    pub fn starting(now: u64, seconds: u64) -> Self {
        Session { started_at: now, ends_at: now + seconds.min(MAX_SECONDS) }
    }

    /// Adds time to whatever is left, never past 30 days from now.
    pub fn extended(self, now: u64, seconds: u64) -> Self {
        let ends_at = (self.ends_at.max(now) + seconds).min(now + MAX_SECONDS);
        Session { ends_at, ..self }
    }

    pub fn remaining(&self, now: u64) -> u64 {
        self.ends_at.saturating_sub(now)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub presets_minutes: Vec<u64>,
    pub default_minutes: u64,
    pub mute_on_lid_close: bool,
    pub power_saver_on_lid_close: bool,
    pub low_power_floor: u8,
    pub end_floor: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            presets_minutes: vec![30, 60, 120, 240, 480],
            default_minutes: 60,
            mute_on_lid_close: true,
            power_saver_on_lid_close: true,
            low_power_floor: 40,
            end_floor: 10,
        }
    }
}

impl Config {
    /// What a start or extend asked for. Zero means the default preset, as
    /// when the prompt is submitted with every field empty.
    pub fn requested_seconds(&self, seconds: u64) -> u64 {
        match seconds {
            0 => self.default_minutes * 60,
            s => s.min(MAX_SECONDS),
        }
    }

    /// Repairs a hand-edited file into a valid config. Returns what it changed
    /// so the caller can log it and write the file back.
    pub fn normalized(mut self) -> (Self, Vec<String>) {
        let mut fixes = Vec::new();
        let max_minutes = MAX_SECONDS / 60;
        let before = self.presets_minutes.len();
        self.presets_minutes.retain(|m| (1..=max_minutes).contains(m));
        if self.presets_minutes.len() != before {
            fixes.push(format!("dropped presets outside 1..={max_minutes} minutes"));
        }
        if self.presets_minutes.is_empty() {
            self.presets_minutes = Config::default().presets_minutes;
            fixes.push("restored the default presets".into());
        }
        if !(1..=max_minutes).contains(&self.default_minutes) {
            self.default_minutes = Config::default().default_minutes;
            fixes.push("reset defaultMinutes to 60".into());
        }
        if self.low_power_floor > 100 {
            self.low_power_floor = 100;
            fixes.push("capped lowPowerFloor at 100".into());
        }
        if self.end_floor > 95 {
            self.end_floor = 95;
            fixes.push("capped endFloor at 95".into());
        }
        if self.end_floor > 0 && self.low_power_floor <= self.end_floor {
            self.low_power_floor = (self.end_floor + 5).min(100);
            fixes.push(format!("raised lowPowerFloor to {} to stay above endFloor", self.low_power_floor));
        }
        (self, fixes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Power {
    /// No internal battery: no floor applies.
    NoBattery,
    Battery { on_battery: bool, percent: f64 },
    /// UPower failed twice in a row. Treated as draining with no readable level.
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Deadline,
    Requested,
    BatteryLow(u8),
    BatteryUnreadable,
}

impl EndReason {
    pub fn describe(self) -> String {
        match self {
            EndReason::Deadline => "The timer ran out.".into(),
            EndReason::Requested => "Ended early.".into(),
            EndReason::BatteryLow(floor) => format!("Battery fell below {floor}%."),
            EndReason::BatteryUnreadable => "The battery level could not be read while on battery.".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaverCause {
    BatteryFloor,
    LidClosed,
}

pub struct Facts<'a> {
    pub now: u64,
    pub session: Option<Session>,
    pub lid_closed: bool,
    pub power: Power,
    pub config: &'a Config,
}

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub keep_awake: bool,
    pub saver_causes: Vec<SaverCause>,
    pub mute: bool,
    pub remind: bool,
    pub end: Option<EndReason>,
}

pub fn plan(f: &Facts) -> Plan {
    let Some(session) = f.session else { return Plan::default() };
    let c = f.config;
    let end = if f.now >= session.ends_at {
        Some(EndReason::Deadline)
    } else {
        match f.power {
            Power::Battery { on_battery: true, percent } if c.end_floor > 0 && percent < c.end_floor as f64 => {
                Some(EndReason::BatteryLow(c.end_floor))
            }
            Power::Unreadable if c.end_floor > 0 => Some(EndReason::BatteryUnreadable),
            _ => None,
        }
    };
    if end.is_some() {
        return Plan { end, ..Plan::default() };
    }
    let mut saver_causes = Vec::new();
    if let Power::Battery { on_battery: true, percent } = f.power
        && percent < c.low_power_floor as f64
    {
        saver_causes.push(SaverCause::BatteryFloor);
    }
    if f.lid_closed && c.power_saver_on_lid_close {
        saver_causes.push(SaverCause::LidClosed);
    }
    Plan {
        keep_awake: true,
        saver_causes,
        mute: f.lid_closed && c.mute_on_lid_close,
        remind: session.remaining(f.now) <= REMIND_BEFORE_END,
        end: None,
    }
}

/// Parses "90", "45m", "2h30m", "1d 4h". A bare number is minutes.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if text.is_empty() {
        return Err("empty duration".into());
    }
    if let Ok(minutes) = text.parse::<u64>() {
        return checked_seconds(minutes.checked_mul(60));
    }
    let mut total: Option<u64> = Some(0);
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit = match ch {
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => return Err(format!("unknown unit '{ch}' in {text:?}")),
        };
        let n: u64 = digits.parse().map_err(|_| format!("missing number before '{ch}' in {text:?}"))?;
        digits.clear();
        total = total.and_then(|t| n.checked_mul(unit).and_then(|s| t.checked_add(s)));
    }
    if !digits.is_empty() {
        return Err(format!("missing unit after {digits} in {text:?}"));
    }
    checked_seconds(total)
}

pub fn seconds_from_fields(days: u64, hours: u64, minutes: u64) -> Result<u64, String> {
    checked_seconds(
        days.checked_mul(86_400)
            .zip(hours.checked_mul(3_600))
            .zip(minutes.checked_mul(60))
            .and_then(|((d, h), m)| d.checked_add(h)?.checked_add(m)),
    )
}

fn checked_seconds(seconds: Option<u64>) -> Result<u64, String> {
    match seconds {
        Some(0) => Err("the duration must be longer than zero".into()),
        Some(s) if s <= MAX_SECONDS => Ok(s),
        _ => Err("the longest session is 30 days".into()),
    }
}

/// "2d 4h", "1h 05m", "12m", "45s": the two largest units.
pub fn format_remaining(seconds: u64) -> String {
    let (d, h, m) = (seconds / 86_400, seconds % 86_400 / 3_600, seconds % 3_600 / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m:02}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

pub fn format_preset(minutes: u64) -> String {
    match (minutes / 1440, minutes % 1440 / 60, minutes % 60) {
        (0, 0, m) => format!("{m} minutes"),
        (0, 1, 0) => "1 hour".into(),
        (0, h, 0) => format!("{h} hours"),
        (1, 0, 0) => "1 day".into(),
        (d, 0, 0) => format!("{d} days"),
        _ => format_remaining(minutes * 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(config: &Config, session: Option<Session>, lid_closed: bool, power: Power) -> Facts<'_> {
        Facts { now: 1_000, session, lid_closed, power, config }
    }

    const ACTIVE: Option<Session> = Some(Session { started_at: 0, ends_at: 5_000 });
    const DRAINING_AT: fn(f64) -> Power = |percent| Power::Battery { on_battery: true, percent };

    #[test]
    fn idle_holds_nothing_even_with_the_lid_closed() {
        let c = Config::default();
        assert_eq!(plan(&facts(&c, None, true, DRAINING_AT(5.0))), Plan::default());
    }

    #[test]
    fn open_lid_on_charger_only_keeps_awake() {
        let c = Config::default();
        let p = plan(&facts(&c, ACTIVE, false, Power::Battery { on_battery: false, percent: 20.0 }));
        assert_eq!(p, Plan { keep_awake: true, ..Plan::default() });
    }

    #[test]
    fn closed_lid_mutes_and_saves_power() {
        let c = Config::default();
        let p = plan(&facts(&c, ACTIVE, true, Power::NoBattery));
        assert_eq!(p, Plan { keep_awake: true, saver_causes: vec![SaverCause::LidClosed], mute: true, ..Plan::default() });
    }

    #[test]
    fn lid_actions_follow_their_switches() {
        let c = Config { mute_on_lid_close: false, power_saver_on_lid_close: false, ..Config::default() };
        let p = plan(&facts(&c, ACTIVE, true, Power::NoBattery));
        assert_eq!(p, Plan { keep_awake: true, ..Plan::default() });
    }

    #[test]
    fn battery_floors() {
        let c = Config::default();
        assert_eq!(plan(&facts(&c, ACTIVE, false, DRAINING_AT(40.0))).saver_causes, vec![]);
        assert_eq!(plan(&facts(&c, ACTIVE, false, DRAINING_AT(39.9))).saver_causes, vec![SaverCause::BatteryFloor]);
        assert_eq!(plan(&facts(&c, ACTIVE, false, DRAINING_AT(9.0))).end, Some(EndReason::BatteryLow(10)));
        let off = Config { end_floor: 0, ..Config::default() };
        assert_eq!(plan(&facts(&off, ACTIVE, false, DRAINING_AT(1.0))).end, None);
    }

    #[test]
    fn unreadable_battery_ends_only_when_the_end_floor_is_on() {
        let c = Config::default();
        assert_eq!(plan(&facts(&c, ACTIVE, false, Power::Unreadable)).end, Some(EndReason::BatteryUnreadable));
        let off = Config { end_floor: 0, ..Config::default() };
        assert_eq!(plan(&facts(&off, ACTIVE, false, Power::Unreadable)).end, None);
    }

    #[test]
    fn deadline_ends_and_releases_everything() {
        let c = Config::default();
        let expired = Some(Session { started_at: 0, ends_at: 1_000 });
        assert_eq!(plan(&facts(&c, expired, true, Power::NoBattery)), Plan { end: Some(EndReason::Deadline), ..Plan::default() });
    }

    #[test]
    fn reminds_in_the_last_five_minutes() {
        let c = Config::default();
        let soon = Some(Session { started_at: 0, ends_at: 1_300 });
        assert!(plan(&facts(&c, soon, false, Power::NoBattery)).remind);
        assert!(!plan(&facts(&c, ACTIVE, false, Power::NoBattery)).remind);
    }

    #[test]
    fn extension_adds_to_what_is_left_and_caps_at_thirty_days() {
        let s = Session::starting(100, 600);
        assert_eq!(s.extended(200, 600).ends_at, 1_300);
        assert_eq!(s.extended(10_000, 600).ends_at, 10_600);
        assert_eq!(s.extended(200, MAX_SECONDS).ends_at, 200 + MAX_SECONDS);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("90"), Ok(5_400));
        assert_eq!(parse_duration("2h30m"), Ok(9_000));
        assert_eq!(parse_duration("1d 4h"), Ok(100_800));
        assert_eq!(parse_duration("30d"), Ok(MAX_SECONDS));
        assert!(parse_duration("31d").is_err());
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("2x").is_err());
        assert!(parse_duration("h").is_err());
        assert!(parse_duration("5").is_ok_and(|s| s == 300));
        assert!(parse_duration("99999999999999999999d").is_err());
    }

    #[test]
    fn fields_combine() {
        assert_eq!(seconds_from_fields(1, 2, 3), Ok(93_780));
        assert!(seconds_from_fields(0, 0, 0).is_err());
        assert!(seconds_from_fields(30, 0, 1).is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(format_remaining(45), "45s");
        assert_eq!(format_remaining(12 * 60 + 5), "12m");
        assert_eq!(format_remaining(3_600 + 5 * 60), "1h 05m");
        assert_eq!(format_remaining(2 * 86_400 + 4 * 3_600 + 59), "2d 4h");
        assert_eq!(format_preset(30), "30 minutes");
        assert_eq!(format_preset(60), "1 hour");
        assert_eq!(format_preset(480), "8 hours");
        assert_eq!(format_preset(1440), "1 day");
        assert_eq!(format_preset(90), "1h 30m");
    }

    #[test]
    fn an_empty_request_means_the_default_preset() {
        let c = Config { default_minutes: 90, ..Config::default() };
        assert_eq!(c.requested_seconds(0), 5_400);
        assert_eq!(c.requested_seconds(120), 120);
        assert_eq!(c.requested_seconds(u64::MAX), MAX_SECONDS);
    }

    #[test]
    fn config_repairs() {
        let (c, fixes) = Config { end_floor: 50, low_power_floor: 40, ..Config::default() }.normalized();
        assert_eq!((c.low_power_floor, c.end_floor), (55, 50));
        assert_eq!(fixes.len(), 1);
        let (c, _) = Config { presets_minutes: vec![0, 99_999], default_minutes: 0, ..Config::default() }.normalized();
        assert_eq!(c.presets_minutes, Config::default().presets_minutes);
        assert_eq!(c.default_minutes, 60);
        assert!(Config::default().normalized().1.is_empty());
    }

    #[test]
    fn config_reads_partial_json_with_defaults() {
        let c: Config = serde_json::from_str(r#"{"endFloor": 0}"#).unwrap();
        assert_eq!(c, Config { end_floor: 0, ..Config::default() });
    }
}
