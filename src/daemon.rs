//! One task owns all state. Every event updates the facts, `model::plan`
//! decides what should be held, and `converge` makes the system match. Each
//! step compares what is held against the plan, so running it twice, or after
//! a crash, ends in the same place.

use crate::model::{self, Config, EndReason, Facts, Lid, LidSetting, Plan, Power, SaverCause, Session};
use crate::store::{Journal, Loaded, SavedBacklight, Store};
use crate::system::{self, System, audio, backlight};
use crate::tray::{self, View};
use futures_util::StreamExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};
use zbus::zvariant::OwnedFd;

pub const BUS_NAME: &str = "app.getinsomnia.Insomnia";
pub const OBJECT_PATH: &str = "/app/getinsomnia/Insomnia";
const LID_DEBOUNCE: Duration = Duration::from_secs(2);
const RETRY_UNDO: Duration = Duration::from_secs(30);

pub enum Command {
    Start(u64),
    Extend(u64),
    End,
    Status,
    Quit,
    OpenPrompt,
    SetLidSetting(LidSetting, bool),
    StopWaitingFor(String),
    SimulateLid(Option<Lid>),
}

pub type Reply = Result<String, String>;

pub enum Event {
    Command(Command, Option<oneshot::Sender<Reply>>),
    Lid,
    Power,
    PowerServiceChanged,
    ProfilesServiceChanged,
    LoginServiceChanged,
}

struct Service {
    events: mpsc::Sender<Event>,
}

#[zbus::interface(name = "app.getinsomnia.Insomnia1")]
impl Service {
    async fn start(&self, seconds: u64) -> zbus::fdo::Result<String> {
        self.ask(Command::Start(seconds)).await
    }
    async fn extend(&self, seconds: u64) -> zbus::fdo::Result<String> {
        self.ask(Command::Extend(seconds)).await
    }
    async fn end(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::End).await
    }
    async fn status(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Status).await
    }
    async fn quit(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Quit).await
    }
    /// Only answers when the daemon was started with INSOMNIA_ALLOW_LID_SIMULATION=1.
    /// "closed", "docked", "open", or "real" to follow the hardware again.
    async fn simulate_lid(&self, state: &str) -> zbus::fdo::Result<String> {
        let lid = match state {
            "closed" => Some(Lid::Closed),
            "docked" => Some(Lid::ClosedDocked),
            "open" => Some(Lid::Open),
            "real" => None,
            other => return Err(zbus::fdo::Error::InvalidArgs(format!("expected closed, docked, open or real, not {other:?}"))),
        };
        self.ask(Command::SimulateLid(lid)).await
    }
}

impl Service {
    async fn ask(&self, command: Command) -> zbus::fdo::Result<String> {
        let (tx, rx) = oneshot::channel();
        let gone = || zbus::fdo::Error::Failed("Insomnia is shutting down".into());
        self.events.send(Event::Command(command, Some(tx))).await.map_err(|_| gone())?;
        rx.await.map_err(|_| gone())?.map_err(zbus::fdo::Error::Failed)
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

struct Daemon {
    store: Store,
    config: Config,
    sys: System,
    session: Option<Session>,
    journal: Journal,
    lid: Lid,
    lid_override: Option<Lid>,
    allow_lid_simulation: bool,
    power: Power,
    power_failures: u32,
    inhibitor: Option<OwnedFd>,
    inhibit_error: Option<String>,
    saver_cookie: Option<u32>,
    saver_error: Option<String>,
    reminded_for: Option<u64>,
    saver: Reversible,
    mute: Reversible,
    darken: Reversible,
    notes: Vec<String>,
}

pub async fn run() -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel(32);
    let session_bus = zbus::connection::Builder::session()
        .and_then(|b| b.name(BUS_NAME))
        .and_then(|b| b.serve_at(OBJECT_PATH, Service { events: tx.clone() }))
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| match e {
            zbus::Error::NameTaken => "Insomnia is already running".to_string(),
            e => format!("could not claim {BUS_NAME} on the session bus: {e}"),
        })?;
    let sys = System::connect(&session_bus).await.map_err(|e| format!("could not reach a system service: {e}"))?;

    forward(sys.login1.receive_lid_closed_changed().await, tx.clone(), || Event::Lid);
    forward(sys.upower.receive_on_battery_changed().await, tx.clone(), || Event::Power);
    forward(sys.device.receive_percentage_changed().await, tx.clone(), || Event::Power);
    forward(sys.device.receive_is_present_changed().await, tx.clone(), || Event::Power);
    forward(sys.upower.inner().receive_owner_changed().await.map_err(|e| e.to_string())?, tx.clone(), || Event::PowerServiceChanged);
    forward(sys.profiles.inner().receive_owner_changed().await.map_err(|e| e.to_string())?, tx.clone(), || Event::ProfilesServiceChanged);
    forward(sys.login1.inner().receive_owner_changed().await.map_err(|e| e.to_string())?, tx.clone(), || Event::LoginServiceChanged);

    let store = Store::from_env();
    let mut d = Daemon {
        lid: sys.lid().await.unwrap_or(Lid::Open),
        config: Config::default(),
        store,
        sys,
        session: None,
        journal: Journal::default(),
        lid_override: None,
        allow_lid_simulation: std::env::var_os("INSOMNIA_ALLOW_LID_SIMULATION").is_some_and(|v| v == "1"),
        power: Power::NoBattery,
        power_failures: 0,
        inhibitor: None,
        inhibit_error: None,
        saver_cookie: None,
        saver_error: None,
        reminded_for: None,
        saver: Reversible::default(),
        mute: Reversible::default(),
        darken: Reversible::default(),
        notes: vec![],
    };
    d.reload_config();
    d.read_power().await;
    d.recover().await;

    let tray = tray::spawn(tx.clone(), d.view()).await;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|e| e.to_string())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).map_err(|e| e.to_string())?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).map_err(|e| e.to_string())?;
    let mut lid_settles_at: Option<Instant> = None;
    let mut shown = d.view();
    let mut cadence = None;
    let mut next_wake = None;

    loop {
        d.converge().await;
        let view = d.view();
        if let Some(tray) = &tray
            && view != shown
        {
            shown = view.clone();
            tray.update(move |t| t.set_view(view)).await;
        }
        let wake = d.wake_interval();
        if wake != cadence {
            cadence = wake;
            next_wake = wake.map(|duration| Instant::now() + duration);
        }
        tokio::select! {
            event = rx.recv() => match event {
                Some(Event::Command(Command::Quit, reply)) => {
                    d.shutdown().await;
                    if let Some(reply) = reply { let _ = reply.send(Ok("Insomnia quit.".into())); }
                    break;
                }
                Some(Event::Command(command, reply)) => {
                    let answer = d.handle(command).await;
                    if let Some(reply) = reply {
                        let _ = reply.send(answer);
                    } else if let Err(e) = answer {
                        d.sys.notify("Insomnia could not complete the action", &e).await;
                        d.note(e);
                    }
                }
                Some(Event::Lid) => lid_settles_at = Some(Instant::now() + LID_DEBOUNCE),
                Some(Event::Power) => d.read_power().await,
                Some(Event::PowerServiceChanged) => d.read_power().await,
                Some(Event::ProfilesServiceChanged) => {
                    d.saver_cookie = None;
                    d.saver = Reversible::default();
                    d.saver_error = None;
                }
                Some(Event::LoginServiceChanged) => d.inhibitor = None,
                None => break,
            },
            _ = sleep_until(lid_settles_at.unwrap_or_else(Instant::now)), if lid_settles_at.is_some() => {
                lid_settles_at = None;
                match d.sys.lid().await {
                    Ok(lid) => d.lid = lid,
                    Err(e) => eprintln!("insomnia: could not read the lid: {e}"),
                }
            }
            _ = sleep_until(next_wake.unwrap_or_else(Instant::now)), if next_wake.is_some() => {
                next_wake = wake.map(|duration| Instant::now() + duration);
                if d.power_failures > 0 { d.read_power().await; }
                if d.session.is_some() && lid_settles_at.is_none() {
                    match d.sys.lid().await {
                        Ok(lid) => d.lid = lid,
                        Err(e) => d.note(format!("Could not read the lid: {e}")),
                    }
                }
            }
            _ = terminate.recv() => { d.shutdown().await; break; }
            _ = interrupt.recv() => { d.shutdown().await; break; }
            _ = hangup.recv() => d.reload_config(),
        }
    }
    if let Some(tray) = tray {
        tray.shutdown().await;
    }
    Ok(())
}

fn forward<S: futures_util::Stream + Send + Unpin + 'static>(mut stream: S, tx: mpsc::Sender<Event>, event: fn() -> Event) {
    tokio::spawn(async move {
        while stream.next().await.is_some() {
            if tx.send(event()).await.is_err() {
                break;
            }
        }
    });
}

impl Daemon {
    /// Runs once at launch. An unexpired session resumes; an expired or
    /// unreadable one ends. Converging afterwards restores any sink a previous
    /// run muted and did not get to unmute.
    async fn recover(&mut self) {
        match self.store.load_journal() {
            Loaded::Ok(j) => self.journal = j,
            Loaded::Missing => {}
            Loaded::MovedAside(to) => self.notes.push(format!(
                "The lid-close journal could not be read and was moved to {}. Check that your outputs are not left muted and the screen is not left dark.",
                to.display()
            )),
        }
        match self.store.load_session() {
            Loaded::Ok(s) if s.ends_at > now() => self.session = Some(s),
            Loaded::Ok(_) => {
                let _ = self.store.delete_session();
                self.sys.notify("Insomnia session ended", "It ran out while Insomnia was not running.").await;
            }
            Loaded::Missing => {}
            Loaded::MovedAside(to) => {
                let line = format!("The session file could not be read, so the session was treated as ended. It was moved to {}.", to.display());
                self.sys.notify("Insomnia session ended", &line).await;
                self.notes.push(line);
            }
        }
    }

    fn facts(&self) -> Facts<'_> {
        Facts {
            now: now(),
            session: self.session,
            lid: self.lid_override.unwrap_or(self.lid),
            power: self.power,
            config: &self.config,
        }
    }

    async fn read_power(&mut self) {
        match self.sys.power().await {
            Ok(power) => {
                self.power = power;
                self.power_failures = 0;
            }
            Err(e) => {
                self.power_failures += 1;
                eprintln!("insomnia: could not read the battery ({} in a row): {e}", self.power_failures);
                if self.power_failures >= 2 {
                    self.power = Power::Unreadable;
                }
            }
        }
    }

    fn wake_interval(&self) -> Option<Duration> {
        if self.session.is_some() {
            Some(Duration::from_secs(1))
        } else if self.journal != Journal::default() || self.saver_cookie.is_some() || self.power_failures > 0 {
            Some(Duration::from_secs(30))
        } else {
            None
        }
    }

    async fn converge(&mut self) {
        let plan = loop {
            let plan = model::plan(&self.facts());
            match plan.end {
                Some(reason) => self.finish(reason).await,
                None => break plan,
            }
        };
        self.converge_inhibitor(&plan).await;
        self.converge_saver(&plan).await;
        match self.mute.step(plan.mute, !self.journal.muted_sinks.is_empty()) {
            Step::Apply => self.mute_unmuted_sinks().await,
            Step::Undo => self.unmute_journaled_sinks().await,
            Step::Wait => {}
        }
        match self.darken.step(plan.darken, self.journal.backlight.is_some()) {
            Step::Apply => self.darken_panel().await,
            Step::Undo => self.restore_panel().await,
            Step::Wait => {}
        }
        if let Some(s) = self.session
            && plan.remind
            && self.reminded_for != Some(s.ends_at)
        {
            self.reminded_for = Some(s.ends_at);
            let body = format!("The session ends in {}. Add time from the tray to keep going.", model::format_remaining(s.remaining(now())));
            self.sys.notify("Insomnia ends soon", &body).await;
        }
    }

    async fn converge_inhibitor(&mut self, plan: &Plan) {
        match (plan.keep_awake, self.inhibitor.is_some()) {
            (true, false) => self.take_inhibitor().await,
            (false, true) => self.inhibitor = None,
            _ => {}
        }
    }

    /// Takes a fresh lock before dropping the old one, so the reason shown by
    /// `systemd-inhibit --list` names the current end time without a gap.
    async fn take_inhibitor(&mut self) {
        let Some(s) = self.session else { return };
        let why = format!("Timed awake session until {}", system::local_clock(s.ends_at, now()));
        match self.sys.inhibit_sleep(&why).await {
            Ok(fd) => {
                self.inhibitor = Some(fd);
                self.inhibit_error = None;
            }
            Err(e) => self.inhibit_error = Some(format!("Could not block sleep: {e}")),
        }
    }

    /// Tried once per cause, so a system without power-profiles-daemon
    /// shows one error instead of calling it every second.
    async fn converge_saver(&mut self, plan: &Plan) {
        match self.saver.step(!plan.saver_causes.is_empty(), self.saver_cookie.is_some()) {
            Step::Apply => {
                let reason = plan.saver_causes.iter().map(|c| saver_cause_text(*c)).collect::<Vec<_>>().join(", ");
                match self.sys.hold_power_saver(&reason).await {
                    Ok(cookie) => {
                        self.saver_cookie = Some(cookie);
                        self.saver_error = None;
                        if plan.saver_causes.contains(&SaverCause::BatteryFloor) {
                            self.sys
                                .notify("Power saver on", &format!("Battery is below {}%.", self.config.low_power_floor))
                                .await;
                        }
                    }
                    Err(e) => self.saver_error = Some(format!("Could not switch to power saver: {e}")),
                }
            }
            Step::Undo => {
                self.saver_error = None;
                if let Some(cookie) = self.saver_cookie {
                    match self.sys.release_power_saver(cookie).await {
                        Ok(()) => self.saver_cookie = None,
                        Err(e) => self.saver_error = Some(format!("Could not release power saver: {e}")),
                    }
                }
            }
            Step::Wait => {}
        }
    }

    /// The journal is written before each change and after each undo, so a
    /// crash at any point leaves a journal that over-reports, never one that
    /// forgets something this app changed. Sinks the user had muted are left
    /// out, so they stay muted after the lid opens.
    async fn mute_unmuted_sinks(&mut self) {
        let sinks = match audio::sinks().await {
            Ok(s) => s,
            Err(e) => return self.note(e),
        };
        let mut unmuted = Vec::new();
        for sink in sinks {
            match audio::is_muted(&sink).await {
                Ok(false) if !self.journal.muted_sinks.contains(&sink) => unmuted.push(sink),
                Ok(_) => {}
                Err(e) => self.note(e),
            }
        }
        if unmuted.is_empty() {
            return;
        }
        let mut next = self.journal.clone();
        next.muted_sinks.extend(unmuted.iter().cloned());
        if let Err(e) = self.store.save_journal(&next) {
            return self.note(format!("Did not mute: the audio journal could not be written ({e})"));
        }
        self.journal = next;
        for sink in unmuted {
            if let Err(e) = audio::set_muted(&sink, true).await {
                self.note(e);
            }
        }
    }

    /// A sink that is not connected keeps its entry, so reconnecting it
    /// later still unmutes it.
    async fn unmute_journaled_sinks(&mut self) {
        let present = audio::sinks().await.unwrap_or_default();
        for sink in self.journal.muted_sinks.clone() {
            if !present.contains(&sink) {
                continue;
            }
            match audio::set_muted(&sink, false).await {
                Ok(()) => self.journal.muted_sinks.retain(|s| *s != sink),
                Err(e) => self.note(e),
            }
        }
        self.save_journal_or_note();
    }

    /// A panel already at 0 is left out of the journal, like a muted sink.
    async fn darken_panel(&mut self) {
        let Some(device) = backlight::device() else { return };
        let level = match backlight::level(&device) {
            Ok(0) => return,
            Ok(level) => level,
            Err(e) => return self.note(e),
        };
        let next = Journal { backlight: Some(SavedBacklight { device: device.clone(), level }), ..self.journal.clone() };
        if let Err(e) = self.store.save_journal(&next) {
            return self.note(format!("Did not turn the screen off: the lid-close journal could not be written ({e})"));
        }
        self.journal = next;
        if let Err(e) = self.sys.set_backlight(&device, 0).await {
            self.note(format!("Could not turn the screen off: {e}"));
        }
    }

    /// Puts the saved level back only while the panel is still at 0. A level
    /// someone set by hand meanwhile stays.
    async fn restore_panel(&mut self) {
        let Some(saved) = self.journal.backlight.clone() else { return };
        match backlight::level(&saved.device) {
            Ok(0) => match self.sys.set_backlight(&saved.device, saved.level).await {
                Ok(()) => self.journal.backlight = None,
                Err(e) => return self.note(format!("Could not turn the screen back on: {e}. Use the brightness keys.")),
            },
            Ok(_) => self.journal.backlight = None,
            Err(e) => return self.note(e),
        }
        self.save_journal_or_note();
    }

    fn note(&mut self, line: String) {
        eprintln!("insomnia: {line}");
        if !self.notes.contains(&line) {
            self.notes.push(line);
        }
    }

    fn save_journal_or_note(&mut self) {
        if let Err(e) = self.store.save_journal(&self.journal) {
            self.note(format!("The lid-close journal could not be written: {e}"));
        }
    }

    async fn finish(&mut self, reason: EndReason) {
        self.session = None;
        self.reminded_for = None;
        if let Err(e) = self.store.delete_session() {
            self.notes.push(format!("The session file could not be removed: {e}"));
        }
        self.sys.notify("Insomnia session ended", &reason.describe()).await;
    }

    async fn shutdown(&mut self) {
        if self.session.is_some() {
            self.finish(EndReason::Requested).await;
        }
        self.converge().await;
    }

    async fn handle(&mut self, command: Command) -> Result<String, String> {
        match command {
            Command::Start(seconds) => {
                let seconds = self.config.requested_seconds(seconds);
                if let Some(s) = self.session {
                    return Err(format!("A session is already running until {}. Use extend to add time.", self.until(s)));
                }
                let session = Session::starting(now(), seconds);
                self.store.save_session(&session).map_err(|e| format!("Could not write the session file: {e}"))?;
                self.session = Some(session);
                self.take_inhibitor().await;
                if self.inhibitor.is_none() {
                    self.session = None;
                    let _ = self.store.delete_session();
                    return Err(self.inhibit_error.clone().unwrap_or_else(|| "Could not block sleep".into()));
                }
                Ok(format!("Keeping awake until {}.", self.until(session)))
            }
            Command::Extend(seconds) => {
                let seconds = self.config.requested_seconds(seconds);
                let s = self.session.ok_or("No session is running.")?.extended(now(), seconds);
                self.store.save_session(&s).map_err(|e| format!("Could not write the session file: {e}"))?;
                self.session = Some(s);
                self.take_inhibitor().await;
                Ok(format!("Keeping awake until {}.", self.until(s)))
            }
            Command::End => {
                self.session.ok_or("No session is running.")?;
                self.finish(EndReason::Requested).await;
                Ok("Session ended.".into())
            }
            Command::Status => Ok(self.status_text()),
            Command::OpenPrompt => {
                let mut prompt = tokio::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
                prompt.arg("prompt");
                if self.session.is_some() {
                    prompt.arg("--extend");
                }
                prompt.spawn().map_err(|e| format!("Could not open the time entry: {e}"))?;
                Ok(String::new())
            }
            Command::SetLidSetting(setting, on) => self.set_config(|c| c.set_lid_setting(setting, on)),
            Command::StopWaitingFor(sink) => {
                self.journal.muted_sinks.retain(|s| *s != sink);
                self.save_journal_or_note();
                Ok(format!("Stopped waiting for {sink}."))
            }
            Command::SimulateLid(lid) => {
                if !self.allow_lid_simulation {
                    return Err("Lid simulation is off. Start the daemon with INSOMNIA_ALLOW_LID_SIMULATION=1.".into());
                }
                self.lid_override = lid;
                Ok(format!("Lid simulation: {lid:?}"))
            }
            Command::Quit => unreachable!("the event loop handles quit"),
        }
    }

    /// At launch and on SIGHUP (`systemctl --user reload insomnia`), so a
    /// hand edit to config.json applies without ending the session.
    fn reload_config(&mut self) {
        let (config, notes) = self.store.load_config();
        for note in notes {
            eprintln!("insomnia: config: {note}");
        }
        self.config = config;
    }

    fn set_config(&mut self, change: impl FnOnce(&mut Config)) -> Result<String, String> {
        let mut next = self.config.clone();
        change(&mut next);
        self.store.save_config(&next).map_err(|e| format!("Could not write config.json: {e}"))?;
        self.config = next;
        Ok("Saved.".into())
    }

    fn until(&self, s: Session) -> String {
        system::local_clock(s.ends_at, now())
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let lid = match self.lid_override.unwrap_or(self.lid) {
            Lid::Open => "Lid open",
            Lid::Closed => "Lid closed",
            Lid::ClosedDocked => "Lid closed, docked: audio and power profile left alone",
        };
        lines.push(format!("{lid}{}", if self.lid_override.is_some() { " (simulated)" } else { "" }));
        lines.push(match self.power {
            Power::NoBattery => "No battery".into(),
            Power::Battery { on_battery, percent } => {
                format!("Battery {percent:.0}%, {}", if on_battery { "discharging" } else { "on AC power" })
            }
            Power::Unreadable => "Battery level unreadable".into(),
        });
        if self.saver_cookie.is_some() {
            lines.push("Power saver held".into());
        }
        if !self.journal.muted_sinks.is_empty() {
            lines.push(format!("Muted {}", self.journal.muted_sinks.join(", ")));
        }
        if let Some(saved) = &self.journal.backlight {
            lines.push(format!("Screen off, saved at level {}", saved.level));
        }
        lines.extend(self.inhibit_error.iter().cloned());
        lines.extend(self.saver_error.iter().cloned());
        lines.extend(self.notes.iter().cloned());
        lines
    }

    fn status_text(&self) -> String {
        let head = match self.session {
            Some(s) => format!(
                "Awake until {} ({} left). Sleep blocked: {}.",
                self.until(s),
                model::format_remaining(s.remaining(now())),
                if self.inhibitor.is_some() { "yes" } else { "no" }
            ),
            None => "No session.".into(),
        };
        std::iter::once(head).chain(self.lines()).collect::<Vec<_>>().join("\n")
    }

    fn view(&self) -> View {
        let muting = model::plan(&self.facts()).mute;
        View {
            remaining: self.session.map(|s| model::format_remaining(s.remaining(now()))),
            until: self.session.map(|s| self.until(s)),
            lines: self.lines(),
            waiting_sinks: if muting { vec![] } else { self.journal.muted_sinks.clone() },
            presets: self.config.presets_minutes.clone(),
            lid_settings: LidSetting::ALL.map(|s| (s, self.config.lid_setting(s))),
        }
    }
}

fn saver_cause_text(cause: SaverCause) -> &'static str {
    match cause {
        SaverCause::BatteryFloor => "battery below the low-power floor",
        SaverCause::LidClosed => "lid closed during an Insomnia session",
    }
}

/// One reversible change: muting, darkening, or the power-saver hold. It is
/// applied once each time the plan asks for it, so one that changed nothing
/// or failed does not try again every tick. Its undo runs
/// when the plan drops it, and again every RETRY_UNDO while the journal
/// still holds an entry, such as a sink that is unplugged.
#[derive(Default)]
struct Reversible {
    applied: bool,
    retry_at: Option<Instant>,
}

#[derive(Debug, PartialEq)]
enum Step {
    Apply,
    Undo,
    Wait,
}

impl Reversible {
    fn step(&mut self, want: bool, journaled: bool) -> Step {
        let now = Instant::now();
        if want {
            self.retry_at = None;
            return if std::mem::replace(&mut self.applied, true) { Step::Wait } else { Step::Apply };
        }
        self.applied = false;
        if !journaled || self.retry_at.is_some_and(|at| now < at) {
            return Step::Wait;
        }
        self.retry_at = Some(now + RETRY_UNDO);
        Step::Undo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_reversible_action_applies_once_per_close_and_retries_its_undo() {
        let mut r = Reversible::default();
        assert_eq!(r.step(false, true), Step::Undo, "a journal left by a crash is undone at launch");
        assert_eq!(r.step(false, true), Step::Wait);
        tokio::time::advance(RETRY_UNDO).await;
        assert_eq!(r.step(false, true), Step::Undo, "an entry that stays is retried");
        assert_eq!(r.step(true, true), Step::Apply);
        assert_eq!(r.step(true, false), Step::Wait, "a close that changed nothing does not try again");
        assert_eq!(r.step(false, true), Step::Undo, "an open undoes at once");
        assert_eq!(r.step(false, false), Step::Wait);
        assert_eq!(r.step(true, false), Step::Apply, "the next close applies again");
    }
}
