//! Prints the license of every grammar from bat's set that asks to be named. `build.rs` bakes those
//! grammars into the program through a build dependency, so the crate list of a release's notices
//! cannot see them; `ci/build-release.sh` appends this output.

use std::io::{self, Write};

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();
    for license in two_face::acknowledgement::listing().for_syntaxes() {
        writeln!(
            out,
            "--------------------------------------------------------------------------------"
        )?;
        writeln!(out, "{}\n\n{}\n", license.rel_path.display(), license.text.trim_end())?;
    }
    Ok(())
}
