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
    assert!(state.is_dirty());
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
    let pasted = state.paste(&mut clipboard, 2, (612.0, 792.0));
    assert_ne!(pasted, src);
    assert_eq!(state.selected, Some(pasted));
    assert!(state.is_dirty());
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
    let first = state.paste(&mut clipboard, 0, (612.0, 792.0));
    let second = state.paste(&mut clipboard, 0, (612.0, 792.0));
    let a = state.item(first).unwrap();
    let (ax, ay) = (a.x_pt, a.y_top_pt);
    let b = state.item(second).unwrap();
    assert!((b.x_pt - (ax + PASTE_OFFSET_PT)).abs() < f32::EPSILON);
    assert!((b.y_top_pt - (ay + PASTE_OFFSET_PT)).abs() < f32::EPSILON);
    assert_eq!(state.items.len(), 3);
}

#[test]
fn paste_clamps_to_bottom_and_right_page_edges() {
    let mut state = OverlayState::default();
    // Near the bottom-right of a Letter page; size 12 gives a
    // one-line height of LEADING * 12 = 14.4 pt.
    let src = state.add(0, 500.0, 785.0, BoxKind::Text);
    state.item_mut(src).unwrap().width_pt = Some(150.0);
    let mut clipboard = state.copy_selected().unwrap();
    let pasted = state.paste(&mut clipboard, 0, (612.0, 792.0));
    let item = state.item(pasted).unwrap();
    assert!((item.x_pt - 462.0).abs() < 1e-4, "got {}", item.x_pt);
    assert!(
        (item.y_top_pt - 777.6).abs() < 1e-4,
        "got {}",
        item.y_top_pt
    );
}

#[test]
fn repeated_paste_stays_on_page_at_the_edge() {
    let mut state = OverlayState::default();
    let src = state.add(0, 100.0, 785.0, BoxKind::Text);
    state.item_mut(src).unwrap().width_pt = Some(150.0);
    let mut clipboard = state.copy_selected().unwrap();
    let mut last = state.paste(&mut clipboard, 0, (612.0, 792.0));
    for _ in 0..3 {
        last = state.paste(&mut clipboard, 0, (612.0, 792.0));
    }
    let item = state.item(last).unwrap();
    assert!(
        (item.y_top_pt - 777.6).abs() < 1e-4,
        "got {}",
        item.y_top_pt
    );
    assert!(item.y_top_pt + LEADING * item.size_pt <= 792.0);
}

#[test]
fn paste_from_larger_page_onto_smaller_page_clamps_inside() {
    let mut state = OverlayState::default();
    // Copied from an A3-sized page (1191 x 842 pt landscape is
    // beside the point: the position alone is off a Letter page).
    let src = state.add(0, 1_000.0, 1_100.0, BoxKind::Text);
    state.item_mut(src).unwrap().width_pt = Some(100.0);
    let mut clipboard = state.copy_selected().unwrap();
    let pasted = state.paste(&mut clipboard, 1, (612.0, 792.0));
    let item = state.item(pasted).unwrap();
    assert_eq!(item.page, 1);
    assert!((item.x_pt - 512.0).abs() < 1e-4, "got {}", item.x_pt);
    assert!(
        (item.y_top_pt - 777.6).abs() < 1e-4,
        "got {}",
        item.y_top_pt
    );
}

#[test]
fn duplicate_clamps_to_page_edge() {
    let mut state = OverlayState::default();
    let src = state.add(0, 100.0, 785.0, BoxKind::Text);
    state.item_mut(src).unwrap().width_pt = Some(150.0);
    let dup = state.duplicate_selected((612.0, 792.0)).unwrap();
    let item = state.item(dup).unwrap();
    assert!(
        (item.y_top_pt - 777.6).abs() < 1e-4,
        "got {}",
        item.y_top_pt
    );
}

#[test]
fn cut_removes_and_copy_without_selection_is_none() {
    let mut state = OverlayState::default();
    assert!(state.copy_selected().is_none());
    assert!(state.duplicate_selected((612.0, 792.0)).is_none());
    let id = state.add(1, 10.0, 20.0, BoxKind::Text);
    state.item_mut(id).unwrap().text = "hello".to_string();
    let dup = state.duplicate_selected((612.0, 792.0)).unwrap();
    assert_eq!(state.items.len(), 2);
    assert_eq!(state.item(dup).unwrap().page, 1);
    state.select(Some(id));
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

#[test]
fn undo_redo_reverses_add_remove_and_cut() {
    let mut state = OverlayState::default();
    assert!(!state.can_undo());
    assert!(!state.undo());
    let id = state.add(0, 10.0, 20.0, BoxKind::Text);
    state.item_mut(id).unwrap().text = "hello".to_string();
    assert!(state.can_undo());
    // Add is one step: undoing it removes the box and the unsaved flag.
    assert!(state.undo());
    assert_eq!(state.items, Vec::new());
    assert_eq!(state.selected, None);
    assert!(!state.is_dirty());
    assert!(state.can_redo());
    assert!(state.redo());
    assert_eq!(state.items.len(), 1);
    assert!(state.is_dirty());
    // Delete, then Cut on the restored box, are each reversible.
    state.remove(id);
    assert_eq!(state.items, Vec::new());
    assert!(state.undo());
    assert_eq!(state.item(id).unwrap().text, "hello");
    state.select(Some(id));
    assert!(state.cut_selected().is_some());
    assert_eq!(state.items, Vec::new());
    assert!(state.undo());
    assert_eq!(state.item(id).unwrap().text, "hello");
    assert!(state.redo());
    assert_eq!(state.items, Vec::new());
}

#[test]
fn new_action_clears_redo_and_undone_add_id_is_not_reused() {
    let mut state = OverlayState::default();
    let first = state.add(0, 10.0, 20.0, BoxKind::Text);
    assert!(state.undo());
    assert!(state.can_redo());
    let second = state.add(0, 30.0, 40.0, BoxKind::Text);
    assert_ne!(second, first, "next_id must not rewind with undo");
    assert!(!state.can_redo());
    assert!(!state.redo());
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.items[0].id, second);
}

#[test]
fn gesture_coalesces_many_mutations_into_one_undo_step() {
    let mut state = OverlayState::default();
    let id = state.add(0, 10.0, 20.0, BoxKind::Text);
    state.begin_gesture();
    for step in 1..=10 {
        let item = state.item_mut(id).unwrap();
        #[allow(clippy::cast_precision_loss)]
        {
            item.x_pt = 10.0 + step as f32;
        }
        item.text = format!("step {step}");
    }
    state.end_gesture();
    // One undo reverts the whole drag/typing burst, not each frame.
    assert!(state.undo());
    let item = state.item(id).unwrap();
    assert!((item.x_pt - 10.0).abs() < f32::EPSILON);
    assert_eq!(item.text, "");
    assert!(state.redo());
    assert_eq!(state.item(id).unwrap().text, "step 10");
    // A gesture that changes nothing records no step.
    state.begin_gesture();
    state.end_gesture();
    assert!(state.undo());
    assert_eq!(state.item(id).unwrap().text, "");
}

#[test]
fn history_is_capped_and_drops_the_oldest_snapshots() {
    let mut state = OverlayState::default();
    let id = state.add(0, 10.0, 20.0, BoxKind::Text);
    for step in 1..=150 {
        state.checkpoint();
        state.item_mut(id).unwrap().text = format!("v{step}");
    }
    // Only the newest 100 snapshots survive; undoing all of them
    // lands on v50, not on the initial empty text.
    let mut undos = 0;
    while state.undo() {
        undos += 1;
    }
    assert_eq!(undos, 100);
    assert_eq!(state.item(id).unwrap().text, "v50");
}

#[test]
fn deleting_the_only_added_box_returns_to_clean() {
    let mut state = OverlayState::default();
    let id = state.add(0, 10.0, 20.0, BoxKind::Text);
    assert!(state.is_dirty());
    state.remove(id);
    assert_eq!(state.items, Vec::new());
    assert!(!state.is_dirty(), "nothing left to save");
}

#[test]
fn dropping_an_empty_selected_box_returns_to_clean() {
    let mut state = OverlayState::default();
    state.add(0, 10.0, 20.0, BoxKind::Text);
    assert!(state.is_dirty());
    state.drop_empty_selected();
    assert_eq!(state.items, Vec::new());
    assert!(
        !state.is_dirty(),
        "dropped empty box leaves nothing to save"
    );
}

#[test]
fn selecting_another_box_drops_the_empty_selected_box() {
    let mut state = OverlayState::default();
    let kept = state.add(0, 10.0, 20.0, BoxKind::Text);
    state.item_mut(kept).unwrap().text = "keep me".to_string();
    // Clicking elsewhere places (and selects) a second, empty box;
    // selecting the first box must drop it, not leave it listed.
    let empty = state.add(0, 50.0, 60.0, BoxKind::Text);
    assert_eq!(state.items.len(), 2);
    state.select(Some(kept));
    assert_eq!(state.selected, Some(kept));
    assert_eq!(state.items.len(), 1);
    assert!(state.item(empty).is_none());
    // Clearing the selection likewise drops a selected empty box,
    // but keeps a non-empty one (only the selection goes away).
    let empty = state.add(0, 50.0, 60.0, BoxKind::Text);
    state.select(None);
    assert_eq!(state.selected, None);
    assert!(state.item(empty).is_none());
    state.select(Some(kept));
    state.select(None);
    assert_eq!(state.selected, None);
    assert_eq!(state.item(kept).unwrap().text, "keep me");
    // Re-selecting the current box must not drop it.
    let empty = state.add(0, 50.0, 60.0, BoxKind::Text);
    state.select(Some(empty));
    assert_eq!(state.selected, Some(empty));
    assert!(state.item(empty).is_some());
}

#[test]
fn adding_a_box_drops_the_previously_selected_empty_box() {
    let mut state = OverlayState::default();
    let first = state.add(0, 10.0, 20.0, BoxKind::Text);
    let second = state.add(0, 50.0, 60.0, BoxKind::Text);
    assert_eq!(state.selected, Some(second));
    assert_eq!(state.items.len(), 1);
    assert!(state.item(first).is_none());
    // The discard and the insertion are one undo step.
    assert!(state.undo());
    assert_eq!(state.items.len(), 1);
    assert!(state.item(first).is_some());
}

#[test]
fn undoing_back_to_restored_boxes_returns_to_clean() {
    let mut loaded = OverlayText::new(BoxId::default(), 0, 10.0, 20.0, BoxKind::Text);
    loaded.text = "loaded".to_string();
    let mut state = OverlayState::default();
    state.restore_embedded(vec![loaded]);
    assert!(!state.is_dirty(), "restored boxes start clean");
    let id = state.items[0].id;
    state.remove(id);
    assert!(state.is_dirty(), "deleting a restored box is a change");
    assert!(state.undo());
    assert!(!state.is_dirty(), "undo returned to the loaded state");
}

#[test]
fn clear_baked_clears_history_so_baked_boxes_stay_gone() {
    let mut state = OverlayState::default();
    state.add(0, 10.0, 20.0, BoxKind::Text);
    state.clear_baked();
    assert!(!state.can_undo());
    assert!(!state.can_redo());
    assert!(!state.undo());
    assert_eq!(state.items, Vec::new());
}
