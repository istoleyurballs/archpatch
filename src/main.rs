use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use alpm::Alpm;
use archpatch::{BackupFile, PatchError, PatchFile};
use clap::Parser;
use tempfile::NamedTempFile;

#[derive(clap::Parser)]
pub struct Args {
    #[arg(long, default_value = "/usr/share/archpatch/patches")]
    patch_path: PathBuf,

    #[command(subcommand)]
    cmds: Commands,
}

#[derive(clap::Subcommand)]
pub enum Commands {
    /// Collect and print the status of the installation.
    Report {
        #[arg(long)]
        skip_pacreport: bool,
    },
    /// Given a backuped file, produce the diff to apply to the original to get the current file.
    Diff {
        target: PathBuf,
        #[arg(long)]
        no_context: bool,
    },
    /// Apply all patches.
    Apply,
}

fn main() {
    let args = Args::parse();

    match &args.cmds {
        Commands::Report { skip_pacreport } => do_report(&args, *skip_pacreport),
        Commands::Diff { target, no_context } => do_diff(&args, target, !no_context),
        Commands::Apply => do_apply(&args),
    }
}

/// This commands extends the output of `pacreport` with patch information.
fn do_report(args: &Args, skip_pacreport: bool) {
    let config = pacmanconf::Config::new().unwrap();
    let alpm = Alpm::new(config.root_dir, config.db_path).unwrap();
    let db = alpm.localdb();

    if !skip_pacreport {
        // Call pacreport with all options.
        let pacreport_output = Command::new("pacreport")
            .arg("--backups")
            .arg("--missing-files")
            .arg("--unowned-files")
            .output()
            .unwrap();
        if !pacreport_output.status.success() {
            eprintln!(
                "Pacreport failed with exit code {}",
                pacreport_output.status.code().unwrap()
            );
            return;
        }
        print!("{}", String::from_utf8_lossy(&pacreport_output.stdout));
    }

    // For each alpm backup file, if changed, check that there is a patch that results in this
    // file.

    println!("Modified Backup Files:");

    let backup_files = archpatch::iter_backup_files(db).collect::<Vec<_>>();
    let patch_files = archpatch::iter_patch_files(&args.patch_path).collect::<Vec<_>>();

    enum BackupStatus {
        Missing,
        UnpatchedChanges,
        PatchOutdated,
        FailedComputeMd5,
    }
    struct BackupReport<'a> {
        status: BackupStatus,
        backup: &'a BackupFile,
        patch: Option<&'a PatchFile>,
    }
    let mut backup_reports = Vec::new();

    for backup in &backup_files {
        if !backup.path.exists() {
            backup_reports.push(BackupReport {
                status: BackupStatus::Missing,
                backup,
                patch: None,
            });
            continue;
        }

        // Compute md5 to see if it changed.
        let Ok(real_md5) = alpm::compute_md5sum(backup.path.as_os_str().as_encoded_bytes()) else {
            backup_reports.push(BackupReport {
                status: BackupStatus::FailedComputeMd5,
                backup,
                patch: None,
            });
            continue;
        };

        if real_md5 == backup.original_md5 {
            // Same file (probably).
            continue;
        }

        // Need to look up if there is a patch.
        let Some(patch) = patch_files.iter().find(|p| p.of == backup.path) else {
            // No patch found, the file is dirty.
            backup_reports.push(BackupReport {
                status: BackupStatus::UnpatchedChanges,
                backup,
                patch: None,
            });
            continue;
        };

        // Look up the original file.
        let original_file = archpatch::read_original_file_from_archive(
            db.pkg(backup.package.as_bytes()).unwrap(),
            &backup.path,
        );

        // Try to apply the patch.
        match patch.apply(original_file) {
            Ok(expected_content) => {
                let real_content = std::fs::read_to_string(&backup.path).unwrap();
                if expected_content != real_content {
                    backup_reports.push(BackupReport {
                        status: BackupStatus::UnpatchedChanges,
                        backup,
                        patch: Some(patch),
                    });
                    continue;
                } else {
                    // The patch is valid.
                    continue;
                }
            }
            Err(PatchError::AlreadyApplied) => {
                panic!("This is like not possible");
            }
            Err(PatchError::OutdatedSource) => {
                backup_reports.push(BackupReport {
                    status: BackupStatus::PatchOutdated,
                    backup,
                    patch: Some(patch),
                });
                continue;
            }
        }
    }

    let pad_to = backup_reports
        .iter()
        .map(|r| r.backup.package.len() + r.backup.path.as_os_str().len())
        .max()
        .unwrap_or(0)
        + 5;

    for report in backup_reports {
        print!(
            "  {}: {:width$} - ",
            &report.backup.package,
            report.backup.path.display(),
            width = pad_to - report.backup.package.len()
        );
        match report {
            BackupReport {
                status: BackupStatus::Missing,
                ..
            } => println!("Missing"),
            BackupReport {
                status: BackupStatus::UnpatchedChanges,
                patch: None,
                ..
            } => println!("Unpatched changes"),
            BackupReport {
                status: BackupStatus::UnpatchedChanges,
                patch: Some(patch),
                ..
            } => println!("More changes that in patch {}", patch.path.display()),
            BackupReport {
                status: BackupStatus::PatchOutdated,
                patch: Some(patch),
                ..
            } => println!("Outdated patch at {}", patch.path.display()),
            BackupReport {
                status: BackupStatus::PatchOutdated,
                patch: None,
                ..
            } => unreachable!(),
            BackupReport {
                status: BackupStatus::FailedComputeMd5,
                ..
            } => println!("Failed to compute md5"),
        }
    }

    // Print patch status.

    println!("Patches Application Status:");

    // For pretty printing.
    let pad_to = patch_files
        .iter()
        .map(|p| p.path.as_os_str().len())
        .max()
        .unwrap()
        + 3;

    for patch in &patch_files {
        print!("  {:width$} - ", patch.path.display(), width = pad_to);

        // Check if duplicated.

        if patch_files
            .iter()
            .any(|other| other.of == patch.of && other.path != patch.path)
        {
            print!("DUPLICATED PATCH TARGET - ");
        }

        // Check if name outside of convention.

        let expected_target = PathBuf::from("/").join(
            patch
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .replace("-", "/")
                .strip_suffix(".patch")
                .unwrap(),
        );

        if expected_target != patch.of {
            print!("BADLY NAMED - ");
        }

        // Try to apply the patch to check its status.

        let Ok(source) = std::fs::read_to_string(&patch.of) else {
            println!("Failed to read targetted file");
            continue;
        };

        match patch.apply(source) {
            Ok(_) => println!("Not applied !!"),
            Err(PatchError::AlreadyApplied) => println!("Applied"),
            Err(PatchError::OutdatedSource) => println!("Outdated !!"),
        }
    }
}

fn do_diff(_args: &Args, target: &Path, with_context: bool) {
    let config = pacmanconf::Config::new().unwrap();
    let alpm = Alpm::new(config.root_dir, config.db_path).unwrap();
    let db = alpm.localdb();

    if !target.exists() {
        eprintln!("Target file doesn't exists !");
        std::process::exit(1);
    }

    let target = target
        .canonicalize()
        .expect("Failed to canonicalize target path");

    // Find the package that owns this file (if any).

    let prefixless_target = &target.to_string_lossy()[1..];

    let Some(pkg) = db
        .pkgs()
        .iter()
        .find_map(|p| p.files().contains(prefixless_target.as_bytes()).map(|_| p))
    else {
        eprintln!("Target file isn't owned by any package");
        std::process::exit(1);
    };

    // Check for changes.

    let mut temp = NamedTempFile::new().expect("Failed to create temp file");
    write!(
        temp,
        "{}",
        archpatch::read_original_file_from_archive(pkg, &target)
    )
    .unwrap();

    // Run git diff

    let diff_output = Command::new("git")
        .arg("diff")
        .arg("--patch")
        .arg(if with_context {
            "--unified"
        } else {
            "--unified=0"
        })
        .arg("--no-index")
        .arg("--color=always")
        .arg(temp.path())
        .arg(&target)
        .output()
        .expect("Failed to git diff");

    if diff_output.status.success() {
        println!("Target is unchanged");
        return;
    }

    let patch = String::from_utf8_lossy(&diff_output.stdout)
        .replace(temp.path().to_str().unwrap(), target.to_str().unwrap());

    println!("{patch}");
}

fn do_apply(args: &Args) {
    let patch_files = archpatch::iter_patch_files(&args.patch_path).collect::<Vec<_>>();

    let mut errored = false;

    for patch in patch_files {
        print!("Patching {}... ", patch.of.display());
        let target_content = std::fs::read_to_string(&patch.of).unwrap();

        match patch.apply(target_content) {
            Ok(patched_content) => match std::fs::write(&patch.of, &patched_content) {
                Ok(_) => println!("OK"),
                Err(err) => {
                    println!("ERR ({err})");
                    errored = true;
                }
            },
            Err(PatchError::AlreadyApplied) => {
                println!("OK (already applied)");
            }
            Err(PatchError::OutdatedSource) => {
                println!("ERROR (patch is outdated)");
                errored = true;
            }
        };
    }

    if errored {
        std::process::exit(1);
    }
}
