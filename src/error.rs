use std::path::PathBuf;

use thiserror::Error;
use tokio::task::JoinError;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("Failed to spawn compile command: {:?}", .0)]
    SpawnFail(#[source] std::io::Error),
    #[error("Failed to wait on compile command: {:?}", .0)]
    WaitFail(#[source] std::io::Error),
    #[error("Invalid compile command specified")]
    InvalidCommand,
    #[error("failed to create necessary files: {:?}", .0)]
    CreateFilesError(#[from] CreateFilesError),
    #[error("Failed to create compile/run directory: {:?}", .0)]
    MktempFail(#[source] std::io::Error),
}

#[derive(Debug, Error)]
#[error("Failed to create file at {}: {:?}", .path.display(), .error)]
pub struct CreateFilesError {
    pub path: PathBuf,
    #[source]
    pub error: std::io::Error,
}

#[derive(Debug, Error)]
pub enum SpawnTestError {
    #[error("Failed to join thread: {:?}", .0)]
    JoinError(JoinError),
    #[error("Invalid run command specified")]
    InvalidCommand,
    #[error("Failed to spawn run command: {:?}", .0)]
    SpawnFail(#[source] std::io::Error),
    #[error("Failed to write to stdin of test program: {:?}", .0)]
    WriteStdinFail(#[source] std::io::Error),
    #[error("Failed to wait on run command: {:?}", .0)]
    WaitFail(#[source] std::io::Error),
}

#[derive(Debug, Error)]
pub enum CompileAndSpawnError {
    #[error("failed to compile compile: {:?}", .0)]
    CompileError(#[from] CompileError),
    #[error("failed to spawn tests: {:?}", .0)]
    SpawnTestError(#[from] SpawnTestError),
}
