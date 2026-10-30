//! The settings window: the snippets, the hotkey and the menu's size.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use eframe::egui;
use radial_domain::{
    Config, HotkeyBinding, Key, MAX_LABEL_CHARS, MAX_SNIPPETS, Modifiers, MouseButton, Snippet,
};

/// How long a fresh capture ignores the mouse, so that the click on the record button is not
/// itself recorded.
const ARMING: Duration = Duration::from_millis(250);
/// How often the capture re-polls the mouse while waiting.
const POLL: Duration = Duration::from_millis(10);

/// Opens the settings window and blocks until it is closed.
///
/// `save` gets each config the user saves and returns a message to show if saving failed.
/// `sample_mouse` reports the mouse button held down right now, with its modifiers, for the
/// press-to-record hotkey capture; `desktop` wires it to the Windows adapter, which is the only
/// one that can see the buttons.
pub fn run(
    config: Config,
    save: impl FnMut(&Config) -> Result<(), String> + 'static,
    sample_mouse: impl Fn() -> Option<(MouseButton, Modifiers)> + 'static,
) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("radial-paste settings")
            .with_inner_size([520.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "radial-paste settings",
        options,
        Box::new(|_| Ok(Box::new(SettingsApp::new(config, save, sample_mouse)))),
    )
}

enum Status {
    None,
    Saved,
    Failed(String),
}

enum RowAction {
    Up(usize),
    Down(usize),
    Remove(usize),
}

/// A press-to-record capture of a new hotkey.
struct Capture {
    /// Ignore mouse buttons until this instant; see [`ARMING`]. Key presses are live at once,
    /// because the click that armed the capture is no key press.
    arm_until: Instant,
    /// Set once a frame has seen every mouse button up. The click that armed the capture still
    /// counts as a press until then, even if the arming time has already run out.
    saw_mouse_up: bool,
}

/// What one frame of an active capture produced.
enum Outcome {
    Nothing,
    /// The new hotkey, as text for the field.
    Record(String),
    /// Esc: stop recording, keep the old hotkey.
    Cancel,
}

struct SettingsApp<S, M> {
    saved: Config,
    draft: Config,
    /// Parsed only on save, so the user can type freely.
    hotkey_text: String,
    /// Some while waiting for the press to record.
    capture: Option<Capture>,
    /// Reports the mouse button held down right now, for the capture.
    sample_mouse: M,
    status: Status,
    save: S,
}

impl<S: FnMut(&Config) -> Result<(), String>, M: Fn() -> Option<(MouseButton, Modifiers)>>
    SettingsApp<S, M>
{
    fn new(config: Config, save: S, sample_mouse: M) -> Self {
        Self {
            hotkey_text: config.hotkey.to_string(),
            saved: config.clone(),
            draft: config,
            capture: None,
            sample_mouse,
            status: Status::None,
            save,
        }
    }

    fn is_dirty(&self) -> bool {
        self.draft != self.saved || self.hotkey_text != self.saved.hotkey.to_string()
    }

    /// The draft with the hotkey field read in, if all of it is valid.
    fn checked(&self) -> Result<Config, String> {
        let hotkey: HotkeyBinding = self.hotkey_text.parse().map_err(|e| format!("{e}"))?;
        let config = Config {
            hotkey,
            ..self.draft.clone()
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    fn hotkey_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hotkey");
        ui.horizontal(|ui| {
            ui.label("Hold");
            ui.add_enabled(
                self.capture.is_none(),
                egui::TextEdit::singleline(&mut self.hotkey_text).hint_text("Ctrl+Shift+Space"),
            );
            let recording = self.capture.is_some();
            let label = if recording {
                "Cancel"
            } else {
                "Record keys…"
            };
            if ui.button(label).clicked() {
                self.capture = (!recording).then_some(Capture {
                    arm_until: Instant::now() + ARMING,
                    saw_mouse_up: false,
                });
            }
        });
        match self.capture_outcome(ui) {
            Outcome::Nothing => {}
            Outcome::Record(text) => {
                self.hotkey_text = text;
                self.capture = None;
            }
            Outcome::Cancel => self.capture = None,
        }
        if self.capture.is_some() {
            ui.weak("Press the key or mouse button to hold. Esc cancels.");
        } else {
            match self.hotkey_text.parse::<HotkeyBinding>() {
                Ok(binding) if binding.modifiers().alt => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Letting go of Alt can open the menu bar of the app you paste into.",
                    );
                }
                Ok(_) => {
                    ui.weak("Hold it, flick toward a snippet and let go. Or tap it and click one.");
                    ui.weak(
                        "Mouse buttons work too: Middle, Back and Forward may stand alone; Left \
                         and Right need Ctrl or Alt.",
                    );
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                }
            }
        }
    }

    /// Runs one frame of an active capture: the next key press, or the mouse button held down
    /// once the arming guard is over, becomes the new hotkey.
    fn capture_outcome(&mut self, ui: &mut egui::Ui) -> Outcome {
        let Some(capture) = self.capture.as_mut() else {
            return Outcome::Nothing;
        };
        let ctx = ui.ctx().clone();
        // The mouse is polled, not waited for: keep repainting even without focus, or a press
        // would be seen only when something else moves.
        ctx.request_repaint_after(POLL);

        // A key press is recorded at once; the window has focus, so egui sees them all. Repeats
        // of a held key do not count.
        let pressed = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        if let Some((egui_key, modifiers)) = pressed {
            if egui_key == egui::Key::Escape {
                return Outcome::Cancel;
            }
            if let Some(key) = domain_key(egui_key) {
                let modifiers = Modifiers {
                    ctrl: modifiers.ctrl,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                };
                return Outcome::Record(recorded(key, modifiers));
            }
        }

        // A mouse button is recorded only once the arming guard is over and every button has
        // been seen up, so that the click on the button above is not itself recorded - even
        // when that click is still held when the guard runs out.
        if Instant::now() >= capture.arm_until {
            let mouse = (self.sample_mouse)();
            capture.saw_mouse_up |= mouse.is_none();
            if capture.saw_mouse_up
                && let Some((button, modifiers)) = mouse
            {
                return Outcome::Record(recorded(Key::Mouse(button), modifiers));
            }
        }

        Outcome::Nothing
    }

    fn menu_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Menu");
        let layout = &mut self.draft.layout;
        egui::Grid::new("menu").num_columns(2).show(ui, |ui| {
            ui.label("Dead zone");
            ui.add(
                egui::DragValue::new(&mut layout.dead_zone)
                    .range(0.0..=200.0)
                    .suffix(" px"),
            );
            ui.end_row();
            ui.label("Radius");
            ui.add(
                egui::DragValue::new(&mut layout.radius)
                    .range(40.0..=400.0)
                    .suffix(" px"),
            );
            ui.end_row();
            ui.label("Wait for modifiers");
            ui.add(
                egui::DragValue::new(&mut self.draft.modifier_release_timeout_ms)
                    .range(0..=2000)
                    .suffix(" ms"),
            );
            ui.end_row();
        });
        ui.checkbox(&mut self.draft.autostart, "Start with Windows");
    }

    fn snippets_ui(&mut self, ui: &mut egui::Ui) {
        let count = self.draft.snippets.len();
        ui.heading(format!("Snippets ({count}/{MAX_SNIPPETS})"));
        ui.weak("The first is at 12 o'clock, the rest follow clockwise.");
        let mut action = None;
        for (index, snippet) in self.draft.snippets.iter_mut().enumerate() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{}.", index + 1));
                    ui.add(
                        egui::TextEdit::singleline(&mut snippet.label)
                            .char_limit(MAX_LABEL_CHARS)
                            .hint_text("Label"),
                    );
                    if ui.add_enabled(index > 0, egui::Button::new("Up")).clicked() {
                        action = Some(RowAction::Up(index));
                    }
                    if ui
                        .add_enabled(index + 1 < count, egui::Button::new("Down"))
                        .clicked()
                    {
                        action = Some(RowAction::Down(index));
                    }
                    if ui.button("Delete").clicked() {
                        action = Some(RowAction::Remove(index));
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut snippet.text)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Text to paste"),
                );
            });
        }
        let snippets = &mut self.draft.snippets;
        match action {
            Some(RowAction::Up(i)) => snippets.swap(i - 1, i),
            Some(RowAction::Down(i)) => snippets.swap(i, i + 1),
            Some(RowAction::Remove(i)) => {
                snippets.remove(i);
            }
            None => {}
        }
        if ui
            .add_enabled(count < MAX_SNIPPETS, egui::Button::new("Add snippet"))
            .clicked()
        {
            snippets.push(Snippet::new("", ""));
        }
    }

    fn actions_ui(&mut self, ui: &mut egui::Ui) {
        let dirty = self.is_dirty();
        let checked = self.checked();
        ui.horizontal(|ui| {
            let can_save = dirty && checked.is_ok();
            if ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .clicked()
                && let Ok(config) = &checked
            {
                self.status = match (self.save)(config) {
                    Ok(()) => {
                        self.saved = config.clone();
                        self.draft = config.clone();
                        self.hotkey_text = config.hotkey.to_string();
                        Status::Saved
                    }
                    Err(e) => Status::Failed(e),
                };
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                self.draft = self.saved.clone();
                self.hotkey_text = self.saved.hotkey.to_string();
                self.status = Status::None;
            }
            let error = ui.visuals().error_fg_color;
            match (&checked, &self.status) {
                (_, Status::Failed(e)) => {
                    ui.colored_label(error, e);
                }
                (Err(e), _) if dirty => {
                    ui.colored_label(error, e);
                }
                (_, Status::Saved) if !self.is_dirty() => {
                    ui.label("Saved");
                }
                _ => {}
            }
        });
    }
}

impl<S: FnMut(&Config) -> Result<(), String>, M: Fn() -> Option<(MouseButton, Modifiers)>>
    eframe::App for SettingsApp<S, M>
{
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(4.0);
            self.actions_ui(ui);
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.hotkey_ui(ui);
                ui.separator();
                self.menu_ui(ui);
                ui.separator();
                self.snippets_ui(ui);
            });
        });
    }
}

/// The text for the hotkey field after recording `key` with `modifiers`: the binding if it is
/// valid, otherwise just the key, so that the hint above can say what is missing.
fn recorded(key: Key, modifiers: Modifiers) -> String {
    HotkeyBinding::new(modifiers, key).map_or_else(|_| key.to_string(), |b| b.to_string())
}

/// The domain key behind the egui key, if there is one the hotkey can use. This rides on the
/// egui key's name matching the domain's spellings (`"A"`, `"7"`, `"F13"`, `"Space"`); keys the
/// hotkey cannot use, like Enter or F25, have names the domain does not parse and drop out here.
fn domain_key(key: egui::Key) -> Option<Key> {
    match key {
        egui::Key::Backtick => Some(Key::Backquote),
        key => key.name().parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_keys_map_to_domain_keys() {
        assert_eq!(domain_key(egui::Key::Q), Some(Key::Letter('Q')));
        assert_eq!(domain_key(egui::Key::Num5), Some(Key::Digit(5)));
        assert_eq!(domain_key(egui::Key::F13), Some(Key::F(13)));
        assert_eq!(domain_key(egui::Key::Space), Some(Key::Space));
        assert_eq!(domain_key(egui::Key::Backtick), Some(Key::Backquote));
        assert_eq!(domain_key(egui::Key::Enter), None);
        assert_eq!(domain_key(egui::Key::F25), None);
        assert_eq!(domain_key(egui::Key::Escape), None);
    }

    #[test]
    fn recording_an_invalid_combo_still_shows_the_key() {
        assert_eq!(recorded(Key::Letter('Q'), Modifiers::default()), "Q");
        assert_eq!(
            recorded(Key::Mouse(MouseButton::Back), Modifiers::default()),
            "MouseBack"
        );
        assert_eq!(
            recorded(Key::Mouse(MouseButton::Left), Modifiers::default()),
            "MouseLeft"
        );
        assert_eq!(
            recorded(
                Key::Mouse(MouseButton::Left),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            ),
            "Ctrl+MouseLeft"
        );
    }
}
