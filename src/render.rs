//! Raster rendering of PDF pages to egui textures via hayro.
//!
//! The hayro `RenderCache` is created fresh on every cache miss: it is
//! documented as per-document (its keys are only unique within one
//! document), so storing one across documents would be unsound.

use anyhow::{Result, bail};
use eframe::egui;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;

/// Caches the last rendered page texture, keyed by
/// `(generation, page, (scale * 100.0) as i32)`: the scale is
/// truncated to whole percents, so zooms within 1% share the
/// cached texture.
#[derive(Default)]
pub struct PageRenderer {
    rendered: Option<(u64, usize, i32)>,
    texture: Option<egui::TextureHandle>,
}

impl PageRenderer {
    /// Display size (points * zoom) of `page_idx`, rotation/crop aware.
    pub fn page_points(pdf: &Pdf, page_idx: usize) -> Option<(f32, f32)> {
        pdf.pages()
            .get(page_idx)
            .map(hayro::hayro_syntax::page::Page::render_dimensions)
    }

    /// Render (or return the cached) texture for a page.
    ///
    /// The pixmap background is opaque white, which is load-bearing:
    /// hayro returns premultiplied RGBA, and premultiplied equals
    /// unmultiplied only at alpha 255, which is what makes
    /// `ColorImage::from_rgba_unmultiplied` correct here.
    #[allow(clippy::cast_precision_loss)]
    pub fn render_page(
        &mut self,
        ctx: &egui::Context,
        pdf: &Pdf,
        page_idx: usize,
        zoom: f32,
        generation: u64,
    ) -> Result<egui::TextureHandle> {
        let pages = pdf.pages();
        let Some(page) = pages.get(page_idx) else {
            bail!("page {page_idx} out of range ({} pages)", pages.len());
        };
        let scale = zoom * ctx.pixels_per_point();
        #[allow(clippy::cast_possible_truncation)]
        let scale_key = (scale * 100.0) as i32;
        let key = (generation, page_idx, scale_key);
        if self.rendered == Some(key)
            && let Some(texture) = &self.texture
        {
            return Ok(texture.clone());
        }
        // Clamp so the raster never exceeds the GPU texture limit.
        let (dim_w, dim_h) = page.render_dimensions();
        let max_side = ctx.input(|i| i.max_texture_side).max(1) as f32;
        let mut scale = scale.max(0.01);
        let longest = (dim_w.max(dim_h) * scale).max(1.0);
        if longest > max_side {
            scale *= max_side / longest;
        }
        let cache = hayro::RenderCache::new();
        let interp = InterpreterSettings::default();
        let pix = hayro::render(
            page,
            &cache,
            &interp,
            &hayro::RenderSettings::default(),
            &hayro::PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: hayro::vello_cpu::color::palette::css::WHITE,
            },
        );
        let width = usize::from(pix.width());
        let height = usize::from(pix.height());
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width, height], pix.data_as_u8_slice());
        let texture = match self.texture.take() {
            Some(mut handle) => {
                handle.set(image, egui::TextureOptions::LINEAR);
                handle
            }
            None => ctx.load_texture("page", image, egui::TextureOptions::LINEAR),
        };
        self.rendered = Some(key);
        self.texture = Some(texture.clone());
        Ok(texture)
    }

    pub fn invalidate(&mut self) {
        self.rendered = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn sample_pdf() -> Pdf {
        let mut doc = lopdf::Document::new();
        let page_tree_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => lopdf::Object::Reference(page_tree_id),
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        });
        doc.set_object(
            page_tree_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![lopdf::Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => lopdf::Object::Reference(page_tree_id),
        });
        doc.trailer
            .set("Root", lopdf::Object::Reference(catalog_id));
        let path =
            std::env::temp_dir().join(format!("pdf-editor-render-test-{}.pdf", std::process::id()));
        doc.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        Pdf::new(bytes).unwrap()
    }

    #[test]
    fn generation_bump_changes_cache_key() {
        let pdf = sample_pdf();
        let ctx = egui::Context::default();
        let mut renderer = PageRenderer::default();
        let _first = renderer.render_page(&ctx, &pdf, 0, 1.0, 1).unwrap();
        assert_eq!(renderer.rendered.map(|k| k.0), Some(1));
        // Same key: cache hit, key unchanged.
        let _first = renderer.render_page(&ctx, &pdf, 0, 1.0, 1).unwrap();
        assert_eq!(renderer.rendered.map(|k| k.0), Some(1));
        // New generation (reopened/reloaded doc): re-renders under new key,
        // so a same-shaped page from another document cannot go stale.
        let _second = renderer.render_page(&ctx, &pdf, 0, 1.0, 2).unwrap();
        assert_eq!(renderer.rendered.map(|k| k.0), Some(2));
    }

    #[test]
    fn out_of_range_page_errors_instead_of_panicking() {
        let pdf = sample_pdf();
        let ctx = egui::Context::default();
        let mut renderer = PageRenderer::default();
        assert!(renderer.render_page(&ctx, &pdf, 99, 1.0, 1).is_err());
    }
}
