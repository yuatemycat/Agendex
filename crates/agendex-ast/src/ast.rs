use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::validator::SyntaxError;

/// Languages supported for AST structural analysis and syntax validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SupportedLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
}

impl SupportedLanguage {
    /// Detect supported language from filename or file path.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Option<Self> {
        let ext = path
            .as_ref()
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "rs" => Some(SupportedLanguage::Rust),
            "py" | "pyw" => Some(SupportedLanguage::Python),
            "js" | "jsx" | "mjs" | "cjs" => Some(SupportedLanguage::JavaScript),
            "ts" | "tsx" | "mts" | "cts" => Some(SupportedLanguage::TypeScript),
            "go" => Some(SupportedLanguage::Go),
            _ => None,
        }
    }
}

/// Structural categories of AST nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Module,
    Enum,
    Trait,
    TypeAlias,
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeKind::Function => write!(f, "function"),
            NodeKind::Method => write!(f, "method"),
            NodeKind::Class => write!(f, "class"),
            NodeKind::Struct => write!(f, "struct"),
            NodeKind::Interface => write!(f, "interface"),
            NodeKind::Module => write!(f, "module"),
            NodeKind::Enum => write!(f, "enum"),
            NodeKind::Trait => write!(f, "trait"),
            NodeKind::TypeAlias => write!(f, "type_alias"),
        }
    }
}

/// Precise source code coordinates with 1-based lines and columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_offset: usize,
    pub end_offset: usize,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// Structural representation of a high-level code element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstNode {
    /// Category of this node (function, class, struct, etc.).
    pub kind: NodeKind,
    /// Identifier name.
    pub name: String,
    /// Language dialect of the source.
    pub language: SupportedLanguage,
    /// Precise source location and span boundaries.
    pub span: SourceSpan,
    /// Optional parent container path (module, enclosing class or struct).
    pub parent_path: Option<String>,
    /// Header signature or declaration preview.
    pub signature: Option<String>,
    /// Exact verbatim source code slice of this node.
    pub body: String,
}

/// Query filter for locating structural AST nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstQuery {
    pub kind: Option<NodeKind>,
    pub name: Option<String>,
    pub parent_path: Option<String>,
    pub language: Option<SupportedLanguage>,
}

impl AstQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_kind(mut self, kind: NodeKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_parent_path(mut self, parent: impl Into<String>) -> Self {
        self.parent_path = Some(parent.into());
        self
    }

    pub fn with_language(mut self, lang: SupportedLanguage) -> Self {
        self.language = Some(lang);
        self
    }

    pub fn matches(&self, node: &AstNode) -> bool {
        if let Some(ref k) = self.kind {
            if &node.kind != k {
                return false;
            }
        }
        if let Some(ref n) = self.name {
            if &node.name != n {
                return false;
            }
        }
        if let Some(ref p) = self.parent_path {
            match &node.parent_path {
                Some(np) => {
                    if np != p
                        && !np.ends_with(&format!("::{}", p))
                        && !np.starts_with(&format!("{}::", p))
                    {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if let Some(ref l) = self.language {
            if &node.language != l {
                return false;
            }
        }
        true
    }
}

/// Errors occurring during AST parsing, querying, or node replacement.
#[derive(Debug, thiserror::Error)]
pub enum AstError {
    #[error("Unsupported language for file: {0}")]
    UnsupportedLanguage(String),

    #[error("Node not found: name='{name}', kind={kind}")]
    NodeNotFound { name: String, kind: NodeKind },

    #[error("Ambiguous node match: found {count} candidates for name='{name}' (specify parent_path to disambiguate)")]
    AmbiguousNode { name: String, count: usize },

    #[error("Parse error in {language:?}: {message}")]
    ParseError {
        language: SupportedLanguage,
        message: String,
    },

    #[error("Validation error: {0}")]
    Validation(#[from] SyntaxError),

    #[error("Patch error: {0}")]
    Patch(#[from] crate::patcher::PatchError),
}

/// Line index for fast, binary-search offset to line/column translation.
pub struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(content: &str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(content.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { line_starts }
    }

    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line_idx = self.line_starts.partition_point(|&start| start <= offset);
        let line = if line_idx == 0 { 1 } else { line_idx };
        let start = self.line_starts[line - 1];
        let col = offset.saturating_sub(start) + 1;
        (line, col)
    }

    pub fn span(&self, start_offset: usize, end_offset: usize) -> SourceSpan {
        let (start_line, start_col) = self.line_col(start_offset);
        let (end_line, end_col) = self.line_col(end_offset);
        SourceSpan {
            start_offset,
            end_offset,
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }
}

/// High-level AST analysis and node query engine.
pub struct AstEngine;

impl AstEngine {
    /// Parse structural AST nodes for the given language.
    pub fn parse(language: SupportedLanguage, content: &str) -> Result<Vec<AstNode>, AstError> {
        let index = LineIndex::new(content);
        match language {
            SupportedLanguage::Rust => parse_rust(content, &index),
            SupportedLanguage::Python => parse_python(content, &index),
            SupportedLanguage::JavaScript | SupportedLanguage::TypeScript => {
                parse_js_ts(content, language, &index)
            }
            SupportedLanguage::Go => parse_go(content, &index),
        }
    }

    /// Infer language from filename and parse structural AST nodes.
    pub fn parse_file(filename: &str, content: &str) -> Result<Vec<AstNode>, AstError> {
        let lang = SupportedLanguage::from_path(filename)
            .ok_or_else(|| AstError::UnsupportedLanguage(filename.to_string()))?;
        Self::parse(lang, content)
    }

    /// Query nodes matching an AstQuery filter.
    pub fn query<'a>(nodes: &'a [AstNode], query: &AstQuery) -> Vec<&'a AstNode> {
        nodes.iter().filter(|n| query.matches(n)).collect()
    }

    /// Filter nodes by identifier name.
    pub fn find_by_name<'a>(nodes: &'a [AstNode], name: &str) -> Vec<&'a AstNode> {
        nodes.iter().filter(|n| n.name == name).collect()
    }

    /// Filter nodes by kind.
    pub fn find_by_kind(nodes: &[AstNode], kind: NodeKind) -> Vec<&AstNode> {
        nodes.iter().filter(|n| n.kind == kind).collect()
    }

    /// Replaces the verbatim source code of a specific AstNode.
    pub fn replace_node(
        content: &str,
        node: &AstNode,
        replacement: &str,
    ) -> Result<String, AstError> {
        if node.span.start_offset > content.len() || node.span.end_offset > content.len() {
            return Err(AstError::ParseError {
                language: node.language,
                message: "Node span boundaries exceed content length".to_string(),
            });
        }
        let mut result = String::with_capacity(content.len() + replacement.len());
        result.push_str(&content[..node.span.start_offset]);
        result.push_str(replacement);
        result.push_str(&content[node.span.end_offset..]);
        Ok(result)
    }

    /// Edits a specific node identified by language, kind, name, and optional parent_path.
    /// Disambiguates candidate nodes and validates post-replacement syntax if requested.
    pub fn edit_node(
        content: &str,
        language: SupportedLanguage,
        kind: NodeKind,
        name: &str,
        parent_path: Option<&str>,
        replacement: &str,
        validate: bool,
    ) -> Result<String, AstError> {
        let nodes = Self::parse(language, content)?;

        let matched: Vec<&AstNode> = nodes
            .iter()
            .filter(|n| n.kind == kind && n.name == name)
            .filter(|n| match parent_path {
                Some(p) => match &n.parent_path {
                    Some(np) => {
                        np == p
                            || np.ends_with(&format!("::{}", p))
                            || np.starts_with(&format!("{}::", p))
                    }
                    None => false,
                },
                None => true,
            })
            .collect();

        if matched.is_empty() {
            return Err(AstError::NodeNotFound {
                name: name.to_string(),
                kind,
            });
        }

        if matched.len() > 1 {
            return Err(AstError::AmbiguousNode {
                name: name.to_string(),
                count: matched.len(),
            });
        }

        let target_node = matched[0];
        let new_content = Self::replace_node(content, target_node, replacement)?;

        if validate {
            let dummy_filename = match language {
                SupportedLanguage::Rust => "code.rs",
                SupportedLanguage::Python => "code.py",
                SupportedLanguage::JavaScript => "code.js",
                SupportedLanguage::TypeScript => "code.ts",
                SupportedLanguage::Go => "code.go",
            };
            crate::validator::validate_syntax(dummy_filename, &new_content)?;
        }

        Ok(new_content)
    }
}

// ---------------------------------------------------------------------------
// Language-Specific AST Parsers
// ---------------------------------------------------------------------------

/// Finds the byte offset of the matching closing brace '}' for an opening brace '{' at open_offset.
fn find_matching_brace(
    content: &str,
    open_offset: usize,
    lang: SupportedLanguage,
) -> Option<usize> {
    let bytes = content.as_bytes();
    let len = bytes.len();
    if open_offset >= len || bytes[open_offset] != b'{' {
        return None;
    }

    let mut depth = 1;
    let mut i = open_offset + 1;

    while i < len {
        let b = bytes[i];

        // Skip single-line comments
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Python comments
        if lang == SupportedLanguage::Python && b == b'#' {
            i += 1;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Multi-line comments
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // String literals
        if b == b'"' || (b == b'\'' && lang != SupportedLanguage::Rust) {
            let quote = b;
            i += 1;
            let mut escaped = false;
            while i < len {
                let cur = bytes[i];
                if escaped {
                    escaped = false;
                } else if cur == b'\\' {
                    escaped = true;
                } else if cur == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Raw backtick strings
        if (lang == SupportedLanguage::Go
            || lang == SupportedLanguage::JavaScript
            || lang == SupportedLanguage::TypeScript)
            && b == b'`'
        {
            i += 1;
            while i < len && bytes[i] != b'`' {
                i += 1;
            }
            if i < len {
                i += 1;
            }
            continue;
        }

        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }

    None
}

/// Parses Rust declarations (functions, methods, structs, enums, traits, modules, type aliases).
fn parse_rust(content: &str, index: &LineIndex) -> Result<Vec<AstNode>, AstError> {
    let mut nodes = Vec::new();
    let re_decl = regex::Regex::new(
        r"(?m)^(?P<indent>[ \t]*)(?:pub(?:\([^)]+\))?[ \t]+)?(?:async[ \t]+)?(?:const[ \t]+)?(?:unsafe[ \t]+)?(?P<kw>fn|struct|enum|trait|mod|impl|type)[ \t]+(?P<ident>[a-zA-Z_][a-zA-Z0-9_]*)?"
    ).map_err(|e| AstError::ParseError {
        language: SupportedLanguage::Rust,
        message: e.to_string(),
    })?;

    // Stack tracking container (module or impl struct)
    // Container: (kind, name, end_offset)
    let mut container_stack: Vec<(NodeKind, String, usize)> = Vec::new();

    for cap in re_decl.captures_iter(content) {
        let full_match = cap.get(0).unwrap();
        let match_start = full_match.start();
        let kw = cap.name("kw").map(|m| m.as_str()).unwrap_or("");
        let ident_opt = cap.name("ident").map(|m| m.as_str());

        // Prune container stack whose scope has ended before match_start
        container_stack.retain(|(_, _, end)| *end > match_start);

        // Derive active parent path
        let active_parent = if container_stack.is_empty() {
            None
        } else {
            let names: Vec<&str> = container_stack
                .iter()
                .map(|(_, name, _)| name.as_str())
                .collect();
            Some(names.join("::"))
        };

        // Find opening brace or semicolon after match_start
        let rest = &content[match_start..];
        let brace_rel = rest.find('{');
        let semi_rel = rest.find(';');

        let (open_offset, is_brace) = match (brace_rel, semi_rel) {
            (Some(b), Some(s)) if b < s => (match_start + b, true),
            (Some(_), Some(s)) => (match_start + s, false),
            (Some(b), None) => (match_start + b, true),
            (None, Some(s)) => (match_start + s, false),
            (None, None) => continue,
        };

        let end_offset = if is_brace {
            match find_matching_brace(content, open_offset, SupportedLanguage::Rust) {
                Some(close) => close + 1,
                None => continue,
            }
        } else {
            open_offset + 1
        };

        let span = index.span(match_start, end_offset);
        let body = content[match_start..end_offset].to_string();
        let header_end = if is_brace { open_offset } else { end_offset };
        let signature = Some(content[match_start..header_end].trim().to_string());

        match kw {
            "fn" => {
                let name = match ident_opt {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let is_method = container_stack
                    .iter()
                    .any(|(k, _, _)| *k == NodeKind::Struct || *k == NodeKind::Trait);
                let kind = if is_method {
                    NodeKind::Method
                } else {
                    NodeKind::Function
                };

                nodes.push(AstNode {
                    kind,
                    name,
                    language: SupportedLanguage::Rust,
                    span,
                    parent_path: active_parent,
                    signature,
                    body,
                });
            }
            "struct" => {
                let name = match ident_opt {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                nodes.push(AstNode {
                    kind: NodeKind::Struct,
                    name: name.clone(),
                    language: SupportedLanguage::Rust,
                    span,
                    parent_path: active_parent,
                    signature,
                    body,
                });
            }
            "enum" => {
                let name = match ident_opt {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                nodes.push(AstNode {
                    kind: NodeKind::Enum,
                    name: name.clone(),
                    language: SupportedLanguage::Rust,
                    span,
                    parent_path: active_parent,
                    signature,
                    body,
                });
            }
            "trait" => {
                let name = match ident_opt {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                nodes.push(AstNode {
                    kind: NodeKind::Trait,
                    name: name.clone(),
                    language: SupportedLanguage::Rust,
                    span,
                    parent_path: active_parent,
                    signature,
                    body,
                });
                if is_brace {
                    container_stack.push((NodeKind::Trait, name, end_offset));
                }
            }
            "mod" => {
                let name = match ident_opt {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                nodes.push(AstNode {
                    kind: NodeKind::Module,
                    name: name.clone(),
                    language: SupportedLanguage::Rust,
                    span,
                    parent_path: active_parent,
                    signature,
                    body,
                });
                if is_brace {
                    container_stack.push((NodeKind::Module, name, end_offset));
                }
            }
            "impl" => {
                // Parse impl [Trait for] TargetStruct
                let header = content[match_start..header_end].trim();
                let impl_target = if let Some(for_idx) = header.find(" for ") {
                    header[for_idx + 5..]
                        .split(|c: char| c.is_whitespace() || c == '{' || c == '<')
                        .next()
                        .unwrap_or("")
                        .trim()
                } else {
                    let after_impl = header.strip_prefix("impl").unwrap_or("").trim();
                    after_impl
                        .split(|c: char| c.is_whitespace() || c == '{' || c == '<')
                        .next()
                        .unwrap_or("")
                        .trim()
                };

                if !impl_target.is_empty() && is_brace {
                    container_stack.push((NodeKind::Struct, impl_target.to_string(), end_offset));
                }
            }
            "type" => {
                let name = match ident_opt {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                nodes.push(AstNode {
                    kind: NodeKind::TypeAlias,
                    name,
                    language: SupportedLanguage::Rust,
                    span,
                    parent_path: active_parent,
                    signature,
                    body,
                });
            }
            _ => {}
        }
    }

    Ok(nodes)
}

/// Parses Python declarations (functions, methods, classes) with indentation scope tracking.
fn parse_python(content: &str, index: &LineIndex) -> Result<Vec<AstNode>, AstError> {
    let mut nodes = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let num_lines = lines.len();

    // Stack: (class_name, class_indent, start_offset)
    let mut class_stack: Vec<(String, usize, usize)> = Vec::new();

    let mut line_byte_offsets = Vec::with_capacity(num_lines);
    let mut cur_offset = 0;
    for l in &lines {
        line_byte_offsets.push(cur_offset);
        cur_offset += l.len() + 1; // + 1 for '\n'
    }

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let indent = line
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum();

        // Prune class stack when indentation returns to or below class level
        while let Some((_, c_indent, _)) = class_stack.last() {
            if indent <= *c_indent {
                class_stack.pop();
            } else {
                break;
            }
        }

        let is_class = trimmed.starts_with("class ");
        let is_def = trimmed.starts_with("def ") || trimmed.starts_with("async def ");

        if !is_class && !is_def {
            continue;
        }

        // Determine node start, taking into account possible leading decorators (@)
        let mut node_start_line = idx;
        while node_start_line > 0 {
            let prev = lines[node_start_line - 1].trim();
            if prev.starts_with('@') {
                node_start_line -= 1;
            } else {
                break;
            }
        }
        let start_offset = line_byte_offsets[node_start_line];

        // Find end of the indented body
        let mut end_line_idx = idx + 1;
        while end_line_idx < num_lines {
            let next_line = lines[end_line_idx];
            let next_trimmed = next_line.trim();

            if next_trimmed.is_empty() || next_trimmed.starts_with('#') {
                end_line_idx += 1;
                continue;
            }

            let next_indent: usize = next_line
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(|c| if c == '\t' { 4 } else { 1 })
                .sum();

            if next_indent <= indent {
                break;
            }
            end_line_idx += 1;
        }

        // Calculate end offset
        let end_offset = if end_line_idx < num_lines {
            line_byte_offsets[end_line_idx]
        } else {
            content.len()
        };

        let span = index.span(start_offset, end_offset);
        let body = content[start_offset..end_offset].to_string();

        if is_class {
            let after_class = &trimmed[6..];
            let class_name = after_class
                .split(|c: char| c == '(' || c == ':' || c.is_whitespace())
                .next()
                .unwrap_or("")
                .trim()
                .to_string();

            if !class_name.is_empty() {
                let parent = class_stack.last().map(|(n, _, _)| n.clone());
                nodes.push(AstNode {
                    kind: NodeKind::Class,
                    name: class_name.clone(),
                    language: SupportedLanguage::Python,
                    span,
                    parent_path: parent,
                    signature: Some(trimmed.to_string()),
                    body,
                });
                class_stack.push((class_name, indent, start_offset));
            }
        } else if is_def {
            let after_def = trimmed
                .strip_prefix("async def ")
                .or_else(|| trimmed.strip_prefix("def "))
                .unwrap_or("");
            let func_name = after_def.split('(').next().unwrap_or("").trim().to_string();

            if !func_name.is_empty() {
                let (kind, parent) = if let Some((c_name, _, _)) = class_stack.last() {
                    (NodeKind::Method, Some(c_name.clone()))
                } else {
                    (NodeKind::Function, None)
                };

                nodes.push(AstNode {
                    kind,
                    name: func_name,
                    language: SupportedLanguage::Python,
                    span,
                    parent_path: parent,
                    signature: Some(trimmed.to_string()),
                    body,
                });
            }
        }
    }

    Ok(nodes)
}

/// Parses JavaScript and TypeScript declarations.
fn parse_js_ts(
    content: &str,
    language: SupportedLanguage,
    index: &LineIndex,
) -> Result<Vec<AstNode>, AstError> {
    let mut nodes = Vec::new();
    let re_decl = regex::Regex::new(
        r"(?m)^(?P<indent>[ \t]*)(?:export[ \t]+)?(?:default[ \t]+)?(?:async[ \t]+)?(?P<kw>function|class|interface|type)[ \t]+(?P<ident>[a-zA-Z_$][a-zA-Z0-9_$]*)"
    ).map_err(|e| AstError::ParseError {
        language,
        message: e.to_string(),
    })?;

    let re_method = regex::Regex::new(
        r"(?m)^[ \t]*(?:public[ \t]+|private[ \t]+|protected[ \t]+)?(?:static[ \t]+)?(?:async[ \t]+)?(?P<mname>[a-zA-Z_$][a-zA-Z0-9_$]*)[ \t]*\([^)]*\)[ \t]*\{"
    ).map_err(|e| AstError::ParseError {
        language,
        message: e.to_string(),
    })?;

    for cap in re_decl.captures_iter(content) {
        let full_match = cap.get(0).unwrap();
        let match_start = full_match.start();
        let kw = cap.name("kw").map(|m| m.as_str()).unwrap_or("");
        let name = cap
            .name("ident")
            .map(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        let rest = &content[match_start..];
        let brace_rel = rest.find('{');
        let semi_rel = rest.find(';');

        let (open_offset, is_brace) = match (brace_rel, semi_rel) {
            (Some(b), Some(s)) if b < s => (match_start + b, true),
            (Some(_), Some(s)) => (match_start + s, false),
            (Some(b), None) => (match_start + b, true),
            (None, Some(s)) => (match_start + s, false),
            (None, None) => continue,
        };

        let end_offset = if is_brace {
            match find_matching_brace(content, open_offset, language) {
                Some(close) => close + 1,
                None => continue,
            }
        } else {
            open_offset + 1
        };

        let span = index.span(match_start, end_offset);
        let body = content[match_start..end_offset].to_string();
        let header_end = if is_brace { open_offset } else { end_offset };
        let signature = Some(content[match_start..header_end].trim().to_string());

        let kind = match kw {
            "function" => NodeKind::Function,
            "class" => NodeKind::Class,
            "interface" => NodeKind::Interface,
            "type" => NodeKind::TypeAlias,
            _ => continue,
        };

        nodes.push(AstNode {
            kind,
            name: name.clone(),
            language,
            span,
            parent_path: None,
            signature,
            body: body.clone(),
        });

        // Scan methods inside classes
        if kind == NodeKind::Class && is_brace {
            let class_body = &content[open_offset + 1..end_offset - 1];

            for mcap in re_method.captures_iter(class_body) {
                let m_match = mcap.get(0).unwrap();
                let m_name = mcap.name("mname").map(|m| m.as_str()).unwrap_or("");
                if m_name == "if" || m_name == "for" || m_name == "while" || m_name == "switch" {
                    continue;
                }
                let m_start = open_offset + 1 + m_match.start();
                let m_brace = open_offset + 1 + m_match.end() - 1;

                if let Some(m_close) = find_matching_brace(content, m_brace, language) {
                    let m_end = m_close + 1;
                    let m_span = index.span(m_start, m_end);
                    let m_body = content[m_start..m_end].to_string();
                    let m_sig = Some(content[m_start..m_brace].trim().to_string());

                    nodes.push(AstNode {
                        kind: NodeKind::Method,
                        name: m_name.to_string(),
                        language,
                        span: m_span,
                        parent_path: Some(name.clone()),
                        signature: m_sig,
                        body: m_body,
                    });
                }
            }
        }
    }

    Ok(nodes)
}

/// Parses Go declarations (package/module, functions, methods, structs, interfaces).
fn parse_go(content: &str, index: &LineIndex) -> Result<Vec<AstNode>, AstError> {
    let mut nodes = Vec::new();

    // 1. Package declaration (module boundary)
    let re_pkg = regex::Regex::new(r"(?m)^package[ \t]+(?P<name>[a-zA-Z0-9_]+)").unwrap();
    if let Some(cap) = re_pkg.captures(content) {
        let m = cap.get(0).unwrap();
        let name = cap.name("name").unwrap().as_str();
        nodes.push(AstNode {
            kind: NodeKind::Module,
            name: name.to_string(),
            language: SupportedLanguage::Go,
            span: index.span(m.start(), m.end()),
            parent_path: None,
            signature: Some(m.as_str().trim().to_string()),
            body: m.as_str().to_string(),
        });
    }

    // 2. Struct and Interface types
    let re_type = regex::Regex::new(
        r"(?m)^type[ \t]+(?P<name>[a-zA-Z0-9_]+)[ \t]+(?P<kind>struct|interface)[ \t]*\{",
    )
    .unwrap();
    for cap in re_type.captures_iter(content) {
        let full = cap.get(0).unwrap();
        let start = full.start();
        let name = cap.name("name").unwrap().as_str();
        let kind_str = cap.name("kind").unwrap().as_str();
        let brace_offset = full.end() - 1;

        if let Some(close) = find_matching_brace(content, brace_offset, SupportedLanguage::Go) {
            let end = close + 1;
            let kind = if kind_str == "struct" {
                NodeKind::Struct
            } else {
                NodeKind::Interface
            };
            nodes.push(AstNode {
                kind,
                name: name.to_string(),
                language: SupportedLanguage::Go,
                span: index.span(start, end),
                parent_path: None,
                signature: Some(content[start..brace_offset].trim().to_string()),
                body: content[start..end].to_string(),
            });
        }
    }

    // 3. Functions and Methods
    let re_func = regex::Regex::new(
        r"(?m)^func[ \t]+(?:\((?P<recv>[^)]+)\)[ \t]+)?(?P<name>[a-zA-Z0-9_]+)[ \t]*\([^)]*\)[ \t]*[^{]*\{"
    ).unwrap();
    for cap in re_func.captures_iter(content) {
        let full = cap.get(0).unwrap();
        let start = full.start();
        let name = cap.name("name").unwrap().as_str();
        let recv_opt = cap.name("recv").map(|m| m.as_str().trim());
        let brace_offset = full.end() - 1;

        if let Some(close) = find_matching_brace(content, brace_offset, SupportedLanguage::Go) {
            let end = close + 1;
            let (kind, parent_path) = match recv_opt {
                Some(recv) => {
                    // Extract receiver type from e.g. "s *Server" or "s Server"
                    let parts: Vec<&str> = recv.split_whitespace().collect();
                    let recv_type = parts.last().unwrap_or(&"").trim_start_matches('*').trim();
                    (NodeKind::Method, Some(recv_type.to_string()))
                }
                None => (NodeKind::Function, None),
            };

            nodes.push(AstNode {
                kind,
                name: name.to_string(),
                language: SupportedLanguage::Go,
                span: index.span(start, end),
                parent_path,
                signature: Some(content[start..brace_offset].trim().to_string()),
                body: content[start..end].to_string(),
            });
        }
    }

    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_parsing_and_queries() {
        let code = r#"
mod storage {
    pub struct Database {
        name: String,
    }

    impl Database {
        pub fn connect() -> Self {
            Self { name: "test".into() }
        }
    }

    pub fn init() {
        println!("init");
    }
}
"#;
        let nodes = AstEngine::parse(SupportedLanguage::Rust, code).unwrap();
        assert!(nodes.len() >= 4);

        // Find module
        let modules = AstEngine::find_by_kind(&nodes, NodeKind::Module);
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "storage");

        // Find struct
        let structs = AstEngine::find_by_name(&nodes, "Database");
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].kind, NodeKind::Struct);
        assert_eq!(structs[0].parent_path.as_deref(), Some("storage"));

        // Find method
        let methods = AstEngine::find_by_name(&nodes, "connect");
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].kind, NodeKind::Method);
        assert_eq!(methods[0].parent_path.as_deref(), Some("storage::Database"));

        // Find function
        let funcs = AstEngine::find_by_name(&nodes, "init");
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].kind, NodeKind::Function);
        assert_eq!(funcs[0].parent_path.as_deref(), Some("storage"));

        // Test AstQuery filter
        let query = AstQuery::new()
            .with_kind(NodeKind::Method)
            .with_parent_path("Database");
        let results = AstEngine::query(&nodes, &query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "connect");
    }

    #[test]
    fn test_python_parsing_and_scopes() {
        let code = r#"
@register
def top_level(x):
    return x * 2

class Agent:
    def __init__(self, name):
        self.name = name

    def execute(self):
        return True
"#;
        let nodes = AstEngine::parse(SupportedLanguage::Python, code).unwrap();
        assert!(nodes.len() >= 4);

        let top_func = AstEngine::find_by_name(&nodes, "top_level");
        assert_eq!(top_func.len(), 1);
        assert_eq!(top_func[0].kind, NodeKind::Function);
        assert!(top_func[0].body.contains("@register"));

        let classes = AstEngine::find_by_kind(&nodes, NodeKind::Class);
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Agent");

        let methods = AstEngine::find_by_kind(&nodes, NodeKind::Method);
        assert_eq!(methods.len(), 2);
        assert!(methods
            .iter()
            .all(|m| m.parent_path.as_deref() == Some("Agent")));
    }

    #[test]
    fn test_javascript_typescript_parsing() {
        let code = r#"
export interface Config {
    port: number;
}

export class Server {
    start() {
        console.log("running");
    }
}

export function helper() {
    return 42;
}
"#;
        let nodes = AstEngine::parse(SupportedLanguage::TypeScript, code).unwrap();
        assert!(nodes.len() >= 4);

        let ifaces = AstEngine::find_by_kind(&nodes, NodeKind::Interface);
        assert_eq!(ifaces.len(), 1);
        assert_eq!(ifaces[0].name, "Config");

        let classes = AstEngine::find_by_kind(&nodes, NodeKind::Class);
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Server");

        let methods = AstEngine::find_by_kind(&nodes, NodeKind::Method);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "start");
        assert_eq!(methods[0].parent_path.as_deref(), Some("Server"));

        let funcs = AstEngine::find_by_kind(&nodes, NodeKind::Function);
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "helper");
    }

    #[test]
    fn test_go_parsing() {
        let code = r#"
package service

type Worker struct {
    id int
}

type Runner interface {
    Run() error
}

func (w *Worker) Process() error {
    return nil
}

func NewWorker(id int) *Worker {
    return &Worker{id: id}
}
"#;
        let nodes = AstEngine::parse(SupportedLanguage::Go, code).unwrap();
        assert!(nodes.len() >= 5);

        let pkg = AstEngine::find_by_kind(&nodes, NodeKind::Module);
        assert_eq!(pkg.len(), 1);
        assert_eq!(pkg[0].name, "service");

        let structs = AstEngine::find_by_kind(&nodes, NodeKind::Struct);
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Worker");

        let ifaces = AstEngine::find_by_kind(&nodes, NodeKind::Interface);
        assert_eq!(ifaces.len(), 1);
        assert_eq!(ifaces[0].name, "Runner");

        let methods = AstEngine::find_by_kind(&nodes, NodeKind::Method);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "Process");
        assert_eq!(methods[0].parent_path.as_deref(), Some("Worker"));

        let funcs = AstEngine::find_by_kind(&nodes, NodeKind::Function);
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "NewWorker");
    }

    #[test]
    fn test_replace_and_edit_node() {
        let code = r#"
mod a {
    pub fn run() {
        println!("a");
    }
}

mod b {
    pub fn run() {
        println!("b");
    }
}
"#;
        // Ambiguous without parent path
        let err = AstEngine::edit_node(
            code,
            SupportedLanguage::Rust,
            NodeKind::Function,
            "run",
            None,
            "pub fn run() { println!(\"new\"); }",
            true,
        )
        .unwrap_err();

        match err {
            AstError::AmbiguousNode { count, .. } => assert_eq!(count, 2),
            _ => panic!("Expected AmbiguousNode, got {:?}", err),
        }

        // Disambiguated with parent_path "a"
        let new_code = AstEngine::edit_node(
            code,
            SupportedLanguage::Rust,
            NodeKind::Function,
            "run",
            Some("a"),
            "pub fn run() {\n        println!(\"patched_a\");\n    }",
            true,
        )
        .unwrap();

        assert!(new_code.contains("patched_a"));
        assert!(new_code.contains("println!(\"b\");"));
    }
}
