use super::*;

fn rect() -> Rect {
    Rect::from_min_size(Pos2::new(10.0, 20.0), eframe::egui::vec2(612.0, 792.0))
}

#[test]
fn screen_pdf_roundtrip_at_zoom_1() {
    let r = rect();
    let (x, y) = screen_to_pdf_top(Pos2::new(110.0, 220.0), r, 1.0);
    assert!((x - 100.0).abs() < 0.001);
    assert!((y - 200.0).abs() < 0.001);
    let back = pdf_top_to_screen(x, y, r, 1.0);
    assert!((back.x - 110.0).abs() < 0.001);
    assert!((back.y - 220.0).abs() < 0.001);
}

#[test]
fn screen_pdf_roundtrip_at_zoom_1_5() {
    let r = rect();
    let (x, y) = screen_to_pdf_top(Pos2::new(160.0, 320.0), r, 1.5);
    assert!((x - 100.0).abs() < 0.001);
    assert!((y - 200.0).abs() < 0.001);
    let back = pdf_top_to_screen(x, y, r, 1.5);
    assert!((back.x - 160.0).abs() < 0.001);
    assert!((back.y - 320.0).abs() < 0.001);
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn wrap_line_breaks_at_words_preserves_spaces_and_hard_breaks_long_words() {
    // 10pt per character, so widths are easy to reason about.
    let measure = |s: &str| s.chars().count() as f32 * 10.0;
    assert_eq!(
        wrap_line("aaaa bbbb cccc", 90.0, &measure),
        vec!["aaaa bbbb", "cccc"]
    );
    assert_eq!(
        wrap_line("  aaaa   bbbb cc", 130.0, &measure),
        vec!["  aaaa   bbbb", "cc"]
    );
    assert_eq!(wrap_line("  indented", 200.0, &measure), vec!["  indented"]);
    assert_eq!(
        wrap_line("aaaaaaaaaa", 45.0, &measure),
        vec!["aaaa", "aaaa", "aa"]
    );
    assert_eq!(wrap_line("", 45.0, &measure), vec![""]);
    assert_eq!(
        wrap_line("aaaa bbbb", 0.0, &measure),
        vec!["aaaa bbbb"],
        "non-positive width disables wrapping"
    );
    assert_eq!(logical_lines("\ta\tb\r\nc\n"), vec!["    a    b", "c"]);
}

#[test]
fn add_selects_and_remove_clears_selection() {
    let mut state = OverlayState::default();
    let id = state.add(0, 10.0, 20.0, BoxKind::Text);
    assert_eq!(state.selected, Some(id));
    assert!(state.dirty);
    state.remove(id);
    assert_eq!(state.selected, None);
    assert_eq!(state.items, Vec::new());
}

#[test]
fn paste_assigns_fresh_id_selects_and_offsets() {
    let mut state = OverlayState::default();
    let src = state.add(0, 100.0, 200.0, BoxKind::Signature);
    {
        let item = state.item_mut(src).unwrap();
        item.text = "Ada".to_string();
        item.size_pt = 40.0;
        item.color = [1, 2, 3];
        item.width_pt = Some(150.0);
    }
    let mut clipboard = state.copy_selected().unwrap();
    let pasted = state.paste(&mut clipboard, 2);
    assert_ne!(pasted, src);
    assert_eq!(state.selected, Some(pasted));
    assert!(state.dirty);
    let item = state.item(pasted).unwrap();
    assert_eq!(item.kind, BoxKind::Signature);
    assert_eq!(item.page, 2);
    assert_eq!(item.text, "Ada");
    assert!((item.size_pt - 40.0).abs() < f32::EPSILON);
    assert_eq!(item.color, [1, 2, 3]);
    assert_eq!(item.width_pt, Some(150.0));
    assert!((item.x_pt - 112.0).abs() < f32::EPSILON);
    assert!((item.y_top_pt - 212.0).abs() < f32::EPSILON);
    // Source box untouched.
    let orig = state.item(src).unwrap();
    assert_eq!(orig.page, 0);
    assert!((orig.x_pt - 100.0).abs() < f32::EPSILON);
}

#[test]
fn repeated_paste_cascades_instead_of_stacking() {
    let mut state = OverlayState::default();
    state.add(0, 100.0, 200.0, BoxKind::Text);
    let mut clipboard = state.copy_selected().unwrap();
    let first = state.paste(&mut clipboard, 0);
    let second = state.paste(&mut clipboard, 0);
    let a = state.item(first).unwrap();
    let (ax, ay) = (a.x_pt, a.y_top_pt);
    let b = state.item(second).unwrap();
    assert!((b.x_pt - (ax + PASTE_OFFSET_PT)).abs() < f32::EPSILON);
    assert!((b.y_top_pt - (ay + PASTE_OFFSET_PT)).abs() < f32::EPSILON);
    assert_eq!(state.items.len(), 3);
}

#[test]
fn cut_removes_and_copy_without_selection_is_none() {
    let mut state = OverlayState::default();
    assert!(state.copy_selected().is_none());
    assert!(state.duplicate_selected().is_none());
    let id = state.add(1, 10.0, 20.0, BoxKind::Text);
    state.item_mut(id).unwrap().text = "hello".to_string();
    let dup = state.duplicate_selected().unwrap();
    assert_eq!(state.items.len(), 2);
    assert_eq!(state.item(dup).unwrap().page, 1);
    state.selected = Some(id);
    let cut = state.cut_selected().unwrap();
    assert_eq!(cut.text, "hello");
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.selected, None);
    assert!(state.item(id).is_none());
}

#[test]
fn removing_earlier_box_keeps_selected_identity() {
    let mut state = OverlayState::default();
    let first = state.add(0, 10.0, 20.0, BoxKind::Text);
    let target = state.add(0, 30.0, 40.0, BoxKind::Text);
    state.item_mut(target).unwrap().text = "keep me".to_string();
    assert_eq!(state.selected, Some(target));
    // Positional selection would now point at a different (or no)
    // box; the stable id must still resolve to the same box.
    state.remove(first);
    assert_eq!(state.selected, Some(target));
    assert_eq!(state.item(target).unwrap().text, "keep me");
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.by_page(0)[0].id, target);
    state.drop_empty_selected();
    assert_eq!(state.selected, Some(target));
    let empty = state.add(0, 50.0, 60.0, BoxKind::Text);
    assert_eq!(state.selected, Some(empty));
    state.drop_empty_selected();
    assert_eq!(state.selected, None);
    assert_eq!(state.items.len(), 1);
}
