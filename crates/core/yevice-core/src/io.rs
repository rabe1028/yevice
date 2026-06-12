use std::io::{self, ErrorKind};
use std::path::Path;

/// IaC input file maximum size in bytes (OOM prevention limit).
pub const MAX_IAC_FILE_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB

/// Read a file as a UTF-8 string, returning an error if the file exceeds
/// [`MAX_IAC_FILE_BYTES`].
///
/// The size is checked via [`std::fs::metadata`] (which follows symlinks)
/// before reading. This is a best-effort OOM guard for local, trusted IaC
/// files: it does not defend against a file that grows between the metadata
/// check and the read (TOCTOU), nor against pseudo-files whose reported
/// length differs from the bytes produced. That is acceptable under the
/// threat model (developer-supplied config files), not adversarial FS races.
///
/// # Errors
///
/// Returns [`io::Error`] with [`ErrorKind::InvalidData`] when the file size
/// exceeds the limit, or any other [`io::Error`] that the underlying
/// [`std::fs`] operations may produce.
pub fn read_to_string_capped(path: &Path) -> io::Result<String> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_IAC_FILE_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file too large: {len} bytes exceeds limit {MAX_IAC_FILE_BYTES}"),
        ));
    }
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn read_to_string_capped_succeeds_for_small_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_small_{}.txt",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"hello world").unwrap();
        drop(f);

        let result = read_to_string_capped(&path).unwrap();
        assert_eq!(result, "hello world");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_to_string_capped_fails_for_oversized_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_large_{}.bin",
            std::process::id()
        ));

        // Write MAX_IAC_FILE_BYTES + 1 bytes so it exceeds the limit.
        let size = (MAX_IAC_FILE_BYTES + 1) as usize;
        let data = vec![b'x'; size];
        std::fs::write(&path, &data).unwrap();

        let err = read_to_string_capped(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("file too large"),
            "error message should mention 'file too large', got: {err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_to_string_capped_succeeds_at_exact_limit() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yevice_core_io_test_exact_{}.bin",
            std::process::id()
        ));

        // Write exactly MAX_IAC_FILE_BYTES bytes (all ASCII spaces).
        let size = MAX_IAC_FILE_BYTES as usize;
        let data = vec![b' '; size];
        std::fs::write(&path, &data).unwrap();

        let result = read_to_string_capped(&path);
        assert!(result.is_ok(), "exact limit should succeed");

        let _ = std::fs::remove_file(&path);
    }
}
