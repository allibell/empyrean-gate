//! Bounded persistent application logging and operator-safe diagnostics export.

use log::{Log, Metadata, Record};
use parking_lot::Mutex;
use regex::Regex;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE: &str = "empyrean-gate.log";
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const BACKUP_FILES: usize = 3;
const EXPORT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct DiagnosticsStatus {
    pub path: String,
    pub active: bool,
    pub error: String,
}

struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    len: u64,
}

impl RotatingFile {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path,
            file: Some(file),
            len,
        })
    }

    fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        // A malformed or unusually large error must not defeat the disk bound.
        let line = &line[..line.len().min(MAX_FILE_BYTES as usize)];
        if self.len > 0 && self.len.saturating_add(line.len() as u64) > MAX_FILE_BYTES {
            self.rotate()?;
        }
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
            self.len = self.file.as_ref().unwrap().metadata()?.len();
        }
        let file = self.file.as_mut().unwrap();
        file.write_all(line)?;
        file.flush()?;
        self.len = self.len.saturating_add(line.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        for index in (1..=BACKUP_FILES).rev() {
            let source = if index == 1 {
                self.path.clone()
            } else {
                backup_path(&self.path, index - 1)
            };
            let destination = backup_path(&self.path, index);
            if source.exists() {
                let _ = std::fs::remove_file(&destination);
                std::fs::rename(source, destination)?;
            }
        }
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.len = 0;
        Ok(())
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), index))
}

struct PersistentLogger {
    console: env_logger::Logger,
    file: Option<Arc<Mutex<RotatingFile>>>,
}

impl Log for PersistentLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.console.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        self.console.log(record);
        let Some(file) = &self.file else { return };
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!(
            "{timestamp} {:<5} {} — {}\n",
            record.level(),
            record.target(),
            record.args()
        );
        // Logging must never take the show down. A disk-full or permission error
        // leaves console logging alive and is deliberately ignored here.
        let _ = file.lock().write_line(line.as_bytes());
    }

    fn flush(&self) {
        self.console.flush();
        if let Some(file) = &self.file {
            let mut file = file.lock();
            if let Some(handle) = file.file.as_mut() {
                let _ = handle.flush();
            }
        }
    }
}

static STATUS: OnceLock<DiagnosticsStatus> = OnceLock::new();

pub fn init() -> DiagnosticsStatus {
    let path = crate::config::config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("logs")
        .join(LOG_FILE);
    let (file, error) = match RotatingFile::open(path.clone()) {
        Ok(file) => (Some(Arc::new(Mutex::new(file))), String::new()),
        Err(error) => (None, error.to_string()),
    };
    let active = file.is_some();
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    let console = builder.build();
    let max_level = console.filter();
    let logger = PersistentLogger { console, file };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max_level);
    }
    let status = DiagnosticsStatus {
        path: path.display().to_string(),
        active,
        error,
    };
    let _ = STATUS.set(status.clone());
    status
}

pub fn status() -> DiagnosticsStatus {
    STATUS.get().cloned().unwrap_or_default()
}

/// Return the newest bounded slice of all retained logs, oldest lines first.
/// Exact configured secrets and credential-shaped URL/query fragments are removed.
pub fn recent_text(secrets: &[&str]) -> std::io::Result<String> {
    let status = status();
    let path = PathBuf::from(status.path);
    let mut bytes = Vec::new();
    for index in (1..=BACKUP_FILES).rev() {
        append_tail(&backup_path(&path, index), &mut bytes)?;
    }
    append_tail(&path, &mut bytes)?;
    if bytes.len() > EXPORT_BYTES {
        bytes.drain(..bytes.len() - EXPORT_BYTES);
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(redact(&text, secrets))
}

fn append_tail(path: &Path, output: &mut Vec<u8>) -> std::io::Result<()> {
    let Ok(mut file) = File::open(path) else {
        return Ok(());
    };
    let len = file.metadata()?.len();
    let keep = (EXPORT_BYTES as u64).min(len);
    file.seek(SeekFrom::Start(len - keep))?;
    file.read_to_end(output)?;
    Ok(())
}

fn redact(text: &str, secrets: &[&str]) -> String {
    let mut safe = text.to_string();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        safe = safe.replace(secret, "[REDACTED]");
    }
    static QUERY_SECRET: OnceLock<Regex> = OnceLock::new();
    let pattern = QUERY_SECRET.get_or_init(|| {
        Regex::new(
            r"(?i)([?&](?:join|token|access_token|auth(?:_token)?|key|api_key|password|secret|signature|sig)=)[^&\s]+",
        )
            .expect("diagnostics redaction regex")
    });
    safe = pattern.replace_all(&safe, "$1[REDACTED]").into_owned();
    static URL_USERINFO: OnceLock<Regex> = OnceLock::new();
    let userinfo = URL_USERINFO.get_or_init(|| {
        Regex::new(r"(?i)(https?://)[^/@\s]+:[^/@\s]+@").expect("URL userinfo redaction regex")
    });
    userinfo.replace_all(&safe, "$1[REDACTED]@").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_exact_and_query_secrets() {
        let safe = redact(
            "join=known-secret https://user:pass@gate/?join=url-secret&x=1&auth_token=also-secret&api_key=key-secret",
            &["known-secret"],
        );
        assert!(!safe.contains("known-secret"));
        assert!(!safe.contains("url-secret"));
        assert!(!safe.contains("also-secret"));
        assert!(!safe.contains("key-secret"));
        assert!(!safe.contains("user:pass"));
        assert_eq!(safe.matches("[REDACTED]").count(), 5);
    }

    #[test]
    fn rotating_file_is_bounded() {
        let dir = std::env::temp_dir().join(format!(
            "empyrean-diagnostics-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join(LOG_FILE);
        let mut file = RotatingFile::open(path.clone()).unwrap();
        let chunk = vec![b'x'; (MAX_FILE_BYTES / 2 + 1) as usize];
        for _ in 0..8 {
            file.write_line(&chunk).unwrap();
        }
        assert!(path.metadata().unwrap().len() <= MAX_FILE_BYTES);
        assert!(backup_path(&path, BACKUP_FILES).exists());
        assert!(!backup_path(&path, BACKUP_FILES + 1).exists());
        drop(file);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_oversized_record_is_truncated_to_the_file_limit() {
        let dir = std::env::temp_dir().join(format!(
            "empyrean-diagnostics-large-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join(LOG_FILE);
        let mut file = RotatingFile::open(path.clone()).unwrap();
        file.write_line(&vec![b'x'; MAX_FILE_BYTES as usize + 100])
            .unwrap();
        assert_eq!(path.metadata().unwrap().len(), MAX_FILE_BYTES);
        drop(file);
        let _ = std::fs::remove_dir_all(dir);
    }
}
