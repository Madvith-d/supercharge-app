use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Context {
    root: PathBuf,
    legacy_source: Option<PathBuf>,
}

pub(super) fn effective_root(current: Option<&OsStr>, legacy: Option<&OsStr>) -> PathBuf {
    current
        .filter(|value| !value.is_empty())
        .or_else(|| legacy.filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(super::default_app_data_root)
}

impl Context {
    fn capture() -> io::Result<Self> {
        let current = std::env::var_os("SUPERCHARGE_APP_HOME");
        let legacy = std::env::var_os("GROK_APP_HOME");
        let root = std::path::absolute(effective_root(current.as_deref(), legacy.as_deref()))?;
        // Even an empty override disables importing default user data.
        let legacy_source = if current.is_none() && legacy.is_none() {
            Some(std::path::absolute(super::legacy_app_data_root())?)
        } else {
            None
        };
        Ok(Self {
            root,
            legacy_source,
        })
    }
}

#[derive(Eq, PartialEq)]
struct RootIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

impl RootIdentity {
    fn read(root: &std::path::Path) -> io::Result<Self> {
        let metadata = fs::metadata(root)?;
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "app root is not a directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                created: metadata.created().ok(),
            })
        }
    }
}

#[derive(Default)]
struct State {
    completed: HashMap<Context, RootIdentity>,
    #[cfg(test)]
    attempts: HashMap<Context, usize>,
}

#[derive(Default)]
struct Initializer {
    state: Mutex<State>,
}

const REQUIRED_DIRS: &[&str] = &[
    "projects",
    "sessions",
    "logs",
    "agent-home",
    "attachments/paste",
    "accounts",
    "workspaces/general",
    "wallpapers/x",
    "wallpapers/imagine",
    "wallpapers/library",
    "cache/video-posters",
    "cache/image-thumbs",
    "plugin-data",
    "skin-presets/.staging/inspect",
    "skin-presets/.staging/upload",
    "skin-catalog-cache",
];

impl Initializer {
    fn ensure(&self, context: Context, repair: bool) -> io::Result<PathBuf> {
        // Only directory setup and legacy file copying run under this lock.
        // Store/settings migrations run after returning, and may reenter setup.
        let mut state = self.state.lock();
        if !repair {
            if let Some(identity) = state.completed.get(&context) {
                if RootIdentity::read(&context.root).as_ref().ok() == Some(identity) {
                    return Ok(context.root);
                }
            }
        }
        state.completed.remove(&context);
        #[cfg(test)]
        {
            *state.attempts.entry(context.clone()).or_default() += 1;
        }
        // Best-effort migration failures must not suppress a later retry.
        let migration = match &context.legacy_source {
            Some(source) => {
                super::legacy_app_data_migration::migrate_legacy_app_data(source, &context.root)
            }
            None => Ok(()),
        };
        for relative in REQUIRED_DIRS {
            fs::create_dir_all(context.root.join(relative))?;
        }
        if migration.is_ok() {
            let identity = RootIdentity::read(&context.root)?;
            state.completed.insert(context.clone(), identity);
        }
        Ok(context.root)
    }
}

static INITIALIZER: OnceLock<Initializer> = OnceLock::new();

pub(super) fn ensure(repair: bool) -> io::Result<PathBuf> {
    INITIALIZER
        .get_or_init(Initializer::default)
        .ensure(Context::capture()?, repair)
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
