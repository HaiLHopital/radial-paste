//! The settings window: the snippets, the hotkey and the menu's size.
#![forbid(unsafe_code)]

use eframe::egui;
use radial_domain::{Config, HotkeyBinding, MAX_LABEL_CHARS, MAX_SNIPPETS, Snippet};

/// Opens the settings window and blocks until it is closed.
///
/// `save` gets each config the user saves and returns a message to show if saving failed.
pub fn run(
    config: Config,
    save: impl FnMut(&Config) -> Result<(), String> + 'static,
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
        Box::new(|_| Ok(Box::new(SettingsApp::new(config, save)))),
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

struct SettingsApp<S> {
    saved: Config,
    draft: Config,
    /// Parsed only on save, so the user can type freely.
    hotkey_text: String,
    status: Status,
    save: S,
}

impl<S: FnMut(&Config) -> Result<(), String>> SettingsApp<S> {
    fn new(config: Config, save: S) -> Self {
        Self {
            hotkey_text: config.hotkey.to_string(),
            saved: config.clone(),
            draft: config,
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
            ui.add(egui::TextEdit::singleline(&mut self.hotkey_text).hint_text("Ctrl+Shift+Space"));
        });
        match self.hotkey_text.parse::<HotkeyBinding>() {
            Ok(binding) if binding.modifiers().alt => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Letting go of Alt can open the menu bar of the app you paste into.",
                );
            }
            Ok(_) => {
                ui.weak("Hold it, flick toward a snippet and let go. Or tap it and click one.");
            }
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e.to_string());
            }
        }
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
        ui.checkbox(
            &mut self.draft.autostart,
            "Start with Windows (not wired up yet)",
        );
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

impl<S: FnMut(&Config) -> Result<(), String>> eframe::App for SettingsApp<S> {
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
