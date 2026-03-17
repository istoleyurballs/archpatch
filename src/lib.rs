mod backup;
mod patch;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

pub use backup::*;
pub use patch::*;

pub fn read_original_file_from_archive(pkg: &alpm::Package, path: &PathBuf) -> String {
    assert!(path.is_absolute());

    let package_archive = PathBuf::from("/var/cache/pacman/pkg").join(format!(
        "{}-{}-{}.pkg.tar.zst",
        pkg.name(),
        pkg.version(),
        pkg.arch().unwrap()
    ));

    // Extract the specific file.

    let tar_output = Command::new("tar")
        .arg("-xOf")
        .arg(package_archive)
        .arg(&path.to_string_lossy()[1..])
        .stdout(Stdio::piped())
        .output()
        .expect("Failed to extract backup file archive");

    if !tar_output.status.success() {
        panic!(
            "tar -xOf exited with status code {}",
            tar_output.status.code().unwrap()
        );
    }

    String::from_utf8(tar_output.stdout).expect("Original file contains non UTF-8 stuff")
}
