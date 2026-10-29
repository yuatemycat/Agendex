//! # Agendex Harness
//!
//! Execution core and execution environment for the Agendex agent framework:
//! - **Persistent Terminals (`terminal`)**: ConPTY (Windows) and PTY (Unix) supervisor using `portable-pty`.
//! - **Background Tasks (`tasks`)**: Synchronous command runner with automatic background daemon transitions,
//!   ring-buffered stdout/stderr logs, task status tracking, process tree kill, and stdin pipes.
//! - **Native Tools (`tools`)**: Sandboxed filesystem operations (`read_file` with line/byte bounds,
//!   `write_file` with overwrite/append controls, `list_directory` with recursive depth limits)
//!   and integrated command runner.

pub mod tasks;
pub mod terminal;
pub mod tools;

// Re-exports for clean ergonomics across Agendex crates
pub use tasks::{
    ManageTaskAction, RunCommandResult, Task, TaskInfo, TaskManager, TaskRingBuffer, TaskStatus,
    TaskStatusInfo,
};
pub use terminal::{
    TerminalConfig, TerminalManager, TerminalOutputBuffer, TerminalSession, TerminalSessionInfo,
};
pub use tools::{
    DirEntryInfo, FindDefinitionArgs, FindDefinitionResult, ListDirArgs, ListDirResult,
    ManageTaskArgs, ReadFileArgs, ReadFileResult, RunCommandArgs, ToolRegistry, WriteFileArgs,
    WriteFileResult, DEFAULT_MAX_READ_BYTES, DEFAULT_MAX_READ_LINES,
};
