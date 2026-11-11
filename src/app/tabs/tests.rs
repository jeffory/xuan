use super::*;

#[test]
fn tabs_share_the_width_between_the_minimum_and_maximum() {
    // Few tabs: each is as wide as allowed.
    let strip = Strip::new(2, 1000.0);
    assert_eq!(strip.tab, MAX_TAB);
    assert_eq!(strip.content, 2.0 * MAX_TAB + GAP);
    assert!(!strip.overflows(1000.0));
    // More tabs: they shrink to share the room exactly.
    let strip = Strip::new(6, 600.0);
    assert!((strip.content - 600.0).abs() < 1e-3);
    assert!(strip.tab > MIN_TAB && strip.tab < MAX_TAB);
    assert!(!strip.overflows(600.0));
    // Too many: they stop at the minimum and the strip overflows.
    let strip = Strip::new(20, 600.0);
    assert_eq!(strip.tab, MIN_TAB);
    assert!(strip.overflows(600.0));
    assert_eq!(Strip::new(0, 600.0).content, 0.0);
}

#[test]
fn scrolling_stays_inside_the_strip_and_reveals_a_tab() {
    let strip = Strip::new(20, 600.0);
    let visible = 400.0;
    let max = strip.content - visible;
    assert_eq!(strip.clamp_scroll(-50.0, visible), 0.0);
    assert_eq!(strip.clamp_scroll(1e6, visible), max);
    // A tab to the right comes in at the right edge, one to the left at the left edge.
    let last = strip.reveal(0.0, 19, visible);
    assert!((last - max).abs() < 1e-3);
    assert_eq!(strip.reveal(last, 0, visible), 0.0);
    let middle = strip.reveal(0.0, 8, visible);
    assert!((middle + visible - (strip.left(8) + strip.tab)).abs() < 1e-3);
    // A tab already in view leaves the offset alone.
    assert_eq!(strip.reveal(middle, 7, visible), middle);
    // A strip that fits never scrolls.
    let fits = Strip::new(2, 600.0);
    assert_eq!(fits.reveal(30.0, 1, 600.0), 0.0);
}

#[test]
fn a_dragged_tab_lands_in_the_nearest_gap() {
    let strip = Strip::new(4, 4.0 * 100.0 + 3.0 * GAP);
    assert_eq!(strip.tab, 100.0);
    assert_eq!(strip.drop_slot(-30.0, 4), 0);
    assert_eq!(strip.drop_slot(40.0, 4), 0);
    assert_eq!(strip.drop_slot(60.0, 4), 1);
    assert_eq!(strip.drop_slot(250.0, 4), 2);
    assert_eq!(strip.drop_slot(1000.0, 4), 4);
    // Moving right, the tab takes the place before the gap; moving left, the gap's.
    assert_eq!(moved_index(0, 4), 3);
    assert_eq!(moved_index(0, 2), 1);
    assert_eq!(moved_index(3, 0), 0);
    assert_eq!(moved_index(1, 1), 1);
    assert_eq!(moved_index(1, 2), 1);
}

#[test]
fn keyboard_switching_wraps_and_nine_is_the_last_tab() {
    assert_eq!(cycle(0, 3, true), 1);
    assert_eq!(cycle(2, 3, true), 0);
    assert_eq!(cycle(0, 3, false), 2);
    assert_eq!(cycle(0, 0, true), 0);
    assert_eq!(nth(1, 3), Some(0));
    assert_eq!(nth(3, 3), Some(2));
    assert_eq!(nth(4, 3), None);
    assert_eq!(nth(9, 3), Some(2));
    assert_eq!(nth(9, 12), Some(11));
    assert_eq!(nth(1, 0), None);
    assert_eq!(others(1, 4, false), [0, 2, 3]);
    assert_eq!(others(1, 4, true), [2, 3]);
    assert!(others(3, 4, true).is_empty());
}

#[test]
fn the_close_button_shows_on_the_selected_or_hovered_tab_and_the_dirty_dot_otherwise() {
    let look = |selected, dirty, hovered| TabLook {
        title: "",
        selected,
        dirty,
        hovered,
        close_hovered: false,
        dragging: false,
    };
    assert!(shows_close(&look(true, false, false)));
    assert!(!shows_close(&look(false, false, false)));
    assert!(shows_close(&look(false, false, true)));
    // A dirty tab shows its dot until the pointer is over it.
    assert!(!shows_close(&look(true, true, false)));
    assert!(shows_close(&look(true, true, true)));
}

#[test]
fn file_uris_escape_what_a_uri_cannot_hold() {
    assert_eq!(
        file_uri(Path::new("/home/me/My Photos/été #1.png")),
        "file:///home/me/My%20Photos/%C3%A9t%C3%A9%20%231.png"
    );
}

#[test]
fn the_bar_line_opens_under_the_selected_tab_only() {
    let bar = Rangef::new(0.0, 1000.0);
    // No selected tab in view: one line across the bar.
    assert_eq!(divider_spans(bar, None), [bar]);
    // A selected tab: the line stops under it and goes on after it.
    assert_eq!(
        divider_spans(bar, Some(Rangef::new(200.0, 420.0))),
        [Rangef::new(0.0, 200.0), Rangef::new(420.0, 1000.0)]
    );
    // At either end there is nothing left on that side.
    assert_eq!(
        divider_spans(bar, Some(Rangef::new(0.0, 220.0))),
        [Rangef::new(220.0, 1000.0)]
    );
    assert_eq!(
        divider_spans(bar, Some(Rangef::new(780.0, 1000.0))),
        [Rangef::new(0.0, 780.0)]
    );
    // Only the part inside the bar counts.
    assert_eq!(
        divider_spans(bar, Some(Rangef::new(-50.0, 100.0))),
        [Rangef::new(100.0, 1000.0)]
    );
}
