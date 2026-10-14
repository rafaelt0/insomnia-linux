//! The StatusNotifierItem. It renders a `View` the daemon hands it and turns
//! clicks into daemon commands; it holds no session state of its own.

use crate::daemon::{Command, Event};
use crate::model::format_preset;
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{MenuItem, TrayMethods};
use std::sync::OnceLock;
use tokio::sync::mpsc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub remaining: Option<String>,
    pub until: Option<String>,
    pub lines: Vec<String>,
    pub waiting_sinks: Vec<String>,
    pub presets: Vec<u64>,
    pub mute_on_lid_close: bool,
    pub power_saver_on_lid_close: bool,
}

pub struct Tray {
    events: mpsc::Sender<Event>,
    view: View,
}

impl Tray {
    pub fn set_view(&mut self, view: View) {
        self.view = view;
    }

    fn send(&self, command: Command) {
        if self.events.try_send(Event::Command(command, None)).is_err() {
            eprintln!("insomnia: tray command dropped: the daemon is busy or gone");
        }
    }
}

pub async fn spawn(events: mpsc::Sender<Event>, view: View) -> Option<ksni::Handle<Tray>> {
    match (Tray { events, view }).assume_sni_available(true).spawn().await {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("insomnia: no tray icon ({e}); the insomnia command still works");
            None
        }
    }
}

fn action(label: impl Into<String>, act: impl Fn(&mut Tray) + Send + 'static) -> MenuItem<Tray> {
    StandardItem { label: label.into(), activate: Box::new(act), ..Default::default() }.into()
}

fn line(label: impl Into<String>) -> MenuItem<Tray> {
    StandardItem { label: label.into(), enabled: false, ..Default::default() }.into()
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "insomnia".into()
    }

    fn title(&self) -> String {
        match &self.view.remaining {
            Some(left) => format!("Insomnia · {left}"),
            None => "Insomnia".into(),
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let title = match (&self.view.remaining, &self.view.until) {
            (Some(left), Some(until)) => format!("Awake until {until} · {left} left"),
            _ => "Insomnia · not keeping awake".into(),
        };
        ksni::ToolTip { title, description: self.view.lines.join("\n"), ..Default::default() }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        icons(self.view.remaining.is_some()).clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::OpenPrompt);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let v = &self.view;
        let mut items = Vec::new();
        match (&v.remaining, &v.until) {
            (Some(left), Some(until)) => {
                items.push(line(format!("Awake until {until} · {left} left")));
                for &minutes in &v.presets {
                    items.push(action(format!("Add {}", format_preset(minutes)), move |t| t.send(Command::Extend(minutes * 60))));
                }
                items.push(action("Add other time…", |t| t.send(Command::OpenPrompt)));
                items.push(action("End session", |t| t.send(Command::End)));
            }
            _ => {
                items.push(line("Keep awake for"));
                for &minutes in &v.presets {
                    items.push(action(format_preset(minutes), move |t| t.send(Command::Start(minutes * 60))));
                }
                items.push(action("Other time…", |t| t.send(Command::OpenPrompt)));
            }
        }
        items.push(MenuItem::Separator);
        items.extend(v.lines.iter().map(line));
        for sink in &v.waiting_sinks {
            let name = sink.clone();
            items.push(action(format!("Stop waiting to unmute {sink}"), move |t| t.send(Command::StopWaitingFor(name.clone()))));
        }
        items.push(MenuItem::Separator);
        items.push(
            CheckmarkItem {
                label: "Mute audio when the lid closes".into(),
                checked: v.mute_on_lid_close,
                activate: Box::new(|t: &mut Tray| t.send(Command::SetMuteOnLidClose(!t.view.mute_on_lid_close))),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            CheckmarkItem {
                label: "Power saver when the lid closes".into(),
                checked: v.power_saver_on_lid_close,
                activate: Box::new(|t: &mut Tray| t.send(Command::SetPowerSaverOnLidClose(!t.view.power_saver_on_lid_close))),
                ..Default::default()
            }
            .into(),
        );
        items.push(action(if v.remaining.is_some() { "Quit and end the session" } else { "Quit" }, |t| t.send(Command::Quit)));
        items
    }
}

const COLOR: &str = "#E6E3DD";
const LENS: &str = "M1.5 12 C7 4.5 17 4.5 22.5 12 C17 19.5 7 19.5 1.5 12 Z";
const PUPIL: &str = "M15.3953 9.8853 C16.5632 11.7605 15.9899 14.2274 14.1147 15.3953 C12.2395 16.5632 9.7726 15.9899 8.6047 14.1147 C7.4368 12.2395 8.0101 9.7726 9.8853 8.6047 C11.1798 7.7984 12.8202 7.7984 14.1147 8.6047 C13.3441 8.5414 12.6681 9.1147 12.6047 9.8853 C12.5414 10.6559 13.1147 11.3319 13.8853 11.3953 C14.6559 11.4586 15.3319 10.8853 15.3953 10.1147 C15.4016 10.0384 15.4016 9.9616 15.3953 9.8853 Z";
const LASHES_ABOVE: &str = "M4.5499 6.1977 L3.1442 3.8341 M8.2636 4.6218 L7.5468 1.9668 M12 4.125 L12 1.375 M15.7364 4.6218 L16.4532 1.9668 M19.4501 6.1977 L20.8558 3.8341";
const LASHES_BELOW: &str = "M4.5499 17.8023 L3.1442 20.1659 M8.2636 19.3782 L7.5468 22.0332 M12 19.875 L12 22.625 M15.7364 19.3782 L16.4532 22.0332 M19.4501 17.8023 L20.8558 20.1659";

/// The macOS menu bar mark on its 24-unit grid: open with a pupil and lashes
/// above while a session runs, a shaded lens with lashes below otherwise.
pub fn eye_svg(open: bool) -> String {
    let stroke = format!(r#"fill="none" stroke="{COLOR}" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round""#);
    let (lens_fill, inner, lashes) = if open {
        ("none", format!(r#"<path d="{PUPIL}" fill="{COLOR}"/>"#), LASHES_ABOVE)
    } else {
        (COLOR, String::new(), LASHES_BELOW)
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="{LENS}" fill="{lens_fill}" stroke="{COLOR}" stroke-width="1.5" stroke-linejoin="round"/>{inner}<path d="{lashes}" {stroke}/></svg>"#
    )
}

fn icons(open: bool) -> &'static Vec<ksni::Icon> {
    static OPEN: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    static CLOSED: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    (if open { &OPEN } else { &CLOSED }).get_or_init(|| [16, 22, 24, 32, 48, 64].iter().map(|&size| render(&eye_svg(open), size)).collect())
}

/// SNI wants ARGB32 in network byte order, not premultiplied.
fn render(svg: &str, size: u32) -> ksni::Icon {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("the eye SVG is valid");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("icon sizes are nonzero");
    let scale = size as f32 / 24.0;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.alpha(), c.red(), c.green(), c.blue()]
        })
        .collect();
    ksni::Icon { width: size as i32, height: size as i32, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_eyes_render_with_ink_inside_the_frame() {
        for open in [true, false] {
            let icon = render(&eye_svg(open), 24);
            assert_eq!(icon.data.len(), 24 * 24 * 4);
            let inked = icon.data.chunks(4).filter(|p| p[0] > 0).count();
            assert!(inked > 60, "open={open}: only {inked} inked pixels");
            let corner_alpha = icon.data[0];
            assert_eq!(corner_alpha, 0, "open={open}: the corner should be transparent");
        }
    }

    #[test]
    fn the_closed_eye_is_mostly_shaded_and_the_open_one_is_not() {
        let ink = |open| render(&eye_svg(open), 48).data.chunks(4).filter(|p| p[0] > 128).count();
        assert!(ink(false) > ink(true));
    }
}
