use serde::{Deserialize, Serialize};

use crate::ast::{AstEngine, SupportedLanguage};

/// Error types detected during syntax validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum SyntaxError {
    #[error(
        "Unmatched delimiter: expected '{expected}', found '{found}' at line {line}, col {col}"
    )]
    UnmatchedDelimiter {
        expected: char,
        found: char,
        line: usize,
        col: usize,
    },

    #[error("Unclosed delimiter '{delimiter}' opened at line {line}, col {col}")]
    UnclosedDelimiter {
        delimiter: char,
        line: usize,
        col: usize,
    },

    #[error("Unclosed string literal starting at line {line}, col {col}")]
    UnclosedString { line: usize, col: usize },

    #[error("Unclosed multi-line comment starting at line {line}, col {col}")]
    UnclosedComment { line: usize, col: usize },

    #[error("Invalid declaration in {language:?} at line {line}: {message}")]
    InvalidDeclaration {
        language: SupportedLanguage,
        line: usize,
        message: String,
    },

    #[error("Indentation error in Python at line {line}: {message}")]
    IndentationError { line: usize, message: String },

    #[error("Empty content is not valid for syntax validation")]
    EmptyContent,

    #[error("General syntax sanity error: {message}")]
    SanityError { message: String },
}

/// Detailed report generated when syntax validation passes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxReport {
    /// True if content successfully passed syntax validation.
    pub is_valid: bool,
    /// Detected language based on filename or explicit parameter.
    pub language: Option<SupportedLanguage>,
    /// Total number of lines analyzed.
    pub lines_checked: usize,
    /// Total number of structural AST nodes detected.
    pub nodes_detected: usize,
    /// True if all brackets, braces, and parentheses are balanced.
    pub delimiters_balanced: bool,
    /// Non-fatal diagnostic warnings.
    pub warnings: Vec<String>,
}

/// Standalone syntax validation engine.
#[derive(Debug, Clone, Default)]
pub struct SyntaxValidator {
    strict_delimiters: bool,
    check_declarations: bool,
}

impl SyntaxValidator {
    /// Create a new SyntaxValidator with strict verification enabled.
    pub fn new() -> Self {
        Self {
            strict_delimiters: true,
            check_declarations: true,
        }
    }

    /// Set whether delimiter balance is strictly enforced.
    pub fn with_strict_delimiters(mut self, strict: bool) -> Self {
        self.strict_delimiters = strict;
        self
    }

    /// Set whether structural declarations are strictly verified.
    pub fn with_declaration_checks(mut self, check: bool) -> Self {
        self.check_declarations = check;
        self
    }

    /// Validates syntax of content for a specified filename and language.
    pub fn validate(&self, filename: &str, content: &str) -> Result<SyntaxReport, SyntaxError> {
        let lang = SupportedLanguage::from_path(filename);
        let lines_checked = content.lines().count();

        // 1. Verify delimiter balance across comments and strings
        if self.strict_delimiters {
            check_delimiters_and_literals(content, lang)?;
        }

        // 2. Language-specific structural declarations checks
        if self.check_declarations {
            if let Some(language) = lang {
                check_language_declarations(language, content)?;
            }
        }

        // 3. Count detected AST nodes if supported language
        let mut nodes_detected = 0;
        let mut warnings = Vec::new();
        if let Some(language) = lang {
            match AstEngine::parse(language, content) {
                Ok(nodes) => {
                    nodes_detected = nodes.len();
                }
                Err(e) => {
                    warnings.push(format!("AST structural scan warning: {}", e));
                }
            }
        }

        Ok(SyntaxReport {
            is_valid: true,
            language: lang,
            lines_checked,
            nodes_detected,
            delimiters_balanced: true,
            warnings,
        })
    }
}

/// Public convenience function to validate source code syntax.
pub fn validate_syntax(filename: &str, content: &str) -> Result<SyntaxReport, SyntaxError> {
    SyntaxValidator::new().validate(filename, content)
}

/// Checks that all parentheses, brackets, and braces are balanced,
/// correctly handling strings and comments for the target language.
fn check_delimiters_and_literals(
    content: &str,
    lang: Option<SupportedLanguage>,
) -> Result<(), SyntaxError> {
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut line = 1;
    let mut col = 1;

    let mut stack: Vec<(char, usize, usize)> = Vec::new();

    while i < len {
        let ch = chars[i];

        // Track line and column numbers
        if ch == '\n' {
            line += 1;
            col = 1;
            i += 1;
            continue;
        }

        // Check for Python comments (#)
        if lang == Some(SupportedLanguage::Python) && ch == '#' {
            while i < len && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }

        // Check for C-style single-line comments (//)
        if i + 1 < len && ch == '/' && chars[i + 1] == '/' {
            while i < len && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }

        // Check for C-style multi-line comments (/* ... */)
        if i + 1 < len && ch == '/' && chars[i + 1] == '*' {
            let start_line = line;
            let start_col = col;
            i += 2;
            col += 2;
            let mut closed = false;
            while i + 1 < len {
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                    i += 1;
                } else if chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                    col += 2;
                    closed = true;
                    break;
                } else {
                    i += 1;
                    col += 1;
                }
            }
            if !closed {
                return Err(SyntaxError::UnclosedComment {
                    line: start_line,
                    col: start_col,
                });
            }
            continue;
        }

        // Check for Python triple-quote strings (""" or ''')
        if lang == Some(SupportedLanguage::Python)
            && i + 2 < len
            && ((ch == '"' && chars[i + 1] == '"' && chars[i + 2] == '"')
                || (ch == '\'' && chars[i + 1] == '\'' && chars[i + 2] == '\''))
        {
            let quote = ch;
            let start_line = line;
            let start_col = col;
            i += 3;
            col += 3;
            let mut closed = false;
            while i + 2 < len {
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                    i += 1;
                } else if chars[i] == quote && chars[i + 1] == quote && chars[i + 2] == quote {
                    i += 3;
                    col += 3;
                    closed = true;
                    break;
                } else {
                    i += 1;
                    col += 1;
                }
            }
            if !closed {
                return Err(SyntaxError::UnclosedString {
                    line: start_line,
                    col: start_col,
                });
            }
            continue;
        }

        // Check for Rust raw strings r#"..."# or r##"..."##
        if lang == Some(SupportedLanguage::Rust) && ch == 'r' {
            let mut hash_count = 0;
            let mut cursor = i + 1;
            while cursor < len && chars[cursor] == '#' {
                hash_count += 1;
                cursor += 1;
            }
            if cursor < len && chars[cursor] == '"' {
                let start_line = line;
                let start_col = col;
                let quote_start = cursor + 1;
                let mut closed = false;
                let mut j = quote_start;
                line += chars[i..quote_start].iter().filter(|&&c| c == '\n').count();
                col += quote_start - i;

                while j < len {
                    if chars[j] == '\n' {
                        line += 1;
                        col = 1;
                        j += 1;
                    } else if chars[j] == '"' {
                        let mut end_hashes = 0;
                        while j + 1 + end_hashes < len && chars[j + 1 + end_hashes] == '#' {
                            end_hashes += 1;
                        }
                        if end_hashes >= hash_count {
                            j += 1 + hash_count;
                            col += 1 + hash_count;
                            closed = true;
                            i = j;
                            break;
                        } else {
                            j += 1;
                            col += 1;
                        }
                    } else {
                        j += 1;
                        col += 1;
                    }
                }
                if !closed {
                    return Err(SyntaxError::UnclosedString {
                        line: start_line,
                        col: start_col,
                    });
                }
                continue;
            }
        }

        // Check for Go raw strings (backtick `...`)
        if (lang == Some(SupportedLanguage::Go)
            || lang == Some(SupportedLanguage::JavaScript)
            || lang == Some(SupportedLanguage::TypeScript))
            && ch == '`'
        {
            let start_line = line;
            let start_col = col;
            i += 1;
            col += 1;
            let mut closed = false;
            while i < len {
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                    i += 1;
                } else if chars[i] == '`' {
                    i += 1;
                    col += 1;
                    closed = true;
                    break;
                } else {
                    i += 1;
                    col += 1;
                }
            }
            if !closed {
                return Err(SyntaxError::UnclosedString {
                    line: start_line,
                    col: start_col,
                });
            }
            continue;
        }

        // Check for normal double-quoted or single-quoted strings
        if ch == '"' || (ch == '\'' && lang != Some(SupportedLanguage::Rust)) {
            let quote = ch;
            let start_line = line;
            let start_col = col;
            i += 1;
            col += 1;
            let mut closed = false;
            let mut escaped = false;

            while i < len {
                let cur = chars[i];
                if cur == '\n' {
                    // Standard strings cannot span multiple lines unescaped
                    return Err(SyntaxError::UnclosedString {
                        line: start_line,
                        col: start_col,
                    });
                }

                if escaped {
                    escaped = false;
                } else if cur == '\\' {
                    escaped = true;
                } else if cur == quote {
                    closed = true;
                    i += 1;
                    col += 1;
                    break;
                }

                i += 1;
                col += 1;
            }

            if !closed {
                return Err(SyntaxError::UnclosedString {
                    line: start_line,
                    col: start_col,
                });
            }
            continue;
        }

        // Check for Rust char literals ('a', '\n', '\'')
        if lang == Some(SupportedLanguage::Rust) && ch == '\'' {
            // Distinguish char literal from lifetime ('a, 'static)
            if i + 1 < len && chars[i + 1] != '\\' && i + 2 < len && chars[i + 2] == '\'' {
                // Single char literal 'x'
                i += 3;
                col += 3;
                continue;
            } else if i + 1 < len && chars[i + 1] == '\\' && i + 3 < len && chars[i + 3] == '\'' {
                // Escaped char literal '\n'
                i += 4;
                col += 4;
                continue;
            }
            // Otherwise likely lifetime 'a or tick; advance 1
            i += 1;
            col += 1;
            continue;
        }

        // Process delimiter tokens
        match ch {
            '(' | '[' | '{' => {
                stack.push((ch, line, col));
            }
            ')' => match stack.pop() {
                Some(('(', _, _)) => {}
                Some((expected, _el, _ec)) => {
                    return Err(SyntaxError::UnmatchedDelimiter {
                        expected: matching_close(expected),
                        found: ')',
                        line,
                        col,
                    });
                }
                None => {
                    return Err(SyntaxError::UnmatchedDelimiter {
                        expected: '(',
                        found: ')',
                        line,
                        col,
                    });
                }
            },
            ']' => match stack.pop() {
                Some(('[', _, _)) => {}
                Some((expected, _el, _ec)) => {
                    return Err(SyntaxError::UnmatchedDelimiter {
                        expected: matching_close(expected),
                        found: ']',
                        line,
                        col,
                    });
                }
                None => {
                    return Err(SyntaxError::UnmatchedDelimiter {
                        expected: '[',
                        found: ']',
                        line,
                        col,
                    });
                }
            },
            '}' => match stack.pop() {
                Some(('{', _, _)) => {}
                Some((expected, _el, _ec)) => {
                    return Err(SyntaxError::UnmatchedDelimiter {
                        expected: matching_close(expected),
                        found: '}',
                        line,
                        col,
                    });
                }
                None => {
                    return Err(SyntaxError::UnmatchedDelimiter {
                        expected: '{',
                        found: '}',
                        line,
                        col,
                    });
                }
            },
            _ => {}
        }

        i += 1;
        col += 1;
    }

    if let Some((unclosed, l, c)) = stack.pop() {
        return Err(SyntaxError::UnclosedDelimiter {
            delimiter: unclosed,
            line: l,
            col: c,
        });
    }

    Ok(())
}

fn matching_close(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        other => other,
    }
}

/// Validates language-specific declaration syntax sanity.
fn check_language_declarations(lang: SupportedLanguage, content: &str) -> Result<(), SyntaxError> {
    match lang {
        SupportedLanguage::Rust => check_rust_declarations(content),
        SupportedLanguage::Python => check_python_declarations(content),
        SupportedLanguage::Go => check_go_declarations(content),
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript => {
            check_js_ts_declarations(content)
        }
    }
}

/// Verifies Rust structural declarations (e.g. fn, struct, enum, trait, mod, impl).
fn check_rust_declarations(content: &str) -> Result<(), SyntaxError> {
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        // Skip blank lines and pure comments
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            continue;
        }

        // Rust fn declaration checks:
        // Reject invalid patterns like "fn {", "fn (", "pub fn {", "async fn {"
        if let Some(rest) = strip_fn_prefix(trimmed) {
            let after_fn = rest.trim_start();
            if after_fn.is_empty() || after_fn.starts_with('{') || after_fn.starts_with(';') {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::Rust,
                    line: line_num,
                    message: format!(
                        "Invalid Rust function declaration: expected identifier after 'fn', found '{}'",
                        after_fn.chars().next().unwrap_or(' ')
                    ),
                });
            }
        }

        // Reject invalid keyword declarations without names
        for kw in &["struct", "enum", "trait", "mod", "impl"] {
            if let Some(rest) = strip_kw_prefix(trimmed, kw) {
                let after_kw = rest.trim_start();
                if after_kw.is_empty() || after_kw.starts_with('{') {
                    return Err(SyntaxError::InvalidDeclaration {
                        language: SupportedLanguage::Rust,
                        line: line_num,
                        message: format!(
                            "Invalid Rust declaration: expected identifier after '{}', found '{}'",
                            kw,
                            after_kw.chars().next().unwrap_or(' ')
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

fn strip_fn_prefix(line: &str) -> Option<&str> {
    let mut s = line;
    for prefix in &[
        "pub(crate)",
        "pub(super)",
        "pub",
        "async",
        "const",
        "unsafe",
        "extern \"C\"",
    ] {
        if let Some(stripped) = s.strip_prefix(prefix) {
            s = stripped.trim_start();
        }
    }
    if let Some(stripped) = s.strip_prefix("fn ") {
        Some(stripped)
    } else if let Some(stripped) = s.strip_prefix("fn{") {
        Some(stripped)
    } else if s == "fn" {
        Some("")
    } else {
        None
    }
}

fn strip_kw_prefix<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let mut s = line;
    for prefix in &["pub(crate)", "pub(super)", "pub"] {
        if let Some(stripped) = s.strip_prefix(prefix) {
            s = stripped.trim_start();
        }
    }
    if let Some(rest) = s.strip_prefix(kw) {
        if rest.starts_with(char::is_whitespace) || rest.starts_with('{') || rest.is_empty() {
            return Some(rest);
        }
    }
    None
}

/// Verifies Python indentation consistency and declaration sanity (def, class).
fn check_python_declarations(content: &str) -> Result<(), SyntaxError> {
    let mut indent_stack: Vec<usize> = vec![0];
    let mut expect_indent = false;

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Calculate leading spaces indentation (tabs = 4 spaces)
        let indent = line
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum();

        let current_indent = *indent_stack.last().unwrap_or(&0);

        if expect_indent {
            if indent <= current_indent {
                return Err(SyntaxError::IndentationError {
                    line: line_num,
                    message: format!(
                        "Expected an indented block after statement ending in ':', found indent {}",
                        indent
                    ),
                });
            }
            indent_stack.push(indent);
            expect_indent = false;
        } else if indent > current_indent {
            return Err(SyntaxError::IndentationError {
                line: line_num,
                message: format!(
                    "Unexpected indent at line {}: indent {} > current level {}",
                    line_num, indent, current_indent
                ),
            });
        } else if indent < current_indent {
            // Must match one of the outer indentation levels
            while let Some(&top) = indent_stack.last() {
                if top == indent {
                    break;
                } else if top < indent {
                    return Err(SyntaxError::IndentationError {
                        line: line_num,
                        message: format!(
                            "Unindent at line {} does not match any outer indentation level (indent {})",
                            line_num, indent
                        ),
                    });
                }
                indent_stack.pop();
            }
        }

        // Check for def or class declarations
        if let Some(stripped) = trimmed.strip_prefix("async def ") {
            let def_name = stripped.split('(').next().unwrap_or("").trim();
            if def_name.is_empty() || def_name.starts_with(':') {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::Python,
                    line: line_num,
                    message: "Expected function name after 'def'".to_string(),
                });
            }
        } else if let Some(stripped) = trimmed.strip_prefix("def ") {
            let def_name = stripped.split('(').next().unwrap_or("").trim();
            if def_name.is_empty() || def_name.starts_with(':') {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::Python,
                    line: line_num,
                    message: "Expected function name after 'def'".to_string(),
                });
            }
        } else if let Some(after_class) = trimmed.strip_prefix("class ") {
            let class_name = after_class.split([':', '(']).next().unwrap_or("").trim();
            if class_name.is_empty() {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::Python,
                    line: line_num,
                    message: "Expected class name after 'class'".to_string(),
                });
            }
        }

        // Flag next line for indentation if current line ends with ':'
        if trimmed.ends_with(':') {
            expect_indent = true;
        }
    }

    Ok(())
}

/// Verifies Go declarations (package, func, type struct, type interface).
fn check_go_declarations(content: &str) -> Result<(), SyntaxError> {
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        if let Some(stripped) = trimmed.strip_prefix("func") {
            let after_func = stripped.trim_start();
            // In Go, func { is invalid (even anonymous func needs parameter list: func() {)
            if after_func.starts_with('{') || after_func.is_empty() {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::Go,
                    line: line_num,
                    message: "Invalid Go function declaration: expected identifier or signature after 'func'".to_string(),
                });
            }
        } else if let Some(stripped) = trimmed.strip_prefix("package") {
            let after_pkg = stripped.trim();
            if after_pkg.is_empty() || after_pkg.starts_with(';') {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::Go,
                    line: line_num,
                    message: "Invalid Go declaration: expected package name after 'package'"
                        .to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Verifies JavaScript / TypeScript declarations.
fn check_js_ts_declarations(content: &str) -> Result<(), SyntaxError> {
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        // Check for function {
        if let Some(stripped) = trimmed.strip_prefix("function") {
            let after_fn = stripped.trim_start();
            if after_fn.starts_with('{') || after_fn.is_empty() {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::JavaScript,
                    line: line_num,
                    message: "Invalid JavaScript function declaration: expected parameter list or identifier after 'function'".to_string(),
                });
            }
        } else if let Some(stripped) = trimmed.strip_prefix("class") {
            let after_class = stripped.trim_start();
            if after_class.starts_with('{') || after_class.is_empty() {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::JavaScript,
                    line: line_num,
                    message:
                        "Invalid JavaScript class declaration: expected class name after 'class'"
                            .to_string(),
                });
            }
        } else if let Some(stripped) = trimmed.strip_prefix("interface") {
            let after_iface = stripped.trim_start();
            if after_iface.starts_with('{') || after_iface.is_empty() {
                return Err(SyntaxError::InvalidDeclaration {
                    language: SupportedLanguage::TypeScript,
                    line: line_num,
                    message: "Invalid TypeScript interface declaration: expected interface name after 'interface'".to_string(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_rust_syntax() {
        let code = r#"
            pub struct Engine {
                power: u32,
            }

            impl Engine {
                pub fn new() -> Self {
                    Self { power: 100 }
                }
            }
        "#;
        let report = validate_syntax("engine.rs", code).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.language, Some(SupportedLanguage::Rust));
        assert!(report.nodes_detected >= 2);
    }

    #[test]
    fn test_invalid_rust_fn_brace() {
        let code = "fn {\n    println!(\"bad\");\n}\n";
        let err = validate_syntax("bad.rs", code).unwrap_err();
        match err {
            SyntaxError::InvalidDeclaration {
                language, message, ..
            } => {
                assert_eq!(language, SupportedLanguage::Rust);
                assert!(message.contains("expected identifier after 'fn'"));
            }
            _ => panic!("Expected InvalidDeclaration error, got {:?}", err),
        }
    }

    #[test]
    fn test_unbalanced_delimiters() {
        let code = "fn test() { let x = (1 + 2]; }";
        let err = validate_syntax("test.rs", code).unwrap_err();
        match err {
            SyntaxError::UnmatchedDelimiter {
                expected, found, ..
            } => {
                assert_eq!(expected, ')');
                assert_eq!(found, ']');
            }
            _ => panic!("Expected UnmatchedDelimiter error, got {:?}", err),
        }
    }

    #[test]
    fn test_unclosed_delimiter() {
        let code = "fn test() { let x = 1;";
        let err = validate_syntax("test.rs", code).unwrap_err();
        match err {
            SyntaxError::UnclosedDelimiter { delimiter, .. } => {
                assert_eq!(delimiter, '{');
            }
            _ => panic!("Expected UnclosedDelimiter error, got {:?}", err),
        }
    }

    #[test]
    fn test_delimiters_in_comments_and_strings_ignored() {
        let code = "
            // Here are braces { [ ( that should be ignored
            /* Block comment with braces { } [ ] ( ) */
            let s = \"string with unmatched { [ ( braces\";
            let raw = \"raw with unmatched } ] ) braces\";
        ";
        let report = validate_syntax("safe.rs", code).unwrap();
        assert!(report.is_valid);
    }

    #[test]
    fn test_python_valid_syntax() {
        let code = r#"
def calculate(a, b):
    # calculate sum
    return a + b

class Math:
    def multiply(self, x, y):
        """Docstring with { braces }"""
        return x * y
"#;
        let report = validate_syntax("math.py", code).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.language, Some(SupportedLanguage::Python));
        assert!(report.nodes_detected >= 3);
    }

    #[test]
    fn test_python_invalid_indentation() {
        let code = "def foo():\n    x = 1\n  y = 2\n";
        let err = validate_syntax("bad.py", code).unwrap_err();
        match err {
            SyntaxError::IndentationError { .. } => {}
            _ => panic!("Expected IndentationError, got {:?}", err),
        }
    }

    #[test]
    fn test_go_valid_syntax() {
        let code = r#"
package main

type Server struct {
    port int
}

func (s *Server) Start() error {
    return nil
}
"#;
        let report = validate_syntax("server.go", code).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.language, Some(SupportedLanguage::Go));
        assert!(report.nodes_detected >= 3);
    }

    #[test]
    fn test_go_invalid_func_declaration() {
        let code = "package main\n\nfunc {\n}\n";
        let err = validate_syntax("bad.go", code).unwrap_err();
        match err {
            SyntaxError::InvalidDeclaration { language, .. } => {
                assert_eq!(language, SupportedLanguage::Go);
            }
            _ => panic!("Expected InvalidDeclaration, got {:?}", err),
        }
    }

    #[test]
    fn test_typescript_valid_syntax() {
        let code = r#"
export interface User {
    id: string;
}

export class UserService {
    getUser(): User {
        return { id: "1" };
    }
}
"#;
        let report = validate_syntax("service.ts", code).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.language, Some(SupportedLanguage::TypeScript));
        assert!(report.nodes_detected >= 2);
    }
}
