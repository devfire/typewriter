//! Raster rendering of PDF pages to egui textures via hayro, off the UI thread.
//!
//! `render_page` never rasterizes synchronously: on a cache miss it
//! queues the page on a single worker thread and returns the previous
//! texture (if any), which the canvas keeps drawing scaled to the new
//! page rect until the worker's result arrives. The worker calls
//! `request_repaint` on completion, so a finished render lands on the
//! next frame instead of waiting for user input. Queued requests are
//! coalesced to the latest one, so a zoom drag does not rasterize
//! every intermediate scale.
//!
//! The hayro `RenderCache` is created fresh on every render: it is
//! documented as per-document (its keys are only unique within one
//! document), so storing one across documents would be unsound. The
//! worker does keep its own parsed `Pdf`, keyed by the identity of
//! the shared bytes `Arc`, so re-renders of one document do not
//! re-parse it.

use anyhow::{Result, bail};
use eframe::egui;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

/// Cache key: generation, page, and scale truncated to whole
/// percents, so zooms within 1% share the cached texture.
/// `generation` is bumped by [`PageRenderer::invalidate`] whenever
/// the underlying document bytes change (open, post-save reload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderKey {
    generation: u64,
    page: usize,
    scale_key: i32,
}

struct RenderRequest {
    key: RenderKey,
    bytes: Arc<Vec<u8>>,
    scale: f32,
    ctx: egui::Context,
}

struct RenderResult {
    key: RenderKey,
    image: Result<egui::ColorImage, String>,
}

/// Async page renderer: owns the last completed texture plus the
/// channels of its lazily spawned worker thread.
#[derive(Default)]
pub struct PageRenderer {
    generation: u64,
    rendered: Option<RenderKey>,
    texture: Option<egui::TextureHandle>,
    pending: Option<RenderKey>,
    failed: Option<(RenderKey, String)>,
    request_tx: Option<Sender<RenderRequest>>,
    result_rx: Option<Receiver<RenderResult>>,
}

impl PageRenderer {
    /// Texture for a page: the exact render when ready, otherwise
    /// the previous page's texture (drawn scaled by the caller), or
    /// `None` before the first render completes.
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
        bytes: Arc<Vec<u8>>,
        page_idx: usize,
        zoom: f32,
    ) -> Result<Option<egui::TextureHandle>> {
        self.poll_results(ctx);
        let pages = pdf.pages();
        let Some(page) = pages.get(page_idx) else {
            bail!("page {page_idx} out of range ({} pages)", pages.len());
        };
        let scale = zoom * ctx.pixels_per_point();
        #[allow(clippy::cast_possible_truncation)]
        let scale_key = (scale * 100.0) as i32;
        let key = RenderKey {
            generation: self.generation,
            page: page_idx,
            scale_key,
        };
        if self.rendered == Some(key)
            && let Some(texture) = &self.texture
        {
            return Ok(Some(texture.clone()));
        }
        if let Some((failed_key, message)) = &self.failed
            && *failed_key == key
        {
            bail!("{message}");
        }
        if self.pending != Some(key) {
            // Clamp so the raster never exceeds the GPU texture limit.
            let (dim_w, dim_h) = page.render_dimensions();
            let max_side = ctx.input(|i| i.max_texture_side).max(1) as f32;
            let mut scale = scale.max(0.01);
            let longest = (dim_w.max(dim_h) * scale).max(1.0);
            if longest > max_side {
                scale *= max_side / longest;
            }
            self.request(key, bytes, scale, ctx);
        }
        Ok(self.texture.clone())
    }

    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.rendered = None;
        self.pending = None;
        self.failed = None;
    }

    /// Queue `key` on the worker, spawning it on first use. A failed
    /// send (dead worker) is remembered like a render failure so the
    /// canvas reports it instead of re-queueing every frame.
    fn request(&mut self, key: RenderKey, bytes: Arc<Vec<u8>>, scale: f32, ctx: &egui::Context) {
        self.ensure_worker();
        let request = RenderRequest {
            key,
            bytes,
            scale,
            ctx: ctx.clone(),
        };
        if let Some(Ok(())) = self.request_tx.as_ref().map(|tx| tx.send(request)) {
            self.pending = Some(key);
        } else {
            self.request_tx = None;
            self.result_rx = None;
            self.failed = Some((key, "render worker stopped".to_string()));
        }
    }

    fn ensure_worker(&mut self) {
        if self.request_tx.is_some() {
            return;
        }
        let (request_tx, request_rx) = channel::<RenderRequest>();
        let (result_tx, result_rx) = channel::<RenderResult>();
        std::thread::spawn(move || worker_loop(&request_rx, &result_tx));
        self.request_tx = Some(request_tx);
        self.result_rx = Some(result_rx);
    }

    /// Install every finished render whose key is still the pending
    /// one; stale results (superseded page/zoom) are dropped.
    fn poll_results(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.result_rx else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    if self.pending != Some(result.key) {
                        continue;
                    }
                    self.pending = None;
                    match result.image {
                        Ok(image) => {
                            let texture = match self.texture.take() {
                                Some(mut handle) => {
                                    handle.set(image, egui::TextureOptions::LINEAR);
                                    handle
                                }
                                None => {
                                    ctx.load_texture("page", image, egui::TextureOptions::LINEAR)
                                }
                            };
                            self.texture = Some(texture);
                            self.rendered = Some(result.key);
                            self.failed = None;
                        }
                        Err(message) => self.failed = Some((result.key, message)),
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.request_tx = None;
                    self.result_rx = None;
                    self.pending = None;
                    break;
                }
            }
        }
    }
}

/// Worker body: render the latest queued request, then wake the UI.
/// Owns its parsed `Pdf`, reused while the bytes `Arc` is unchanged.
fn worker_loop(request_rx: &Receiver<RenderRequest>, result_tx: &Sender<RenderResult>) {
    let mut cached: Option<(Arc<Vec<u8>>, Pdf)> = None;
    while let Ok(mut request) = request_rx.recv() {
        // Coalesce: only the newest queued render is worth doing.
        while let Ok(next) = request_rx.try_recv() {
            request = next;
        }
        let ctx = request.ctx.clone();
        let image = render_on_worker(&mut cached, &request);
        if result_tx
            .send(RenderResult {
                key: request.key,
                image,
            })
            .is_err()
        {
            return;
        }
        ctx.request_repaint();
    }
}

fn render_on_worker(
    cached: &mut Option<(Arc<Vec<u8>>, Pdf)>,
    request: &RenderRequest,
) -> Result<egui::ColorImage, String> {
    let cache_is_current = cached
        .as_ref()
        .is_some_and(|(bytes, _)| Arc::ptr_eq(bytes, &request.bytes));
    if !cache_is_current {
        let pdf = Pdf::new(Arc::clone(&request.bytes)).map_err(|err| format!("{err:?}"))?;
        *cached = Some((Arc::clone(&request.bytes), pdf));
    }
    let Some((_, pdf)) = cached.as_ref() else {
        return Err("render worker lost its document".to_string());
    };
    let pages = pdf.pages();
    let Some(page) = pages.get(request.key.page) else {
        return Err(format!(
            "page {} out of range ({} pages)",
            request.key.page,
            pages.len()
        ));
    };
    let cache = hayro::RenderCache::new();
    let interp = InterpreterSettings::default();
    let pix = hayro::render(
        page,
        &cache,
        &interp,
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: request.scale,
            y_scale: request.scale,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [usize::from(pix.width()), usize::from(pix.height())],
        pix.data_as_u8_slice(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn sample_pdf() -> (Pdf, Arc<Vec<u8>>) {
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
        let file = crate::test_util::TempFile::new("render");
        doc.save(file.path()).unwrap();
        let bytes = Arc::new(std::fs::read(file.path()).unwrap());
        let pdf = Pdf::new(Arc::clone(&bytes)).unwrap();
        (pdf, bytes)
    }

    /// Drive `render_page` until the worker's texture for this
    /// exact zoom (at the renderer's current generation) lands; a
    /// stale previous texture does not count.
    fn render_until_ready(
        renderer: &mut PageRenderer,
        ctx: &egui::Context,
        pdf: &Pdf,
        bytes: &Arc<Vec<u8>>,
        zoom: f32,
    ) -> egui::TextureHandle {
        #[allow(clippy::cast_possible_truncation)]
        let expected = RenderKey {
            generation: renderer.generation,
            page: 0,
            scale_key: (zoom * ctx.pixels_per_point() * 100.0) as i32,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let texture = renderer
                .render_page(ctx, pdf, Arc::clone(bytes), 0, zoom)
                .unwrap();
            if renderer.rendered == Some(expected) {
                return texture.expect("a completed render installs its texture");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "render never completed"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn first_render_is_async_then_cached() {
        let (pdf, bytes) = sample_pdf();
        let ctx = egui::Context::default();
        let mut renderer = PageRenderer::default();
        // Nothing to show before the worker finishes: no texture yet,
        // and the UI thread did not rasterize synchronously.
        let first = renderer
            .render_page(&ctx, &pdf, Arc::clone(&bytes), 0, 1.0)
            .unwrap();
        assert!(first.is_none());
        let _texture = render_until_ready(&mut renderer, &ctx, &pdf, &bytes, 1.0);
        assert_eq!(renderer.rendered.map(|k| k.generation), Some(0));
        // Same key: cache hit, key unchanged.
        let _texture = render_until_ready(&mut renderer, &ctx, &pdf, &bytes, 1.0);
        assert_eq!(renderer.rendered.map(|k| k.generation), Some(0));
    }

    #[test]
    fn previous_texture_stays_up_while_new_zoom_renders() {
        let (pdf, bytes) = sample_pdf();
        let ctx = egui::Context::default();
        let mut renderer = PageRenderer::default();
        let _texture = render_until_ready(&mut renderer, &ctx, &pdf, &bytes, 1.0);
        // A new zoom queues a worker render; the old texture is
        // returned immediately so the canvas can keep showing it scaled.
        let stale = renderer
            .render_page(&ctx, &pdf, Arc::clone(&bytes), 0, 2.0)
            .unwrap();
        assert!(stale.is_some());
        let _texture = render_until_ready(&mut renderer, &ctx, &pdf, &bytes, 2.0);
        #[allow(clippy::cast_possible_truncation)]
        let expected = (2.0 * ctx.pixels_per_point() * 100.0) as i32;
        assert_eq!(renderer.rendered.map(|k| k.scale_key), Some(expected));
    }

    #[test]
    fn generation_bump_changes_cache_key() {
        let (pdf, bytes) = sample_pdf();
        let ctx = egui::Context::default();
        let mut renderer = PageRenderer::default();
        let _first = render_until_ready(&mut renderer, &ctx, &pdf, &bytes, 1.0);
        // Invalidating (reopened/reloaded doc) bumps generation and re-renders
        // under a new key, so a same-shaped page from another document cannot
        // go stale.
        renderer.invalidate();
        let _second = render_until_ready(&mut renderer, &ctx, &pdf, &bytes, 1.0);
        assert_eq!(renderer.rendered.map(|k| k.generation), Some(1));
    }

    #[test]
    fn out_of_range_page_errors_instead_of_panicking() {
        let (pdf, bytes) = sample_pdf();
        let ctx = egui::Context::default();
        let mut renderer = PageRenderer::default();
        assert!(renderer.render_page(&ctx, &pdf, bytes, 99, 1.0).is_err());
    }
}
