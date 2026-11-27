//! Test probes of shipped paint code: a painter notes what it painted in this pass, and a test
//! reads what the last finished pass painted. The notes live in `ctx.data` under their key, tagged
//! with the pass that wrote them, so the first note of a new pass drops the notes of the one
//! before and a pass that notes nothing leaves nothing to read.

/// Notes `item` under `key` for the pass being painted.
pub fn note<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: &'static str, item: T) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        let notes = data.get_temp_mut_or_default::<Notes<T>>(egui::Id::unique(key));
        if notes.pass != pass {
            *notes = Notes { pass, items: Vec::new() };
        }
        notes.items.push(item);
    });
}

/// What the last finished pass noted under `key`; empty when that pass noted nothing.
pub fn last<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: &'static str) -> Vec<T> {
    let notes = ctx.data(|data| data.get_temp::<Notes<T>>(egui::Id::unique(key)));
    match notes {
        Some(notes) if notes.pass + 1 == ctx.cumulative_pass_nr() => notes.items,
        _ => Vec::new(),
    }
}

/// The items one pass noted under a key.
#[derive(Clone)]
struct Notes<T> {
    pass: u64,
    items: Vec<T>,
}

impl<T> Default for Notes<T> {
    fn default() -> Self {
        Self { pass: 0, items: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "frame_probe_test";

    #[test]
    fn the_notes_of_a_pass_are_read_in_the_next_and_gone_after_an_empty_one() {
        let ctx = egui::Context::default();
        let mut seen = Vec::new();
        for paints in [vec![1_u32, 2], Vec::new(), Vec::new()] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                seen.push(last::<u32>(ui.ctx(), KEY));
                for item in &paints {
                    note(ui.ctx(), KEY, *item);
                }
            });
            // No renderer takes the font atlas here.
            output.textures_delta.clear();
        }
        assert_eq!(seen, [Vec::new(), vec![1, 2], Vec::new()]);
    }
}
