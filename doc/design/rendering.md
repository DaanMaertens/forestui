# Rendering, focus and the mouse

ratatui redraws the whole frame every event; there are no persistent widget
objects and no CSS. Three consequences shape the code:

1. **Focus is an index, not a widget.** `app/detail.rs::content()` walks the
   current selection once and produces the pane's full node list — text, cards,
   and every focusable control in order. `App::detail_items()` collects the
   focusable items from that list and `ui/detail.rs` renders the same list, so
   item N on screen is item N in the key handler *by construction*. To add or
   move a control, change the content walk; both consumers follow automatically.

   Activation goes through one more indirection: each frame the renderer
   snapshots the drawn items onto `App::drawn_items` (with each control's
   enabled bit), and a click or `Enter` resolves against that snapshot — never
   against a freshly derived list, which a background event drained in the same
   batch may already have reshaped. The snapshot is what was on screen, which
   is what the user acted on; a control drawn as disabled must not fire.
2. **Layout is `Layout`/`Constraint`, not stylesheets.** Sizes are computed per
   frame; nothing is "auto height".
3. **Mouse support is manual.** There is no widget tree to hit-test against, so
   every frame the renderers record the rectangle of each clickable thing via
   `App::push_hit(rect, HitTarget::…)`, and `App::handle_mouse` resolves a click
   against that list (last recorded wins, so a modal takes clicks from the panes
   it covers). A control that is drawn but never recorded is dead to the mouse —
   that was a real regression: the first Rust build enabled no mouse capture at
   all and every click did nothing.

   The mouse modes behind hover are terminal state owned by
   `src/terminal.rs`; see `doc/design/terminal-input-modes.md`.

Controls are drawn as three-row bordered boxes from one builder in
`ui/widgets.rs`: `button_box`/`button_box_width` for modal buttons (Textual's
`min-width: 10`) and the same code via `boxed_rows`/`boxed_width` with no
minimum for the detail pane, whose boxes hug their labels. Use the matching
width function for the rectangle you record, so the hit region matches what
was drawn.
