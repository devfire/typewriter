use super::{CloseState, Mode, TypewriterApp};
use crate::overlay::{BoxKind, OverlayState};
use crate::render::PageRenderer;
use crate::test_util::TempFile;
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
        close_state: CloseState::Idle,
        box_layouts: std::collections::HashMap::new(),
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
