use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::ast::{AstEngine, AstError, AstNode, NodeKind, SupportedLanguage};

/// A located definition matching a symbol lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionMatch {
    /// Exact identifier name of the symbol.
    pub symbol: String,
    /// AST category (Function, Struct, Class, Enum, Interface, Trait, TypeAlias).
    pub kind: NodeKind,
    /// Absolute or relative file path where definition was found.
    pub path: PathBuf,
    /// 1-indexed line number where the declaration begins.
    pub line: usize,
    /// Header signature or declaration preview snippet.
    pub preview: String,
    /// Optional parent container (enclosing class, struct, module, or receiver).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
}

/// Checks whether an AST node matches the requested symbol.
/// Supports both simple identifiers (e.g. `connect`) and scoped symbols (e.g. `Database::connect` or `Server.start`).
fn matches_symbol(node: &AstNode, symbol: &str) -> bool {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return false;
    }

    if let Some((parent, child)) = symbol.rsplit_once("::") {
        return node.name == child && node.parent_path.as_deref() == Some(parent);
    }

    if let Some((parent, child)) = symbol.rsplit_once('.') {
        return node.name == child && node.parent_path.as_deref() == Some(parent);
    }

    node.name == symbol
}

/// Converts an AstNode into a DefinitionMatch for a specific file path.
fn node_to_definition_match(node: &AstNode, file_path: &Path) -> DefinitionMatch {
    let preview = node
        .signature
        .clone()
        .unwrap_or_else(|| node.body.lines().next().unwrap_or("").trim().to_string());

    DefinitionMatch {
        symbol: node.name.clone(),
        kind: node.kind,
        path: file_path.to_path_buf(),
        line: node.span.start_line,
        preview,
        parent_path: node.parent_path.clone(),
    }
}

/// Locates AST definitions for a symbol within raw source content and filename.
pub fn find_definitions_in_content(
    symbol: &str,
    filename: &str,
    content: &str,
) -> Result<Vec<DefinitionMatch>, AstError> {
    let nodes = AstEngine::parse_file(filename, content)?;
    let p = Path::new(filename);
    let mut matches = Vec::new();
    for node in &nodes {
        if matches_symbol(node, symbol) {
            matches.push(node_to_definition_match(node, p));
        }
    }
    Ok(matches)
}

/// Locates AST definitions (function, struct, class, enum, interface, trait, type)
/// for a symbol across workspace files or a specific file/directory path.
pub fn find_definition(
    symbol: &str,
    path: Option<&Path>,
) -> Result<Vec<DefinitionMatch>, AstError> {
    let symbol = symbol.trim();
    if symbol.is_empty() {
        return Ok(Vec::new());
    }

    let target_path = path.unwrap_or_else(|| Path::new("."));

    // Case 1: Target path points to a single file
    if target_path.is_file() {
        if SupportedLanguage::from_path(target_path).is_none() {
            return Ok(Vec::new());
        }
        let content = match std::fs::read_to_string(target_path) {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        let filename = target_path.to_string_lossy();
        return find_definitions_in_content(symbol, &filename, &content);
    }

    // Case 2: Target path points to a directory — search recursively
    let mut matches = Vec::new();
    let walker = ignore::WalkBuilder::new(target_path)
        .standard_filters(true)
        .hidden(true)
        .filter_entry(|entry| {
            let file_name = entry.file_name().to_string_lossy();
            // Skip common build, cache, and dependency folders
            if entry.file_type().is_some_and(|ft| ft.is_dir())
                && (file_name == "target"
                    || file_name == "node_modules"
                    || file_name == ".git"
                    || file_name == "vendor"
                    || file_name == ".venv"
                    || file_name == "dist"
                    || file_name == "build"
                    || file_name == "__pycache__")
            {
                return false;
            }
            true
        })
        .build();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let file_path = entry.path();
        if SupportedLanguage::from_path(file_path).is_none() {
            continue;
        }

        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let filename = file_path.to_string_lossy();
        if let Ok(nodes) = AstEngine::parse_file(&filename, &content) {
            for node in &nodes {
                if matches_symbol(node, symbol) {
                    matches.push(node_to_definition_match(node, file_path));
                }
            }
        }
    }

    // Sort matches deterministically by file path and then line number
    matches.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_definition_rust() {
        let rs_code = r#"
pub struct EngineConfig {
    pub max_tokens: usize,
}

pub trait EngineRunner {
    fn run(&self) -> bool;
}

pub enum EngineState {
    Idle,
    Running,
}

pub type TokenCount = usize;

pub fn start_engine(config: EngineConfig) -> bool {
    true
}
"#;
        let defs = find_definitions_in_content("EngineConfig", "engine.rs", rs_code).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, NodeKind::Struct);
        assert_eq!(defs[0].symbol, "EngineConfig");
        assert_eq!(defs[0].line, 2);

        let trait_defs = find_definitions_in_content("EngineRunner", "engine.rs", rs_code).unwrap();
        assert_eq!(trait_defs.len(), 1);
        assert_eq!(trait_defs[0].kind, NodeKind::Trait);

        let enum_defs = find_definitions_in_content("EngineState", "engine.rs", rs_code).unwrap();
        assert_eq!(enum_defs.len(), 1);
        assert_eq!(enum_defs[0].kind, NodeKind::Enum);

        let type_defs = find_definitions_in_content("TokenCount", "engine.rs", rs_code).unwrap();
        assert_eq!(type_defs.len(), 1);
        assert_eq!(type_defs[0].kind, NodeKind::TypeAlias);

        let fn_defs = find_definitions_in_content("start_engine", "engine.rs", rs_code).unwrap();
        assert_eq!(fn_defs.len(), 1);
        assert_eq!(fn_defs[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_find_definition_python() {
        let py_code = r#"
class DatabaseManager:
    def __init__(self, uri: str):
        self.uri = uri

    def connect(self):
        return True

def standalone_helper():
    return 123
"#;
        let class_defs = find_definitions_in_content("DatabaseManager", "db.py", py_code).unwrap();
        assert_eq!(class_defs.len(), 1);
        assert_eq!(class_defs[0].kind, NodeKind::Class);

        let method_defs = find_definitions_in_content("connect", "db.py", py_code).unwrap();
        assert_eq!(method_defs.len(), 1);
        assert_eq!(method_defs[0].kind, NodeKind::Method);
        assert_eq!(
            method_defs[0].parent_path.as_deref(),
            Some("DatabaseManager")
        );

        let scoped_defs =
            find_definitions_in_content("DatabaseManager.connect", "db.py", py_code).unwrap();
        assert_eq!(scoped_defs.len(), 1);
        assert_eq!(scoped_defs[0].symbol, "connect");

        let fn_defs = find_definitions_in_content("standalone_helper", "db.py", py_code).unwrap();
        assert_eq!(fn_defs.len(), 1);
        assert_eq!(fn_defs[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_find_definition_typescript_and_javascript() {
        let ts_code = r#"
export interface UserSession {
    id: string;
}

export enum SessionStatus {
    Active,
    Expired,
}

export type SessionId = string;

export class SessionRegistry {
    validate(id: string): boolean {
        return true;
    }
}

export function createSession(): UserSession {
    return { id: "123" };
}
"#;
        let iface = find_definitions_in_content("UserSession", "session.ts", ts_code).unwrap();
        assert_eq!(iface.len(), 1);
        assert_eq!(iface[0].kind, NodeKind::Interface);

        let enm = find_definitions_in_content("SessionStatus", "session.ts", ts_code).unwrap();
        assert_eq!(enm.len(), 1);
        assert_eq!(enm[0].kind, NodeKind::Enum);

        let typ = find_definitions_in_content("SessionId", "session.ts", ts_code).unwrap();
        assert_eq!(typ.len(), 1);
        assert_eq!(typ[0].kind, NodeKind::TypeAlias);

        let cls = find_definitions_in_content("SessionRegistry", "session.ts", ts_code).unwrap();
        assert_eq!(cls.len(), 1);
        assert_eq!(cls[0].kind, NodeKind::Class);

        let mth =
            find_definitions_in_content("SessionRegistry.validate", "session.ts", ts_code).unwrap();
        assert_eq!(mth.len(), 1);
        assert_eq!(mth[0].kind, NodeKind::Method);

        let fn_match = find_definitions_in_content("createSession", "session.ts", ts_code).unwrap();
        assert_eq!(fn_match.len(), 1);
        assert_eq!(fn_match[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_find_definition_go() {
        let go_code = r#"
package cluster

type NodeInfo struct {
    IP string
}

type NodeCluster interface {
    Leader() string
}

type ClusterID string

func (n *NodeInfo) Ping() bool {
    return true
}

func Bootstrap() *NodeInfo {
    return &NodeInfo{IP: "127.0.0.1"}
}
"#;
        let strct = find_definitions_in_content("NodeInfo", "cluster.go", go_code).unwrap();
        assert_eq!(strct.len(), 1);
        assert_eq!(strct[0].kind, NodeKind::Struct);

        let iface = find_definitions_in_content("NodeCluster", "cluster.go", go_code).unwrap();
        assert_eq!(iface.len(), 1);
        assert_eq!(iface[0].kind, NodeKind::Interface);

        let typ = find_definitions_in_content("ClusterID", "cluster.go", go_code).unwrap();
        assert_eq!(typ.len(), 1);
        assert_eq!(typ[0].kind, NodeKind::TypeAlias);

        let mth = find_definitions_in_content("Ping", "cluster.go", go_code).unwrap();
        assert_eq!(mth.len(), 1);
        assert_eq!(mth[0].kind, NodeKind::Method);
        assert_eq!(mth[0].parent_path.as_deref(), Some("NodeInfo"));

        let fnc = find_definitions_in_content("Bootstrap", "cluster.go", go_code).unwrap();
        assert_eq!(fnc.len(), 1);
        assert_eq!(fnc[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_find_definition_directory_search() {
        let tmp = std::env::temp_dir().join(format!(
            "agendex_def_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(tmp.join("src")).unwrap();

        std::fs::write(
            tmp.join("src").join("main.rs"),
            "pub struct GlobalApp;\npub fn run_app() {}\n",
        )
        .unwrap();

        std::fs::write(
            tmp.join("src").join("worker.py"),
            "class BackgroundWorker:\n    def execute(self):\n        pass\n",
        )
        .unwrap();

        std::fs::write(
            tmp.join("src").join("handler.go"),
            "package main\n\ntype HandlerFunc func()\nfunc Serve() {}\n",
        )
        .unwrap();

        // 1. Find Rust struct
        let app_defs = find_definition("GlobalApp", Some(&tmp)).unwrap();
        assert_eq!(app_defs.len(), 1);
        assert_eq!(app_defs[0].kind, NodeKind::Struct);
        assert_eq!(app_defs[0].line, 1);

        // 2. Find Python class
        let py_defs = find_definition("BackgroundWorker", Some(&tmp)).unwrap();
        assert_eq!(py_defs.len(), 1);
        assert_eq!(py_defs[0].kind, NodeKind::Class);

        // 3. Find Go function
        let go_defs = find_definition("Serve", Some(&tmp)).unwrap();
        assert_eq!(go_defs.len(), 1);
        assert_eq!(go_defs[0].kind, NodeKind::Function);

        // 4. Nonexistent symbol
        let none_defs = find_definition("NonExistentSymbol", Some(&tmp)).unwrap();
        assert!(none_defs.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
