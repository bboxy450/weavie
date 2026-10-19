//! Persistent source selections, isolated Git worktrees, and Cargo source patches.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::Graph;
use crate::backend::{self, Descriptor, Target};

const STATE: &str = "weavie-session.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub ready: bool,
    pub directory: PathBuf,
    pub cache: PathBuf,
    pub target: Target,
    pub root_repository: String,
    pub cargo_root: String,
    pub repositories: BTreeMap<String, Binding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub input: String,
    pub node: String,
    pub locked: Map<String, Value>,
    pub store_path: PathBuf,
    pub source_path: PathBuf,
    pub editable: bool,
    pub crates: BTreeMap<String, String>,
    pub patch_sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryStatus {
    pub input: String,
    pub path: PathBuf,
    pub editable: bool,
    pub revision: Option<String>,
    pub changes: Option<String>,
}

pub fn valid_alias(alias: &str) -> Result<()> {
    ensure!(
        !alias.is_empty()
            && alias
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
        "invalid repository/session name {alias:?}; use letters, digits, _ or -"
    );
    Ok(())
}

pub fn relative_path(path: &str) -> Result<()> {
    ensure!(!path.is_empty(), "empty repository-relative path");
    ensure!(
        Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "path must stay inside its repository: {path:?}"
    );
    Ok(())
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let temporary = path.with_extension("weavie-tmp");
    fs::write(&temporary, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(unix)]
fn symlink(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, destination)
        .with_context(|| format!("linking {} to {}", destination.display(), source.display()))
}

#[cfg(not(unix))]
fn symlink(_: &Path, _: &Path) -> Result<()> {
    bail!("workspace symlinks currently require Unix")
}

fn replace_link(source: &Path, destination: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        ensure!(
            metadata.file_type().is_symlink(),
            "refusing to replace non-symlink {}",
            destination.display()
        );
        fs::remove_file(destination)?;
    }
    symlink(source, destination)
}

fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    FileExt::lock_exclusive(&file)?;
    Ok(file)
}

#[derive(Debug)]
struct GitSource {
    url: String,
    revision: String,
    directory: String,
    submodules: bool,
    lfs: bool,
}

fn string_attribute<'a>(locked: &'a Map<String, Value>, name: &str) -> Result<&'a str> {
    locked
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Git source lacks {name:?}"))
}

impl GitSource {
    fn from_locked(locked: &Map<String, Value>) -> Result<Self> {
        let kind = string_attribute(locked, "type")?;
        let url = match kind {
            "git" => string_attribute(locked, "url")?
                .trim_start_matches("git+")
                .to_owned(),
            "github" | "gitlab" | "sourcehut" => {
                let default = match kind {
                    "github" => "github.com",
                    "gitlab" => "gitlab.com",
                    _ => "git.sr.ht",
                };
                let host = locked
                    .get("host")
                    .and_then(Value::as_str)
                    .unwrap_or(default);
                let owner = string_attribute(locked, "owner")?;
                let repo = string_attribute(locked, "repo")?;
                format!("https://{host}/{owner}/{repo}.git")
            }
            other => bail!(
                "cannot create a Git worktree for {other:?} input; use a Git-backed input for editing"
            ),
        };
        ensure!(!url.starts_with('-'), "invalid Git remote URL");
        let revision = locked
            .get("rev")
            .and_then(Value::as_str)
            .or_else(|| {
                locked
                    .get("dirtyRev")
                    .and_then(Value::as_str)
                    .and_then(|rev| rev.strip_suffix("-dirty"))
            })
            .ok_or_else(|| anyhow!("Git source lacks a base commit"))?
            .to_owned();
        ensure!(
            [40, 64].contains(&revision.len()) && revision.bytes().all(|b| b.is_ascii_hexdigit()),
            "expected a full locked Git commit"
        );
        let directory = locked
            .get("dir")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_owned();
        relative_path(&directory)?;
        Ok(Self {
            url,
            revision,
            directory,
            submodules: locked
                .get("submodules")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            lfs: locked.get("lfs").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

impl Session {
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        if let Some(path) = explicit {
            return Self::load(path);
        }
        if let Some(directory) = Self::current_directory()? {
            return Self::load(&directory);
        }
        bail!("not inside a weavie session; use --session DIRECTORY or cd into an opened session")
    }

    pub fn current_directory() -> Result<Option<PathBuf>> {
        let current = std::env::current_dir()?;
        Ok(current
            .ancestors()
            .find(|path| path.join(STATE).is_file())
            .map(Path::to_owned))
    }

    pub fn load(directory: &Path) -> Result<Self> {
        let directory = directory
            .canonicalize()
            .context("locating session directory")?;
        let state: Self = serde_json::from_slice(&fs::read(directory.join(STATE))?)?;
        ensure!(
            state.schema_version == 1,
            "unsupported session state version"
        );
        ensure!(
            state.directory == directory,
            "session moved; stored worktree and GC-root paths must remain valid"
        );
        Ok(state)
    }

    /// Create only the selected target's sources. Existing paths are never overwritten.
    pub fn create(
        target: Target,
        directory: &Path,
        descriptor: Descriptor,
        graph: &Graph,
        root_locked: Option<Map<String, Value>>,
        edit: &[String],
    ) -> Result<Self> {
        relative_path(&descriptor.cargo_root)?;
        let editing: BTreeSet<_> = if edit.is_empty() {
            BTreeSet::from([descriptor.root_repository.clone()])
        } else {
            edit.iter().cloned().collect()
        };
        for alias in &editing {
            ensure!(
                descriptor.repositories.contains_key(alias),
                "repository {alias:?} is not in this target"
            );
        }
        let absolute = if directory.is_absolute() {
            directory.to_owned()
        } else {
            std::env::current_dir()?.join(directory)
        };
        ensure!(
            !absolute.exists(),
            "session directory already exists: {}",
            absolute.display()
        );
        let parent = absolute
            .parent()
            .ok_or_else(|| anyhow!("session directory needs a parent"))?;
        fs::create_dir_all(parent)?;
        let directory = parent.canonicalize()?.join(
            absolute
                .file_name()
                .ok_or_else(|| anyhow!("invalid session directory"))?,
        );
        let cache = target.root.join(".weavie/cache/git");
        let mut repositories = BTreeMap::new();
        let mut inputs = BTreeSet::new();
        for (alias, repo) in descriptor.repositories {
            valid_alias(&alias)?;
            ensure!(
                inputs.insert(repo.input.clone()),
                "input {:?} has multiple workspace aliases",
                repo.input
            );
            for path in repo.crates.values() {
                relative_path(path)?;
            }
            let (node, locked) = if repo.input == "self" {
                (
                    graph.root.clone(),
                    root_locked
                        .clone()
                        .ok_or_else(|| anyhow!("root source has no locked Git metadata"))?,
                )
            } else {
                let node = graph.resolve_input(&repo.input)?;
                let locked = graph.nodes[&node]
                    .locked
                    .clone()
                    .ok_or_else(|| anyhow!("no source metadata for {}", repo.input))?;
                (node, locked)
            };
            if editing.contains(&alias) {
                GitSource::from_locked(&locked).with_context(|| format!("editing {alias}"))?;
            }
            ensure!(
                repo.store_path.is_dir(),
                "Nix source is unavailable: {}",
                repo.store_path.display()
            );
            repositories.insert(
                alias.clone(),
                Binding {
                    input: repo.input,
                    node,
                    locked,
                    store_path: repo.store_path,
                    source_path: directory.join("worktrees").join(&alias),
                    editable: false,
                    crates: repo.crates,
                    patch_sources: repo.patch_sources,
                },
            );
        }
        // Two aliases of one lock node cannot have independent effective overrides.
        let mut instances = BTreeSet::new();
        for binding in repositories.values() {
            ensure!(
                instances.insert(&binding.node),
                "target maps multiple aliases to one lock node; use one shared repository binding"
            );
        }
        fs::create_dir(&directory)?;
        for child in ["src", "worktrees", "gcroots"] {
            fs::create_dir(directory.join(child))?;
        }
        let mut session = Self {
            schema_version: 1,
            ready: false,
            directory,
            cache,
            target,
            root_repository: descriptor.root_repository,
            cargo_root: descriptor.cargo_root,
            repositories,
        };
        session.save()?;
        let result = (|| {
            for (alias, binding) in &session.repositories {
                // Register real roots for store sources, independently of the visible links.
                if binding.store_path.starts_with("/nix/store") {
                    let store_name = binding
                        .store_path
                        .strip_prefix("/nix/store")?
                        .components()
                        .next()
                        .ok_or_else(|| anyhow!("invalid Nix source path"))?;
                    let store_root = Path::new("/nix/store").join(store_name);
                    backend::capture(
                        Command::new("nix-store")
                            .arg("--realise")
                            .arg(store_root)
                            .arg("--add-root")
                            .arg(session.directory.join("gcroots").join(alias))
                            .arg("--indirect"),
                    )?;
                }
                symlink(
                    &binding.store_path,
                    &session.directory.join("worktrees").join(alias),
                )?;
                symlink(
                    &binding.source_path,
                    &session.directory.join("src").join(alias),
                )?;
            }
            for alias in &editing {
                session.promote(alias)?;
            }
            session.write_cargo_config()?;
            session.ready = true;
            session.save()
        })();
        result.with_context(|| {
            format!(
                "opening {}; partial state retained there, no worktrees discarded",
                session.directory.display()
            )
        })?;
        Ok(session)
    }

    fn save(&self) -> Result<()> {
        atomic_write(
            &self.directory.join(STATE),
            &serde_json::to_vec_pretty(self)?,
        )
    }

    fn session_lock(&self) -> Result<File> {
        lock(&self.directory.join("session.lock"))
    }

    fn promote(&mut self, alias: &str) -> Result<()> {
        let binding = self
            .repositories
            .get(alias)
            .ok_or_else(|| anyhow!("unknown repository {alias:?}"))?;
        if binding.editable {
            return Ok(());
        }
        let source = GitSource::from_locked(&binding.locked)?;
        let copy_root_edits = binding.input == "self" && binding.locked.contains_key("dirtyRev");
        fs::create_dir_all(&self.cache)?;
        let identity = digest(&source.url);
        let _cache_lock = lock(&self.cache.join(format!("{identity}.lock")))?;
        let cache = self.cache.join(format!("{identity}.git"));
        if !cache.exists() {
            backend::capture(
                Command::new("git")
                    .args(["init", "--bare", "--quiet", "--"])
                    .arg(&cache),
            )?;
            backend::capture(Command::new("git").arg("--git-dir").arg(&cache).args([
                "remote",
                "add",
                "origin",
                &source.url,
            ]))?;
        } else {
            let origin = backend::capture(
                Command::new("git")
                    .arg("--git-dir")
                    .arg(&cache)
                    .args(["remote", "get-url", "origin"]),
            )?;
            ensure!(
                String::from_utf8(origin.stdout)?.trim() == source.url,
                "Git cache identity mismatch"
            );
        }
        let object = format!("{}^{{commit}}", source.revision);
        let present = Command::new("git")
            .arg("--git-dir")
            .arg(&cache)
            .args(["cat-file", "-e", &object])
            .output()?
            .status
            .success();
        let pin = format!("refs/weavie/pins/{}", source.revision);
        if !present {
            backend::capture(Command::new("git").arg("--git-dir").arg(&cache).args([
                "fetch",
                "--no-tags",
                "origin",
                &format!("{}:{pin}", source.revision),
            ]))?;
        }
        backend::capture(Command::new("git").arg("--git-dir").arg(&cache).args([
            "update-ref",
            &pin,
            &source.revision,
        ]))?;
        let destination = self.directory.join("worktrees").join(alias);
        ensure!(
            fs::read_link(&destination)? == binding.store_path,
            "read-only source link was changed; refusing to replace it"
        );
        fs::remove_file(&destination)?;
        let branch = format!(
            "weavie/{}/{alias}",
            &digest(&self.directory.to_string_lossy())[..16]
        );
        let added = backend::capture(
            Command::new("git")
                .arg("--git-dir")
                .arg(&cache)
                .args(["worktree", "add", "-b", &branch])
                .arg(&destination)
                .arg(&source.revision),
        );
        if let Err(error) = added {
            if !destination.exists() {
                symlink(&binding.store_path, &destination)?;
            }
            return Err(error);
        }
        // Record ownership before any optional post-checkout operations can fail.
        let binding = self
            .repositories
            .get_mut(alias)
            .expect("binding was validated");
        binding.editable = true;
        binding.source_path = destination.join(&source.directory);
        replace_link(
            &binding.source_path,
            &self.directory.join("src").join(alias),
        )?;
        self.save()?;
        if copy_root_edits {
            let head = backend::capture(
                Command::new("git")
                    .arg("-C")
                    .arg(&self.target.root)
                    .args(["rev-parse", "HEAD"]),
            )?;
            ensure!(
                String::from_utf8(head.stdout)?.trim() == source.revision,
                "composition HEAD changed while opening the session"
            );
            let diff = backend::capture(
                Command::new("git")
                    .arg("-C")
                    .arg(&self.target.root)
                    .args(["diff", "--binary", "HEAD"]),
            )?;
            use std::io::Write;
            use std::process::Stdio;
            let mut apply = Command::new("git")
                .arg("-C")
                .arg(&destination)
                .args(["apply", "--binary", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            apply
                .stdin
                .take()
                .ok_or_else(|| anyhow!("opening git apply input"))?
                .write_all(&diff.stdout)?;
            let applied = apply.wait_with_output()?;
            ensure!(
                applied.status.success(),
                "copying tracked root edits: {}",
                String::from_utf8_lossy(&applied.stderr)
            );
        }
        if source.submodules {
            backend::capture(Command::new("git").arg("-C").arg(&destination).args([
                "submodule",
                "update",
                "--init",
                "--recursive",
            ]))?;
        }
        if source.lfs {
            backend::capture(
                Command::new("git")
                    .arg("-C")
                    .arg(&destination)
                    .args(["lfs", "pull"]),
            )?;
        }
        Ok(())
    }

    pub fn edit(&mut self, alias: &str) -> Result<()> {
        let _lock = self.session_lock()?;
        *self = Self::load(&self.directory)?;
        ensure!(
            self.ready,
            "session creation is incomplete; inspect its recorded paths"
        );
        self.promote(alias)?;
        self.write_cargo_config()?;
        self.save()
    }

    pub fn effective_target(&self) -> Target {
        let mut target = self.target.clone();
        if let Some(root) = self.repositories.values().find(|b| b.input == "self") {
            target.root = if root.editable {
                root.source_path.clone()
            } else {
                root.store_path.clone()
            };
        }
        target
    }

    /// Freeze read-only sources and substitute live Git worktrees for editable inputs.
    pub fn overrides(&self) -> Result<Vec<(String, String)>> {
        self.repositories
            .values()
            .filter(|binding| binding.input != "self")
            .map(|binding| {
                let reference = if binding.editable {
                    let source = GitSource::from_locked(&binding.locked)?;
                    let worktree = binding
                        .source_path
                        .ancestors()
                        .nth(
                            Path::new(&source.directory)
                                .components()
                                .filter(|c| *c != Component::CurDir)
                                .count(),
                        )
                        .ok_or_else(|| anyhow!("invalid worktree source directory"))?;
                    let mut url = Url::from_directory_path(worktree)
                        .map_err(|_| anyhow!("invalid local worktree URL"))?;
                    if source.directory != "." {
                        url.query_pairs_mut().append_pair("dir", &source.directory);
                    }
                    if source.submodules {
                        url.query_pairs_mut().append_pair("submodules", "1");
                    }
                    if source.lfs {
                        url.query_pairs_mut().append_pair("lfs", "1");
                    }
                    format!("git+{url}")
                } else {
                    format!("path:{}", binding.store_path.display())
                };
                Ok((binding.input.clone(), reference))
            })
            .collect()
    }

    fn manifest(&self) -> PathBuf {
        self.repositories[&self.root_repository]
            .source_path
            .join(&self.cargo_root)
            .join("Cargo.toml")
    }

    pub fn cargo_config(&self) -> PathBuf {
        self.directory.join("cargo-config.toml")
    }

    pub fn cargo_home(&self) -> PathBuf {
        self.directory.join("cargo-home")
    }

    fn write_cargo_config(&self) -> Result<()> {
        let mut sources = BTreeMap::<String, BTreeMap<String, BTreeMap<String, String>>>::new();
        for (alias, binding) in &self.repositories {
            for (name, directory) in &binding.crates {
                relative_path(directory)?;
                let path = binding.source_path.join(directory);
                let manifest: toml::Value = toml::from_str(
                    &fs::read_to_string(path.join("Cargo.toml"))
                        .with_context(|| format!("reading crate {name} in {alias}"))?,
                )?;
                ensure!(
                    manifest
                        .get("package")
                        .and_then(|p| p.get("name"))
                        .and_then(toml::Value::as_str)
                        == Some(name),
                    "crate mapping {alias}/{directory} does not name package {name:?}"
                );
                for source in &binding.patch_sources {
                    let patch = sources.entry(source.clone()).or_default();
                    ensure!(
                        !patch.contains_key(name),
                        "multiple selected crates named {name:?} patch {source:?}; choose one instance per source"
                    );
                    patch.insert(
                        name.clone(),
                        BTreeMap::from([("path".into(), path.to_string_lossy().into_owned())]),
                    );
                }
            }
        }
        let configuration = BTreeMap::from([("patch", sources)]);
        atomic_write(
            &self.cargo_config(),
            toml::to_string(&configuration)?.as_bytes(),
        )?;
        // Cargo wrappers and build scripts can launch Cargo again without the
        // parent's --config arguments. Give those children the same patches.
        let home = self.cargo_home();
        fs::create_dir_all(&home)?;
        let inherited = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
        let mut inherited_config = toml::Value::Table(Default::default());
        if let Some(inherited) = inherited {
            let inherited = if inherited.is_absolute() {
                inherited
            } else {
                std::env::current_dir()?.join(inherited)
            };
            for name in ["config.toml", "config"] {
                if inherited.join(name).is_file() {
                    inherited_config = toml::from_str(&fs::read_to_string(inherited.join(name))?)?;
                    break;
                }
            }
            if inherited != home {
                for name in ["registry", "git", "credentials.toml", "credentials"] {
                    let source = inherited.join(name);
                    let destination = home.join(name);
                    if source.exists() && fs::symlink_metadata(&destination).is_err() {
                        symlink(&source, &destination)?;
                    }
                }
            }
        }
        let patch = inherited_config
            .as_table_mut()
            .ok_or_else(|| anyhow!("invalid inherited Cargo configuration"))?
            .entry("patch")
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .ok_or_else(|| anyhow!("invalid Cargo patch configuration"))?;
        let local = toml::Value::try_from(&configuration)?;
        for (source, entries) in local["patch"]
            .as_table()
            .ok_or_else(|| anyhow!("invalid generated patches"))?
        {
            let target = patch
                .entry(source)
                .or_insert_with(|| toml::Value::Table(Default::default()))
                .as_table_mut()
                .ok_or_else(|| anyhow!("invalid Cargo source patches"))?;
            target.extend(
                entries
                    .as_table()
                    .ok_or_else(|| anyhow!("invalid package patches"))?
                    .clone(),
            );
        }
        atomic_write(
            &home.join("config.toml"),
            toml::to_string(&inherited_config)?.as_bytes(),
        )
    }

    pub fn status(&self) -> Result<BTreeMap<String, RepositoryStatus>> {
        let mut result = BTreeMap::new();
        for (alias, binding) in &self.repositories {
            let (revision, changes) = if binding.editable {
                let revision = backend::capture(
                    Command::new("git")
                        .arg("-C")
                        .arg(&binding.source_path)
                        .args(["rev-parse", "HEAD"]),
                )?;
                let changes = backend::capture(
                    Command::new("git")
                        .arg("-C")
                        .arg(&binding.source_path)
                        .args(["status", "--short"]),
                )?;
                (
                    Some(String::from_utf8(revision.stdout)?.trim().into()),
                    Some(String::from_utf8(changes.stdout)?),
                )
            } else {
                (
                    binding
                        .locked
                        .get("rev")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    None,
                )
            };
            result.insert(
                alias.clone(),
                RepositoryStatus {
                    input: binding.input.clone(),
                    path: binding.source_path.clone(),
                    editable: binding.editable,
                    revision,
                    changes,
                },
            );
        }
        Ok(result)
    }

    pub fn build(&self, arguments: &[String]) -> Result<()> {
        let _lock = self.session_lock()?;
        ensure!(self.ready, "session creation is incomplete");
        let target = self.effective_target();
        backend::run(
            backend::apply_overrides(
                backend::nix().args(["build", &target.installable(), "--no-link"]),
                &self.overrides()?,
            )
            .args(arguments),
        )
    }

    /// Run Cargo with every selected source patched in, optionally inside Nix's dev shell.
    fn cargo_command(&self, native: bool) -> Result<Command> {
        let manifest = self.manifest();
        let directory = manifest
            .parent()
            .ok_or_else(|| anyhow!("invalid Cargo root"))?;
        let mut command = if native {
            Command::new("cargo")
        } else {
            let target = self.effective_target();
            let mut command = backend::nix();
            command.args(["develop", &target.installable()]);
            backend::apply_overrides(&mut command, &self.overrides()?);
            command.args(["--command", "cargo"]);
            command
        };
        command
            .current_dir(directory)
            .env("CARGO_TARGET_DIR", self.directory.join("target"))
            .env("CARGO_HOME", self.cargo_home())
            .arg("--config")
            .arg(self.cargo_config());
        Ok(command)
    }

    pub fn cargo(&self, arguments: &[String], native: bool) -> Result<()> {
        let _lock = self.session_lock()?;
        ensure!(self.ready, "session creation is incomplete");
        self.write_cargo_config()?;
        let mut metadata = self.cargo_command(native)?;
        metadata.args(["metadata", "--format-version", "1"]);
        let mut flags = arguments.iter();
        while let Some(flag) = flags.next() {
            if [
                "--offline",
                "--locked",
                "--frozen",
                "--all-features",
                "--no-default-features",
            ]
            .contains(&flag.as_str())
                || flag.starts_with("--features=")
            {
                metadata.arg(flag);
            } else if flag == "--features" || flag == "-F" {
                metadata.arg(flag).arg(
                    flags
                        .next()
                        .ok_or_else(|| anyhow!("missing Cargo features"))?,
                );
            }
        }
        let output = backend::capture(&mut metadata)?;
        let metadata: Value = serde_json::from_slice(&output.stdout)?;
        for binding in self.repositories.values() {
            for (name, relative) in &binding.crates {
                let expected = binding
                    .source_path
                    .join(relative)
                    .join("Cargo.toml")
                    .canonicalize()?;
                for package in metadata["packages"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Cargo metadata lacks packages"))?
                {
                    if package["name"].as_str() == Some(name) {
                        let actual = Path::new(
                            package["manifest_path"]
                                .as_str()
                                .ok_or_else(|| anyhow!("Cargo package lacks manifest path"))?,
                        )
                        .canonicalize()?;
                        ensure!(
                            actual == expected,
                            "Cargo resolved {name:?} outside the selected source: {}; check patchSources, version constraints and Cargo.lock",
                            actual.display()
                        );
                    }
                }
            }
        }
        backend::run(self.cargo_command(native)?.args(arguments))
    }

    pub fn graph(&self) -> Result<Graph> {
        self.effective_target().graph(&self.overrides()?)
    }
}
