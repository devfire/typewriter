# PDF Typewriter Plan (egui, form-filling only)

## Context
- Ask: beautiful but DRY/KISS `egui` PDF editor; NEVER edit existing PDF text; typewriter-only overlay for form filling.
- Repo is greenfield: `Cargo.toml` (empty `[dependencies]`, edition 2024) + `src/main.rs` (`println!("Hello, world!")`). No existing code to reuse or migrate.
- End state: native `eframe` app. Open any PDF, raster-view pages, click to place typewriter text boxes, drag to move, edit/delete overlay text only, Save + Save As a new PDF with text baked as content-stream overlay. Fixed Helvetica family; per-box size + color. Original page content bytes untouched (overlay appended as new content stream).
- Decisions from user (2026-11-12): renderer `hayro` (pure Rust); save `Save + Save As`; styling `size + color`.

## Approach (ordered, each step leaves a building app)

### 1. Dependencies + entry (`Cargo.toml`, `src/main.rs`)
- Set `Cargo.toml`: `eframe = "0.32"` (pulls `egui`), `egui = "0.32"`, `lopdf = "0.36"`, `hayro = "0.9"`, `rfd = "0.15"`. All pure-Rust except `rfd` OS dialogs; no system PDF libs. Pin exact versions at implementation time with `cargo add`; record chosen versions in commit message.
- `src/main.rs` (~20 lines): `fn main() -> eframe::Result<()>`; `eframe::run_native("Typewriter PDF", options, Box::new(|cc| Ok(Box::new(app::TypewriterApp::new(cc)))))` with `viewport().with_inner_size([1200.0, 800.0])`. No logic here.

### 2. PDF model + overlay writer (`src/pdf.rs`, new, ≤300 LOC)
- Struct `PdfDoc { bytes: Vec<u8>, doc: lopdf::Document, page_ids: Vec<lopdf::ObjectId>, sizes_pt: Vec<(f32, f32)> }`.
- `PdfDoc::open(path) -> anyhow::Result<Self>`: `std::fs::read` + `Document::load_mem(&bytes)` (keeps `bytes` for `hayro::Pdf::new(bytes.clone())` reuse). `page_ids` from `doc.get_pages()` sorted by page number (BTreeMap iteration order). `sizes_pt` from each page dict `/MediaBox` array `[x0 y0 x1 y1]` → `(x1-x0, y1-y0)`; missing/invalid MediaBox → `anyhow::bail!("page {n}: missing MediaBox")`.
- Struct `OverlayText { page: usize, x_pt: f32, y_top_pt: f32, text: String, size_pt: f32, color: [u8; 3] }` lives in `overlay.rs`; `pdf.rs` consumes `&[OverlayText]`.
- `PdfDoc::bake_and_save(&mut self, boxes: &[OverlayText], out: &Path) -> anyhow::Result<()>`:
  - Group boxes by page. For each page with ≥1 box: `page_id = page_ids[page]`; `font_alias: String = self.ensure_helvetica(page_id)` (below); build one `lopdf::content::Content<Vec<Operation>>` with operations per box, then `self.doc.add_to_page_content(page_id, content)?`. NEVER call `get_and_decode_page_content`/rewrite existing ops.
  - Per box ops (append order): `q`, `BT`, `rg` (color `r g B` as `0.0–1.0` floats with 3 decimals), `/ALIAS size Tf`, `1 0 0 1 x y Tm` where `y = page_h_pt - y_top_pt` (PDF bottom-left origin), one `(escaped) Tj` per line with `0 -leading Td` between lines (`leading = 1.2 * size`), `ET`, `Q`.
  - `escape_tj(s)`: escape `\` → `\\`, `(` → `\(`, `)` → `\)`; split on `\n`; drop trailing empty line from final newline; non-Latin1 chars → replace with `?` (Helvetica WinAnsi limitation; document in status bar tooltip, not an error).
  - `ensure_helvetica(page_id) -> String`: read page `/Resources` (`doc.get_page_resources(page_id)` if available in chosen lopdf version, else inline dict lookup — implementer checks `lopdf::Document` API at build time and uses whichever exists; unverified — confirm first). If `/Font` subdict contains a face whose `/BaseFont` contains `Helvetica` (any weight), return that alias. Else create `dict!{"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica","Encoding"=>"WinAnsiEncoding"}` via `doc.new_object_id` + `doc.objects.insert`, insert into Resources `/Font` as `/F_TW`, return `"F_TW"`. If page lacks `/Resources`, create `dict!{"Font" => dict{"F_TW" => ref}}`.
  - Save: `self.doc.save(out)` for Save As; for Save-overwrite write to same source path (caller passes it). After successful save, if target == source path, reload `bytes/doc/pages/sizes` from disk so renderer and writer stay in sync.
- Contingency: if `add_to_page_content` signature differs in resolved lopdf version, build the new stream manually with `lopdf::Stream::new(dict!{}, content.encode()?)` + `doc.new_object_id` and append its reference to the page `/Contents` array (single or array form); same observable result.

### 3. Raster renderer (`src/render.rs`, new, ≤200 LOC)
- Struct `PageRenderer { cache: hayro::RenderCache<'static>, interp: hayro_interpret::InterpreterSettings, zoom: f32, rendered_page: usize, rendered_zoom: f32, texture: Option<egui::TextureHandle> }`. Reuse one `RenderCache` across pages (per hayro example). `interp = InterpreterSettings::default()` (bundled standard-font data via `get_font_data`; no custom font resolver).
- `render_page(&mut self, ctx: &egui::Context, pdf_bytes: &[u8], page_idx: usize, zoom: f32) -> (egui::TextureHandle, f32)`:
  - `let pdf = hayro_syntax::Pdf::new(pdf_bytes.to_vec()).unwrap()` (hayro takes ownership; cache the `Pdf` per open document in `app.rs` instead of rebuilding per frame — see step 5).
  - `let page = &pdf.pages()[page_idx]`; `scale = 2.0 * zoom` (crisp on hidpi; single constant, no per-monitor logic); `pix = hayro::render(page, &cache, &interp, &RenderSettings::default(), &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE })`.
  - Convert: `pix.width()/height()`, `pix.take_u8()` RGBA → `egui::ColorImage::from_rgba_unmultiplied([w,h], &rgba)` → `ctx.load_texture(format!("page-{page_idx}@{zoom:.2}"), img, TextureOptions::LINEAR)`. Cache key `(page_idx, (zoom*100.0) as i32)`; skip re-render when unchanged.
  - Return texture + `points_per_pt = scale_display`: caller computes `page_points_w = size_pt.0 * zoom` (72pt = 1 logical inch; egui points ≈ pt at zoom 1).
  - Contingency: if a PDF fails `Pdf::new` (encrypted — hayro unsupported), show `status: "Cannot open encrypted PDFs"` and keep previous document open. No password UI (out of scope, one inline line).

### 4. Overlay state + coordinate math (`src/overlay.rs`, new, ≤200 LOC)
- `struct OverlayText` (fields per step 2) + `struct OverlayState { items: Vec<OverlayText>, selected: Option<usize>, dirty: bool }` with `add(page,x,y_top)/remove(i)/by_page(p) -> Vec<(usize,&OverlayText)>`.
- Single source of truth, pure functions (unit-tested):
  - `fn screen_to_pdf_top(screen: egui::Pos2, page_rect: egui::Rect, size_pt: (f32,f32), zoom: f32) -> (f32,f32)`: `x_pt = (screen.x-page_rect.min.x)/zoom`, `y_top_pt = (screen.y-page_rect.min.y)/zoom`, clamped to page bounds.
  - `fn pdf_top_to_screen(x_pt,y_top_pt, page_rect, zoom) -> Pos2` (inverse).
  - Text baseline preview offset: single-line preview draws at `screen + (0, size_pt*zoom*0.8)`; multi-line steps `1.2*size*zoom`. Keep constants in this file only.
- Empty text boxes are dropped on commit (no zero-length `Tj`). Newline allowed inside box (multi-line `Tj` lines on save).

### 5. App shell + beautiful UI (`src/app.rs`, new, ≤450 LOC)
- `struct TypewriterApp { pdf_path: Option<PathBuf>, pdf_bytes: Vec<u8>, pdf: Option<hayro_syntax::Pdf>, pdf_doc: Option<pdf::PdfDoc>, renderer: render::PageRenderer, overlays: overlay::OverlayState, page: usize, zoom: f32, mode: Mode, draft: String, draft_size: f32, draft_color: egui::Color32, status: String }`, `enum Mode { Navigate, Type }`. `impl eframe::App for TypewriterApp::update`.
- Layout (top-down, no extra crates):
  - Top toolbar: `Open…` (rfd `.pdf` filter), page `< 3 / 12 >`, zoom out/label/in + `Fit` (zoom = available_w / size_pt.0), `Type` toggle button (accent-filled when active), `Save`, `Save As…` (disabled unless `pdf_bytes` loaded; `Save` disabled unless `dirty`). Keyboard: `Ctrl+O/S`, `+/−`, `Esc` (commit/cancel draft), `Delete` (remove selected).
  - Left sidebar (~260px): "Text boxes" list for current page (each row: first-line preview + size; click selects, ✕ deletes) + inspector for selection/draft: `TextEdit::multiline`, size slider `8–36`, color picker `ColorEdit::button`, hint "click page to place".
  - Center canvas: `ScrollArea::both` + page texture via `egui::Image`, page shadow (`Rect` fill + blur via `Frame` stroke/shadow). Overlays painted as `SelectableLabel`-like text at `pdf_top_to_screen` positions with selected outline. Click in `Type` mode (and no draft open): `screen_to_pdf_top` → create draft box, focus editor. Drag selected box: update `x_pt/y_top_pt` live (`dirty=true`).
  - Status bar: filename, page size pt, zoom %, box count, dirty dot, save result / error strings.
- Theme (`src/theme.rs`, ≤120 LOC, or fold into `app.rs` if <80 lines): `fn apply(ctx)`: `Visuals` — light: warm paper background `#F5F1E8`, dark: `#1E1F24`; accent `#2F6FED`; `rounding 8`, `item_spacing [8,6]`; headings via `RichText::size(15)`. Respect system theme (`ctx.theme()`), only override accents/spacing.
- `Pdf` caching: `hayro_syntax::Pdf` borrows its input bytes — store `pdf_bytes: Vec<u8>` and `pdf: Option<Pdf<'static>>` is impossible (borrow). Solution: implementer MUST use `hayro::hayro_syntax::Pdf::new(pdf_bytes.clone())` once per open and keep the owned `Pdf` in an `Option` alongside leaked/static bytes via `bytes::Bytes`/`Arc` if the API requires `'static`, OR re-parse per render (simpler, acceptable: `Pdf::new` per page change only, not per frame; texture cache avoids per-frame cost). Choose at build time by reading the `Pdf::new` signature; keep the re-parse-per-page-change fallback.

### 6. DRY/KISS guards (apply throughout)
- No second text engine: preview uses egui default font; baked PDF text is always Helvetica — never try to match/mimic original fonts.
- One code path for Save and Save As (`bake_and_save` + path arg). No sidecar files, no AcroForm fields, no OR-search of fonts.
- Coordinate transforms exist ONLY in `overlay.rs`; UI and writer call them.

## Critical files & anchors
- `Cargo.toml` — add `eframe/egui/lopdf/hayro/rfd`; versions resolved at build.
- `src/pdf.rs` — `PdfDoc::bake_and_save`, `ensure_helvetica`, `escape_tj`; anchors: `Document::add_to_page_content(page_id, Content)` (lopdf docs, verified via Context7 this session).
- `src/render.rs` — `hayro::render(page, &cache, &interp, &RenderSettings::default(), &PixmapSettings{..})` + `ColorImage::from_rgba_unmultiplied` → `ctx.load_texture` (both APIs verified this session via docs.rs render.rs example + egui docs).
- `src/overlay.rs` — `screen_to_pdf_top` / `pdf_top_to_screen` (only transforms in codebase).
- `src/app.rs` — `impl eframe::App`, toolbar/sidebar/canvas/statusbar layout, `rfd::FileDialog` open/save wiring.

## Verification
- `cargo clippy --workspace --all-targets -- -Dclippy::pedantic` clean (repo rule) + `cargo test`.
- New unit tests (`overlay.rs`, `pdf.rs`): `screen↔pdf` roundtrip at zoom 1.0/1.5 (`(10pt,20pt)` ↔ screen offset exact); `escape_tj("a(b)\\c") == "a\\(b\\)\\\\c"`; `Tm` y-flip: box at `y_top=20` on 800pt page emits `y=780.0`; multi-line `"a\nb"` emits two `Tj` ops with `0 -14.4 Td` at size 12.
- End-to-end manual (once): `cargo run` → Open a 2-page PDF → zoom 150% → Type-mode click places box → type `Hello 12pt blue` + second line → drag box → Save As `/tmp/out.pdf` → `cargo run` reopen `/tmp/out.pdf` shows baked text at same position; system viewer (evince/okular) shows identical; `Save` overwrites only after user confirmation dialog and sets dirty=false.
- Regression guard: reopening a saved file shows previously baked text as page pixels (NOT as editable boxes) — proves append-only, no double-bake.

## Assumptions & contingencies
- Assumes `hayro 0.9` `render()` + `Pixmap::take_u8/into_png` shape from the upstream `render.rs` example read this session; if the resolved version renamed `take_u8`, use `as_slice`/`into_png` bytes decoded to RGBA (same result).
- Assumes `InterpreterSettings::default()` renders standard PDFs without custom font resolver; if CJK PDFs show tofu, add resolver mapping `StandardFont::*` to bundled Liberation fonts (pattern in the example read this session) — implementer adds only then.
- Encrypted PDFs: show status error, keep old doc (no password UI).
- Non-Latin1 glyphs bake as `?` (Helvetica WinAnsi); noted in UI tooltip, not blocking.
