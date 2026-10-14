//! The daemon's D-Bus interface as seen by the CLI and the prompt.

use crate::daemon::{BUS_NAME, OBJECT_PATH};
use std::future::Future;

#[zbus::proxy(interface = "app.getinsomnia.Insomnia1", default_service = "app.getinsomnia.Insomnia", default_path = "/app/getinsomnia/Insomnia")]
pub trait Insomnia {
    fn start(&self, seconds: u64) -> zbus::Result<String>;
    fn extend(&self, seconds: u64) -> zbus::Result<String>;
    fn end(&self) -> zbus::Result<String>;
    fn status(&self) -> zbus::Result<String>;
    fn quit(&self) -> zbus::Result<String>;
    fn simulate_lid(&self, state: &str) -> zbus::Result<String>;
}

/// Runs one call on a private runtime and turns D-Bus failures into the
/// sentence a person should read.
pub fn call<F, Fut>(f: F) -> Result<String, String>
where
    F: FnOnce(InsomniaProxy<'static>) -> Fut,
    Fut: Future<Output = zbus::Result<String>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let connection = zbus::Connection::session().await.map_err(|e| format!("No session bus: {e}"))?;
        let proxy = InsomniaProxy::builder(&connection)
            .destination(BUS_NAME)
            .and_then(|b| b.path(OBJECT_PATH))
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string())?;
        f(proxy).await.map_err(|e| match e {
            zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => {
                "Insomnia is not running. Start it with `systemctl --user start insomnia` or run `insomnia`.".to_string()
            }
            zbus::Error::MethodError(_, Some(message), _) => message,
            zbus::Error::FDO(e) => match *e {
                zbus::fdo::Error::ServiceUnknown(_) => {
                    "Insomnia is not running. Start it with `systemctl --user start insomnia` or run `insomnia`.".to_string()
                }
                zbus::fdo::Error::Failed(message) => message,
                other => other.to_string(),
            },
            other => other.to_string(),
        })
    })
}
