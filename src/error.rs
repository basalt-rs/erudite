//! Errors that may arise while using erudite
use std::path::PathBuf;

use thiserror::Error;
use tokio::task::JoinError;

use crate::runner::CompileResult;

/// An error that might occur while compiling the program
#[derive(Debug, Error)]
pub enum CompileError {
    /// Failed to spawn compile command
    #[error("Failed to spawn compile command: {:?}", .0)]
    SpawnFail(#[source] std::io::Error),
    /// Failed to wait on compile command
    #[error("Failed to wait on compile command: {:?}", .0)]
    WaitFail(#[source] std::io::Error),
    /// Invalid compile command specified
    #[error("Invalid compile command specified")]
    InvalidCommand,
    /// Failed to create necessary files
    #[error("Failed to create necessary files: {:?}", .0)]
    CreateFilesError(#[from] CreateFilesError),
    /// Failed to create compile/run directory
    #[error("Failed to create compile/run directory: {:?}", .0)]
    MktempFail(#[source] std::io::Error),
    /// Failed to compile solution (either timeout or nonzero exit code)
    #[error("Failed to compile solution: {:?}", .0)]
    CompileFail(CompileResult),
}

/// There was an error trying to create a file while running a test suite
#[derive(Debug, Error)]
#[error("Failed to create file at {}: {:?}", .path.display(), .error)]
pub struct CreateFilesError {
    /// The path to the file trying to be created
    pub path: PathBuf,
    #[source]
    pub(crate) error: std::io::Error,
}

/// There was an error spawning, writing to, or reading from a test
#[derive(Debug, Error)]
pub enum SpawnTestError {
    /// Failed to join thread
    #[error("Failed to join thread: {:?}", .0)]
    JoinError(JoinError),
    /// Invalid run command specified
    #[error("Invalid run command specified")]
    InvalidCommand,
    /// Failed to spawn run command
    #[error("Failed to spawn run command: {:?}", .0)]
    SpawnFail(#[source] std::io::Error),
    /// Failed to write to stdin of test program
    #[error("Failed to write to stdin of test program: {:?}", .0)]
    WriteStdinFail(#[source] std::io::Error),
    /// Failed to wait on run command
    #[error("Failed to wait on run command: {:?}", .0)]
    WaitFail(#[source] std::io::Error),
}
