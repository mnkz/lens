use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Button, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, TextFormat, Vec2,
};

use crate::LensApp;

pub const BG: Color32 = Color32::from_rgb(0, 0, 0);
pub const TEXT: Color32 = Color32::from_rgb(244, 244, 242);
pub const MUTED: Color32 = Color32::from_rgb(138, 138, 136);
pub const LINE: Color32 = Color32::from_rgb(38, 38, 38);
pub const STROKE: Color32 = Color32::from_rgb(42, 42, 42);

enum Click {
    Copy,
    Text,
    Lens,
    Save,
    Pin,
    Web,
    Folder,
    Startup,
    Thumb(std::path::PathBuf),
}

impl LensApp {
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui) {
        self.lens_slot = None;
        let mut click = None;
        let frame = Frame::new()
            .fill(BG)
            .stroke(Stroke::new(1.0, STROKE))
            .inner_margin(Margin::same(18))
            .corner_radius(CornerRadius::ZERO);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            self.header(ui);
            if self.current.is_some() && self.lens_page.is_some() {
                click = self.shot(ui);
                let room = ui.available_height();
                let h = (room - 64.0).max(0.0);
                if h >= 48.0 {
                    let (mut rect, _) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
                    let screen = ui.ctx().viewport_rect();
                    rect.min.x = screen.min.x;
                    rect.max.x = screen.max.x;
                    self.lens_slot = Some(rect);
                }
                ui.add_space(8.0);
                hairline(ui);
                ui.add_space(8.0);
                self.help(ui, &mut click);
            } else {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        if self.current.is_none() {
                            self.empty(ui);
                        } else {
                            click = self.shot(ui);
                        }
                        ui.add_space(16.0);
                        hairline(ui);
                        ui.add_space(8.0);
                        self.help(ui, &mut click);
                    });
            }
        });
        let ctx = ui.ctx().clone();
        if let Some(click) = click {
            match click {
                Click::Copy => self.copy_current(),
                Click::Text => self.copy_text_current(),
                Click::Lens => self.launch_lens(&ctx),
                Click::Save => self.save_current(),
                Click::Pin => self.pin_current(),
                Click::Web => self.web_current(),
                Click::Folder => self.open_folder(),
                Click::Startup => self.toggle_startup(),
                Click::Thumb(path) => self.open_thumb(&ctx, path),
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let width = (ui.available_width() - 28.0).max(0.0);
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click_and_drag());
            let job = LayoutJob::single_section(
                "LENS".to_string(),
                TextFormat {
                    font_id: FontId::new(12.0, FontFamily::Proportional),
                    extra_letter_spacing: 2.6,
                    color: TEXT,
                    ..Default::default()
                },
            );
            let painter = ui.painter();
            let galley = painter.layout_job(job);
            let pos = Pos2::new(rect.left(), rect.center().y - galley.size().y * 0.5);
            painter.galley(pos, galley, TEXT);
            if resp.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            resp.on_hover_cursor(egui::CursorIcon::Grab);
            let close = Button::new(RichText::new("×").size(18.0).color(MUTED))
                .frame(false)
                .fill(BG);
            if ui
                .add_sized(Vec2::new(28.0, 22.0), close)
                .on_hover_text("Ranger dans la zone de notification")
                .clicked()
            {
                self.hide(ui.ctx());
            }
        });
    }

    fn empty(&self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.label(RichText::new("Impr écran").size(28.0).color(TEXT));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Les deux écrans se figent.")
                .size(14.0)
                .color(TEXT),
        );
        ui.label(
            RichText::new("Tu traces. L'image part dans le presse-papiers.")
                .size(13.0)
                .color(MUTED),
        );
    }

    fn shot(&mut self, ui: &mut egui::Ui) -> Option<Click> {
        let mut click = None;
        if let Some(tex) = &self.shot_tex {
            let max_h = if self.lens_page.is_some() {
                96.0
            } else {
                220.0
            };
            ui.add_space(8.0);
            ui.add(
                egui::Image::from_texture(egui::load::SizedTexture::from_handle(tex))
                    .max_width(ui.available_width())
                    .max_height(max_h),
            );
        }
        if let Some(current) = &self.current {
            ui.label(
                RichText::new(format!("{} × {}", current.w, current.h))
                    .size(12.0)
                    .color(MUTED),
            );
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).size(13.0).color(TEXT));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            if chip(ui, "Copier", false) {
                click = Some(Click::Copy);
            }
            if chip(ui, "Texte", false) {
                click = Some(Click::Text);
            }
            if chip(ui, "Lens", false) {
                click = Some(Click::Lens);
            }
            if chip(ui, "Sauver", false) {
                click = Some(Click::Save);
            }
            if chip(ui, "Épingler", false) {
                click = Some(Click::Pin);
            }
            if self
                .current
                .as_ref()
                .is_some_and(|cur| !cur.text.is_empty() || !cur.selection.is_empty())
                && chip(ui, "Web", false)
            {
                click = Some(Click::Web);
            }
        });
        if self.lens_page.is_some() {
            return click;
        }
        let body = self
            .current
            .as_ref()
            .map(|cur| {
                if cur.selection.is_empty() {
                    cur.text.clone()
                } else {
                    cur.selection.clone()
                }
            })
            .unwrap_or_default();
        if !body.is_empty() {
            ui.add_space(4.0);
            section(ui, "TEXTE");
            ui.add(egui::Label::new(RichText::new(body).size(13.0).color(TEXT)).selectable(true));
        }
        if !self.thumbs.is_empty() {
            ui.add_space(6.0);
            section(ui, "RÉCENT");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for thumb in &self.thumbs {
                    let size = thumb.tex.size();
                    let th = 56.0;
                    let tw = th * size[0] as f32 / (size[1] as f32).max(1.0);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(tw, th), Sense::click());
                    ui.painter().image(
                        thumb.tex.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, STROKE),
                        egui::StrokeKind::Inside,
                    );
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        click = Some(Click::Thumb(thumb.path.clone()));
                    }
                }
            });
        }
        click
    }

    fn help(&self, ui: &mut egui::Ui, click: &mut Option<Click>) {
        ui.label(
            RichText::new("L'image reste dans le presse-papiers.")
                .size(12.0)
                .color(MUTED),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            if text_button(ui, "Dossier") {
                *click = Some(Click::Folder);
            }
            let startup = if self.startup {
                "Retirer du démarrage"
            } else {
                "Lancer avec Windows"
            };
            if text_button(ui, startup) {
                *click = Some(Click::Startup);
            }
        });
    }
}

fn section(ui: &mut egui::Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter();
    let job = LayoutJob::single_section(
        label.to_string(),
        TextFormat {
            font_id: FontId::new(11.0, FontFamily::Proportional),
            extra_letter_spacing: 1.4,
            color: MUTED,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    painter.galley(
        Pos2::new(rect.left(), rect.center().y - galley.size().y * 0.5),
        galley,
        MUTED,
    );
}

fn chip(ui: &mut egui::Ui, label: &str, on: bool) -> bool {
    let text = RichText::new(label)
        .size(12.0)
        .color(if on { BG } else { TEXT });
    ui.add(
        Button::new(text)
            .fill(if on { TEXT } else { BG })
            .stroke(Stroke::new(1.0, if on { TEXT } else { STROKE }))
            .corner_radius(CornerRadius::same(2))
            .min_size(Vec2::new(0.0, 26.0)),
    )
    .clicked()
}

fn text_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        Button::new(RichText::new(label).size(12.0).color(TEXT))
            .frame(false)
            .fill(BG),
    )
    .clicked()
}

fn hairline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, LINE);
}
