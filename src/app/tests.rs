use super::{
    CanvasView, CloseState, Mode, PendingAction, PromptChoice, SaveMode, TypewriterApp,
    overwrite_needs_confirmation,
};
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
        inspector_gesture: None,
        clipboard: None,
        close_state: CloseState::Idle,
        overwrite_confirmed_for: None,
        box_layouts: std::collections::HashMap::new(),
        canvas_view: None,
        dialog_rx: None,
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
fn preview_wrap_uses_helvetica_metrics_not_ui_font() {
    let mut overlays = OverlayState::default();
    let id = overlays.add(0, 0.0, 0.0, BoxKind::Text);
    let item = overlays.item_mut(id).unwrap();
    item.text = "The quick brown fox jumps over the lazy dog".to_string();
    item.size_pt = 12.0;
    item.width_pt = Some(235.0);
    let item = overlays.item(id).unwrap();
    // 237.4 pt wide in Helvetica at 12 pt, so the baked PDF wraps it
    // at 235 pt; egui's proportional font measures ~233.7 pt and the
    // old preview kept it on one line.
    let lines = super::canvas::box_display_lines(item);
    assert_eq!(lines.len(), 2);
    assert!(
        lines
            .iter()
            .all(|line| crate::pdf::helvetica_width_pt(line, 12.0) <= 235.0)
    );
    assert_eq!(lines.join(" "), item.text);
}

#[test]
fn text_preview_font_measures_like_helvetica() {
    let ctx = egui::Context::default();
    super::theme::apply_theme(&ctx);
    let sentence = "The quick brown fox jumps over the lazy dog";
    // Measure at 120 pt: egui rounds glyph advances to whole pixels,
    // which at 12 pt would swamp the metric comparison.
    let helvetica = crate::pdf::helvetica_width_pt(sentence, 120.0);
    // Fonts only exist after a first run(); measure inside it.
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        let measure = |text: &str, font: egui::FontId| {
            ctx.fonts(|fonts| {
                fonts
                    .layout_no_wrap(text.to_string(), font, egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let preview = measure(sentence, crate::text_font::font_id(120.0));
        assert!(
            (preview - helvetica).abs() < 4.0,
            "Liberation preview {preview} vs Helvetica {helvetica}"
        );
        // Guard against silently falling back to egui's default face:
        // its `i` is ~12% wider than Helvetica's, Liberation's matches.
        let narrow = "iiiiiiiiii";
        let preview_i = measure(narrow, crate::text_font::font_id(120.0));
        let helvetica_i = crate::pdf::helvetica_width_pt(narrow, 120.0);
        assert!((preview_i - helvetica_i).abs() < 4.0);
        let fallback_i = measure(narrow, egui::FontId::proportional(120.0));
        assert!(
            (fallback_i - helvetica_i).abs() > 20.0,
            "fallback i {fallback_i} vs Helvetica {helvetica_i}"
        );
    });
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
    let btn = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = run_sidebar(&mut app, vec![Event::PointerMoved(handle)]);
    let _ = run_sidebar(&mut app, vec![btn(handle, true)]);
    let dragged_width = run_sidebar(&mut app, vec![Event::PointerMoved(narrow)]);
    let _ = run_sidebar(&mut app, vec![btn(narrow, false)]);

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
    for i in 0_u8..20 {
        let pos = Pos2::new(120.0 + f32::from(i), 200.0);
        let width = run_sidebar(&mut app, vec![Event::PointerMoved(pos)]);
        assert!(
            (width - dragged_width).abs() <= f32::EPSILON,
            "empty-state sidebar crept from {dragged_width} to {width} on frame {i}"
        );
    }

    for kind in [BoxKind::Text, BoxKind::Signature] {
        let id = app.overlays.add(0, 10.0, 20.0, kind);
        app.overlays.item_mut(id).unwrap().text = "WWWWWWWWWWWWWWWWWWWWWWWWWW".to_string();
        for i in 0_u8..10 {
            let pos = Pos2::new(140.0 + f32::from(i), 220.0);
            let width = run_sidebar(&mut app, vec![Event::PointerMoved(pos)]);
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
fn refresh_after_save_preserves_page_and_zoom() {
    let path = write_pdf(3);
    let mut app = test_app();
    let id = app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.resize_box = Some(id);
    app.focus_editor = true;
    assert!(app.load(path.path().to_path_buf()));
    assert_eq!(app.resize_box, None);
    assert!(!app.focus_editor);
    app.page = 2;
    app.zoom = 1.7;
    app.refresh_after_save(path.path());
    assert_eq!(app.page, 2);
    assert!((app.zoom - 1.7).abs() < f32::EPSILON);
    assert!(
        app.status.starts_with("Saved "),
        "unexpected status: {}",
        app.status
    );
}

#[test]
fn changing_page_clears_selection_and_drops_empty_box() {
    let mut app = test_app();
    let kept = app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.overlays.item_mut(kept).unwrap().text = "keep me".to_string();
    let empty = app.overlays.add(0, 50.0, 60.0, BoxKind::Text);
    app.change_page(1);
    assert_eq!(app.page, 1);
    assert_eq!(app.overlays.selected, None);
    assert!(app.overlays.item(empty).is_none(), "empty box dropped");
    assert_eq!(app.overlays.items.len(), 1);
    // A non-empty selection is cleared too, but its box stays put:
    // Delete/Duplicate must never act on an invisible box.
    app.overlays.select(Some(kept));
    app.change_page(2);
    assert_eq!(app.page, 2);
    assert_eq!(app.overlays.selected, None);
    assert_eq!(app.overlays.item(kept).unwrap().text, "keep me");
}

#[test]
fn refresh_after_save_restores_baked_boxes_and_keeps_place() {
    // The old disk reload re-read the file after every save; the
    // in-memory refresh must produce the same observable state:
    // boxes back as live overlays, preview rebuilt, place kept,
    // and the target recorded as the open path (Save As).
    let path = write_pdf(2);
    let out = TempFile::new("app-refresh");
    let mut app = test_app();
    assert!(app.load(path.path().to_path_buf()));
    let id = app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.overlays.item_mut(id).unwrap().text = "hello".to_string();
    app.page = 1;
    app.zoom = 1.7;
    let target = out.path().to_path_buf();
    app.pdf_doc
        .as_mut()
        .unwrap()
        .bake_and_save(&mut app.overlays, &target)
        .unwrap();
    assert!(app.overlays.items.is_empty(), "bake drains overlays");
    app.refresh_after_save(&target);
    assert_eq!(app.page, 1);
    assert!((app.zoom - 1.7).abs() < f32::EPSILON);
    assert_eq!(app.overlays.items.len(), 1);
    assert_eq!(app.overlays.items[0].text, "hello");
    assert!(
        !app.overlays.is_dirty(),
        "restored boxes are the clean state"
    );
    assert!(
        app.status.starts_with("Saved "),
        "unexpected status: {}",
        app.status
    );
    assert_eq!(app.pdf_path.as_deref(), Some(target.as_path()));
    assert!(app.hayro_pdf.is_some(), "preview rebuilt from memory");
}

#[test]
fn opening_another_pdf_with_unsaved_boxes_prompts_instead_of_loading() {
    let current = write_pdf(2);
    let next = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);

    app.begin_open(next.path().to_path_buf());

    assert_eq!(
        app.close_state,
        CloseState::Prompting(PendingAction::Open(next.path().to_path_buf()))
    );
    assert_eq!(app.overlays.items.len(), 1);
    assert!(app.overlays.is_dirty());
    assert_eq!(app.pdf_path.as_deref(), Some(current.path()));
    assert_eq!(app.page_count(), 2);
}

#[test]
fn cancelling_pending_open_keeps_current_document_and_boxes() {
    let current = write_pdf(2);
    let next = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.begin_open(next.path().to_path_buf());

    app.resolve_prompt(&egui::Context::default(), PromptChoice::Cancel);

    assert_eq!(app.close_state, CloseState::Idle);
    assert_eq!(app.overlays.items.len(), 1);
    assert!(app.overlays.is_dirty());
    assert_eq!(app.pdf_path.as_deref(), Some(current.path()));
    assert_eq!(app.page_count(), 2);
}

/// Run one frame whose only input is a window-close request and
/// return the viewport commands `guard_close` emitted.
fn close_request_commands(app: &mut TypewriterApp) -> Vec<egui::ViewportCommand> {
    let ctx = egui::Context::default();
    let mut input = egui::RawInput::default();
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..egui::ViewportInfo::default()
        },
    );
    let output = ctx.run(input, |ctx| app.guard_close(ctx));
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map_or_else(Vec::new, |viewport| viewport.commands.clone())
}

#[test]
fn window_close_during_pending_open_prompt_is_cancelled_and_reaimed_at_close() {
    let current = write_pdf(2);
    let next = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.begin_open(next.path().to_path_buf());
    assert_eq!(
        app.close_state,
        CloseState::Prompting(PendingAction::Open(next.path().to_path_buf()))
    );

    let commands = close_request_commands(&mut app);

    assert!(
        commands.contains(&egui::ViewportCommand::CancelClose),
        "close during Prompting(Open) must be cancelled, got {commands:?}"
    );
    assert_eq!(app.close_state, CloseState::Prompting(PendingAction::Close));
    assert_eq!(app.overlays.items.len(), 1);
    assert!(app.overlays.is_dirty());
    assert_eq!(app.pdf_path.as_deref(), Some(current.path()));
}

#[test]
fn window_close_during_pending_close_prompt_is_cancelled() {
    let mut app = test_app();
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.close_state = CloseState::Prompting(PendingAction::Close);

    let commands = close_request_commands(&mut app);

    assert!(
        commands.contains(&egui::ViewportCommand::CancelClose),
        "second close during Prompting(Close) must be cancelled, got {commands:?}"
    );
    assert_eq!(app.close_state, CloseState::Prompting(PendingAction::Close));
    assert_eq!(app.overlays.items.len(), 1);
    assert!(app.overlays.is_dirty());
}

#[test]
fn window_close_without_prompt_still_prompts_only_when_dirty() {
    // Dirty + Idle: existing behaviour — cancel and prompt.
    let mut app = test_app();
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    let commands = close_request_commands(&mut app);
    assert!(commands.contains(&egui::ViewportCommand::CancelClose));
    assert_eq!(app.close_state, CloseState::Prompting(PendingAction::Close));

    // Clean + Idle and Allowed: the close must pass through.
    for state in [CloseState::Idle, CloseState::Allowed] {
        let mut app = test_app();
        app.close_state = state.clone();
        let commands = close_request_commands(&mut app);
        assert!(
            !commands.contains(&egui::ViewportCommand::CancelClose),
            "close in {state:?} must pass through, got {commands:?}"
        );
        assert_eq!(app.close_state, state);
    }
}

#[test]
fn discarding_pending_open_loads_new_pdf_and_clears_boxes() {
    let current = write_pdf(2);
    let next = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.begin_open(next.path().to_path_buf());

    app.resolve_prompt(&egui::Context::default(), PromptChoice::Discard);

    assert_eq!(app.close_state, CloseState::Idle);
    assert_eq!(app.overlays.items, Vec::new());
    assert!(!app.overlays.is_dirty());
    assert_eq!(app.pdf_path.as_deref(), Some(next.path()));
    assert_eq!(app.page_count(), 1);
}

#[test]
fn discarding_pending_open_to_missing_file_keeps_boxes() {
    let current = write_pdf(2);
    let missing = TempFile::new("missing");
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.begin_open(missing.path().to_path_buf());

    app.resolve_prompt(&egui::Context::default(), PromptChoice::Discard);

    assert_eq!(app.close_state, CloseState::Idle);
    assert_eq!(app.overlays.items.len(), 1);
    assert!(app.overlays.is_dirty());
    assert_eq!(app.pdf_path.as_deref(), Some(current.path()));
    assert_eq!(app.page_count(), 2);
}

#[test]
fn failed_save_keeps_pending_open_and_boxes() {
    // No document loaded: `save` fails before any dialog, exercising
    // the same failed-save branch as a cancelled overwrite prompt.
    let next = write_pdf(1);
    let mut app = test_app();
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.begin_open(next.path().to_path_buf());

    app.resolve_prompt(&egui::Context::default(), PromptChoice::Save);

    assert_eq!(
        app.close_state,
        CloseState::Prompting(PendingAction::Open(next.path().to_path_buf()))
    );
    assert_eq!(app.overlays.items.len(), 1);
    assert!(app.overlays.is_dirty());
    assert_eq!(app.pdf_path, None);
}

#[test]
fn overwrite_confirmation_is_needed_only_until_that_file_is_confirmed() {
    let confirmed = std::path::PathBuf::from("confirmed.pdf");
    let other = std::path::PathBuf::from("other.pdf");
    assert!(overwrite_needs_confirmation(None, &confirmed));
    assert!(!overwrite_needs_confirmation(Some(&confirmed), &confirmed));
    assert!(overwrite_needs_confirmation(Some(&other), &confirmed));
}

#[test]
fn saving_from_pending_open_prompt_writes_without_second_confirmation() {
    let current = write_pdf(2);
    let next = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));
    let id = app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.overlays.item_mut(id).unwrap().text = "Ada".to_string();
    app.begin_open(next.path().to_path_buf());

    app.resolve_prompt(&egui::Context::default(), PromptChoice::Save);

    // The prompt completes only when `save` actually wrote the
    // file; a second (overwrite) dialog would leave it Prompting.
    assert_eq!(app.close_state, CloseState::Idle);
    assert_eq!(app.overwrite_confirmed_for.as_deref(), Some(current.path()));
    assert_eq!(app.pdf_path.as_deref(), Some(next.path()));
    assert_eq!(app.overlays.items, Vec::new());
    assert_eq!(app.page_count(), 1);
}

#[test]
fn confirmed_in_place_save_writes_repeatedly_without_asking_again() {
    let path = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(path.path().to_path_buf()));
    // As recorded by a first confirmed overwrite (or prompt Save).
    app.overwrite_confirmed_for = Some(path.path().to_path_buf());

    for text in ["Ada", "Grace"] {
        let id = app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
        app.overlays.item_mut(id).unwrap().text = text.to_string();
        assert!(app.save(SaveMode::InPlace), "save of {text} must write");
        assert!(!app.overlays.is_dirty());
        // The save reloads the same path; consent must survive it,
        // or the next Ctrl+S would ask again.
        assert_eq!(app.overwrite_confirmed_for.as_deref(), Some(path.path()));
        assert_eq!(app.pdf_path.as_deref(), Some(path.path()));
    }
}

#[test]
fn opening_another_pdf_without_unsaved_boxes_loads_immediately() {
    let current = write_pdf(2);
    let next = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(current.path().to_path_buf()));

    app.begin_open(next.path().to_path_buf());

    assert_eq!(app.close_state, CloseState::Idle);
    assert_eq!(app.pdf_path.as_deref(), Some(next.path()));
    assert_eq!(app.page_count(), 1);
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
fn typing_burst_with_key_release_frames_is_one_undo_step() {
    use eframe::egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

    let mut app = test_app();
    app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.focus_editor = true;

    let ctx = Context::default();
    let run = |app: &mut TypewriterApp, events: Vec<Event>, time: f64| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            time: Some(time),
            events,
            ..RawInput::default()
        };
        let _output = ctx.run(input, |ctx| app.sidebar(ctx));
    };
    // Grant the editor focus.
    run(&mut app, Vec::new(), 0.0);
    run(&mut app, Vec::new(), 0.016);

    // egui-winit repaints on every key release: each keystroke is a
    // changed (press) frame followed by an unchanged (release) frame.
    // The release frames must not split the burst into per-key steps.
    for (index, ch) in "hello".chars().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let press_time = 0.1 + index as f64 * 0.05;
        run(&mut app, vec![Event::Text(ch.to_string())], press_time);
        run(
            &mut app,
            vec![Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            press_time + 0.02,
        );
    }
    assert_eq!(app.overlays.items[0].text, "hello");
    assert!(app.overlays.undo(), "typing burst must be undoable");
    assert_eq!(
        app.overlays.items[0].text, "",
        "one undo must revert the whole burst, not the last keystroke"
    );
    assert!(app.overlays.redo());
    assert_eq!(app.overlays.items[0].text, "hello");

    // After an idle gap the next keystroke is a separate undo step.
    run(&mut app, Vec::new(), 2.0);
    run(&mut app, vec![Event::Text("!".to_owned())], 2.1);
    run(&mut app, Vec::new(), 2.12);
    assert_eq!(app.overlays.items[0].text, "hello!");
    assert!(app.overlays.undo());
    assert_eq!(app.overlays.items[0].text, "hello");
    assert!(app.overlays.undo());
    assert_eq!(app.overlays.items[0].text, "");
}

#[test]
fn inspector_native_undo_is_per_box_for_both_editors() {
    use eframe::egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

    for kind in [BoxKind::Text, BoxKind::Signature] {
        let mut app = test_app();
        let id_a = app.overlays.add(0, 10.0, 20.0, kind);
        app.overlays.item_mut(id_a).unwrap().text = "Box A text".to_string();
        let id_b = app.overlays.add(0, 10.0, 20.0, kind);
        app.overlays.item_mut(id_b).unwrap().text = "Box B text".to_string();
        // Typing makes the boxes non-empty so selecting between them
        // never drops one as an empty placeholder.
        app.overlays.end_gesture();

        let ctx = Context::default();
        super::theme::apply_theme(&ctx);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0));
        let run = |app: &mut TypewriterApp, events: Vec<Event>| {
            let _output = ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..RawInput::default()
                },
                |ctx| app.sidebar(ctx),
            );
        };
        let undo_key = || Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };

        // Focus A's editor (as placement does) and type once, giving
        // A's native undo history one entry.
        app.overlays.select(Some(id_a));
        app.focus_editor = true;
        run(&mut app, Vec::new());
        run(&mut app, Vec::new());
        run(&mut app, vec![Event::Text("X".to_owned())]);
        let edited_a = app.overlays.item(id_a).unwrap().text.clone();
        assert_ne!(
            edited_a, "Box A text",
            "typing in {kind:?} editor A must change its text"
        );

        // Switch to B. Its editor must start with B's text, not A's.
        app.overlays.select(Some(id_b));
        app.focus_editor = true;
        run(&mut app, Vec::new());
        run(&mut app, Vec::new());
        assert_eq!(
            app.overlays.item(id_b).unwrap().text,
            "Box B text",
            "selecting {kind:?} editor B must not pull in A's text"
        );
        // Native undo in B must not restore A's text: histories are
        // per-box. With a shared widget ID this becomes "Box A text".
        run(&mut app, vec![undo_key()]);
        assert_eq!(
            app.overlays.item(id_b).unwrap().text,
            "Box B text",
            "native undo in {kind:?} editor B must not insert A's text"
        );
        assert_eq!(
            app.overlays.item(id_a).unwrap().text,
            edited_a,
            "native undo in B must leave {kind:?} editor A alone"
        );

        // Returning to A keeps A's own native history: undo reverts
        // the typed "X" and restores A's original text.
        app.overlays.select(Some(id_a));
        app.focus_editor = true;
        run(&mut app, Vec::new());
        run(&mut app, Vec::new());
        run(&mut app, vec![undo_key()]);
        assert_eq!(
            app.overlays.item(id_a).unwrap().text,
            "Box A text",
            "returning to {kind:?} editor A must keep its own undo history"
        );
        assert_eq!(
            app.overlays.item(id_b).unwrap().text,
            "Box B text",
            "undo in A must leave {kind:?} editor B alone"
        );
    }
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
    assert_eq!(app.overlays.items, Vec::new());
    assert_eq!(app.overlays.selected, None);
    app.page = 1;
    app.paste_clipboard();
    assert_eq!(app.overlays.items.len(), 1);
    assert_eq!(app.overlays.items[0].page, 1);
}

#[test]
fn copy_cut_paste_events_copy_cut_and_paste_box() {
    use eframe::egui::{Context, Event, Pos2, RawInput, Rect, Vec2};

    // Real eframe/egui-winit never delivers Ctrl/Cmd+C/X/V as
    // `Event::Key`: it intercepts them and emits `Event::Copy`,
    // `Event::Cut`, or `Event::Paste` instead. Simulate that wire
    // format; `Event::Key` simulations cannot catch this regression.
    let mut app = test_app();
    let path = write_pdf(2);
    assert!(app.load(path.path().to_path_buf()));
    let id = app.overlays.add(0, 50.0, 60.0, BoxKind::Text);
    app.overlays.item_mut(id).unwrap().text = "Ada".to_string();

    let ctx = Context::default();
    let run = |app: &mut TypewriterApp, events: Vec<Event>| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events,
            ..RawInput::default()
        };
        let _output = ctx.run(input, |ctx| app.handle_shortcuts(ctx));
    };

    run(&mut app, vec![Event::Copy]);
    assert!(app.clipboard.is_some(), "Event::Copy must copy the box");

    run(&mut app, vec![Event::Paste("Ada".to_string())]);
    assert_eq!(
        app.overlays.items.len(),
        2,
        "Event::Paste must paste the box"
    );

    // The pasted box is selected, so Cut removes it and re-seeds the clipboard.
    run(&mut app, vec![Event::Cut]);
    assert_eq!(app.overlays.items.len(), 1, "Event::Cut must cut the box");
    assert!(app.clipboard.is_some());
}

#[test]
fn ctrl_s_saves_while_text_edit_has_focus_but_ctrl_c_stays_with_editor() {
    use eframe::egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

    let mut app = test_app();
    let path = write_pdf(1);
    assert!(app.load(path.path().to_path_buf()));
    // As recorded by a first confirmed overwrite (see
    // `confirmed_in_place_save_writes_repeatedly_without_asking_again`),
    // so Ctrl+S writes without showing the overwrite dialog.
    app.overwrite_confirmed_for = Some(path.path().to_path_buf());
    let id = app.overlays.add(0, 50.0, 60.0, BoxKind::Text);
    app.overlays.item_mut(id).unwrap().text = "Ada".to_string();
    assert!(app.overlays.is_dirty());

    let ctx = Context::default();
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0));
    let mut editor_text = "Ada".to_string();
    // Focus a text editor, as the inspector does while typing.
    let _output = ctx.run(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut editor_text))
                    .request_focus();
            });
        },
    );

    let key = |key: Key| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    };
    // Mirror `update`: shortcuts run before the panels, and the
    // editor is redrawn every frame so its focus persists into the
    // next frame's `handle_shortcuts` call.
    let mut run_shortcuts = |app: &mut TypewriterApp, events: Vec<Event>| {
        let _output = ctx.run(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |ctx| {
                app.handle_shortcuts(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut editor_text));
                });
            },
        );
    };

    run_shortcuts(&mut app, vec![key(Key::C)]);
    assert!(
        app.clipboard.is_none(),
        "Ctrl+C in a text editor must not copy the box"
    );
    assert!(app.overlays.is_dirty());

    run_shortcuts(&mut app, vec![key(Key::S)]);
    assert!(
        !app.overlays.is_dirty(),
        "Ctrl+S in a text editor must save the document"
    );
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
    // Canvas painting assumes the app theme's fonts are registered
    // (as `TypewriterApp::new` does in production).
    super::theme::apply_theme(ctx);
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
    let item = app.overlays.item_mut(id).unwrap();
    item.text = "Hello".to_string();
    item.width_pt = Some(80.0);

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(612.0, 792.0));
    drag_canvas(
        &Context::default(),
        &mut app,
        screen,
        Pos2::new(176.0, 110.0),
        Pos2::new(226.0, 130.0),
    );

    let item = app.overlays.item(id).unwrap();
    assert!((item.x_pt - 100.0).abs() <= f32::EPSILON);
    assert!((item.y_top_pt - 100.0).abs() <= f32::EPSILON);
    assert!((item.width_pt.unwrap() - 130.0).abs() <= f32::EPSILON);
}

#[test]
fn double_clicking_resize_handle_resets_width_to_auto_fit() {
    use eframe::egui::{Context, Event, Modifiers, PointerButton, Pos2, Rect, Vec2};

    let mut app = test_app();
    let id = app.overlays.add(0, 100.0, 100.0, BoxKind::Text);
    let item = app.overlays.item_mut(id).unwrap();
    item.text = "Hello".to_string();
    item.width_pt = Some(80.0);

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(612.0, 792.0));
    let ctx = Context::default();
    super::theme::apply_theme(&ctx);
    // Inner quadrant of the 16x16 handle centred on the box's
    // bottom-right corner (180, ~114); same point the drag test uses.
    let handle = Pos2::new(176.0, 110.0);
    let btn = |pressed| Event::PointerButton {
        pos: handle,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    for events in [
        Vec::new(),
        vec![Event::PointerMoved(handle)],
        vec![btn(true)],
        vec![btn(false)],
        vec![btn(true)],
        vec![btn(false)],
    ] {
        let _ = run_canvas(&ctx, &mut app, screen, 1.0, events);
    }

    assert_eq!(app.overlays.item(id).unwrap().width_pt, None);
}

#[test]
fn dragging_box_clamps_to_page_bounds() {
    use eframe::egui::{Context, Pos2, Rect, Vec2};

    let mut app = test_app();
    let id = app.overlays.add(0, 100.0, 100.0, BoxKind::Text);
    let item = app.overlays.item_mut(id).unwrap();
    item.text = "Hello".to_string();
    item.width_pt = Some(80.0);

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
    assert!((item.x_pt - max_left).abs() <= 1e-4, "got {}", item.x_pt);
    assert!(
        (item.y_top_pt - max_top).abs() <= 1e-4,
        "got {}",
        item.y_top_pt
    );
}

#[test]
fn undo_redo_reverts_a_canvas_drag_as_one_step() {
    use eframe::egui::{Context, Pos2, Rect, Vec2};

    let mut app = test_app();
    let id = app.overlays.add(0, 100.0, 100.0, BoxKind::Text);
    let item = app.overlays.item_mut(id).unwrap();
    item.text = "Hello".to_string();
    item.width_pt = Some(80.0);

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(612.0, 792.0));
    drag_canvas(
        &Context::default(),
        &mut app,
        screen,
        Pos2::new(120.0, 105.0),
        Pos2::new(220.0, 205.0),
    );
    let dragged = app.overlays.item(id).unwrap();
    assert!((dragged.x_pt - 200.0).abs() <= 1e-4, "got {}", dragged.x_pt);

    app.undo();
    let item = app.overlays.item(id).unwrap();
    assert!(
        (item.x_pt - 100.0).abs() <= f32::EPSILON,
        "got {}",
        item.x_pt
    );
    assert!((item.y_top_pt - 100.0).abs() <= f32::EPSILON);
    app.redo();
    let item = app.overlays.item(id).unwrap();
    assert!((item.x_pt - 200.0).abs() <= 1e-4, "got {}", item.x_pt);
}

#[test]
fn undo_mid_drag_still_records_rest_of_drag() {
    use eframe::egui::{Context, Event, Modifiers, PointerButton, Pos2, Rect, Vec2};

    let mut app = test_app();
    let id = app.overlays.add(0, 100.0, 100.0, BoxKind::Text);
    let item = app.overlays.item_mut(id).unwrap();
    item.text = "Hello".to_string();
    item.width_pt = Some(80.0);

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(612.0, 792.0));
    let ctx = Context::default();
    super::theme::apply_theme(&ctx);
    let btn = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let from = Pos2::new(120.0, 105.0);
    let mid = Pos2::new(170.0, 155.0);
    let to = Pos2::new(220.0, 205.0);
    for events in [
        Vec::new(),
        vec![Event::PointerMoved(from)],
        vec![btn(from, true)],
        vec![Event::PointerMoved(mid)],
    ] {
        let _ = run_canvas(&ctx, &mut app, screen, 1.0, events);
    }
    assert!((app.overlays.item(id).unwrap().x_pt - 150.0).abs() <= 1e-4);
    // Undo while the pointer is still down closes the gesture;
    // the remaining drag frames must start a new one.
    app.undo();
    assert!((app.overlays.item(id).unwrap().x_pt - 100.0).abs() <= f32::EPSILON);
    for events in [vec![Event::PointerMoved(to)], vec![btn(to, false)]] {
        let _ = run_canvas(&ctx, &mut app, screen, 1.0, events);
    }
    assert!((app.overlays.item(id).unwrap().x_pt - 150.0).abs() <= 1e-4);
    // The post-undo segment is undoable as its own step: the box
    // returns to where the undo left it, and is not removed.
    app.undo();
    let item = app.overlays.item(id).unwrap();
    assert!((item.x_pt - 100.0).abs() <= f32::EPSILON);
    assert!((item.y_top_pt - 100.0).abs() <= f32::EPSILON);
    app.redo();
    assert!((app.overlays.item(id).unwrap().x_pt - 150.0).abs() <= 1e-4);
}

#[test]
fn app_undo_redo_covers_delete_and_reports_nothing_at_stack_ends() {
    let mut app = test_app();
    app.undo();
    assert_eq!(app.status, "");
    let id = app.overlays.add(0, 10.0, 20.0, BoxKind::Text);
    app.overlays.remove(id);
    assert_eq!(app.overlays.items, Vec::new());
    app.undo();
    assert_eq!(app.overlays.items.len(), 1);
    assert_eq!(app.status, "Undid");
    app.redo();
    assert_eq!(app.overlays.items, Vec::new());
    assert_eq!(app.status, "Redid");
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
    let geom = crate::geometry::PageGeometry {
        id: (0, 0),
        rect: crate::geometry::PageRect::new(0.0, 0.0, 612.0, page_h_pt),
        rotation: crate::geometry::Rotation::R0,
    };
    let baked_text_y = crate::overlay::text_matrix(text_item, &geom)[5];
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
    let baked_sig_y = crate::overlay::text_matrix(sig_item, &geom)[5];
    assert!(
        (baseline_pdf_y("Ada Lovelace") - baked_sig_y).abs() <= 1e-3,
        "signature preview baseline must match baked baseline"
    );
}
