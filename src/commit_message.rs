/// This module combines commit messages, from multiple submodule commit
/// messages into a single mono commit message, and splitting a mono commit
/// message apart again. The extra footer `Git-Toprepo-Ref: PATH [SHA1]`
/// identifies the source for each message. This is also used when different
/// messages for a single mono commit is wanted for the submodules involved.
///
/// # Commit message combination algorithm
/// After the steps described below, the commit messages are concatenated with a
/// single empty line as separator. The reverse operation of splitting is not
/// perfect, i.e. multiple empty lines or no trailing newline will not be
/// reproduced after splitting the mon commit messages.
///
/// 1. Automatic commit messages like: "Update submodules" are ignored.
/// 2. The footer `Git-Toprepo-Ref: PATH [SHA1]` is added to each commit
///    message.
/// 3. Commit message with the same subject and body are deduplicated, the
///    footers are concatenated.
/// 4. Commit messages without footers have their footers placed first.
/// 5. Footers are deduplicated by putting `Git-Toprepo-Ref` lines immediately
///    after each other.
/// 6. When splitting, the footers after the last `Git-Toprepo-Ref` line are
///    applied to all the commit messages. The typical use case is that Gerrit's
///    `Change-Id` has been added and should apply to all the commits.
use crate::git::CommitId;
use crate::git::GitPath;
use crate::repo::ExpandedOrRemovedSubmodule;
use crate::repo::ExpandedSubmodule;
use crate::util::IterSingleUnique as _;
use anyhow::Result;
use anyhow::anyhow;
use bstr::BStr;
use bstr::ByteSlice;
use gix::prelude::ObjectIdExt as _;
use itertools::Itertools as _;
use std::collections::HashMap;

/// The rewritten commit messages gets this additinal footer in the form
/// `Git-Toprepo-Ref: path commit-id`.
///
/// This footer is useful for users to find which the original commit ids were
/// by simply using `git-log`.
///
/// The footer is also used by `git-toprepo` to split a commit message into
/// multiple commit messages for different submodules, when pushing a
/// cherry-picked mono commit into multiple repositories.
const GIT_TOPREPO_FOOTER_PREFIX: &str = "Git-Toprepo-Ref:";

/// Instead of an empty path, use this in the commit message footer.
const TOPREPO_DISPLAY_PATH: &str = "<top>";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PushMessage {
    /// The commit message for the commit to push to a remote.
    pub subject_and_body: String,
    /// Footer for the commit message.
    pub footer: String,
    /// Any potential topic for the commit to belong to.
    pub topic: Option<String>,
}

impl PushMessage {
    /// Concatenate subject, body and footer.
    pub fn full_message(&self) -> String {
        let subject_and_body = &self.subject_and_body;
        if self.footer.is_empty() {
            subject_and_body.to_owned()
        } else {
            // Make sure there is at least one empty line before the footer.
            let subject_and_body = subject_and_body.strip_suffix('\n').unwrap_or(subject_and_body);
            let subject_and_body = subject_and_body.strip_suffix('\n').unwrap_or(subject_and_body);
            format!("{subject_and_body}\n\n{}", self.footer)
        }
    }
}

pub fn calculate_mono_commit_message_from_commits(
    repo: &gix::Repository,
    source_path: &GitPath,
    source_commit_id: &CommitId,
    source_commit: &gix::objs::CommitRef<'_>,
    submod_updates: &HashMap<GitPath, ExpandedOrRemovedSubmodule>,
) -> String {
    let sub_commit_infos = submod_updates
        .iter()
        .filter_map(|(path, submod)| {
            if path == source_path {
                return None;
            }
            let _scope_guard_path = crate::log::scope(format!("Path {path}"));
            let (submod_message, status) = match submod {
                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::Expanded(submod)) => {
                    let submod_message =
                        match get_and_decode_commit_message(repo, submod.orig_commit_id) {
                            Ok(decoded_message) => Some(CommitMessage::from_full(decoded_message)),
                            Err(err) => {
                                log::warn!(
                                    "Failed to get commit message {} at {path}: {err:#}",
                                    submod.orig_commit_id
                                );
                                None
                            }
                        };
                    (submod_message, submod.orig_commit_id.to_string())
                }
                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::KeptAsSubmodule(
                    submod_commit_id,
                )) => {
                    // Normal submodule bumps are not recorded, the subrepo has not
                    // even been resolved so cannot find the commit message.
                    // TODO: 2025-09-22 Should we try to load the commit message from the subrepo if available?
                    (None, format!("{submod_commit_id} (submodule)"))
                }
                ExpandedOrRemovedSubmodule::Expanded(
                    ExpandedSubmodule::CommitMissingInSubRepo(submod),
                ) => {
                    // Normal submodule bumps are not recorded and the commit message cannot be resolved anyway.
                    (None, format!("{} not found", submod.orig_commit_id))
                }
                ExpandedOrRemovedSubmodule::Expanded(ExpandedSubmodule::UnknownSubmodule(
                    submod_commit_id,
                )) => {
                    // Normal submodule bumps are not recorded. With an unknown subrepo, the commit message cannot be resolved anyway.
                    (None, format!("{submod_commit_id} unknown submodule"))
                }
                ExpandedOrRemovedSubmodule::Expanded(
                    ExpandedSubmodule::RegressedNotFullyImplemented(submod),
                ) => {
                    // A short commit id is enough as a subject line, the full id is in the footer.
                    let short_submod_commit_id = submod.orig_commit_id.attach(repo).shorten_or_id();
                    (
                        Some(CommitMessage {
                            subject_and_body: format!(
                                "Regressed (not fully implemented) to {short_submod_commit_id}\n",
                            ),
                            footer: String::new(),
                        }),
                        format!("{} regressed", submod.orig_commit_id),
                    )
                }
                ExpandedOrRemovedSubmodule::Removed => {
                    // The submodule has been removed, so no commit message.
                    (None, "removed".to_owned())
                }
            };
            Some(CommitMessageInfo {
                path: path.clone(),
                message: submod_message,
                status,
            })
        })
        .collect_vec();
    // Add the source repo commit message.
    let source_info = {
        let _scope_guard_path = crate::log::scope(format!("Path {source_path}/",));
        CommitMessageInfo {
            path: source_path.clone(),
            message: Some(CommitMessage::from_full(decode_commit_message(
                source_commit,
            ))),
            status: source_commit_id.to_string(),
        }
    };
    calculate_mono_commit_message(source_info, sub_commit_infos)
}

#[derive(Debug, Clone, PartialEq)]
struct CommitMessage {
    pub subject_and_body: String,
    pub footer: String,
}

impl CommitMessage {
    fn from_full(full_message: String) -> Self {
        let (subject_and_body, footer) = full_message.trim_end().split("\n\n").fold(
            (String::new(), None),
            |(mut msg_but_last, prev_chunk), chunk| {
                if msg_but_last.is_empty() {
                    // Subject paragraph.
                    (format!("{chunk}\n"), None)
                } else {
                    // prev_chunk was not the footer, append to the body.
                    if let Some(prev_chunk) = prev_chunk {
                        msg_but_last.push('\n');
                        msg_but_last.push_str(prev_chunk);
                        msg_but_last.push('\n');
                    }
                    if chunk.lines().all(|line| is_footer_line(line.into())) {
                        // chunk is a potential footer.
                        (msg_but_last, Some(chunk))
                    } else {
                        // chunk is not a footer.
                        msg_but_last.push('\n');
                        msg_but_last.push_str(chunk);
                        msg_but_last.push('\n');
                        (msg_but_last, None)
                    }
                }
            },
        );
        Self {
            subject_and_body,
            footer: footer.map_or(String::new(), |footer_str| {
                debug_assert!(!footer_str.ends_with('\n'));
                format!("{footer_str}\n")
            }),
        }
    }
}

#[derive(Debug)]
pub struct CommitMessageInfo {
    path: GitPath,
    message: Option<CommitMessage>,
    status: String,
}

#[derive(Debug)]
pub struct CommitMessageInfoWithMessage {
    path: GitPath,
    message: CommitMessage,
    status: String,
}

impl CommitMessageInfoWithMessage {
    fn format_toprepo_footer(&self) -> String {
        let path_if_empty = if self.path.is_empty() {
            TOPREPO_DISPLAY_PATH
        } else {
            ""
        };
        format!(
            "{GIT_TOPREPO_FOOTER_PREFIX} {}{path_if_empty} {}\n",
            self.path, self.status
        )
    }
}

/// Construct a commit message from the submodule updates.
///
/// If the toprepo commit message starts with `Update git submodules`, the
/// message is assumed to be generated by Gerrit and optionally include the
/// commit messages of all updated submodules, for example:
/// ```text
/// Update git submodules
///
/// * Update subx from branch 'main'
///   to abc123
///   - New algo
///
///     Change-Id: I0123456789abcdef0123456789abcdef01234567
///
///   - Parent commit to new algo
///
///     Change-Id: I89abcdef0123456789abcdef0123456789abcdef0
/// ```
///
/// First of all, the parent commit messages are not wanted. Secondly, when
/// cherry-picking the commit and pushing it to a different branch, the
/// submodule commit ids will be wrong and we don't want to push the whole
/// message to the toprepo change anyway. Therefore, compose a message from the
/// interesting messages only.
fn calculate_mono_commit_message(
    source_info: CommitMessageInfo,
    sub_infos: Vec<CommitMessageInfo>,
) -> String {
    let mut interesting_messages = Vec::with_capacity(sub_infos.len() + 1);
    let mut boring_messages = Vec::with_capacity(sub_infos.len() + 1);
    for info in sub_infos {
        if info
            .message
            .as_ref()
            .is_some_and(|msg| is_interesting_message(&msg.subject_and_body))
        {
            interesting_messages.push(CommitMessageInfoWithMessage {
                path: info.path,
                message: info.message.unwrap(),
                status: info.status,
            });
        } else {
            boring_messages.push(info);
        }
    }
    // Add the source repo commit message.
    if let Some(source_message) = &source_info.message
        && (is_interesting_message(&source_message.subject_and_body)
            || interesting_messages.is_empty())
    {
        // Even if the message is boring, if there are no submodule messages,
        // use the source repo message anyway.
        interesting_messages.push(CommitMessageInfoWithMessage {
            path: source_info.path,
            message: source_info.message.unwrap(),
            status: source_info.status,
        });
    } else if interesting_messages.is_empty() {
        // No interesting messages among the submodules. Use a default boring
        // message.
        interesting_messages.push(CommitMessageInfoWithMessage {
            path: source_info.path,
            message: CommitMessage {
                subject_and_body: "Update git submodules\n".to_owned(),
                footer: String::new(),
            },
            status: source_info.status,
        });
    } else {
        boring_messages.push(source_info);
    }

    // In case of just one message, put footers for the boring messages there as well.
    let boring_subject_and_body = interesting_messages
        .iter()
        .map(|info| &info.message.subject_and_body)
        .single_unique()
        .cloned()
        .unwrap_or_default();
    let mut all_messages = interesting_messages;
    for boring_info in boring_messages {
        all_messages.push(CommitMessageInfoWithMessage {
            path: boring_info.path,
            message: CommitMessage {
                subject_and_body: boring_subject_and_body.clone(),
                footer: boring_info
                    .message
                    .map_or_else(|| String::new(), |msg| msg.footer),
            },
            status: boring_info.status,
        })
    }

    let mut all_combined_messages = Vec::new();

    // Group messages by subject_and_body, then by footer.
    all_messages.sort_by(|a, b| {
        let a_key = (&a.message.subject_and_body, &a.message.footer);
        let b_key = (&b.message.subject_and_body, &b.message.footer);
        a_key.cmp(&b_key)
    });
    // Go through each subject and body chunk.
    for text_chunk in
        all_messages.chunk_by_mut(|a, b| a.message.subject_and_body == b.message.subject_and_body)
    {
        let mut one_combined_message = text_chunk.first().unwrap().message.subject_and_body.clone();
        let empty_subject_and_body = one_combined_message.is_empty();
        if !empty_subject_and_body {
            debug_assert!(one_combined_message.ends_with('\n'));
            one_combined_message.push('\n');
        }

        let mut footer_chunks = Vec::new();
        for footer_chunk in text_chunk.chunk_by_mut(|a, b| a.message.footer == b.message.footer) {
            footer_chunk.sort_by(|a, b| a.path.cmp(&b.path));
            let first_info = footer_chunk.first().unwrap();
            if first_info.message.footer.is_empty() {
                // Paths for empty footers are always placed first.
                for info in footer_chunk {
                    one_combined_message.push_str(&info.format_toprepo_footer());
                }
            } else {
                footer_chunks.push(footer_chunk);
            }
        }
        // Sort by "smallest" path.
        footer_chunks.sort_by(|a_chunk, b_chunk| {
            let a_key = &a_chunk.first().unwrap().path;
            let b_key = &b_chunk.first().unwrap().path;
            a_key.cmp(b_key)
        });
        for footer_chunk in footer_chunks {
            // Add a footer and all the paths in order.
            let footer_str = &footer_chunk.first().unwrap().message.footer;
            debug_assert!(footer_str.ends_with('\n'));
            one_combined_message.push_str(footer_str);
            for info in footer_chunk {
                one_combined_message.push_str(&info.format_toprepo_footer());
            }
        }

        // The combined messages should be sorted by smallest path, but a
        // chunk with empty subject and body should still be last.
        let smallest_path = text_chunk.iter().map(|info| &info.path).min().unwrap();
        all_combined_messages.push((empty_subject_and_body, smallest_path, one_combined_message));
    }

    all_combined_messages.sort_by(
        |(a_empty_subject_and_body, a_smallest_path, _),
         (b_empty_subject_and_body, b_smallest_path, _)| {
            let a_key = (*a_empty_subject_and_body, a_smallest_path);
            let b_key = (*b_empty_subject_and_body, b_smallest_path);
            a_key.cmp(&b_key)
        },
    );
    let mut mono_message = String::new();
    for (_, _, combined_message) in all_combined_messages {
        if !mono_message.is_empty() {
            mono_message.push('\n');
        }
        mono_message.push_str(&combined_message);
    }
    mono_message
}

/// Decoding problems are logged as debug message because the user cannot repair
/// the history anyway.
fn get_and_decode_commit_message(repo: &gix::Repository, commit_id: CommitId) -> Result<String> {
    let commit = repo
        .find_commit(commit_id)
        .map_err(|err| anyhow!("Failed to find original commit {commit_id}: {err:#}"))?;
    let commit = commit.decode()?;
    let _scope_guard = crate::log::scope(format!("Commit {commit_id}"));
    Ok(decode_commit_message(&commit))
}

/// Best effort decoding the commit message, logging any error.
// TODO: 2025-09-22 How to only log at tips? Fix that later if an issue arises.
fn decode_commit_message(commit: &gix::objs::CommitRef<'_>) -> String {
    let encoding = if let Some(encoding_name) = commit.encoding {
        encoding_rs::Encoding::for_label_no_replacement(encoding_name).unwrap_or_else(|| {
            log::debug!("Unknown commit message encoding {encoding_name:?}, assuming UTF-8");
            encoding_rs::UTF_8
        })
    } else {
        encoding_rs::UTF_8
    };
    let (message, had_errors) = encoding.decode_without_bom_handling(commit.message);
    if had_errors {
        log::debug!("Commit message decoding errors");
    }
    message.into_owned()
}

/// Split a commit message into multiple messages for different submodules
/// according to toprepo footers.
///
/// Returns a mapping from submodule path to commit message and any potential
/// last message not containing a footer.
///
/// # Examples
/// ```
/// use git_toprepo::commit_message::PushMessage;
/// use git_toprepo::commit_message::split_commit_message;
/// use git_toprepo::git::GitPath;
///
/// let full_message = "\
/// Subject line
///
/// Topic: not-the-footer-yet
///
/// Body line 1
/// Body line 2
///
/// Footer-Key: value
/// Git-Toprepo-Ref: sub1 0123456789abcdef0123456789abcdef01234567
/// Git-Toprepo-Ref: sub2 89abcdef0123456789abcdef0123456789abcdef0
/// Topic: my-topic
///
/// Subject 2
///
/// Another-Footer: another value
/// Git-Toprepo-Ref: <top> fedcba9876543210fedcba9876543210fedcba98
///
/// Residual message
///
/// Topic: with-topic
/// ";
/// let (messages, residual) = split_commit_message(full_message.to_owned()).unwrap();
/// let expected_sub_push_message = PushMessage {
///     message: "Subject line
///
/// Topic: not-the-footer-yet
///
/// Body line 1
/// Body line 2
///
/// Footer-Key: value
/// "
///     .to_owned(),
///     topic: Some("my-topic".to_owned()),
/// };
/// assert_eq!(
///     messages,
///     std::collections::HashMap::from_iter(vec![
///         (GitPath::from("sub1"), expected_sub_push_message.clone()),
///         (GitPath::from("sub2"), expected_sub_push_message),
///         (
///             GitPath::from(""),
///             PushMessage {
///                 message: "Subject 2
///
/// Another-Footer: another value
/// "
///                 .to_owned(),
///                 topic: None,
///             }
///         ),
///     ])
/// );
/// assert_eq!(
///     residual,
///     Some(PushMessage {
///         message: "Residual message\n".to_owned(),
///         topic: Some("with-topic".to_owned()),
///     })
/// );
/// ```
pub fn split_commit_message(
    full_message: String,
) -> Result<(HashMap<GitPath, PushMessage>, Option<PushMessage>)> {
    #[derive(Debug, Default, Clone)]
    struct PerPathData {
        /// Extracted topic footer.
        topic: Option<String>,
        /// The footer part, including the trailing newline unless empty.
        footer: String,
    }
    impl PerPathData {
        fn is_empty(&self) -> bool {
            self.topic.is_none() && self.footer.is_empty()
        }

        fn merge(&self, other: &Self) -> Result<Self> {
            if let Some(old_topic) = &self.topic
                && let Some(new_topic) = &other.topic
            {
                anyhow::bail!("Multiple topic footers: {new_topic} {old_topic}");
            }
            Ok(Self {
                topic: self.topic.clone().or(other.topic.clone()),
                footer: format!("{}{}", self.footer, other.footer),
            })
        }
    }

    #[derive(Debug)]
    enum SplitState {
        /// Empty line before the subject.
        BeforeSubject,
        /// The subject line (the first paragraph) of the commit message.
        Subject { message: String },
        /// The body of the commit message.
        Body { message: String },
        /// So far conforming to be a footer of a commit message. At the end of
        /// a paragraph, continue with the commit message to find the first
        /// footer with a git-toprepo key.
        MaybeFooter {
            /// The subject and body of the commit message, usually with multiple trailing newlines.
            subject_and_body: String,
            /// The full commit message so far, including all footer lines.
            full_message: String,
            /// Collected footer data so far.
            pending_data: PerPathData,
        },
        /// So far conforming to be a footer of a commit message, containing at
        /// least one TopRepo footer line. If it turns out to be a valid footer
        /// paragraph, this is the end of the commit message and the next
        /// paragraph starts a message for a different submodule.
        ToprepoFooter {
            /// The subject and body of the commit message, usually with multiple trailing newlines.
            subject_and_body: String,
            /// The full commit message so far, including all footers.
            full_message: String,
            /// Collected footer data so far.
            pending_data: PerPathData,
            /// The submodule paths found so far and their corresponting footers.
            paths: Vec<(GitPath, PerPathData)>,
        },
    }
    impl SplitState {
        fn add_toprepo_footer_path(&mut self, path: GitPath) {
            match self {
                SplitState::MaybeFooter {
                    subject_and_body,
                    full_message,
                    pending_data,
                } => {
                    *self = SplitState::ToprepoFooter {
                        subject_and_body: std::mem::take(subject_and_body),
                        full_message: std::mem::take(full_message),
                        pending_data: Default::default(),
                        paths: vec![(path, std::mem::take(pending_data))],
                    }
                }
                SplitState::ToprepoFooter {
                    pending_data,
                    paths,
                    ..
                } => {
                    let footer_data = if pending_data.is_empty() {
                        // Use the same footer as the previous path.
                        let last_data = &paths.last().unwrap().1;
                        last_data.clone()
                    } else {
                        std::mem::take(pending_data)
                    };
                    paths.push((path, footer_data));
                }
                _ => unreachable!(
                    "TopRepo footer path can only be added in MaybeFooter or ToprepoFooter state"
                ),
            }
        }
    }

    let mut state = SplitState::BeforeSubject;
    let mut all_messages = HashMap::new();
    for line in (full_message + "\n\n").lines() {
        if line.is_empty() {
            // End of the paragraph.
            match &mut state {
                SplitState::BeforeSubject => {}
                SplitState::Subject { message } | SplitState::Body { message } => {
                    let new_message = std::mem::take(message) + "\n";
                    state = SplitState::MaybeFooter {
                        subject_and_body: new_message.clone(),
                        full_message: new_message,
                        pending_data: Default::default(),
                    };
                }
                SplitState::MaybeFooter {
                    subject_and_body,
                    full_message,
                    pending_data,
                } => {
                    // No TopRepo footer found, maybe the next paragraph is the
                    // actualy footer and this was just a footer pattern in the body
                    // of the commit message.
                    *full_message += "\n";
                    *subject_and_body = full_message.clone();
                    *pending_data = PerPathData::default();
                }
                SplitState::ToprepoFooter {
                    subject_and_body,
                    full_message: _,
                    pending_data,
                    paths,
                } => {
                    let mut subject_and_body = subject_and_body.as_str();
                    while subject_and_body.ends_with("\n\n") {
                        subject_and_body = &subject_and_body[..subject_and_body.len() - 1];
                    }
                    for (path, footer_data) in paths {
                        // Append the unassociated pending footer to all the
                        // messages, which can e.g. be Gerrit's Change-Id.
                        let merged_footer_data = footer_data.merge(pending_data)?;
                        if all_messages
                            .insert(
                                path.clone(),
                                PushMessage {
                                    subject_and_body: subject_and_body.to_owned(),
                                    footer: merged_footer_data.footer,
                                    topic: merged_footer_data.topic,
                                },
                            )
                            .is_some()
                        {
                            anyhow::bail!("Multiple commit messages for submodule {path}",);
                        }
                    }
                    state = SplitState::BeforeSubject;
                }
            }
        } else {
            // Next line in the paragraph.
            match &mut state {
                SplitState::BeforeSubject => {
                    state = SplitState::Subject {
                        message: format!("{line}\n"),
                    };
                }
                SplitState::Subject { message } | SplitState::Body { message } => {
                    message.push_str(line);
                    message.push('\n');
                }
                SplitState::MaybeFooter {
                    subject_and_body: _,
                    full_message,
                    pending_data,
                }
                | SplitState::ToprepoFooter {
                    subject_and_body: _,
                    full_message,
                    pending_data,
                    paths: _,
                } => {
                    full_message.push_str(line);
                    full_message.push('\n');
                    if let Some(new_topic) = line.strip_prefix("Topic:") {
                        let new_topic = new_topic.trim();
                        if let Some(old_topic) = &pending_data.topic {
                            anyhow::bail!("Multiple topic footers: {new_topic} {old_topic}");
                        }
                        pending_data.topic.replace(new_topic.to_owned());
                        // Skip this line from the commit message.
                        continue;
                    }
                    match get_toprepo_footer_subrepo_path(line)? {
                        Some(path) => {
                            state.add_toprepo_footer_path(path);
                        }
                        None => {
                            if is_footer_line(line.as_bytes().as_bstr()) {
                                // Continue in the footer.
                                pending_data.footer.push_str(line);
                                pending_data.footer.push('\n');
                            } else {
                                // This paragraph is not a footer at all.
                                state = SplitState::Body {
                                    message: std::mem::take(full_message),
                                };
                            }
                        }
                    }
                }
            }
        }
    }
    let residual_message = match (state, Default::default()) {
        (SplitState::BeforeSubject, _) => None,
        (
            SplitState::Subject {
                message: mut subject_and_body,
            },
            footer_data,
        )
        | (
            SplitState::Body {
                message: mut subject_and_body,
            },
            footer_data,
        )
        | (
            SplitState::MaybeFooter {
                mut subject_and_body,
                full_message: _,
                pending_data: footer_data,
            },
            _,
        ) => {
            // All lines in the message include a newline, so at lease one character and one newline exists.
            debug_assert!(subject_and_body.len() >= 2);
            while subject_and_body.ends_with("\n\n") {
                subject_and_body.pop();
            }
            Some(PushMessage {
                subject_and_body,
                footer: footer_data.footer,
                topic: footer_data.topic,
            })
        }
        (SplitState::ToprepoFooter { .. }, _) => {
            unreachable!("Toprepo footer has been followed by an empty line")
        }
    };
    Ok((all_messages, residual_message))
}

/// Extracts the submodule path from a TopRepo footer line, if any.
///
/// # Examples
/// ```
/// use git_toprepo::git::GitPath;
/// use git_toprepo::commit_message::get_toprepo_footer_subrepo_path_for_tests_only as get_toprepo_footer_subrepo_path;
///
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: path/to/submodule 0123456789abcdef").unwrap(),
///     Some(GitPath::from("path/to/submodule")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref:   path 0123456789abcdef").unwrap(),
///     Some(GitPath::from("path")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: path").unwrap(),
///     Some(GitPath::from("path")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: <top>").unwrap(),
///     Some(GitPath::from("")),
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Other-Footer: foo bar").unwrap(),
///     None,
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Not a footer").unwrap(),
///     None,
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref: ").unwrap_err().to_string(),
///     "Empty submodule path in TopRepo footer \"Git-Toprepo-Ref: \"",
/// );
/// assert_eq!(
///     get_toprepo_footer_subrepo_path("Git-Toprepo-Ref:    ").unwrap_err().to_string(),
///     "Empty submodule path in TopRepo footer \"Git-Toprepo-Ref:    \"",
/// );
/// ```
#[doc(hidden)]
fn get_toprepo_footer_subrepo_path(line: &str) -> Result<Option<GitPath>> {
    let Some(value) = line.strip_prefix(GIT_TOPREPO_FOOTER_PREFIX) else {
        return Ok(None);
    };
    // Looking for the next whitespace will trim the end.
    let value = value.trim_start();
    let subrepo_path = if let Some(idx) = value.find(|c: char| c.is_whitespace()) {
        &value[..idx]
    } else {
        value
    };
    if subrepo_path.is_empty() {
        anyhow::bail!("Empty submodule path in TopRepo footer {line:?}");
    }
    Ok(Some(GitPath::new(
        if subrepo_path == TOPREPO_DISPLAY_PATH {
            "".into()
        } else {
            subrepo_path.as_bytes().into()
        },
    )))
}

pub fn get_toprepo_footer_subrepo_path_for_tests_only(line: &str) -> Result<Option<GitPath>> {
    get_toprepo_footer_subrepo_path(line)
}

fn is_interesting_message(message: &str) -> bool {
    if message.starts_with("Update git submodules\n") {
        // A boring message generated by Gerrit.
        return false;
    }
    true
}

/// Check if the commit message has a footer section.
///
/// # Examples
/// ```
/// use git_toprepo::commit_message::extract_commit_message_footer;
///
/// let verify_no_footer = |msg: &str| {
///     assert_eq!(
///         extract_commit_message_footer(msg.into()),
///         (msg.into(), None)
///     );
/// };
///
/// verify_no_footer("Subject line\nmore subject");
/// verify_no_footer("Subject line\n\nBody (invalid footer line)\n");
/// verify_no_footer("Subject line\n\nInvalid_Key: value\nValid-Key: value");
///
/// assert_eq!(
///     extract_commit_message_footer("Subject line\n\nFooter-Key: value".into()),
///     ("Subject line\n\n".into(), Some("Footer-Key: value".into())),
/// );
/// assert_eq!(
///     extract_commit_message_footer("Subject line\n\nFooter-Key: value".into()),
///     ("Subject line\n\n".into(), Some("Footer-Key: value".into()))
/// );
/// verify_no_footer("Subject line\n\nFooter Key: value");
/// assert_eq!(
///     extract_commit_message_footer("Subject line\n\nBody\n\nFooter-Key: value".into()),
///     (
///         "Subject line\n\nBody\n\n".into(),
///         Some("Footer-Key: value".into())
///     )
/// );
/// verify_no_footer("Subject line\n\nBody\n\nFooter Key: value");
/// assert_eq!(
///     extract_commit_message_footer(
///         "Subject line\n\nFooter-Key: value\nAnother-Footer: another value".into()
///     ),
///     (
///         "Subject line\n\n".into(),
///         Some("Footer-Key: value\nAnother-Footer: another value".into())
///     )
/// );
/// verify_no_footer("Subject line\n\nFooter Key: value\nAnother-Footer: value\n");
///
/// verify_no_footer("Subject line\n\nBad^Key: value");
/// verify_no_footer("Subject line\n\nBad_Key: value");
///
/// assert_eq!(
///     extract_commit_message_footer(
///         "With CRLF, spaces\nand extra newlines\n\r\n\r\nFooter-Key: value\r\n   \n".into()
///     ),
///     (
///         "With CRLF, spaces\nand extra newlines\n\r\n\r\n".into(),
///         Some("Footer-Key: value\r\n   \n".into())
///     )
/// );
/// ```
pub fn extract_commit_message_footer(message: &BStr) -> (&BStr, Option<&BStr>) {
    let mut footer_start = None;
    let mut line_start = 0;
    let mut between_paragraphs = false;
    while line_start < message.len() {
        let line_end = line_start
            + message[line_start..]
                .find_char('\n')
                .unwrap_or_else(|| message.len() - line_start);
        let line = &message[line_start..line_end];
        if line.trim_start().is_empty() {
            between_paragraphs = true;
        } else {
            if is_footer_line(line) {
                if between_paragraphs {
                    footer_start = Some(line_start);
                }
            } else {
                // Non-empty line that is not a footer, then the whole paragraph is discarded.
                footer_start = None;
            }
            between_paragraphs = false;
        }
        line_start = line_end + 1;
    }
    (
        &message[0..footer_start.unwrap_or_else(|| message.len())],
        footer_start.map(|idx| &message[idx..message.len()]),
    )
}

/// Check if the commit message has a footer section.
///
/// # Examples
/// ```
/// use git_toprepo::commit_message::commit_message_has_footer;
///
/// assert!(!commit_message_has_footer("Subject line".into()));
/// assert!(commit_message_has_footer(
///     "Subject line\n\nFooter-Key: value".into()
/// ));
pub fn commit_message_has_footer(message: &BStr) -> bool {
    extract_commit_message_footer(message).1.is_some()
}

/// Check if the line is a proper footer line.
///
/// # Examples
/// ```
/// # use git_toprepo::commit_message::is_footer_line_for_tests_only as is_footer_line;
///
/// assert!(is_footer_line("Valid-Key: value".into()));
/// assert!(is_footer_line("Valid-Key1:value".into()));
/// assert!(!is_footer_line("Normal line".into()));
/// assert!(!is_footer_line("Invalid_Key: value".into()));
/// assert!(!is_footer_line("Invalid^Key: value".into()));
/// assert!(!is_footer_line(":Something".into()));
/// assert!(!is_footer_line("".into()));
/// ```
#[doc(hidden)]
fn is_footer_line(line: &BStr) -> bool {
    let Some(idx) = line.find_byte(b':') else {
        return false;
    };
    let key = line[..idx].as_bstr();
    if key.is_empty() {
        return false;
    }
    for c in key.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    true
}

pub fn is_footer_line_for_tests_only(line: &BStr) -> bool {
    is_footer_line(line)
}

#[cfg(test)]

mod tests {
    use super::*;

    const SUBJECTS: &[&str] = &["Subject line", "Subject...\n... lines"];
    const BODIES: &[&str] = &[
        // Verify both with and without trailing newline on subject.
        "",
        "\n",
        "\n\nSingle body line\n",
        // Verify without trailing newline and extra start newline.
        "\n\n\nOne\nparagraph",
        "\n\nFirst\nparagraph\n\nSecond\nparagraph\n",
    ];
    const FOOTERS: &[(Option<&str>, &str)] = &[
        (None, ""),
        (Some("X"), "\n\nFooter: A\nTopic: X\nFooter: B"),
        (Some("Y"), "\n\nTopic: Y\nFooter: C\n\n"),
        (Some("Z"), "\n\nFooter: D\nTopic: Z\n"),
        (Some("W"), "\n\nTopic: W"),
        (None, "\n\nFooter: E\n\n"),
    ];

    #[test]
    fn exhaustive_combine_and_split() {
        let mut messages = Vec::with_capacity(5);
        for repo_count in 1..4 {
            many_combinations_impl(repo_count, &mut messages);
            assert!(messages.is_empty());
        }
    }

    fn many_combinations_impl(repo_count: usize, messages: &mut Vec<PushMessage>) {
        if repo_count > 0 {
            for subject in SUBJECTS {
                for body in BODIES {
                    let subject_and_body = format!("{subject}{body}");
                    for (topic, footer) in FOOTERS {
                        messages.push(PushMessage {
                            subject_and_body: subject_and_body.clone(),
                            topic: topic.map(|s| s.to_owned()),
                            footer: (*footer).to_owned(),
                        });
                        many_combinations_impl(repo_count - 1, messages);
                        messages.pop();
                    }
                }
            }
        } else {
            let mut msg_infos_iter =
                messages
                    .iter()
                    .enumerate()
                    .map(|(idx, msg)| CommitMessageInfo {
                        path: GitPath::new(format!("sub/{idx}-path").into()),
                        message: Some(CommitMessage {
                            subject_and_body: msg.subject_and_body.clone(),
                            footer: msg.footer.clone(),
                        }),
                        status: format!("status {idx}"),
                    });
            let source_info = msg_infos_iter.next().unwrap();
            let sub_infos = msg_infos_iter.collect_vec();
            let mono_message = calculate_mono_commit_message(source_info, sub_infos);
            let (parts, residual) = split_commit_message(mono_message).unwrap();
            assert!(residual.is_none());
            for (idx, msg) in messages.iter().enumerate() {
                let path = GitPath::new(format!("sub/{idx}-path").into());
                assert_eq!(parts.get(&path).unwrap(), msg);
            }
        }
    }
}
