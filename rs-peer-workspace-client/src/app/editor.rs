use eframe::egui;
use rs_peer_workspace_shared::project::EditorSource;

use super::state::WorkspaceApp;
use super::types::BottomTab;

impl WorkspaceApp {
    pub fn draw_bottom(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bottom")
            .resizable(true)
            .default_height(220.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.active_bottom_tab, BottomTab::Output, "Output");
                    ui.selectable_value(&mut self.active_bottom_tab, BottomTab::Tasks, "Tasks");
                    ui.selectable_value(
                        &mut self.active_bottom_tab,
                        BottomTab::Terminal,
                        "Terminal",
                    );
                });
                ui.separator();

                match self.active_bottom_tab {
                    BottomTab::Output => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for line in &self.output_lines {
                                ui.label(line);
                            }
                        });
                    }
                    BottomTab::Tasks => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for line in &self.task_lines {
                                ui.label(line);
                            }
                        });
                    }
                    BottomTab::Terminal => self.draw_terminal_tabs(ui),
                }
            });
    }

    pub fn draw_editor(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.open_files.is_empty() {
                ui.heading("No file open");
                ui.label("Open a .txt, .py, or .m file from the explorer.");
                return;
            }

            ui.horizontal_wrapped(|ui| {
                for (idx, tab) in self.open_files.iter().enumerate() {
                    let title = if tab.dirty {
                        format!("{}*", tab.title)
                    } else {
                        tab.title.clone()
                    };
                    if ui
                        .selectable_label(self.selected_editor == Some(idx), title)
                        .clicked()
                    {
                        self.selected_editor = Some(idx);
                    }
                }
            });
            ui.separator();

            let mut save_clicked = false;
            if let Some(idx) = self.selected_editor {
                if let Some(tab) = self.open_files.get_mut(idx) {
                    ui.horizontal(|ui| {
                        ui.label(&tab.path);
                        if let EditorSource::Remote { connection_name } = &tab.source {
                            let transport = self
                                .connections
                                .get(connection_name)
                                .map(|state| state.transport.clone())
                                .unwrap_or_else(|| "Disconnected".to_string());
                            ui.separator();
                            ui.label(format!("Transport: {transport}"));
                        }
                        if ui.button("Save").clicked() {
                            save_clicked = true;
                        }
                    });

                    let response = ui.add(
                        egui::TextEdit::multiline(&mut tab.content)
                            .desired_rows(32)
                            .code_editor(),
                    );
                    if response.changed() {
                        tab.dirty = true;
                    }
                }
            }

            if save_clicked {
                self.save_active_editor();
            }
        });
    }

    pub fn draw_terminal_tabs(&mut self, ui: &mut egui::Ui) {
        if self.terminals.is_empty() {
            ui.label("No terminal open.");
            return;
        }

        ui.horizontal_wrapped(|ui| {
            for (idx, terminal) in self.terminals.iter().enumerate() {
                if ui
                    .selectable_label(self.selected_terminal == Some(idx), &terminal.title)
                    .clicked()
                {
                    self.selected_terminal = Some(idx);
                }
            }
        });
        ui.separator();

        let mut run = None;
        if let Some(idx) = self.selected_terminal {
            if let Some(term) = self.terminals.get_mut(idx) {
                ui.label(format!("Connection: {}", term.connection_name));
                ui.add(
                    egui::TextEdit::multiline(&mut term.output)
                        .desired_rows(10)
                        .interactive(false),
                );
                ui.horizontal(|ui| {
                    let input_width = (ui.available_width() - 80.0).clamp(140.0, 720.0);
                    ui.add(egui::TextEdit::singleline(&mut term.input).desired_width(input_width));
                    if ui.button("Run").clicked() {
                        let command = term.input.trim().to_string();
                        if !command.is_empty() {
                            run = Some((idx, command));
                            term.input.clear();
                        }
                    }
                });
            }
        }

        if let Some((idx, command)) = run {
            self.run_terminal(idx, command);
        }
    }
}
