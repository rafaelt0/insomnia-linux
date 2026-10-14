use std::fs::File;
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::{sleep, timeout};
use zbus::{Connection, connection::Builder, zvariant::OwnedFd};

const LOGIN: &str = "org.freedesktop.login1";
const UPOWER: &str = "org.freedesktop.UPower";
const PROFILES: &str = "org.freedesktop.UPower.PowerProfiles";

struct Login(Arc<AtomicBool>);

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Login {
    fn inhibit(&self, _what: &str, _who: &str, _why: &str, _mode: &str) -> OwnedFd {
        std::os::fd::OwnedFd::from(File::open("/dev/null").unwrap()).into()
    }

    #[zbus(property)]
    fn lid_closed(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn docked(&self) -> bool {
        self.0.load(SeqCst)
    }
}

struct Battery;

#[zbus::interface(name = "org.freedesktop.UPower")]
impl Battery {
    #[zbus(property)]
    fn on_battery(&self) -> bool {
        true
    }
}

struct Device {
    broken: bool,
}

#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl Device {
    #[zbus(property)]
    fn is_present(&self) -> zbus::fdo::Result<bool> {
        if self.broken { Err(zbus::fdo::Error::Failed("battery read unavailable".into())) } else { Ok(true) }
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> u32 {
        2
    }

    #[zbus(property)]
    fn percentage(&self) -> f64 {
        80.0
    }
}

#[derive(Default)]
struct Holds {
    active: AtomicBool,
    reject_release: AtomicBool,
    releases: AtomicU32,
}

struct Profiles(Arc<Holds>);

#[zbus::interface(name = "org.freedesktop.UPower.PowerProfiles")]
impl Profiles {
    fn hold_profile(&self, profile: &str, _reason: &str, _application_id: &str) -> u32 {
        assert_eq!(profile, "power-saver");
        assert!(!self.0.active.swap(true, SeqCst), "duplicate hold");
        42
    }

    fn release_profile(&self, cookie: u32) -> zbus::fdo::Result<()> {
        assert_eq!(cookie, 42);
        self.0.releases.fetch_add(1, SeqCst);
        if self.0.reject_release.load(SeqCst) {
            Err(zbus::fdo::Error::Failed("temporary release failure".into()))
        } else {
            assert!(self.0.active.swap(false, SeqCst), "released an absent hold");
            Ok(())
        }
    }
}

struct Fixture {
    bus: Child,
    daemon: Option<Child>,
    dir: PathBuf,
    address: String,
}

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("insomnia-services-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut bus = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().unwrap();
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap()).read_line(&mut address).unwrap();
        Self { bus, daemon: None, dir, address: address.trim().into() }
    }

    fn spawn_daemon(&mut self) {
        std::fs::write(self.dir.join("config.json"), r#"{"muteOnLidClose":false,"darkenOnLidClose":false}"#).unwrap();
        self.daemon = Some(
            Command::new(env!("CARGO_BIN_EXE_insomnia"))
                .env("DBUS_SYSTEM_BUS_ADDRESS", &self.address)
                .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
                .env("INSOMNIA_HOME", &self.dir)
                .stdout(Stdio::null())
                .stderr(File::create(self.dir.join("daemon.log")).unwrap())
                .spawn()
                .unwrap(),
        );
    }

    async fn battery(&self, broken: bool) -> Connection {
        Builder::address(self.address.as_str())
            .unwrap()
            .name(UPOWER)
            .unwrap()
            .serve_at("/org/freedesktop/UPower", Battery)
            .unwrap()
            .serve_at("/org/freedesktop/UPower/devices/DisplayDevice", Device { broken })
            .unwrap()
            .build()
            .await
            .unwrap()
    }

    async fn profiles(&self, holds: Arc<Holds>) -> Connection {
        Builder::address(self.address.as_str())
            .unwrap()
            .name(PROFILES)
            .unwrap()
            .serve_at("/org/freedesktop/UPower/PowerProfiles", Profiles(holds))
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(daemon) = &mut self.daemon {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = self.bus.kill();
        let _ = self.bus.wait();
        if std::thread::panicking() {
            eprintln!("daemon log:\n{}", std::fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default());
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn eventually<F, Fut>(description: &str, seconds: u64, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    timeout(Duration::from_secs(seconds), async {
        while !check().await {
            sleep(Duration::from_millis(150)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out: {description}"));
}

async fn status(proxy: &zbus::Proxy<'_>) -> String {
    proxy.call("Status", &()).await.unwrap()
}

async fn exercise(fixture: &mut Fixture) {
    let docked = Arc::new(AtomicBool::new(false));
    let _login = Builder::address(fixture.address.as_str())
        .unwrap()
        .name(LOGIN)
        .unwrap()
        .serve_at("/org/freedesktop/login1", Login(docked.clone()))
        .unwrap()
        .build()
        .await
        .unwrap();
    let battery = fixture.battery(false).await;
    let holds = Arc::new(Holds::default());
    let profiles = fixture.profiles(holds.clone()).await;
    fixture.spawn_daemon();
    let client = Builder::address(fixture.address.as_str()).unwrap().build().await.unwrap();
    let proxy = zbus::Proxy::new(&client, "app.getinsomnia.Insomnia", "/app/getinsomnia/Insomnia", "app.getinsomnia.Insomnia1").await.unwrap();
    eventually("daemon ready", 5, || async { proxy.call::<_, _, String>("Status", &()).await.is_ok() }).await;
    let _: String = proxy.call("Start", &(120u64,)).await.unwrap();
    eventually("closed lid holds power saver", 3, || async { holds.active.load(SeqCst) }).await;

    docked.store(true, SeqCst);
    eventually("silent docking releases power saver", 4, || async { !holds.active.load(SeqCst) }).await;
    docked.store(false, SeqCst);
    eventually("silent undocking reacquires power saver", 4, || async { holds.active.load(SeqCst) }).await;

    holds.reject_release.store(true, SeqCst);
    let releases = holds.releases.load(SeqCst);
    docked.store(true, SeqCst);
    eventually("release failure reached", 4, || async { holds.releases.load(SeqCst) > releases }).await;
    let failed_at = Instant::now();
    let text = status(&proxy).await;
    assert!(text.contains("Power saver held") && text.contains("Could not release power saver"), "{text}");
    holds.reject_release.store(false, SeqCst);
    eventually("failed release retries with its cookie", 33, || async { !holds.active.load(SeqCst) }).await;
    assert!(failed_at.elapsed() >= Duration::from_secs(29), "release retried before its backoff");
    assert_eq!(holds.releases.load(SeqCst), releases + 2);

    docked.store(false, SeqCst);
    eventually("hold before service restart", 4, || async { holds.active.load(SeqCst) }).await;
    profiles.close().await.unwrap();
    sleep(Duration::from_millis(300)).await;
    let replacement_holds = Arc::new(Holds::default());
    let _profiles = fixture.profiles(replacement_holds.clone()).await;
    eventually("new power-profile service receives a hold", 4, || async { replacement_holds.active.load(SeqCst) }).await;

    assert!(status(&proxy).await.contains("Battery 80%"));
    battery.close().await.unwrap();
    let _battery = fixture.battery(true).await;
    eventually("unreadable battery ends the session", 4, || async { status(&proxy).await.starts_with("No session.") }).await;
    assert!(!fixture.dir.join("session.json").exists());
    eventually("ending releases power saver", 3, || async { !replacement_holds.active.load(SeqCst) }).await;
}

#[tokio::test]
async fn daemon_reconciles_service_changes_and_failed_release() {
    let mut fixture = Fixture::new();
    timeout(Duration::from_secs(58), exercise(&mut fixture)).await.expect("private-bus scenarios exceeded 58 seconds");
}
