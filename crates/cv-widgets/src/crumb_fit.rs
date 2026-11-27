//! Fits a breadcrumb trail into the room it has (the title bar's, the folder summary's), like
//! Azure DevOps or Windows Explorer shorten an address: the middle gives way, the ends stay readable.
//!
//! In order, until the trail fits: long middle crumbs are cut to a shorter width (least important
//! first), then middle crumbs fold into one "…" crumb right after the first one (and the room they
//! leave lets cut crumbs that are still shown grow whole again, most important first), and only
//! when nothing but the first crumb, "…" and the last crumb are left, the last and then the first
//! are cut further. Pure arithmetic on measured widths, so it is tested without fonts.

/// Narrowest a first or last crumb is cut to: a few letters and the ellipsis stay visible.
const MIN_CUT_WIDTH: f32 = 48.0;

/// One crumb's measured width, padding included: whole (up to the usual cap) and cut short.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrumbWidth {
    pub full: f32,
    /// At most `full`; the same for crumbs that are short anyway.
    pub short: f32,
}

/// The widths the trail is laid out with besides the crumbs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    /// A separator caret with its gaps, between two neighbors.
    pub separator: f32,
    /// The "…" crumb.
    pub overflow: f32,
}

/// The width each crumb is shown at, `None` when it is folded into the "…" crumb.
pub fn fit(
    crumbs: &[CrumbWidth],
    anchor: usize,
    spacing: Spacing,
    budget: f32,
) -> Vec<Option<f32>> {
    let mut shown: Vec<Option<f32>> = crumbs.iter().map(|crumb| Some(crumb.full)).collect();
    let Some(last) = crumbs.len().checked_sub(1) else {
        return shown;
    };
    if width(&shown, spacing) <= budget {
        return shown;
    }
    for index in fold_order(crumbs.len(), anchor) {
        shown[index] = Some(crumbs[index].short);
        if width(&shown, spacing) <= budget {
            return shown;
        }
    }
    for index in fold_order(crumbs.len(), anchor) {
        shown[index] = None;
        if width(&shown, spacing) <= budget {
            uncut(crumbs, &mut shown, anchor, spacing, budget);
            return shown;
        }
    }
    for index in [last, 0] {
        let others = width(&shown, spacing) - shown[index].unwrap_or(0.0);
        let room = (budget - others).max(MIN_CUT_WIDTH);
        shown[index] = shown[index].map(|current| current.min(room));
        if width(&shown, spacing) <= budget {
            break;
        }
    }
    shown
}

/// Shows cut crumbs whole again where the folds left room, the most important first.
fn uncut(
    crumbs: &[CrumbWidth],
    shown: &mut [Option<f32>],
    anchor: usize,
    spacing: Spacing,
    budget: f32,
) {
    for index in fold_order(crumbs.len(), anchor).rev() {
        let Some(current) = shown[index] else {
            continue;
        };
        if current < crumbs[index].full {
            shown[index] = Some(crumbs[index].full);
            if width(shown, spacing) > budget {
                shown[index] = Some(current);
            }
        }
    }
}

/// Total width of the trail as `fit` returned it, with separators and the "…" crumb.
fn width(shown: &[Option<f32>], spacing: Spacing) -> f32 {
    let visible = shown.iter().flatten().count();
    let is_folded = visible < shown.len();
    let items = visible + usize::from(is_folded);
    let crumbs: f32 = shown.iter().flatten().sum();
    let overflow = if is_folded { spacing.overflow } else { 0.0 };
    crumbs + overflow + spacing.separator * items.saturating_sub(1) as f32
}

/// The middle crumbs, least important first: the levels between the first crumb and the route's
/// own crumb (`anchor`, the pull request), then the page's levels below it from the outermost in,
/// then the second to last crumb, and the anchor last of all. The first and the last crumb never
/// fold. Folding in this order always starts right after the first crumb, where "…" sits.
fn fold_order(count: usize, anchor: usize) -> impl DoubleEndedIterator<Item = usize> {
    let last = count.saturating_sub(1);
    let second_to_last = last.saturating_sub(1);
    let is_anchor_middle = anchor > 0 && anchor < last;
    let is_second_to_last_below = second_to_last > anchor;
    (1..anchor.min(last))
        .chain((anchor + 1)..second_to_last)
        .chain(is_second_to_last_below.then_some(second_to_last))
        .chain(is_anchor_middle.then_some(anchor))
}

#[cfg(test)]
mod tests {
    use super::{CrumbWidth, MIN_CUT_WIDTH, Spacing, fit, width};

    const SPACING: Spacing = Spacing { separator: 15.0, overflow: 28.0 };

    fn crumbs(widths: &[(f32, f32)]) -> Vec<CrumbWidth> {
        widths.iter().map(|&(full, short)| CrumbWidth { full, short }).collect()
    }

    /// The deep files trail from the bug report: app, connection, repository, pull request, then
    /// eight folders and the file.
    fn deep() -> Vec<CrumbWidth> {
        crumbs(&[
            (80.0, 80.0),
            (170.0, 160.0),
            (292.0, 172.0),
            (120.0, 120.0),
            (36.0, 36.0),
            (68.0, 68.0),
            (46.0, 46.0),
            (188.0, 172.0),
            (30.0, 30.0),
            (72.0, 72.0),
            (210.0, 210.0),
        ])
    }

    /// The order `fit` folds `count` crumbs of one width that cannot be cut: each tighter budget
    /// folds one more, until only the ends are left (then they are cut instead).
    fn fold_sequence(count: usize, anchor: usize) -> Vec<usize> {
        let trail = crumbs(&vec![(100.0, 100.0); count]);
        let mut order = Vec::new();
        for folds in 1..count {
            let visible = (count - folds) as f32;
            let budget = visible * (100.0 + SPACING.separator) + SPACING.overflow;
            let shown = fit(&trail, anchor, SPACING, budget);
            let folded: Vec<usize> = (0..count).filter(|index| shown[*index].is_none()).collect();
            if folded.len() < folds {
                break;
            }
            assert_eq!(folded.len(), folds, "{budget}: one fold per step: {shown:?}");
            let newly: Vec<usize> =
                folded.into_iter().filter(|index| !order.contains(index)).collect();
            order.extend(newly);
        }
        order
    }

    #[test]
    fn folds_spare_the_ends_and_keep_the_anchor_longest() {
        assert_eq!(fold_sequence(11, 3), [1, 2, 4, 5, 6, 7, 8, 9, 3]);
        assert_eq!(fold_sequence(4, 3), [1, 2], "a pull request without page levels");
        assert_eq!(fold_sequence(2, 1), [0_usize; 0], "two crumbs never fold");
        assert_eq!(fold_sequence(5, 3), [1, 2, 3], "one page level: the anchor is second to last");
    }

    #[test]
    fn a_trail_that_fits_stays_whole() {
        let trail = deep();
        let natural: f32 =
            trail.iter().map(|crumb| crumb.full).sum::<f32>() + SPACING.separator * 10.0;
        let shown = fit(&trail, 3, SPACING, natural);
        assert!(shown.iter().zip(&trail).all(|(shown, crumb)| *shown == Some(crumb.full)));
    }

    #[test]
    fn long_middle_crumbs_are_cut_before_any_folds() {
        let trail = deep();
        let natural =
            width(&trail.iter().map(|crumb| Some(crumb.full)).collect::<Vec<_>>(), SPACING);
        let shown = fit(&trail, 3, SPACING, natural - 50.0);
        assert!(
            shown.iter().all(Option::is_some),
            "cutting the connection and the repository is enough"
        );
        assert_eq!(shown[1], Some(160.0));
        assert_eq!(shown[2], Some(172.0));
        assert_eq!(
            shown[7],
            Some(188.0),
            "a later crumb stays whole while cutting earlier ones suffices"
        );
    }

    #[test]
    fn crumbs_cut_before_a_fold_grow_whole_where_the_fold_left_room() {
        let trail = deep();
        let shown = fit(&trail, 3, SPACING, 1020.0);
        assert_eq!(&shown[1..3], [None, None], "the connection and the repository fold");
        assert_eq!(shown[7], Some(188.0), "the long folder shows whole again");
        assert!(width(&shown, SPACING) <= 1020.0);
    }

    #[test]
    fn folding_keeps_the_first_the_anchor_and_the_last() {
        let trail = deep();
        for budget in [900.0, 700.0, 560.0, 490.0] {
            let shown = fit(&trail, 3, SPACING, budget);
            assert!(width(&shown, SPACING) <= budget, "{budget}: {shown:?}");
            assert_eq!(shown[0], Some(80.0));
            assert_eq!(shown[10], Some(210.0), "{budget}: the current page stays whole");
            assert!(
                shown[3].is_some(),
                "{budget}: the pull request is the last middle crumb to go"
            );
            let first_folded = shown.iter().position(Option::is_none);
            assert_eq!(first_folded, Some(1), "{budget}: \"…\" sits right after the first crumb");
        }
    }

    #[test]
    fn a_tight_budget_folds_everything_and_cuts_the_last_crumb() {
        let trail = deep();
        let shown = fit(&trail, 3, SPACING, 260.0);
        assert!(shown[1..10].iter().all(Option::is_none));
        assert_eq!(shown[0], Some(80.0));
        assert!(shown[10].is_some_and(|last| (MIN_CUT_WIDTH..210.0).contains(&last)));
        assert!(width(&shown, SPACING) <= 260.0);
        let tiny = fit(&trail, 3, SPACING, 120.0);
        assert_eq!(tiny[10], Some(MIN_CUT_WIDTH));
        assert!(
            tiny[0].is_some_and(|first| first < 80.0),
            "the first crumb gives way after the last"
        );
    }
}
