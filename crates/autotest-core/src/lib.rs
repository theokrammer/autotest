pub mod cache;
pub mod ir;
pub mod merger;
pub mod mock;
pub mod project_index;
pub mod scaffolder;
pub mod solver;

pub use autotest_proto as proto;
pub use cache::{AstHashes, DependencyGraph};
pub use ir::*;
pub use merger::{MergeMode, MergeReport, ScenarioMerger, StaleAssertionDrift};
pub use mock::FixtureDeduplicator;
pub use project_index::{sample_from_regex, ClassIndexEntry, ProjectIndex};
pub use scaffolder::EntrypointScaffolder;
pub use solver::{SolverConfig, SymbolicSolver};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Validation error: {0}")]
    Validation(String),
}
