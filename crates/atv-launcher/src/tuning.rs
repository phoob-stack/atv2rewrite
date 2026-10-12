//! The Config tab: the player's own physics tuning, kept in `tuning.cfg`
//! beside the launcher's settings and read by the game at every race. The
//! game's files are never changed.

use std::path::PathBuf;

use atv_sim::tuning::{self, Setting, Tuning, GROUPS, SCALES};
use atv_sim::VehicleParams;
use eframe::egui;

use crate::game::Catalog;

pub fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "atv2").map(|d| d.config_dir().join(tuning::FILE))
}

/// The tab's state: the tuning being edited and the quad shown.
pub struct Config {
    pub tuning: Tuning,
    saved: Tuning,
    pub quad: String,
}

impl Config {
    pub fn load(quad: &str) -> Self {
        let tuning = path().map(|p| Tuning::load(&p)).unwrap_or_default();
        Self { saved: tuning.clone(), tuning, quad: quad.to_string() }
    }

    /// Writes the file when something changed.
    pub fn save_if_changed(&mut self) {
        if self.tuning == self.saved {
            return;
        }
        let Some(p) = path() else { return };
        match self.tuning.save(&p) {
            Ok(()) => self.saved = self.tuning.clone(),
            Err(e) => eprintln!("could not save {}: {e}", p.display()),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, catalog: Option<&Catalog>) {
        ui.label(
            egui::RichText::new("Your own tuning on top of the game's. It is used from the next race, and the game's files are never changed.")
                .size(12.0)
                .weak(),
        );
        ui.add_space(10.0);
        crate::section(ui, "Physics (every quad)", |ui| self.physics(ui));
        ui.add_space(14.0);
        match catalog.filter(|c| !c.params.is_empty()) {
            Some(cat) => crate::section(ui, "Per ATV", |ui| self.per_quad(ui, cat)),
            None => crate::section(ui, "Per ATV", |ui| {
                ui.label("Choose your game data on the Play tab to tune each quad.");
            }),
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.tuning.is_empty(), egui::Button::new("Reset everything")).clicked() {
                self.tuning.scales.clear();
                self.tuning.quads.clear();
            }
            if let Some(p) = path() {
                ui.label(egui::RichText::new(format!("Saved in {}", p.display())).size(11.0).weak());
            }
        });
        ui.add_space(8.0);
        self.save_if_changed();
    }

    fn physics(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().slider_width = 240.0;
        egui::Grid::new("scales").num_columns(3).spacing([16.0, 8.0]).show(ui, |ui| {
            for s in &SCALES {
                ui.label(s.label).on_hover_text(s.hint);
                let mut v = self.tuning.scale(s.key);
                let slider = egui::Slider::new(&mut v, s.min..=s.max).suffix("%").step_by(1.0).fixed_decimals(0);
                if ui.add(slider).on_hover_text(s.hint).changed() {
                    self.tuning.set_scale(s.key, v);
                }
                reset_button(ui, v != 100.0, "Back to the game's 100%", || self.tuning.set_scale(s.key, 100.0));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.checkbox(&mut self.tuning.computer_riders, "Computer riders use this tuning too")
            .on_hover_text("Off: only the players' quads are tuned, so computer riders race as in the original.");
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.tuning.scales.is_empty(), egui::Button::new("Reset physics")).clicked() {
                self.tuning.scales.clear();
            }
        });
    }

    fn per_quad(&mut self, ui: &mut egui::Ui, cat: &Catalog) {
        if !cat.params.contains_key(&self.quad) {
            self.quad = cat.quads.first().cloned().unwrap_or_default();
        }
        let changed = |t: &Tuning, q: &str| tuning::settings().any(|s| t.quad_value(q, s.key).is_some());
        ui.horizontal(|ui| {
            ui.label("Quad");
            let shown = if changed(&self.tuning, &self.quad) { format!("{} (tuned)", self.quad) } else { self.quad.clone() };
            egui::ComboBox::from_id_salt("tune_quad").width(200.0).selected_text(shown).show_ui(ui, |ui| {
                for q in &cat.quads {
                    let text = if changed(&self.tuning, q) { format!("{q} (tuned)") } else { q.clone() };
                    ui.selectable_value(&mut self.quad, q.clone(), text);
                }
            });
            let quad = self.quad.clone();
            if ui.add_enabled(changed(&self.tuning, &quad), egui::Button::new(format!("Reset {quad}"))).clicked() {
                for s in tuning::settings() {
                    self.tuning.set_quad_value(&quad, s.key, None);
                }
            }
        });
        let Some(game) = cat.params.get(&self.quad) else { return };
        ui.add_space(6.0);
        egui::Grid::new("per_quad").num_columns(4).spacing([16.0, 6.0]).show(ui, |ui| {
            for (title, settings) in GROUPS {
                ui.add_space(6.0);
                ui.end_row();
                ui.label(egui::RichText::new(title).strong());
                let quad = self.quad.clone();
                let any = settings.iter().any(|s| self.tuning.quad_value(&quad, s.key).is_some());
                if ui.add_enabled(any, egui::Button::new(format!("Reset {}", title.to_lowercase())).small()).clicked() {
                    for s in settings {
                        self.tuning.set_quad_value(&quad, s.key, None);
                    }
                }
                ui.end_row();
                for s in settings {
                    self.setting_row(ui, s, game);
                    ui.end_row();
                }
            }
        });
    }

    fn setting_row(&mut self, ui: &mut egui::Ui, s: &Setting, game: &VehicleParams) {
        let quad = self.quad.clone();
        let original = s.game_value(game);
        let set = self.tuning.quad_value(&quad, s.key);
        let mut v = set.unwrap_or(original);
        ui.label(s.label).on_hover_text(s.key);
        let drag = egui::DragValue::new(&mut v).speed(s.speed).max_decimals(s.decimals.max(2)).min_decimals(s.decimals).suffix(match s.unit {
            "" => String::new(),
            "°" | "×" => s.unit.to_string(),
            u => format!(" {u}"),
        });
        let resp = ui.add_sized([110.0, 18.0], drag);
        if resp.changed() {
            let same = (v - original).abs() <= original.abs() * 1e-4 + 1e-6;
            self.tuning.set_quad_value(&quad, s.key, (!same).then_some(v));
        }
        let text = format!("game {}", number(original, s.decimals));
        if set.is_some() {
            ui.label(egui::RichText::new(text).size(12.0).color(crate::ACCENT));
        } else {
            ui.label(egui::RichText::new(text).size(12.0).weak());
        }
        reset_button(ui, set.is_some(), "Back to the game's value", || self.tuning.set_quad_value(&quad, s.key, None));
    }
}

fn reset_button(ui: &mut egui::Ui, enabled: bool, hint: &str, reset: impl FnOnce()) {
    if ui.add_enabled(enabled, egui::Button::new("↺").small()).on_hover_text(hint).clicked() {
        reset();
    }
}

/// The game's value with at least `decimals` digits, more if it has them.
fn number(v: f32, decimals: usize) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        let keep = s.find('.').unwrap() + 1 + decimals;
        while s.len() > keep && s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number(0.26, 3), "0.260");
        assert_eq!(number(13500.0, 0), "13500");
        assert_eq!(number(1.25, 1), "1.25");
        assert_eq!(number(35.0, 1), "35.0");
    }
}
