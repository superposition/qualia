//! View-state tests for the panel split.
//!
//! These run without a terminal: they pin the one table that drives both the
//! tab-bar labels and the digit keys, and the transitions the key handler calls.

use qualia_watch::view::{self, ViewMode, ViewState, VIEW_LABELS};

#[test]
fn label_table_drives_keys_and_labels() {
    for (index, (label, mode)) in VIEW_LABELS.iter().enumerate() {
        let digit = char::from_digit(index as u32 + 1, 10).expect("table fits in single digits");
        assert_eq!(
            view::view_for_digit(digit),
            Some(*mode),
            "digit {digit} must select {}",
            label
        );
        assert_eq!(view::digit_for_view(*mode), Some(digit));
        assert_eq!(view::label_for(*mode), *label);
        assert_eq!(view::view_index(*mode), Some(index));
        assert!(!label.is_empty(), "every view needs a tab label");
    }
}

#[test]
fn every_view_appears_exactly_once() {
    for (index, (_, mode)) in VIEW_LABELS.iter().enumerate() {
        let repeats = VIEW_LABELS
            .iter()
            .filter(|(_, other)| other == mode)
            .count();
        assert_eq!(repeats, 1, "{mode:?} appears {repeats} times");
        assert_eq!(view::view_index(*mode), Some(index));
    }
}

#[test]
fn unbound_keys_select_nothing() {
    for key in ['0', 'x', ' ', '\t'] {
        assert_eq!(view::view_for_digit(key), None, "key {key:?} must be unbound");
    }
    let past_the_end = char::from_digit(VIEW_LABELS.len() as u32 + 1, 10).unwrap();
    assert_eq!(view::view_for_digit(past_the_end), None);
}

#[test]
fn tab_cycles_through_the_table_in_order_then_wraps() {
    let mut state = ViewState::new();
    assert_eq!(state.view(), VIEW_LABELS[0].1);
    for step in 1..=VIEW_LABELS.len() {
        state.next_view();
        assert_eq!(state.view(), VIEW_LABELS[step % VIEW_LABELS.len()].1);
    }
}

#[test]
fn backtab_walks_the_table_backwards() {
    let mut state = ViewState::new();
    for step in 0..VIEW_LABELS.len() {
        state.prev_view();
        let expected = (VIEW_LABELS.len() - 1 - step) % VIEW_LABELS.len();
        assert_eq!(state.view(), VIEW_LABELS[expected].1);
    }
}

#[test]
fn selecting_a_view_clears_its_scroll() {
    let mut state = ViewState::new();
    state.set_view(ViewMode::Detail);
    state.scroll(40);
    assert_eq!(state.detail_scroll(), 40);
    state.set_view(ViewMode::Hex);
    assert_eq!(state.detail_scroll(), 0, "leaving a panel resets its scroll");
    state.scroll(7);
    assert_eq!(state.hex_scroll(), 7);
    state.set_view(ViewMode::Detail);
    assert_eq!(state.hex_scroll(), 0);
}

#[test]
fn moving_the_selected_layer_clamps_and_resets_scroll() {
    let mut state = ViewState::new();
    assert_eq!(state.layer(), 0);
    state.move_layer(-1);
    assert_eq!(state.layer(), 0, "cannot move above the first layer");

    state.set_view(ViewMode::Detail);
    state.scroll(12);
    state.move_layer(1);
    assert_eq!(state.layer(), 1);
    assert_eq!(state.detail_scroll(), 0, "changing layer resets the panel");

    state.select_layer(usize::MAX);
    assert_eq!(state.layer(), qualia_watch::view::LAYER_COUNT - 1);
    state.move_layer(1);
    assert_eq!(state.layer(), qualia_watch::view::LAYER_COUNT - 1);
}

#[test]
fn scroll_never_goes_below_zero() {
    let mut state = ViewState::new();
    state.set_view(ViewMode::Hex);
    state.scroll(-5);
    assert_eq!(state.hex_scroll(), 0);
    state.scroll(3);
    state.scroll(-1);
    assert_eq!(state.hex_scroll(), 2);
}

#[test]
fn the_long_dump_panels_scroll_and_the_others_ignore_it() {
    let mut state = ViewState::new();
    state.set_view(ViewMode::Weights);
    state.scroll(5);
    assert_eq!(state.hex_scroll(), 5);
    state.set_view(ViewMode::Overview);
    state.scroll(5);
    assert_eq!(state.hex_scroll(), 0);
    assert_eq!(state.detail_scroll(), 0);
}
