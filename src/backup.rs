use std::path::PathBuf;

pub fn iter_backup_files(db: &alpm::Db) -> impl Iterator<Item = BackupFile> {
    db.pkgs().iter().flat_map(|p| {
        p.backup().iter().map(|b| BackupFile {
            package: p.name().to_string(),
            path: PathBuf::from("/").join(b.name()),
            original_md5: b.hash().to_string(),
        })
    })
}

#[derive(Debug)]
pub struct BackupFile {
    pub package: String,
    pub path: PathBuf,
    pub original_md5: String,
}
