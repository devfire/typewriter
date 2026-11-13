//! Light/dark palettes, typography, and widget geometry.

use super::{ACCENT, RADIUS};
use crate::signature;
use eframe::egui;
use std::sync::Arc;

pub(super) fn apply_theme(ctx: &egui::Context) {
    // Signature preview font; first (only) entry of its own family so the
    // UI's proportional text is untouched.
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        signature::FONT_KEY.to_string(),
        Arc::new(egui::FontData::from_static(signature::FONT_BYTES)),
    );
    fonts.families.insert(
        egui::FontFamily::Name(signature::FONT_FAMILY.into()),
        vec![signature::FONT_KEY.to_string()],
    );
    ctx.set_fonts(fonts);
    for (theme, dark) in [(egui::Theme::Light, false), (egui::Theme::Dark, true)] {
        ctx.set_visuals_of(theme, themed_visuals(dark));
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (egui::TextStyle::Small, egui::FontId::proportional(12.5)),
                (egui::TextStyle::Body, egui::FontId::proportional(14.5)),
                (egui::TextStyle::Button, egui::FontId::proportional(14.5)),
                (egui::TextStyle::Heading, egui::FontId::proportional(20.0)),
                (egui::TextStyle::Monospace, egui::FontId::monospace(13.5)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.spacing.interact_size = egui::vec2(40.0, 36.0);
            style.spacing.window_margin = egui::Margin::same(12);
            style.spacing.menu_margin = egui::Margin::same(10);
            style.spacing.indent = 18.0;
            style.spacing.slider_width = 150.0;
        });
    }
}

/// Palette for one theme. Light: airy blue-grey surfaces on white cards.
/// Dark: deep slate surfaces. Accent, selection, and radius are shared.
fn themed_visuals(dark: bool) -> egui::Visuals {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark {
        v.panel_fill = egui::Color32::from_rgb(0x16, 0x18, 0x20);
        v.window_fill = egui::Color32::from_rgb(0x1C, 0x1F, 0x28);
        v.extreme_bg_color = egui::Color32::from_rgb(0x0D, 0x0F, 0x14);
        v.faint_bg_color = egui::Color32::from_rgb(0x20, 0x23, 0x2E);
        v.code_bg_color = egui::Color32::from_rgb(0x20, 0x23, 0x2E);
        v.warn_fg_color = egui::Color32::from_rgb(0xFB, 0xBF, 0x24);
    } else {
        v.panel_fill = egui::Color32::from_rgb(0xF3, 0xF6, 0xFA);
        v.window_fill = egui::Color32::WHITE;
        v.extreme_bg_color = egui::Color32::from_rgb(0xDE, 0xE5, 0xEF);
        v.faint_bg_color = egui::Color32::WHITE;
        v.code_bg_color = egui::Color32::from_rgb(0xEE, 0xF2, 0xF7);
        v.warn_fg_color = egui::Color32::from_rgb(0xD9, 0x77, 0x06);
    }
    v.error_fg_color = egui::Color32::from_rgb(0xDC, 0x26, 0x26);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    v.window_corner_radius = egui::CornerRadius::same(14);
    v.menu_corner_radius = egui::CornerRadius::same(RADIUS);
    let radius = egui::CornerRadius::same(RADIUS);
    for widgets in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widgets.corner_radius = radius;
    }
    if dark {
        v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(0x26, 0x2A, 0x36);
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(0x26, 0x2A, 0x36);
        v.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x37, 0x3C, 0x4B));
        v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0x30, 0x36, 0x44);
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x30, 0x36, 0x44);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    } else {
        v.widgets.inactive.weak_bg_fill = egui::Color32::WHITE;
        v.widgets.inactive.bg_fill = egui::Color32::WHITE;
        v.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0xCB, 0xD5, 0xE1));
        v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0xDB, 0xE7, 0xFE);
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(0xDB, 0xE7, 0xFE);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    }
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
    v.striped = true;
    v
}
