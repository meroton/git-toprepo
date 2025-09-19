/// Error indicating that the current directory is not a configured git-toprepo
/// TODO(terminology pr#172): This will be renamed when terminology is finalized
#[derive(thiserror::Error, Debug)]
#[error("Not a monorepo")]
pub struct NotAMonorepo {
    #[source]
    pub source: Option<anyhow::Error>,
}

impl NotAMonorepo {
    pub fn new(source: anyhow::Error) -> Self {
        Self { source: Some(source) }
    }
}

impl Default for NotAMonorepo {
    fn default() -> Self {
        Self { source: None }
    }
}

#[derive(thiserror::Error, Debug, PartialEq)]
#[error("Already a monorepo")]
pub struct AlreadyAMonorepo;