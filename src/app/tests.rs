use super::{CanvasView, CloseState, Mode, TypewriterApp};
use crate::overlay::{BoxKind, OverlayState};
use crate::render::PageRenderer;
use crate::test_util::TempFile;
use eframe::egui;
use lopdf::{Document, Object, dictionary};

fn test_app() -> TypewriterApp {
    TypewriterApp {
        pdf_path: None,
        pdf_doc: None,
        hayro_pdf: None,
        renderer: PageRenderer::default(),
        overlays: OverlayState::default(),
        page: 0,
        zoom: 1.0,
        mode: Mode::Navigate,
        status: String::new(),
        focus_editor: false,
        fit_pending: false,
        resize_box: None,
        clipboard: None,
        close_state: CloseState::Idle,
        box_layouts: std::collections::HashMap::new(),
        canvas_view: None,
    }
}

#[test]
fn moving_canvas_origin_preserves_page_screen_position() {
    let mut app = test_app();
    let previous_origin = egui::pos2(600.0, 150.0);
    let previous_offset = egui::vec2(120.0, 80.0);
    app.canvas_view = Some(CanvasView {
        page: 0,
        zoom_bits: 1.0_f32.to_bits(),
        origin: previous_origin,
        scroll_offset: previous_offset,
    });

    let new_origin = egui::pos2(400.0, 150.0);
    let new_offset = app
        .canvas_scroll_offset(
            new_origin,
            egui::vec2(900.0, 700.0),
            egui::vec2(1200.0, 1000.0),
        )
        .unwrap();

    assert_eq!(
        new_origin - new_offset,
        previous_origin - previous_offset,
        "moving the sidebar must not drag the page across the screen"
    );
}

#[test]
fn sidebar_width_is_not_pushed_out_by_hint_text_when_resized_narrow() {
    use eframe::egui::{Context, Event, PointerButton, Pos2, RawInput, Rect, Vec2};

    let mut app = test_app();
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 640.0));
    let ctx = Context::default();
    super::theme::apply_theme(&ctx);

    let run_sidebar = |app: &mut TypewriterApp, events: Vec<Event>| -> f32 {
        let mut width = 0.0;
        let _ = ctx.run(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |ctx| {
                app.sidebar(ctx);
                width = ctx.used_rect().width();
            },
        );
        width
    };

    // Establish initial layout (default 300px inner + 2*14px margin + stroke).
    let _ = run_sidebar(&mut app, Vec::new());
    let initial_width = run_sidebar(&mut app, Vec::new());

    // Drag the sidebar's right resize border from initial_width down to 250px (min_width),
    // release, then move the mouse around without holding any button.
    let handle = Pos2::new(initial_width, 300.0);
    let narrow = Pos2::new(250.0, 300.0);
    let _ = run_sidebar(&mut app, vec![Event::PointerMoved(handle)]);
    let _ = run_sidebar(
        &mut app,
        vec![Event::PointerButton {
            pos: handle,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let dragged_width = run_sidebar(&mut app, vec![Event::PointerMoved(narrow)]);
    let _ = run_sidebar(
        &mut app,
        vec![Event::PointerButton {
            pos: narrow,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );

    assert!(
        (initial_width - 300.0).abs() <= f32::EPSILON,
        "sidebar should start at default 300px instead of drifting on layout, got {initial_width}"
    );
    assert!(
        (dragged_width - 250.0).abs() <= f32::EPSILON,
        "sidebar should narrow to 250px when dragged, got {dragged_width}"
    );

    // Subsequent frames with mouse movement (both empty state and with selected boxes)
    // must keep the narrowed width rather than creeping or snapping back out.
    for i in 0..20 {
        let width = run_sidebar(
            &mut app,
            vec![Event::PointerMoved(Pos2::new(
                120.0 + f32::from(u8::try_from(i).unwrap()),
                200.0,
            ))],
        );
        assert!(
            (width - dragged_width).abs() <= f32::EPSILON,
            "empty-state sidebar crept from {dragged_width} to {width} on frame {i}"
        );
    }

    for kind in [BoxKind::Text, BoxKind::Signature] {
        let id = app.overlays.add(0, 10.0, 20.0, kind);
        app.overlays.item_mut(id).unwrap().text = "WWWWWWWWWWWWWWWWWWWWWWWWWW".to_string();
        for i in 0..10 {
            let width = run_sidebar(
                &mut app,
                vec![Event::PointerMoved(Pos2::new(
                    140.0 + f32::from(u8::try_from(i).unwrap()),
                    220.0,
                ))],
            );
            assert!(
                (width - dragged_width).abs() <= f32::EPSILON,
                "sidebar with {kind:?} crept from {dragged_width} to {width} on frame {i}"
            );
        }
    }
}

/// Write a minimal `pages`-page PDF to a temp file (deleted on drop).
fn write_pdf(pages: usize) -> TempFile {
    let mut doc = Document::new();
    let page_tree_id = doc.new_object_id();
    let mut kids = Vec::new();
    for _ in 0..pages {
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(page_tree_id),
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(Object::Reference(page_id));
    }
    doc.set_object(
        page_tree_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => i32::try_from(pages).unwrap(),
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(page_tree_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));
    let file = TempFile::new("app");
    doc.save(file.path()).unwrap();
    file
}

#[test]
fn reload_after_save_preserves_page_and_zoom() {
    let path = write_pdf(3);
    let mut app = test_app();
    assert!(app.load(path.path().to_path_buf()));
    app.page = 2;
    app.zoom = 1.7;
    app.reload_after_save(path.path());
    assert_eq!(app.page, 2);
    assert!((app.zoom - 1.7).abs() < f32::EPSILON);
    assert!(
        app.status.starts_with("Saved "),
        "unexpected status: {}",
        app.status
    );
}

#[test]
fn reload_after_save_clamps_page_to_new_page_count() {
    let path = write_pdf(3);
    let shorter = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(path.path().to_path_buf()));
    app.page = 2;
    app.reload_after_save(shorter.path());
    assert_eq!(app.page, 0);
}

#[test]
fn failed_reload_after_save_is_surfaced_not_masked_by_saved() {
    let path = write_pdf(3);
    let mut app = test_app();
    assert!(app.load(path.path().to_path_buf()));
    app.page = 1;
    app.zoom = 1.7;
    let missing = path
        .path()
        .with_file_name("pdf-editor-app-test-no-such-file.pdf");
    app.reload_after_save(&missing);
    assert!(
        app.status.contains("reload failed"),
        "reload failure masked by status: {}",
        app.status
    );
    // The still-loaded document keeps the user's place.
    assert_eq!(app.page, 1);
    assert!((app.zoom - 1.7).abs() < f32::EPSILON);
    assert_eq!(app.page_count(), 3);
}

#[test]
fn placed_box_editor_focuses_so_typing_edits_the_box() {
    use eframe::egui::{Context, Event, Pos2, RawInput, Rect, Vec2};

    let mut app = test_app();
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    // Set by canvas placement (see `focus_editor = true` on page click).
    app.focus_editor = true;

    let ctx = Context::default();
    let run = |app: &mut TypewriterApp, events: Vec<Event>| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events,
            ..RawInput::default()
        };
        let _output = ctx.run(input, |ctx| app.sidebar(ctx));
    };

    // Frame that renders the inspector: consumes the one-shot flag and
    // requests focus. One more frame lets egui grant it.
    run(&mut app, Vec::new());
    assert!(!app.focus_editor, "focus_editor must be one-shot");
    run(&mut app, Vec::new());

    // Typing without clicking the sidebar must land in the new box.
    run(&mut app, vec![Event::Text("hi".to_owned())]);
    assert_eq!(app.overlays.items[0].text, "hi");
}

#[test]
fn inspector_stays_on_screen_with_many_boxes() {
    use eframe::egui::{Context, Pos2, RawInput, Rect, Shape, Vec2};

    let mut app = test_app();
    for _ in 0..30 {
        app.overlays.add(0, 10.0, 20.0, BoxKind::Signature);
    }
    // Minimum window height (see `main.rs`): the regression case,
    // where 30 list entries pushed the Size slider off-screen.
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 640.0));
    let ctx = Context::default();
    // Registers the Momo Signature font the inspector preview needs
    // (`test_app` bypasses `TypewriterApp::new`, which normally does).
    super::theme::apply_theme(&ctx);
    // Panels size themselves from the previous frame, so run a few
    // frames (as the real app does) before measuring the last one.
    let mut output = None;
    for _ in 0..4 {
        output = Some(ctx.run(
            RawInput {
                screen_rect: Some(screen),
                ..RawInput::default()
            },
            |ctx| app.sidebar(ctx),
        ));
    }
    let output = output.expect("at least one frame ran");
    // The slider's "Size" label is painted text; find it among the
    // emitted shapes and require it inside the screen rect.
    let size_labels: Vec<Pos2> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.job.text == "Size" => Some(text.pos),
            _ => None,
        })
        .collect();
    assert_eq!(size_labels.len(), 1, "expected one Size slider label");
    assert!(
        screen.contains(size_labels[0]),
        "Size label at {:?} must be on screen {screen:?}",
        size_labels[0]
    );
}

#[test]
fn copy_then_paste_lands_on_current_page_and_cut_clears_source() {
    let mut app = test_app();
    // Paste guards on an open document.
    app.pdf_doc = None;
    let id = app.overlays.add(0, 50.0, 60.0, BoxKind::Signature);
    app.overlays.item_mut(id).unwrap().text = "Ada".to_string();
    app.copy_selected();
    assert!(app.clipboard.is_some());
    app.paste_clipboard();
    assert_eq!(app.overlays.items.len(), 1, "no document: paste is a no-op");

    let path = write_pdf(3);
    assert!(app.load(path.path().to_path_buf()));
    // Clipboard survives load by design (cross-document paste).
    app.page = 2;
    app.paste_clipboard();
    assert_eq!(app.overlays.items.len(), 1);
    let pasted = app.overlays.selected.unwrap();
    let item = app.overlays.item(pasted).unwrap();
    assert_eq!(item.kind, BoxKind::Signature);
    assert_eq!(item.page, 2);
    assert_eq!(item.text, "Ada");

    app.cut_selected();
    assert!(app.overlays.items.is_empty());
    assert_eq!(app.overlays.selected, None);
    app.page = 1;
    app.paste_clipboard();
    assert_eq!(app.overlays.items.len(), 1);
    assert_eq!(app.overlays.items[0].page, 1);
}

fn run_canvas(
    ctx: &egui::Context,
    app: &mut TypewriterApp,
    screen: egui::Rect,
    zoom: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..egui::RawInput::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let page_resp = ui.allocate_rect(screen, egui::Sense::click_and_drag());
                app.draw_boxes(ui, screen, 0, zoom, &page_resp);
            });
        },
    )
}

fn drag_canvas(
    ctx: &egui::Context,
    app: &mut TypewriterApp,
    screen: egui::Rect,
    from: egui::Pos2,
    to: egui::Pos2,
) {
    use eframe::egui::{Event, Modifiers, PointerButton};
    let btn = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    for events in [
        Vec::new(),
        vec![Event::PointerMoved(from)],
        vec![btn(from, true)],
        vec![Event::PointerMoved(to)],
        vec![btn(to, false)],
    ] {
        let _ = run_canvas(ctx, app, screen, 1.0, events);
    }
}

#[test]
fn resize_handle_inner_quadrant_resizes_instead_of_dragging_box() {
    use eframe::egui::{Context, Pos2, Rect, Vec2};

    let mut app = test_app();
    let id = app.overlays.add(0, 100.0, 100.0, BoxKind::Text);
    {
        let item = app.overlays.item_mut(id).unwrap();
        item.text = "Hello".to_string();
        item.width_pt = Some(80.0);
    }

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(612.0, 792.0));
    drag_canvas(
        &Context::default(),
        &mut app,
        screen,
        Pos2::new(176.0, 110.0),
        Pos2::new(226.0, 130.0),
    );

    let item = app.overlays.item(id).unwrap();
    assert!(
        (item.x_pt - 100.0).abs() <= f32::EPSILON && (item.y_top_pt - 100.0).abs() <= f32::EPSILON,
        "dragging inner quadrant of resize handle must not move box, got ({}, {})",
        item.x_pt,
        item.y_top_pt
    );
    assert!(
        (item.width_pt.unwrap() - 130.0).abs() <= f32::EPSILON,
        "dragging inner quadrant of resize handle by +50pt must widen box from 80 to 130, got {:?}",
        item.width_pt
    );
}

#[test]
fn dragging_box_clamps_to_page_bounds() {
    use eframe::egui::{Context, Pos2, Rect, Vec2};

    let mut app = test_app();
    let id = app.overlays.add(0, 100.0, 100.0, BoxKind::Text);
    {
        let item = app.overlays.item_mut(id).unwrap();
        item.text = "Hello".to_string();
        item.width_pt = Some(80.0);
    }

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(612.0, 792.0));
    drag_canvas(
        &Context::default(),
        &mut app,
        screen,
        Pos2::new(120.0, 105.0),
        Pos2::new(900.0, 1000.0),
    );

    let item = app.overlays.item(id).unwrap();
    let max_left = 612.0 - 80.0;
    let max_top = 792.0 - crate::overlay::LEADING * item.size_pt;
    assert!(
        (item.x_pt - max_left).abs() <= 1e-4,
        "x_pt should clamp to {max_left}, got {}",
        item.x_pt
    );
    assert!(
        (item.y_top_pt - max_top).abs() <= 1e-4,
        "y_top_pt should clamp to {max_top}, got {}",
        item.y_top_pt
    );
}

#[test]
fn preview_baseline_matches_baked_baseline() {
    use eframe::egui::{Context, Pos2, Rect, Shape, Vec2};

    let mut app = test_app();
    let text_id = app.overlays.add(0, 72.0, 120.0, BoxKind::Text);
    {
        let item = app.overlays.item_mut(text_id).unwrap();
        item.text = "First line\nSecond line".to_string();
        item.size_pt = 18.0;
    }
    let sig_id = app.overlays.add(0, 72.0, 240.0, BoxKind::Signature);
    app.overlays.item_mut(sig_id).unwrap().text = "Ada Lovelace".to_string();

    let zoom = 1.5;
    let page_h_pt = 792.0;
    let screen = Rect::from_min_size(
        Pos2::new(20.0, 30.0),
        Vec2::new(612.0 * zoom, page_h_pt * zoom),
    );
    let ctx = Context::default();
    super::theme::apply_theme(&ctx);
    let output = run_canvas(&ctx, &mut app, screen, zoom, Vec::new());

    let baseline_pdf_y = |line_text: &str| -> f32 {
        let shape = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(t) if t.galley.job.text == line_text => Some(t),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing painted text shape for {line_text:?}"));
        let screen_baseline_y = shape.pos.y + shape.galley.rows[0].glyphs[0].pos.y;
        let top_pt = (screen_baseline_y - screen.min.y) / zoom;
        page_h_pt - top_pt
    };

    let text_item = app.overlays.item(text_id).unwrap();
    let baked_text_y = crate::overlay::text_matrix(text_item, (0.0, 0.0, 612.0, page_h_pt), 0)[5];
    assert!(
        (baseline_pdf_y("First line") - baked_text_y).abs() <= 1e-3,
        "line 0 preview baseline must match baked baseline"
    );
    let baked_line2_y = baked_text_y - crate::overlay::LEADING * text_item.size_pt;
    assert!(
        (baseline_pdf_y("Second line") - baked_line2_y).abs() <= 1e-3,
        "line 1 preview baseline must match baked baseline"
    );

    let sig_item = app.overlays.item(sig_id).unwrap();
    let baked_sig_y = crate::overlay::text_matrix(sig_item, (0.0, 0.0, 612.0, page_h_pt), 0)[5];
    assert!(
        (baseline_pdf_y("Ada Lovelace") - baked_sig_y).abs() <= 1e-3,
        "signature preview baseline must match baked baseline"
    );
}
