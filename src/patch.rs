use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use itertools::Itertools;

pub fn iter_patch_files(patch_patch: &Path) -> impl Iterator<Item = PatchFile> {
    if !patch_patch.exists() {
        eprintln!("Patch directory doesn't exists !");
        std::process::exit(1);
    }

    std::fs::read_dir(patch_patch)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .filter(|e| e.path().extension() == Some(OsStr::new("patch")))
        .map(|e| PatchFile::read(e.path()).unwrap())
}

#[derive(Debug)]
pub enum PatchError {
    AlreadyApplied,
    OutdatedSource,
}

#[derive(Debug)]
pub struct PatchFile {
    pub path: PathBuf,
    pub of: PathBuf,
    pub patch: String,
}

impl PatchFile {
    pub fn read(path: PathBuf) -> std::io::Result<Self> {
        let patch = std::fs::read_to_string(&path).unwrap();
        let parsed = gitpatch::Patch::from_single(&patch).unwrap();

        let patch_target_a = Path::new(&*parsed.old.path).strip_prefix("a").unwrap();
        let patch_target_b = Path::new(&*parsed.new.path).strip_prefix("b").unwrap();
        assert_eq!(patch_target_a, patch_target_b);

        let of = patch_target_a;

        Ok(Self {
            path,
            of: PathBuf::from("/").join(of),
            patch,
        })
    }

    pub fn as_gitpatch(&self) -> gitpatch::Patch<'_> {
        gitpatch::Patch::from_single(&self.patch).unwrap()
    }

    pub fn apply(&self, source: String) -> Result<String, PatchError> {
        let patch = self.as_gitpatch();

        let mut prev_hunk_end_in_source = 1;
        let mut target = String::with_capacity(source.len());
        let mut hunk_results = Vec::new();

        for hunk in &patch.hunks {
            // Fast forward to current range

            for l in source
                .lines()
                .skip(prev_hunk_end_in_source - 1)
                .take(hunk.old_range.start as usize - prev_hunk_end_in_source)
            {
                target.push_str(l);
                target.push('\n');
            }

            // Now construct the real hunks

            let mut expected_source_hunk = String::new();
            let mut target_hunk = String::new();

            for change in &hunk.lines {
                match change {
                    gitpatch::Line::Context(context) => {
                        expected_source_hunk.push_str(context);
                        expected_source_hunk.push('\n');
                        target_hunk.push_str(context);
                        target_hunk.push('\n');
                    }
                    gitpatch::Line::Add(added) => {
                        target_hunk.push_str(added);
                        target_hunk.push('\n');
                    }
                    gitpatch::Line::Remove(removed) => {
                        expected_source_hunk.push_str(removed);
                        expected_source_hunk.push('\n');
                    }
                }
            }

            assert_eq!(
                expected_source_hunk.lines().count(),
                hunk.old_range.count as _
            );
            assert_eq!(target_hunk.lines().count(), hunk.new_range.count as _);

            // Now compare to source to decide if it was successful

            let actual_source = source
                .lines()
                .skip(hunk.old_range.start as usize - 1)
                .take(hunk.old_range.count as _)
                .chain(std::iter::once(""))
                .join("\n");

            if actual_source == expected_source_hunk {
                hunk_results.push(Ok(()));
            } else {
                let actual_source_as_target = source
                    .lines()
                    .skip(hunk.new_range.start as usize - 1)
                    .take(hunk.new_range.count as _)
                    .chain(std::iter::once(""))
                    .join("\n");
                if actual_source_as_target == target_hunk {
                    hunk_results.push(Err(PatchError::AlreadyApplied));
                } else {
                    hunk_results.push(Err(PatchError::OutdatedSource));
                }
            }

            target.push_str(&target_hunk);
            prev_hunk_end_in_source = (hunk.old_range.start + hunk.old_range.count) as _;
        }

        // Paste end of file

        for line in source.lines().skip(prev_hunk_end_in_source - 1) {
            target.push_str(line);
            target.push('\n');
        }
        if !patch.end_newline {
            target.pop();
        }

        // To determine the error status of the function we need to look for those cases:
        // - All hunks successfully applied: no errors
        // - All hunks returned 'already applied': already applied
        // - Anything else: malformed/outdated source

        if hunk_results.iter().all(|r| matches!(r, Ok(()))) {
            Ok(target)
        } else if hunk_results
            .iter()
            .all(|r| matches!(r, Err(PatchError::AlreadyApplied)))
        {
            Err(PatchError::AlreadyApplied)
        } else {
            Err(PatchError::OutdatedSource)
        }
    }
}
