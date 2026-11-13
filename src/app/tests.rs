use super::{CloseState, Mode, TypewriterApp};
use crate::overlay::OverlayState;
use crate::render::PageRenderer;
use lopdf::{Document, Object, dictionary};
use std::path::PathBuf;

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
    }
}

/// Write a minimal `pages`-page PDF and return its path.
fn write_pdf(pages: usize) -> PathBuf {
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
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pdf-editor-app-test-{}-{stamp}.pdf",
        std::process::id()
    ));
    doc.save(&path).unwrap();
    path
}

#[test]
fn reload_after_save_preserves_page_and_zoom() {
    let path = write_pdf(3);
    let mut app = test_app();
    assert!(app.load(path.clone()));
    app.page = 2;
    app.zoom = 1.7;
    app.reload_after_save(&path);
    assert_eq!(app.page, 2);
    assert!((app.zoom - 1.7).abs() < f32::EPSILON);
    assert!(
        app.status.starts_with("Saved "),
        "unexpected status: {}",
        app.status
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn reload_after_save_clamps_page_to_new_page_count() {
    let path = write_pdf(3);
    let shorter = write_pdf(1);
    let mut app = test_app();
    assert!(app.load(path));
    app.page = 2;
    app.reload_after_save(&shorter);
    assert_eq!(app.page, 0);
    let _ = std::fs::remove_file(&shorter);
}

#[test]
fn failed_reload_after_save_is_surfaced_not_masked_by_saved() {
    let path = write_pdf(3);
    let mut app = test_app();
    assert!(app.load(path.clone()));
    app.page = 1;
    app.zoom = 1.7;
    let missing = path.with_file_name("pdf-editor-app-test-no-such-file.pdf");
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
    let _ = std::fs::remove_file(&path);
}
