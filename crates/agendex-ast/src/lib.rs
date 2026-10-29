pub mod ast;
pub mod definition;
pub mod diff;
pub mod patcher;
pub mod validator;

pub use ast::{AstEngine, AstError, AstNode, AstQuery, NodeKind, SourceSpan, SupportedLanguage};
pub use definition::{find_definition, find_definitions_in_content, DefinitionMatch};
pub use diff::{generate_unified_diff, myers_diff, DiffOp};
pub use patcher::{AstPatcher, PatchError, PatchResult, StreamPatchResult};
pub use validator::{validate_syntax, SyntaxError, SyntaxReport, SyntaxValidator};
