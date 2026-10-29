use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Error types encountered during AST and file patching operations.
#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("Target string was empty")]
    EmptyTarget,

    #[error("Target block not found in content (preview: '{target_preview}')")]
    TargetNotFound { target_preview: String },

    #[error("Regex compilation error: {0}")]
    Regex(#[from] regex::Error),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Atomic rename failed from '{from}' to '{to}': {reason}")]
    AtomicRename {
        from: String,
        to: String,
        reason: String,
    },

    #[error("Syntax validation error: {0}")]
    Syntax(#[from] crate::validator::SyntaxError),
}

/// Statistics and output from an exact block replacement operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchResult {
    /// Number of occurrences of the target block replaced.
    pub occurrences: usize,
    /// Length of the original content in bytes.
    pub original_len: usize,
    /// Length of the patched content in bytes.
    pub patched_len: usize,
    /// The resulting patched content.
    pub content: String,
}

/// Statistics and output from a streaming regex substitution operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPatchResult {
    /// Total number of lines read from input.
    pub lines_processed: usize,
    /// Number of distinct lines where regex matches were replaced.
    pub lines_modified: usize,
    /// Total count of individual regex match replacements performed.
    pub total_replacements: usize,
    /// Total number of bytes written to output.
    pub bytes_written: u64,
}

/// Syntax and whitespace-strict patching engine for exact block and streaming edits.
#[derive(Debug, Clone)]
pub struct AstPatcher {
    /// When true, allows fallback newline normalization (\r\n <-> \n) if exact match fails.
    lenient_newlines: bool,
    /// When true, validates syntax before applying patches to supported source files.
    syntax_validation: bool,
}

impl Default for AstPatcher {
    fn default() -> Self {
        Self {
            lenient_newlines: false,
            syntax_validation: true,
        }
    }
}

impl AstPatcher {
    /// Create a new AstPatcher with strict whitespace and exact newline matching.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure whether to permit newline-normalization fallback if exact matching fails.
    pub fn with_lenient_newlines(mut self, lenient: bool) -> Self {
        self.lenient_newlines = lenient;
        self
    }

    /// Configure whether syntax validation is enforced before writing patches.
    pub fn with_syntax_validation(mut self, enabled: bool) -> Self {
        self.syntax_validation = enabled;
        self
    }

    /// Performs exact whitespace-matched contiguous block replacement on an in-memory string.
    ///
    /// Follows strict agent patching rules:
    /// 1. Target must match existing text exactly, including whitespace and line breaks.
    /// 2. If target appears multiple times, every occurrence is replaced.
    /// 3. If replacement is empty, the target block is deleted.
    /// 4. If target is empty, returns `PatchError::EmptyTarget`.
    /// 5. If target is not found, returns `PatchError::TargetNotFound`.
    pub fn patch_string(
        &self,
        content: &str,
        target: &str,
        replacement: &str,
    ) -> Result<PatchResult, PatchError> {
        if target.is_empty() {
            return Err(PatchError::EmptyTarget);
        }

        let occurrences = content.matches(target).count();

        if occurrences > 0 {
            let patched = content.replace(target, replacement);
            return Ok(PatchResult {
                occurrences,
                original_len: content.len(),
                patched_len: patched.len(),
                content: patched,
            });
        }

        // Optional newline-normalization fallback (CRLF <-> LF)
        if self.lenient_newlines {
            let norm_target = target.replace("\r\n", "\n");
            let norm_content = content.replace("\r\n", "\n");
            let norm_occurrences = norm_content.matches(&norm_target).count();

            if norm_occurrences > 0 {
                let norm_replacement = replacement.replace("\r\n", "\n");
                let is_crlf = content.contains("\r\n");
                let mut patched = norm_content.replace(&norm_target, &norm_replacement);
                if is_crlf {
                    patched = patched.replace('\n', "\r\n");
                }

                return Ok(PatchResult {
                    occurrences: norm_occurrences,
                    original_len: content.len(),
                    patched_len: patched.len(),
                    content: patched,
                });
            }
        }

        let target_preview = if target.len() > 80 {
            format!("{}...", target[..80].escape_debug())
        } else {
            target.escape_debug().to_string()
        };

        Err(PatchError::TargetNotFound { target_preview })
    }

    /// Patches a file in place using exact block replacement with atomic temp-file rename.
    pub fn patch_file<P: AsRef<Path>>(
        &self,
        path: P,
        target: &str,
        replacement: &str,
    ) -> Result<PatchResult, PatchError> {
        let path_ref = path.as_ref();
        let original_content = fs::read_to_string(path_ref)?;
        let result = self.patch_string(&original_content, target, replacement)?;

        // Syntax Validation Gate: reject patch if resulting syntax is broken
        if self.syntax_validation {
            let filename = path_ref.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if crate::ast::SupportedLanguage::from_path(filename).is_some() {
                crate::validator::validate_syntax(filename, &result.content)?;
            }
        }

        // Atomically replace file via temp sibling file
        let parent = path_ref.parent().unwrap_or_else(|| Path::new("."));
        let temp_filename = format!(
            ".tmp_patch_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let temp_path = parent.join(temp_filename);

        if let Err(e) = fs::write(&temp_path, &result.content) {
            let _ = fs::remove_file(&temp_path);
            return Err(PatchError::Io(e));
        }

        // Atomic rename
        if let Err(e) = fs::rename(&temp_path, path_ref) {
            // Windows fallback: if destination is locked or already exists, attempt removal or copy
            if let Err(copy_err) = fs::copy(&temp_path, path_ref) {
                let _ = fs::remove_file(&temp_path);
                return Err(PatchError::AtomicRename {
                    from: temp_path.display().to_string(),
                    to: path_ref.display().to_string(),
                    reason: format!("rename error: {}, copy fallback error: {}", e, copy_err),
                });
            }
            let _ = fs::remove_file(&temp_path);
        }

        Ok(result)
    }

    /// Asynchronously patches a file in place using exact block replacement.
    pub async fn patch_file_async<P: AsRef<Path>>(
        &self,
        path: P,
        target: &str,
        replacement: &str,
    ) -> Result<PatchResult, PatchError> {
        let path_ref = path.as_ref();
        let original_content = tokio::fs::read_to_string(path_ref).await?;
        let result = self.patch_string(&original_content, target, replacement)?;

        if self.syntax_validation {
            let filename = path_ref.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if crate::ast::SupportedLanguage::from_path(filename).is_some() {
                crate::validator::validate_syntax(filename, &result.content)?;
            }
        }

        tokio::fs::write(path_ref, &result.content).await?;
        Ok(result)
    }

    /// Edits a specific AST node in a file in place, with validation and atomic rename.
    pub fn edit_node_in_file<P: AsRef<Path>>(
        &self,
        path: P,
        kind: crate::ast::NodeKind,
        name: &str,
        parent_path: Option<&str>,
        replacement: &str,
    ) -> Result<PatchResult, PatchError> {
        let path_ref = path.as_ref();
        let original_content = fs::read_to_string(path_ref)?;
        let filename = path_ref.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let language = crate::ast::SupportedLanguage::from_path(filename).ok_or_else(|| {
            PatchError::TargetNotFound {
                target_preview: format!("Unsupported language for file {}", filename),
            }
        })?;

        let new_content = crate::ast::AstEngine::edit_node(
            &original_content,
            language,
            kind,
            name,
            parent_path,
            replacement,
            self.syntax_validation,
        )
        .map_err(|e| match e {
            crate::ast::AstError::Validation(ve) => PatchError::Syntax(ve),
            other => PatchError::TargetNotFound {
                target_preview: format!("AST edit error: {}", other),
            },
        })?;

        let result = PatchResult {
            occurrences: 1,
            original_len: original_content.len(),
            patched_len: new_content.len(),
            content: new_content,
        };

        // Atomically replace file via temp sibling file
        let parent = path_ref.parent().unwrap_or_else(|| Path::new("."));
        let temp_filename = format!(
            ".tmp_patch_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let temp_path = parent.join(temp_filename);

        if let Err(e) = fs::write(&temp_path, &result.content) {
            let _ = fs::remove_file(&temp_path);
            return Err(PatchError::Io(e));
        }

        // Atomic rename
        if let Err(e) = fs::rename(&temp_path, path_ref) {
            if let Err(copy_err) = fs::copy(&temp_path, path_ref) {
                let _ = fs::remove_file(&temp_path);
                return Err(PatchError::AtomicRename {
                    from: temp_path.display().to_string(),
                    to: path_ref.display().to_string(),
                    reason: format!("rename error: {}, copy fallback error: {}", e, copy_err),
                });
            }
            let _ = fs::remove_file(&temp_path);
        }

        Ok(result)
    }

    /// Streams a line-by-line regex substitution from an arbitrary `BufRead` to `Write`.
    ///
    /// Memory usage remains bounded to the size of individual lines, making this suitable
    /// for multi-megabyte or gigabyte files where sed-style regex replacements are required.
    pub fn stream_replace_regex_reader<R: BufRead, W: Write>(
        &self,
        mut reader: R,
        mut writer: W,
        pattern: &str,
        replacement: &str,
    ) -> Result<StreamPatchResult, PatchError> {
        let regex = Regex::new(pattern)?;
        let mut line_buf = String::new();
        let mut lines_processed = 0;
        let mut lines_modified = 0;
        let mut total_replacements = 0;
        let mut bytes_written = 0u64;

        loop {
            line_buf.clear();
            let bytes_read = reader.read_line(&mut line_buf)?;
            if bytes_read == 0 {
                break;
            }

            lines_processed += 1;

            if regex.is_match(&line_buf) {
                let match_count = regex.find_iter(&line_buf).count();
                total_replacements += match_count;
                lines_modified += 1;

                let replaced = regex.replace_all(&line_buf, replacement);
                writer.write_all(replaced.as_bytes())?;
                bytes_written += replaced.len() as u64;
            } else {
                writer.write_all(line_buf.as_bytes())?;
                bytes_written += line_buf.len() as u64;
            }
        }

        writer.flush()?;

        Ok(StreamPatchResult {
            lines_processed,
            lines_modified,
            total_replacements,
            bytes_written,
        })
    }

    /// Streams regex sed-style substitution directly across a large file on disk.
    ///
    /// Reads from the target file and writes to a temporary sibling file, atomically
    /// replacing the source file on successful completion. Cleans up the temporary file
    /// if an error occurs.
    pub fn stream_replace_regex_file<P: AsRef<Path>>(
        &self,
        path: P,
        pattern: &str,
        replacement: &str,
    ) -> Result<StreamPatchResult, PatchError> {
        let path_ref = path.as_ref();
        let parent = path_ref.parent().unwrap_or_else(|| Path::new("."));
        let temp_filename = format!(
            ".tmp_stream_patch_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let temp_path = parent.join(temp_filename);

        let input_file = File::open(path_ref)?;
        let output_file = File::create(&temp_path)?;

        let reader = BufReader::with_capacity(64 * 1024, input_file);
        let writer = BufWriter::with_capacity(64 * 1024, output_file);

        let result = match self.stream_replace_regex_reader(reader, writer, pattern, replacement) {
            Ok(res) => res,
            Err(err) => {
                let _ = fs::remove_file(&temp_path);
                return Err(err);
            }
        };

        if let Err(e) = fs::rename(&temp_path, path_ref) {
            if let Err(copy_err) = fs::copy(&temp_path, path_ref) {
                let _ = fs::remove_file(&temp_path);
                return Err(PatchError::AtomicRename {
                    from: temp_path.display().to_string(),
                    to: path_ref.display().to_string(),
                    reason: format!("rename error: {}, copy fallback error: {}", e, copy_err),
                });
            }
            let _ = fs::remove_file(&temp_path);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_exact_block_single_replacement() {
        let patcher = AstPatcher::new();
        let input = "fn hello() {\n    println!(\"old\");\n}\n";
        let target = "    println!(\"old\");";
        let replacement = "    println!(\"new\");";

        let result = patcher.patch_string(input, target, replacement).unwrap();
        assert_eq!(result.occurrences, 1);
        assert_eq!(result.content, "fn hello() {\n    println!(\"new\");\n}\n");
    }

    #[test]
    fn test_exact_block_multiple_occurrences() {
        let patcher = AstPatcher::new();
        let input = "let x = 1;\nlet y = 1;\nlet z = 1;\n";
        let target = "1";
        let replacement = "42";

        let result = patcher.patch_string(input, target, replacement).unwrap();
        assert_eq!(result.occurrences, 3);
        assert_eq!(result.content, "let x = 42;\nlet y = 42;\nlet z = 42;\n");
    }

    #[test]
    fn test_whitespace_and_indentation_sensitivity() {
        let patcher = AstPatcher::new();
        let input = "    fn compute() {\n        return 10;\n    }\n";
        // Target with incorrect indentation must fail
        let target = "  fn compute() {\n      return 10;\n  }\n";
        let replacement = "    fn compute() {\n        return 20;\n    }\n";

        let err = patcher
            .patch_string(input, target, replacement)
            .unwrap_err();
        match err {
            PatchError::TargetNotFound { .. } => {}
            _ => panic!("Expected TargetNotFound, got {:?}", err),
        }
    }

    #[test]
    fn test_target_deletion_with_empty_replacement() {
        let patcher = AstPatcher::new();
        let input = "line1\nline_to_delete\nline3\n";
        let target = "line_to_delete\n";
        let replacement = "";

        let result = patcher.patch_string(input, target, replacement).unwrap();
        assert_eq!(result.occurrences, 1);
        assert_eq!(result.content, "line1\nline3\n");
    }

    #[test]
    fn test_empty_target_returns_error() {
        let patcher = AstPatcher::new();
        let input = "some code";
        let err = patcher.patch_string(input, "", "replacement").unwrap_err();
        match err {
            PatchError::EmptyTarget => {}
            _ => panic!("Expected EmptyTarget, got {:?}", err),
        }
    }

    #[test]
    fn test_lenient_newline_fallback() {
        let patcher = AstPatcher::new().with_lenient_newlines(true);
        // Input has CRLF, target has LF
        let input = "line 1\r\nline 2\r\nline 3\r\n";
        let target = "line 2\n";
        let replacement = "line 2 modified\n";

        let result = patcher.patch_string(input, target, replacement).unwrap();
        assert_eq!(result.occurrences, 1);
        assert!(result.content.contains("line 2 modified"));
    }

    #[test]
    fn test_patch_file_in_place() {
        let patcher = AstPatcher::new();
        let temp_dir = std::env::temp_dir().join(format!("agenda_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();
        let test_file = temp_dir.join("test_file.txt");

        fs::write(&test_file, "header\nold_value\nfooter\n").unwrap();

        let result = patcher
            .patch_file(&test_file, "old_value", "new_value")
            .unwrap();
        assert_eq!(result.occurrences, 1);

        let final_content = fs::read_to_string(&test_file).unwrap();
        assert_eq!(final_content, "header\nnew_value\nfooter\n");

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_streaming_regex_reader() {
        let patcher = AstPatcher::new();
        let raw_data = "INFO 2026-01-01 User logged in\nDEBUG 2026-01-01 Cache hit\nWARN 2026-01-01 Disk threshold\n";
        let reader = Cursor::new(raw_data);
        let mut writer = Cursor::new(Vec::new());

        let result = patcher
            .stream_replace_regex_reader(reader, &mut writer, r"\d{4}-\d{2}-\d{2}", "<DATE>")
            .unwrap();

        assert_eq!(result.lines_processed, 3);
        assert_eq!(result.lines_modified, 3);
        assert_eq!(result.total_replacements, 3);

        let output_str = String::from_utf8(writer.into_inner()).unwrap();
        assert_eq!(
            output_str,
            "INFO <DATE> User logged in\nDEBUG <DATE> Cache hit\nWARN <DATE> Disk threshold\n"
        );
    }

    #[test]
    fn test_streaming_regex_file_substitution() {
        let patcher = AstPatcher::new();
        let temp_dir =
            std::env::temp_dir().join(format!("agenda_stream_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();
        let test_file = temp_dir.join("large_log.txt");

        let mut sample_lines = String::new();
        for i in 1..=100 {
            sample_lines.push_str(&format!("entry_{}: count=0, status=PENDING\n", i));
        }
        fs::write(&test_file, sample_lines).unwrap();

        let result = patcher
            .stream_replace_regex_file(&test_file, r"status=PENDING", "status=DONE")
            .unwrap();

        assert_eq!(result.lines_processed, 100);
        assert_eq!(result.lines_modified, 100);
        assert_eq!(result.total_replacements, 100);

        let content = fs::read_to_string(&test_file).unwrap();
        assert!(content.contains("status=DONE"));
        assert!(!content.contains("status=PENDING"));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
