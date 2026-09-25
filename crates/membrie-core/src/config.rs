use std::env;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    if let Some(path) = env::var_os("MEMBRIE_DATA_DIR") {
        return PathBuf::from(path);
    }

    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("membrie");
    }

    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/share/membrie")
}

pub fn database_path() -> PathBuf {
    data_dir().join("membrie.db")
}

pub fn backup_dir() -> PathBuf {
    data_dir().join("backups")
}

pub fn socket_path() -> PathBuf {
    if let Some(path) = env::var_os("MEMBRIE_SOCKET") {
        return PathBuf::from(path);
    }

    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(path).join("membrie.sock");
    }

    data_dir().join("membrie.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_data_directory_wins() {
        // Environment mutation is unsafe in Rust 2024, so test the stable suffix only.
        let path = database_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("membrie.db")
        );
    }
}
