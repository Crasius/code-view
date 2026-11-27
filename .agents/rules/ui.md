---
paths:
  - "crates/review-app/src/**"
  - "crates/review-app/assets/**"
  - "crates/cv-*/src/**"
  - "crates/cv-base/assets/**"
---

# UI: egui, theme, UX acceptance

The app is egui, but it must feel like a fluid modern web app, not like a "native Rust" tool. The diff viewer comes first: fast, readable, syntax colours per language.

## Design order
1. Direction: the harness design skills (direction, build, review, in that order). Do not skip them for small changes.
2. Build with the theme and the shared widgets. Keep the existing design system; extend it, replace it only when the owner asks.
3. Review against the acceptance list below.

## Theme and widgets
- Colours, sizes, spacing, motion and fonts come from `cv_base::theme` (`palette`, `style`, `motion`, `fonts`, `syntax`). Features never use raw colours, sizes or font names. A missing token goes into the theme first.
- Both palettes (`Palette::LIGHT`, `Palette::DARK`) get every new colour. Text contrast is at least 4.5:1 on its fill.
- Icons come from the bundled Phosphor fonts through `cv_base::theme::icons`. No emoji as icons.
- Reuse `cv-widgets` (buttons, rows, tabs, states, skeleton, splitter, badges, elide, ...) before you write a new one. A pattern used twice becomes a widget.
- Motion goes through `cv_base::theme::motion` and respects the system's reduced-motion setting.
- A scroll area whose offset the app places (a jump to a line, a restored position) goes through `cv_widgets::scroll::placed`, because egui measures the area's end from last frame's content and would spring an offset past it back; its regression tests are in `cv-widgets` (`scroll.rs`).

## Messages and states
- No dead ends. Every hint, error and empty state carries the direct action: "Open on GitHub/GitLab/Azure DevOps" with the exact URL, Retry, Refresh, or "Add token"/"Fix token" into the setup at the right step. Name the platform concretely.
- Loading shows the final layout (skeletons) so nothing jumps when data arrives. A note that appears later must not push content that is already visible.
- The same rule holds for reports to the owner: give the exact URL, not "open it on the platform".

## Acceptance for every UI change
- Light and dark palette.
- The smallest window size and a large one; text that does not fit elides, nothing clips or overlaps (`crates/review-app/src/app/layout_audit_walkthrough.rs`, `app/clip_check.rs`).
- Keyboard only: every interactive element is reachable and shows a visible focus.
- Zoom levels other than 100 % (`crates/cv-shell/src/zoom.rs`).
- Frame cost stays low on large pull requests: no per-frame allocation or layout of the whole diff; check with the frame statistics (Ctrl+Shift+F) for diff and list changes.
- Prove it with a kittest walkthrough that asserts the new state: accessibility tree, geometry, focus, repaint requests. A green build is not proof.
- Agents never take, render or read screenshots, not in tests and not to check their own work. The owner judges the look in the shipped build and gives the UI feedback; tell them what changed and where to find it.
