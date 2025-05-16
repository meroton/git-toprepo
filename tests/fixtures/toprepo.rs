use bstr::ByteSlice;
use git_toprepo::git::CommitId;
use git_toprepo::git::GitPath;
use git_toprepo::git::git_command;
use git_toprepo::git::git_update_submodule_in_index;
use git_toprepo::util::CommandExtension as _;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

pub fn commit_env() -> HashMap<String, String> {
    HashMap::from(
        [
            ("GIT_AUTHOR_NAME", "A Name"),
            ("GIT_AUTHOR_EMAIL", "a@no.domain"),
            ("GIT_AUTHOR_DATE", "2023-01-02T03:04:05Z+01:00"),
            ("GIT_COMMITTER_NAME", "C Name"),
            ("GIT_COMMITTER_EMAIL", "c@no.domain"),
            ("GIT_COMMITTER_DATE", "2023-06-07T08:09:10Z+01:00"),
        ]
        .map(|(k, v)| (k.into(), v.into())),
    )
}

fn git_commit(repo: &Path, env: &HashMap<String, String>, message: &str) -> CommitId {
    let file_name = message.to_owned() + ".txt";
    {
        std::fs::File::create_new(repo.join(&file_name)).unwrap();
    }
    git_command(repo)
        .args(["add", &file_name])
        .check_success_with_stderr()
        .unwrap();

    git_command(repo)
        .args(["commit", "--allow-empty", "-m", message])
        .envs(env)
        .check_success_with_stderr()
        .unwrap();

    // Returns commit hash as String.
    // TODO: Return Result<String> instead?
    let output = git_command(repo)
        .args(["rev-parse", "HEAD"])
        .envs(env)
        .check_success_with_stderr()
        .unwrap();
    let commit_id_hex = git_toprepo::util::trim_newline_suffix(output.stdout.to_str().unwrap());
    CommitId::from_hex(commit_id_hex.as_bytes()).unwrap()
}

fn git_checkout(repo: &Path, commit_id: &CommitId) {
    git_command(repo)
        .args(["reset", "--hard"])
        .arg(format!("{}", commit_id.to_hex()))
        .check_success_with_stderr()
        .unwrap();
}

fn git_merge(repo: &Path, commit_ids: &[&CommitId]) {
    let commit_ids: Vec<String> = commit_ids
        .iter()
        .map(|id| format!("{}", id.to_hex()))
        .collect();
    // Skip checking exit code, merging conflicts in submodules will fail.
    git_command(repo)
        .args(["merge", "--no-ff", "--no-commit", "--strategy=ours"])
        .args(["-m", "Dummy"])
        .args(commit_ids)
        .envs(commit_env())
        .check_success_with_stderr()
        .unwrap();
}

fn git_add_local_submodule_to_index(repo: &Path, path: &GitPath, url: &str) {
    let path_str = path.to_str().unwrap();
    git_command(repo)
        .args(["-c", "protocol.file.allow=always"])
        .args(["submodule", "add", "--force", url, path_str])
        .check_success_with_stderr()
        .unwrap();
    git_command(repo)
        .args(["submodule", "deinit", "-f", path_str])
        .check_success_with_stderr()
        .unwrap();
}

#[derive(Debug)]
/// A struct for example repo structures. The example repo consists of repos
/// `top` and `sub`, with `sub` being a submodule in `top`. The commit history
/// is shown below:
/// ```text
/// top  A---B---C---D-------E---F---G----Ha--Ia-----J---------------N
///          |       |       |       |\    |   |   / | \      \     /|
///          |       |       |       | Hb--------Ib  |  K---L--M(10) |
///          |       |       |       |  |  |   |  |  |  |   |        |
/// sub  1---2-------3---4---5---6---7----8a--9a----10-------------13
///                                   \ |         | /   |   |      /
///                                    8b--------9b-----11--12----/
/// ```
/// The commit N is pointing to commit 11 in the submodule, which is a bad merge
/// because even if N keeps the submodule K was pointong to, the submodule
/// pointer goes backwards in relation to M.
///
/// # Examples
///
/// ```rust
/// // The crate `tempfile` is used here to create temporary directories for
/// // testing.
/// use tempfile::tempdir;
/// use git_toprepo::util::GitTopRepoExample;
///
/// let tmp_dir = tempdir().unwrap();
/// let tmp_path = tmp_dir.path().to_path_buf();
///
/// // Use this instead for a persistent directory:
/// // let tmp_path = tmp_dir.into_path();
///
/// let repo = GitTopRepoExample::new(&tmp_path);
/// let top_repo_path = repo.init_server_top();
/// assert!(!top_repo_path.exists());
/// ```
pub struct GitTopRepoExample {
    pub tmp_path: PathBuf,
    // TODO: store top/sub paths?
}

impl GitTopRepoExample {
    pub fn new(tmp_path: PathBuf) -> GitTopRepoExample {
        GitTopRepoExample { tmp_path }
    }

    /// Sets up the repo structure and returns the top repo path.
    pub fn init_server_top(&self) -> PathBuf {
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let sub_repo = self.tmp_path.join("sub").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&sub_repo).unwrap();

        let sub_path = GitPath::new(b"sub".into());

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        git_command(&sub_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Create the following commit history:
        // top  A---B---C---D-------E---F---G
        //          |       |       |       |
        // sub  1---2-------3---4---5---6---7
        git_commit(&sub_repo, &env, "1");
        let sub_rev_2 = git_commit(&sub_repo, &env, "2");
        let sub_rev_3 = git_commit(&sub_repo, &env, "3");
        git_commit(&sub_repo, &env, "4");
        let sub_rev_5 = git_commit(&sub_repo, &env, "5");
        git_commit(&sub_repo, &env, "6");
        let sub_rev_7 = git_commit(&sub_repo, &env, "7");

        git_commit(&top_repo, &env, "A");
        git_add_local_submodule_to_index(&top_repo, &GitPath::new("sub".into()), "../sub/");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_2).unwrap();
        git_commit(&top_repo, &env, "B");
        git_commit(&top_repo, &env, "C");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_3).unwrap();
        git_commit(&top_repo, &env, "D");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_5).unwrap();
        git_commit(&top_repo, &env, "E");
        git_commit(&top_repo, &env, "F");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_7).unwrap();
        let top_rev_g = git_commit(&top_repo, &env, "G");

        // Continue with:
        // top  --G----Ha--Ia-----J---------------N
        //        |\    |   |   / | \      \     /|
        //        | Hb--------Ib  |  K---L--M(10) |
        //        |  |  |   |  |  |  |   |        |
        // sub  --7----8a--9a----10--------------13
        //         \ |         | / \ |   |       /
        //          8b---------9b   11---12-----/
        let sub_rev_8b = git_commit(&sub_repo, &env, "8b");
        let sub_rev_9b = git_commit(&sub_repo, &env, "9b");
        git_checkout(&sub_repo, &sub_rev_7);
        let sub_rev_8a = git_commit(&sub_repo, &env, "8a");
        let sub_rev_9a = git_commit(&sub_repo, &env, "9a");
        git_merge(&sub_repo, &[&sub_rev_9b]);
        let sub_rev_10 = git_commit(&sub_repo, &env, "10");
        let sub_rev_11 = git_commit(&sub_repo, &env, "11");
        let sub_rev_12 = git_commit(&sub_repo, &env, "12");
        git_checkout(&sub_repo, &sub_rev_10);
        git_merge(&sub_repo, &[&sub_rev_12]);
        let sub_rev_13 = git_commit(&sub_repo, &env, "13");

        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_8b).unwrap();
        git_commit(&top_repo, &env, "Hb");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_9b).unwrap();
        let top_rev_ib = git_commit(&top_repo, &env, "Ib");
        git_checkout(&top_repo, &top_rev_g);
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_8a).unwrap();
        git_commit(&top_repo, &env, "Ha");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_9a).unwrap();
        git_commit(&top_repo, &env, "Ia");
        git_merge(&top_repo, &[&top_rev_ib]);
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_10).unwrap();
        let top_rev_j = git_commit(&top_repo, &env, "J");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_11).unwrap();
        git_commit(&top_repo, &env, "K");
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_12).unwrap();
        let top_rev_l = git_commit(&top_repo, &env, "L");
        git_checkout(&top_repo, &top_rev_j);
        git_merge(&top_repo, &[&top_rev_l]);
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_10).unwrap();
        let top_rev_m = git_commit(&top_repo, &env, "M");
        git_checkout(&top_repo, &top_rev_j);
        git_merge(&top_repo, &[&top_rev_m]);
        git_update_submodule_in_index(&top_repo, &sub_path, &sub_rev_13).unwrap();
        git_commit(&top_repo, &env, "N");

        top_repo
    }

    /// Sets up the repo structure and returns the top repo path.
    pub fn merge_with_one_submodule_a(&self) -> PathBuf {
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let subx_repo = self.tmp_path.join("subx").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&subx_repo).unwrap();

        let subx_path = GitPath::new(b"subx".into());

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        git_command(&subx_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Create the following commit history for:
        // subX-release  4---5---6
        //              /|      /|
        // subX        1-+-2---3 |
        //             | |     | |
        // top-main    A-+-----B |
        //              \|      \|
        // top-release   C-------D
        let subx_rev_1 = git_commit(&subx_repo, &env, "x-main-1");
        git_commit(&subx_repo, &env, "x-main-2");
        let subx_rev_3 = git_commit(&subx_repo, &env, "x-main-3");
        git_checkout(&subx_repo, &subx_rev_1);
        let subx_rev_4 = git_commit(&subx_repo, &env, "x-release-4");
        git_commit(&subx_repo, &env, "x-release-5");
        git_merge(&subx_repo, &[&subx_rev_3]);
        let subx_rev_6 = git_commit(&subx_repo, &env, "x-release-6");

        git_add_local_submodule_to_index(&top_repo, &subx_path, "../subx/");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_1).unwrap();
        let top_rev_a = git_commit(&top_repo, &env, "A1-main");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_3).unwrap();
        let top_rev_b = git_commit(&top_repo, &env, "B3-main");
        git_checkout(&top_repo, &top_rev_a);
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_4).unwrap();
        git_commit(&top_repo, &env, "C4-release");
        git_merge(&top_repo, &[&top_rev_b]);
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_6).unwrap();
        git_commit(&top_repo, &env, "D6-release");

        top_repo
    }

    /// Sets up the repo structure and returns the top repo path.
    pub fn merge_with_one_submodule_b(&self) -> PathBuf {
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let subx_repo = self.tmp_path.join("subx").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&subx_repo).unwrap();

        let subx_path = GitPath::new(b"subx".into());

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        git_command(&subx_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Create the following commit history for:
        // subX-release  5---6---7----
        //              /    |  /     \
        // subX-main   1---2-+-3---4---8
        //             |     | |       |
        // top-main    A-----+-B-------F
        //              \    |   /-E--/
        // top-release   ----C-----D-/
        let subx_rev_1 = git_commit(&subx_repo, &env, "x-main-1");
        git_commit(&subx_repo, &env, "x-main-2");
        let subx_rev_3 = git_commit(&subx_repo, &env, "x-main-3");
        git_checkout(&subx_repo, &subx_rev_1);
        git_commit(&subx_repo, &env, "x-release-5");
        let subx_rev_6 = git_commit(&subx_repo, &env, "x-release-6");
        git_merge(&subx_repo, &[&subx_rev_3]);
        let subx_rev_7 = git_commit(&subx_repo, &env, "x-release-7");
        git_checkout(&subx_repo, &subx_rev_3);
        git_commit(&subx_repo, &env, "x-main-4");
        git_merge(&subx_repo, &[&subx_rev_7]);
        let subx_rev_8 = git_commit(&subx_repo, &env, "x-release-8");

        git_add_local_submodule_to_index(&top_repo, &subx_path, "../subx/");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_1).unwrap();
        let top_rev_a = git_commit(&top_repo, &env, "A1-main");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_6).unwrap();
        let top_rev_c = git_commit(&top_repo, &env, "C6-release");
        let top_rev_d = git_commit(&top_repo, &env, "D6-release");
        git_checkout(&top_repo, &top_rev_c);
        let top_rev_e = git_commit(&top_repo, &env, "E6-release");
        git_checkout(&top_repo, &top_rev_a);
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_3).unwrap();
        git_commit(&top_repo, &env, "B3-main");
        git_merge(&top_repo, &[&top_rev_d, &top_rev_e]);
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_8).unwrap();
        git_commit(&top_repo, &env, "F8-release");

        top_repo
    }

    /// Sets up the repo structure and returns the top repo path.
    pub fn merge_with_two_submodules(&self) -> PathBuf {
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let subx_repo = self.tmp_path.join("subx").to_path_buf();
        let suby_repo = self.tmp_path.join("suby").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&subx_repo).unwrap();
        std::fs::create_dir_all(&suby_repo).unwrap();

        let subx_path = GitPath::new(b"subx".into());
        let suby_path = GitPath::new(b"suby".into());

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        git_command(&subx_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        git_command(&suby_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Create the following commit history for:
        // subX/Y-release 4---5---6
        //               /|      /|
        // subX/Y-main  1-+-2---3 |
        //              | |     | |
        // top-main     A-+-----B |
        //               \|      \|
        // top-release    C-------D
        let subx_rev_1 = git_commit(&subx_repo, &env, "x-main-1");
        git_commit(&subx_repo, &env, "x-main-2");
        let subx_rev_3 = git_commit(&subx_repo, &env, "x-main-3");
        git_checkout(&subx_repo, &subx_rev_1);
        let subx_rev_4 = git_commit(&subx_repo, &env, "x-release-4");
        git_commit(&subx_repo, &env, "x-release-5");
        git_merge(&subx_repo, &[&subx_rev_3]);
        let subx_rev_6 = git_commit(&subx_repo, &env, "x-release-6");

        let suby_rev_1 = git_commit(&suby_repo, &env, "y-main-1");
        git_commit(&suby_repo, &env, "y-main-2");
        let suby_rev_3 = git_commit(&suby_repo, &env, "y-main-3");
        git_checkout(&suby_repo, &suby_rev_1);
        let suby_rev_4 = git_commit(&suby_repo, &env, "y-release-4");
        git_commit(&suby_repo, &env, "y-release-5");
        git_merge(&suby_repo, &[&suby_rev_3]);
        let suby_rev_6 = git_commit(&suby_repo, &env, "y-release-6");

        git_add_local_submodule_to_index(&top_repo, &subx_path, "../subx/");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_1).unwrap();
        git_add_local_submodule_to_index(&top_repo, &suby_path, suby_repo.to_str().unwrap());
        git_update_submodule_in_index(&top_repo, &suby_path, &suby_rev_1).unwrap();
        let top_rev_a = git_commit(&top_repo, &env, "A1-main");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_3).unwrap();
        git_update_submodule_in_index(&top_repo, &suby_path, &suby_rev_3).unwrap();
        let top_rev_b = git_commit(&top_repo, &env, "B3-main");
        git_checkout(&top_repo, &top_rev_a);
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_4).unwrap();
        git_update_submodule_in_index(&top_repo, &suby_path, &suby_rev_4).unwrap();
        git_commit(&top_repo, &env, "C4-release");
        git_merge(&top_repo, &[&top_rev_b]);
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_6).unwrap();
        git_update_submodule_in_index(&top_repo, &suby_path, &suby_rev_6).unwrap();
        git_commit(&top_repo, &env, "D6-release");

        top_repo
    }

    /// Sets up the repo structure and returns the top repo path.
    pub fn submodule_removal(&self) -> PathBuf {
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let subx_repo = self.tmp_path.join("subx").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&subx_repo).unwrap();

        let subx_path = GitPath::new(b"subx".into());

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        git_command(&subx_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Create the following commit history:
        // subX  1---2  x x
        //       |   |  | |
        // top   A1--B2-+-C0--E0
        //        \     |    /
        // top     -----D0---
        let subx_rev_1 = git_commit(&subx_repo, &env, "1");
        let subx_rev_2 = git_commit(&subx_repo, &env, "2");

        git_add_local_submodule_to_index(&top_repo, &GitPath::new("subx".into()), "../subx/");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_1).unwrap();
        let top_rev_a = git_commit(&top_repo, &env, "A");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_2).unwrap();
        git_commit(&top_repo, &env, "B");
        git_command(&top_repo)
            .args(["rm", "subx"])
            .check_success_with_stderr()
            .unwrap();
        let top_rev_c = git_commit(&top_repo, &env, "C");
        git_checkout(&top_repo, &top_rev_a);
        git_command(&top_repo)
            .args(["rm", "subx"])
            .check_success_with_stderr()
            .unwrap();
        git_commit(&top_repo, &env, "D");
        git_merge(&top_repo, &[&top_rev_c]);
        git_commit(&top_repo, &env, "E");

        top_repo
    }

    /// Sets up the repo structure and returns the top repo path.
    pub fn move_submodule(&self) -> PathBuf {
        let env = commit_env();
        let top_repo = self.tmp_path.join("top").to_path_buf();
        let subx_repo = self.tmp_path.join("subx").to_path_buf();

        std::fs::create_dir_all(&top_repo).unwrap();
        std::fs::create_dir_all(&subx_repo).unwrap();

        let subx_path = GitPath::new(b"subx".into());
        let suby_path = GitPath::new(b"suby".into());

        git_command(&top_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();
        git_command(&subx_repo)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .envs(&env)
            .check_success_with_stderr()
            .unwrap();

        // Create the following commit history:
        // subZ                   /-3
        // subY     /-2-*--3---3-*  |
        // subX  1-/  |  \-2   |  \-3
        //       |    |    |   |    |
        // top   A1---B2---C---D----E
        let subx_rev_1 = git_commit(&subx_repo, &env, "1");
        let subx_rev_2 = git_commit(&subx_repo, &env, "2");
        git_commit(&subx_repo, &env, "3");

        git_add_local_submodule_to_index(&top_repo, &GitPath::new("subx".into()), "../subx/");
        git_update_submodule_in_index(&top_repo, &subx_path, &subx_rev_1).unwrap();
        git_commit(&top_repo, &env, "A");
        git_command(&top_repo)
            .args(["mv", "subx", "suby"])
            .check_success_with_stderr()
            .unwrap();
        git_update_submodule_in_index(&top_repo, &suby_path, &subx_rev_2).unwrap();
        git_commit(&top_repo, &env, "B");
        git_command(&top_repo)
            .args(["mv", "suby", "subx"])
            .check_success_with_stderr()
            .unwrap();
        git_add_local_submodule_to_index(&top_repo, &GitPath::new("suby".into()), "../subx/");
        git_commit(&top_repo, &env, "C");
        git_command(&top_repo)
            .args(["rm", "-ff", "subx"])
            .check_success_with_stderr()
            .unwrap();
        git_commit(&top_repo, &env, "D");
        git_command(&top_repo)
            .args(["mv", "suby", "subz"])
            .check_success_with_stderr()
            .unwrap();
        git_add_local_submodule_to_index(&top_repo, &GitPath::new("subx".into()), "../subx/");
        git_commit(&top_repo, &env, "E");

        top_repo
    }
}
