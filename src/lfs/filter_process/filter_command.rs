use crate::git::GitPath;

/// Parsed command header from the long running Git filter process.
pub(super) enum FilterCommand {
    Clean { pathname: GitPath },
    Smudge { pathname: GitPath, can_delay: bool },
    ListAvailableBlobs,
    EndOfFile,
}
