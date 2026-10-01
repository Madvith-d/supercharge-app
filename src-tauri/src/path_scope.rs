//! Unified path allowlist for absolute filesystem access.
//!
//! Used by loopback media HTTP / legacy `media://` (SEC-01) and
//! `fs_read_absolute` / `fs_write_absolute` (SEC-09).
//! Only trusted project roots, the App data root, system temp, and explicitly
//! granted one-off paths may be read/written.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::{Mutex, RwLock};

#[derive(Default)]
struct ScopeRoots {
    paths: Vec<PathBuf>,
    store_generation: u64,
    next_refresh: u64,
    published_refresh: u64,
}

impl ScopeRoots {
    fn begin_refresh(&mut self) -> (u64, u64) {
        self.next_refresh += 1;
        (self.store_generation, self.next_refresh)
    }

    fn publish(&mut self, generation: (u64, u64), next: Vec<PathBuf>) -> bool {
        if generation.0 != self.store_generation {
            return false;
        }
        if generation.1 > self.published_refresh {
            self.paths = next;
            self.published_refresh = generation.1;
        }
        true
    }
}

fn roots() -> &'static RwLock<ScopeRoots> {
    static R: OnceLock<RwLock<ScopeRoots>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(ScopeRoots::default()))
}

fn project_mutations() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

/// Serialize project writes without holding the authorization lock during I/O.
pub(crate) fn with_project_store_mutation<T>(mutation: impl FnOnce() -> T) -> T {
    mutate_project_store(roots(), project_mutations(), mutation)
}

fn mutate_project_store<T>(
    scope: &RwLock<ScopeRoots>,
    mutations: &Mutex<()>,
    mutation: impl FnOnce() -> T,
) -> T {
    let _guard = mutations.lock();
    scope.write().store_generation += 1;
    let result = mutation();
    scope.write().store_generation += 1;
    result
}

fn refresh_roots(
    scope: &RwLock<ScopeRoots>,
    mutations: &Mutex<()>,
    build: impl Fn() -> Vec<PathBuf>,
) {
    loop {
        let generation = {
            let _guard = mutations.lock();
            scope.write().begin_refresh()
        };
        let next = build();
        if scope.write().publish(generation, next) {
            return;
        }
        // A store write overtook this snapshot; rebuild synchronously.
    }
}

fn trusted_project_roots(
    projects: Vec<crate::store::Project>,
    mut canonicalize: impl FnMut(&Path) -> Option<PathBuf>,
) -> Vec<PathBuf> {
    projects
        .into_iter()
        .filter(crate::store::Project::is_trusted_local)
        .filter_map(|p| canonicalize(Path::new(&p.path)))
        .collect()
}

fn extra_grants() -> &'static RwLock<Vec<PathBuf>> {
    static G: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();
    G.get_or_init(|| RwLock::new(Vec::new()))
}

/// Rebuild allowlisted roots from the project store + app data + temp.
/// Call on startup and whenever projects are added / removed / relocated / trusted.
pub fn refresh_from_store() {
    refresh_roots(roots(), project_mutations(), collect_roots);
}

fn collect_roots() -> Vec<PathBuf> {
    let mut next = trusted_project_roots(crate::store::load_projects_metadata(), |path| {
        path.canonicalize().ok()
    });

    if let Ok(app) = crate::paths::app_data_root().canonicalize() {
        next.push(app);
    } else {
        // Dir may not exist yet — still allow the logical root.
        next.push(crate::paths::app_data_root());
    }

    if let Ok(tmp) = std::env::temp_dir().canonicalize() {
        next.push(tmp);
    } else {
        next.push(std::env::temp_dir());
    }

    // Agent media lives under SUPERCHARGE_HOME (or the independent app home).
    // Resolve without initializing directories or migrating settings.
    let session_data_mode = crate::store::load_path_scope_session_data_mode();
    let agent_home = if session_data_mode.trim().eq_ignore_ascii_case("shared") {
        crate::paths::shared_supercharge_home()
    } else {
        crate::paths::agent_home_dir()
    };
    if let Ok(c) = agent_home.canonicalize() {
        next.push(c);
    } else {
        next.push(agent_home);
    }

    // CLI default home (`~/.grok`): marketplace-cache logos + installed-plugins
    // assets for Settings → Extensions cards (may differ from independent agent-home).
    let user_grok = crate::process_util::user_home().join(".grok");
    if let Ok(c) = user_grok.canonicalize() {
        next.push(c);
    } else {
        next.push(user_grok);
    }

    // User-level agent instructions (`~/.agents/AGENTS.md`).
    let user_agents = crate::process_util::user_home().join(".agents");
    if let Ok(c) = user_agents.canonicalize() {
        next.push(c);
    } else {
        next.push(user_agents);
    }

    // Dedup while preserving order.
    let mut seen = std::collections::HashSet::new();
    next.retain(|p| seen.insert(p.clone()));

    next
}

/// Grant a one-off absolute path (e.g. user-picked file outside projects).
///
/// Files are granted **exactly** — never the parent directory. Granting the
/// parent used to let loopback media serve siblings (e.g. `~/.ssh/id_rsa`
/// classified from journal history also unlocked `id_rsa.pub`). Re-reads of
/// the same file still work because `is_allowed` treats the grant as a root
/// that matches that path.
pub fn grant_path(path: &Path) {
    let grant = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut g = extra_grants().write();
    if !g.iter().any(|x| x == &grant) {
        g.push(grant);
        // Cap grants so a long-running process cannot grow unbounded.
        const MAX_GRANTS: usize = 256;
        if g.len() > MAX_GRANTS {
            let drain = g.len() - MAX_GRANTS;
            g.drain(0..drain);
        }
    }
}

/// True when `path` sits under an allowed root (after canonicalize when possible).
pub fn is_allowed(path: &Path) -> bool {
    let candidate = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    is_allowed_canonical(&candidate)
}

fn is_allowed_canonical(path: &Path) -> bool {
    if roots().read().paths.is_empty() {
        // Lazy init on first check (tests / early calls before setup).
        refresh_from_store();
    }
    let under_root = roots()
        .read()
        .paths
        .iter()
        .any(|r| path_under_root(path, r));
    if under_root {
        return true;
    }
    extra_grants()
        .read()
        .iter()
        .any(|r| path_under_root(path, r))
}

fn path_under_root(path: &Path, root: &Path) -> bool {
    if path == root {
        return true;
    }
    // Use component-wise prefix so `/foo` does not match `/foobar`.
    let mut path_comps = path.components();
    for rc in root.components() {
        match path_comps.next() {
            Some(pc) if pc == rc => {}
            _ => return false,
        }
    }
    true
}

/// Canonicalize + allowlist gate. Returns the canonical path on success.
pub fn require_allowed(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("path not found: {e}"))?;
    if !is_allowed_canonical(&canonical) {
        tracing::warn!(
            path = %canonical.display(),
            "path_scope: denied absolute path outside allowlisted roots"
        );
        return Err("path not allowed: outside trusted project or app data roots".into());
    }
    Ok(canonical)
}

/// Serializes tests that mutate the process-global roots/grants.
/// `tokio::sync` so async media_server tests can hold it across `.await`.
#[cfg(test)]
pub(crate) static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
#[path = "path_scope/tests.rs"]
mod tests;
