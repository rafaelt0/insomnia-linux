//! The Days / Hours / Minutes entry, as a layer-shell overlay at the top
//! right of the screen. A short-lived process: it sends one command to the
//! daemon over D-Bus and exits.

use crate::client;
use crate::model::seconds_from_fields;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const CSS: &str = r#"
window.insomnia-prompt { background: none; }
.panel { background: alpha(#202224, 0.94); border-radius: 18px; padding: 10px 12px; border: 1px solid alpha(#E6E3DD, 0.12); }
.title { color: #E6E3DD; font-weight: 600; margin: 0 6px; }
entry.pill { background: alpha(#E6E3DD, 0.08); color: #E6E3DD; border-radius: 999px; border: none; outline: none;
             box-shadow: none; min-height: 30px; padding: 0 12px; transition: background 150ms ease, box-shadow 250ms ease; }
entry.pill:focus-within { background: alpha(#E6E3DD, 0.16); box-shadow: 0 0 0 3px alpha(#E6E3DD, 0.22); }
.hint { color: alpha(#E6E3DD, 0.6); font-size: 0.85em; margin: 6px 6px 0; }
.hint.error { color: #FF8A80; }
"#;

pub fn run(extend: bool) -> glib::ExitCode {
    let app = gtk4::Application::builder().application_id("app.getinsomnia.Insomnia.Prompt").build();
    app.connect_activate(move |app| build(app, extend));
    app.run_with_args(&[] as &[&str])
}

fn build(app: &gtk4::Application, extend: bool) {
    if let Some(existing) = app.active_window() {
        existing.present();
        return;
    }
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(CSS);
    gtk4::style_context_add_provider_for_display(
        &gdk::Display::default().expect("GTK has a display"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = gtk4::ApplicationWindow::builder().application(app).title("Insomnia").resizable(false).build();
    window.add_css_class("insomnia-prompt");
    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("insomnia"));
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Right, true);
        window.set_margin(Edge::Top, 8);
        window.set_margin(Edge::Right, 8);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
    }

    let fields: Vec<gtk4::Entry> = ["Days", "Hours", "Minutes"]
        .iter()
        .map(|unit| {
            let entry = gtk4::Entry::builder()
                .placeholder_text(*unit)
                .input_purpose(gtk4::InputPurpose::Digits)
                .max_length(4)
                .width_chars(7)
                .xalign(0.5)
                .build();
            entry.add_css_class("pill");
            entry
        })
        .collect();

    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    row.append(&gtk4::Label::builder().label(if extend { "Add" } else { "Keep awake for" }).css_classes(["title"]).build());
    fields.iter().for_each(|f| row.append(f));
    let default_hint = if extend { "Enter adds the time · Esc cancels" } else { "Empty starts the default · up to 30 days · Esc cancels" };
    let hint = gtk4::Label::builder().label(default_hint).xalign(0.0).css_classes(["hint"]).build();
    let panel = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    panel.add_css_class("panel");
    panel.append(&row);
    panel.append(&hint);
    window.set_child(Some(&panel));

    for field in &fields {
        let (fields, hint, window) = (fields.clone(), hint.clone(), window.clone());
        field.connect_activate(move |_| submit(&fields, &hint, &window, extend));
    }
    let keys = gtk4::EventControllerKey::new();
    let closing = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            closing.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
    fields[0].grab_focus();
}

fn submit(fields: &[gtk4::Entry], hint: &gtk4::Label, window: &gtk4::ApplicationWindow, extend: bool) {
    let show_error = |message: String| {
        hint.set_label(&message);
        hint.add_css_class("error");
    };
    let mut values = [0u64; 3];
    for (value, field) in values.iter_mut().zip(fields) {
        let text = field.text();
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match text.parse() {
            Ok(n) => *value = n,
            Err(_) => return show_error(format!("{text:?} is not a whole number")),
        }
    }
    let seconds = if values == [0, 0, 0] && fields.iter().all(|f| f.text().trim().is_empty()) {
        0
    } else {
        match seconds_from_fields(values[0], values[1], values[2]) {
            Ok(s) => s,
            Err(e) => return show_error(e),
        }
    };
    let result = client::call(|proxy| async move {
        if extend { proxy.extend(seconds).await } else { proxy.start(seconds).await }
    });
    match result {
        Ok(_) => window.close(),
        Err(e) => show_error(e),
    }
}
