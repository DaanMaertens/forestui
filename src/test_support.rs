//! Helpers shared by the unit tests of several modules.

use std::path::Path;
use std::process::Command;

/// Writes an executable that can be run straight away without failing on
/// `ETXTBSY` ("Text file busy").
///
/// Writing it in-process with `std::fs::write` and running it is racy: tests
/// run in parallel threads, and a thread that spawns a process while the file
/// is still open for writing hands that child a copy of the write handle,
/// which it holds until its own exec. Linux refuses to run a file while any
/// process has it open for writing. The contents are therefore staged under
/// another name and the executable itself is written by `install`, a child
/// process that starts nothing else, so the write handle never exists inside
/// the test binary for a sibling thread to pass on.
pub fn write_executable(path: &Path, contents: &str) {
    let staged = path.with_extension("staged");
    std::fs::write(&staged, contents).expect("stage the executable");
    let status = Command::new("install")
        .arg("-m")
        .arg("755")
        .arg(&staged)
        .arg(path)
        .status()
        .expect("install runs");
    assert!(status.success(), "install {}", path.display());
    std::fs::remove_file(&staged).expect("remove the staged copy");
}
