//! Frame-budget benchmark of the file tree at 4K: a 10k-file tree. `#[ignore]`d; see
//! `cv_testkit::frame_bench` for how to run it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{pos2, vec2};
use review_core::FileCategory;
use review_git::{ChangeKind, ChangedFile};

use crate::{FileTree, TreeInputs, VoteOffer, side_panel};
use cv_testkit::frame_bench::{Bench, Report, SCALES, finish};

const TREE_FILES: usize = 10_000;

fn many_files() -> Vec<ChangedFile> {
    let categories = [
        FileCategory::Code,
        FileCategory::Code,
        FileCategory::Tests,
        FileCategory::Docs,
        FileCategory::Config,
    ];
    let kinds = [
        ChangeKind::Modified,
        ChangeKind::Added,
        ChangeKind::Modified,
        ChangeKind::Deleted,
        ChangeKind::Renamed,
    ];
    (0..TREE_FILES)
        .map(|index| {
            let path = format!(
                "src/area_{}/module_{}/feature_{}/component_with_a_longer_name_{index}.rs",
                index % 7,
                index / 7 % 13,
                index / 91 % 11
            );
            ChangedFile {
                old_path: (index % 5 == 4).then(|| format!("src/old/place_{index}.rs")),
                old_oid: format!("{index:040x}"),
                new_oid: format!("{:040x}", index + 1),
                additions: Some((index * 7 % 400) as u32),
                deletions: Some((index * 3 % 200) as u32),
                category: categories[index % categories.len()],
                ..ChangedFile::sample(&path, kinds[index % kinds.len()])
            }
        })
        .collect()
}

struct TreeState {
    tree: FileTree,
    files: Arc<Vec<ChangedFile>>,
    viewed: HashSet<String>,
    thread_counts: HashMap<String, usize>,
}

fn bench_tree(pixels_per_point: f32, reports: &mut Vec<Report>) {
    let files = Arc::new(many_files());
    let viewed = files.iter().step_by(3).map(|file| file.path.clone()).collect();
    let thread_counts = files.iter().step_by(17).map(|file| (file.path.clone(), 2)).collect();
    let state = TreeState { tree: FileTree::default(), files, viewed, thread_counts };
    let mut bench = Bench::new(pixels_per_point, state, |ui, state: &mut TreeState| {
        let viewed = &state.viewed;
        let is_viewed = |file: &ChangedFile| viewed.contains(&file.path);
        state.tree.refresh(&state.files, &is_viewed, 0);
        let inputs = TreeInputs {
            files: &state.files,
            selected: Some(5),
            selected_folder: None,
            is_viewed: &is_viewed,
            thread_counts: &state.thread_counts,
            vote: VoteOffer::None,
        };
        side_panel::show(ui, 340.0, &mut state.tree, &inputs, false);
    });
    let rect = bench.ctx.content_rect();
    let pointer = pos2(rect.left() + 150.0, rect.center().y);
    bench.warm_up(pointer);
    reports.extend(bench.scroll_patterns("tree 10k files", pointer, vec2(0.0, -1.0)));
}

#[test]
#[ignore = "release-mode frame budget benchmark; see cv_testkit::frame_bench"]
fn frame_budget_tree() {
    let mut reports = Vec::new();
    for pixels_per_point in SCALES {
        bench_tree(pixels_per_point, &mut reports);
    }
    finish(&reports);
}
