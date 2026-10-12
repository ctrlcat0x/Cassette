//! Background account lifecycle and cancelable, bounded login-helper IPC.
use anyhow::{Context, Result, bail};
use raag::auth::{Account, ActiveSession, CookieJar, LoginReply, SessionMaterial, SignInFailure};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Snapshot {
    pub generation: u64,
    pub session: Option<Arc<ActiveSession>>,
}
impl Snapshot {
    pub fn scope(&self) -> String {
        self.session
            .as_ref()
            .map_or_else(|| "guest".into(), |s| s.account().cache_scope())
    }
}
pub enum AuthEvent {
    Changed(u64, Option<String>),
    Finished(u64, Result<(), String>),
}
#[derive(Debug)]
struct BrowserUnavailable;
impl std::fmt::Display for BrowserUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("System login browser unavailable")
    }
}
impl std::error::Error for BrowserUnavailable {}
struct Shared {
    snapshot: Mutex<Snapshot>,
    attempt: AtomicU64,
    operation: Mutex<()>,
    validation: Mutex<()>,
    browser: Mutex<()>,
}
#[derive(Clone)]
pub struct SessionAccess(Arc<Shared>);
impl SessionAccess {
    pub fn snapshot(&self) -> Snapshot {
        self.0.snapshot.lock().unwrap().clone()
    }
    pub fn current(&self, generation: u64) -> bool {
        self.0.snapshot.lock().unwrap().generation == generation
    }
}
pub struct AuthController {
    access: SessionAccess,
    events: Receiver<AuthEvent>,
    sender: SyncSender<AuthEvent>,
    busy: bool,
    deleting: bool,
    deletion_failed: bool,
    cleanup_scope: Option<String>,
    last_persist: Instant,
}
impl AuthController {
    pub fn new() -> Self {
        let access = SessionAccess(Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                generation: 0,
                session: None,
            }),
            attempt: AtomicU64::new(1),
            operation: Mutex::new(()),
            validation: Mutex::new(()),
            browser: Mutex::new(()),
        }));
        let (sender, events) = mpsc::sync_channel(8);
        let this = Self {
            access,
            events,
            sender,
            busy: true,
            deleting: false,
            deletion_failed: false,
            cleanup_scope: None,
            last_persist: Instant::now(),
        };
        this.spawn(1, "Saved YouTube session could not be restored. Sign in again; your local library is unchanged.", |shared, epoch| {
            let Some(bytes) = raag::auth::storage::load()
                .context("Could not restore the protected YouTube session")?
            else {
                return Ok(None);
            };
            let material: SessionMaterial =
                serde_json::from_slice(&bytes).context("Saved YouTube session is invalid")?;
            if raag::cache::offline_only() { return commit(shared, epoch, material); }
            let material =
                validate_candidate(shared, epoch, material.cookies.clone(), Some(&material))
                    .context("YouTube session needs sign-in again")?;
            commit(shared, epoch, material)
        });
        this
    }
    pub fn access(&self) -> SessionAccess {
        self.access.clone()
    }
    pub fn snapshot(&self) -> Snapshot {
        self.access.snapshot()
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    pub fn can_cancel(&self) -> bool {
        self.busy && !self.deleting
    }
    pub fn can_sign_out(&self) -> bool {
        self.snapshot().session.is_some() || self.deletion_failed
    }
    pub fn shutdown(&mut self) {
        self.access.0.attempt.fetch_add(1, Ordering::AcqRel);
        let shared = self.access.0.clone();
        let delete = self.deleting || self.deletion_failed;
        let (done, completed) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _operation = shared.operation.lock().unwrap();
            if delete {
                let _ = raag::auth::storage::delete();
            } else {
                let session = shared.snapshot.lock().unwrap().session.clone();
                if let Some(session) = session {
                    let _ = session.persist();
                }
            }
            let _ = done.send(());
        });
        // Only used after the window closes, never during rendering or interaction.
        let _ = completed.recv_timeout(Duration::from_secs(2));
    }
    pub fn account(&self) -> Option<Account> {
        self.snapshot().session.map(|s| s.account())
    }
    pub fn poll(&mut self) -> Option<AuthEvent> {
        while let Ok(event) = self.events.try_recv() {
            if let AuthEvent::Finished(epoch, result) = &event {
                if *epoch != self.access.0.attempt.load(Ordering::Acquire) {
                    continue;
                }
                self.busy = false;
                if self.deleting {
                    self.deletion_failed = result.is_err();
                    if result.is_ok() {
                        self.cleanup_scope = None;
                    }
                    self.deleting = false;
                }
            }
            if let AuthEvent::Changed(generation, _) = &event
                && !self.access.current(*generation)
            {
                continue;
            }
            return Some(event);
        }
        // Cookie rotation persistence is debounced and always off the UI thread.
        if !self.busy && self.last_persist.elapsed() > Duration::from_secs(60) {
            self.last_persist = Instant::now();
            let shared = self.access.0.clone();
            thread::spawn(move || {
                let _operation = shared.operation.lock().unwrap();
                let session = shared.snapshot.lock().unwrap().session.clone();
                if let Some(session) = session {
                    let _ = session.persist();
                }
            });
        }
        None
    }
    fn spawn(
        &self,
        epoch: u64,
        failure_message: &'static str,
        task: impl FnOnce(&Shared, u64) -> Result<Option<String>> + Send + 'static,
    ) {
        let shared = self.access.0.clone();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = task(&shared, epoch);
            if shared.attempt.load(Ordering::Acquire) != epoch {
                return;
            }
            match result {
                Ok(notice) => {
                    let generation = shared.snapshot.lock().unwrap().generation;
                    let _ = sender.send(AuthEvent::Changed(generation, notice));
                    let _ = sender.send(AuthEvent::Finished(epoch, Ok(())));
                }
                Err(error) if error.is::<raag::auth::LoginCanceled>() => {
                    let generation = shared.snapshot.lock().unwrap().generation;
                    let _ = sender.send(AuthEvent::Changed(
                        generation,
                        Some("Sign-in canceled".into()),
                    ));
                    let _ = sender.send(AuthEvent::Finished(epoch, Ok(())));
                }
                Err(error) if error.is::<BrowserUnavailable>() => {
                    let message = if cfg!(target_os = "windows") {
                        "The login browser could not start. Install or repair Microsoft Edge WebView2 Runtime, then retry. Cookie import is also available."
                    } else if cfg!(target_os = "linux") {
                        "The login browser could not start. Check that WebKitGTK 4.1 and a graphical desktop session are available. Cookie import is also available."
                    } else {
                        "The system login browser could not start. Retry or import a fresh YouTube cookie file."
                    };
                    let _ = sender.send(AuthEvent::Finished(epoch, Err(message.into())));
                }
                Err(error) if error.downcast_ref::<SignInFailure>().is_some() => {
                    let failure = *error.downcast_ref::<SignInFailure>().unwrap();
                    record_failure(failure);
                    let _ = sender.send(AuthEvent::Finished(
                        epoch,
                        Err(format!(
                            "{failure} Your previous session was kept. [{}]",
                            failure.code()
                        )),
                    ));
                }
                Err(_) => {
                    let _ = sender.send(AuthEvent::Finished(epoch, Err(failure_message.into())));
                }
            }
        });
    }
    pub fn sign_in(&mut self, imported: Option<PathBuf>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let epoch = self.access.0.attempt.fetch_add(1, Ordering::AcqRel) + 1;
        self.spawn(epoch, "Could not sign in. Complete Google sign-in and select a YouTube channel, or import a fresh cookie file. Your previous session was kept.", move |shared, epoch| {
            let material = match imported {
                Some(path) => {
                    let cookies = CookieJar::import(&path).map_err(|_| SignInFailure::CookieImport)?;
                    validate_candidate(shared, epoch, cookies, None)?
                }
                None => capture(shared, epoch)?,
            };
            if shared.attempt.load(Ordering::Acquire) != epoch {
                bail!("Canceled");
            }
            commit(shared, epoch, material)
        });
    }
    pub fn cancel(&mut self) {
        if self.deleting {
            return;
        }
        self.access.0.attempt.fetch_add(1, Ordering::AcqRel);
        // The worker notices cancellation within 100ms and reaps its helper.
        self.busy = false;
    }
    pub fn sign_out(&mut self) {
        if self.deleting {
            return;
        }
        self.cancel();
        if self.account().is_some() {
            self.cleanup_scope = Some(self.snapshot().scope());
        }
        let cleanup_scope = self.cleanup_scope.clone();
        let shared = self.access.0.clone();
        {
            let mut snapshot = shared.snapshot.lock().unwrap();
            snapshot.generation += 1;
            snapshot.session = None;
        }
        self.busy = true;
        self.deleting = true;
        let epoch = shared.attempt.load(Ordering::Acquire);
        self.spawn(
            epoch,
            "Sign-out cleanup is incomplete. Retry Sign out before closing Cassette.",
            move |shared, epoch| {
                let _operation = shared.operation.lock().unwrap();
                let _ = epoch;
                raag::auth::storage::delete().context("Could not remove protected session")?;
                if let Some(scope) = cleanup_scope {
                    // Scope is our SHA-1 cache key, never a server-provided path.
                    if scope.len() == 40 && scope.bytes().all(|b| b.is_ascii_hexdigit()) {
                        let path = raag::library::data_dir().join("pages").join(scope);
                        match std::fs::remove_dir_all(path) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                Ok(Some("Signed out of YouTube Music".into()))
            },
        );
    }
}
impl Drop for AuthController {
    fn drop(&mut self) {
        self.access.0.attempt.fetch_add(1, Ordering::AcqRel);
    }
}

fn commit(shared: &Shared, epoch: u64, material: SessionMaterial) -> Result<Option<String>> {
    let session = Arc::new(ActiveSession::new(material).map_err(|_| SignInFailure::SessionExport)?);
    let _operation = shared.operation.lock().unwrap();
    if shared.attempt.load(Ordering::Acquire) != epoch {
        bail!("Canceled");
    }
    let previous = shared.snapshot.lock().unwrap().session.clone();
    let warning = session.persist().err().map(|_| "Signed in for this session. Protected storage is unavailable; this sign-in will not survive restarting Cassette.".into());
    let mut snapshot = shared.snapshot.lock().unwrap();
    if shared.attempt.load(Ordering::Acquire) != epoch {
        // A canceled save must not survive restart.
        drop(snapshot);
        if let Some(previous) = previous {
            let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&previous.material())?);
            raag::auth::storage::save(&bytes)?;
        } else {
            raag::auth::storage::delete()?;
        }
        bail!("Canceled");
    }
    snapshot.generation += 1;
    snapshot.session = Some(session);
    Ok(warning)
}

fn claim<'a>(
    shared: &Shared,
    epoch: u64,
    slot: &'a Mutex<()>,
) -> Result<std::sync::MutexGuard<'a, ()>> {
    loop {
        if shared.attempt.load(Ordering::Acquire) != epoch {
            return Err(raag::auth::LoginCanceled.into());
        }
        match slot.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(50)),
            Err(_) => bail!("Authentication worker unavailable"),
        }
    }
}
fn validate_candidate(
    shared: &Shared,
    epoch: u64,
    cookies: CookieJar,
    previous: Option<&SessionMaterial>,
) -> Result<SessionMaterial> {
    let _validation = claim(shared, epoch, &shared.validation)?;
    raag::catalog::Catalog::validate(cookies, previous)
}
fn capture(shared: &Shared, epoch: u64) -> Result<SessionMaterial> {
    let _browser = claim(shared, epoch, &shared.browser)?;
    // Parent owns cleanup, including cancellation/forced helper termination.
    let profile = tempfile::Builder::new()
        .prefix("raag-login-")
        .tempdir()
        .map_err(|_| SignInFailure::SessionExport)?;
    raag::auth::storage::restrict(profile.path()).map_err(|_| SignInFailure::SessionExport)?;
    let mut command = Command::new(std::env::current_exe().map_err(|_| BrowserUnavailable)?);
    command
        .arg("--login-helper")
        .arg(profile.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|_| BrowserUnavailable)?;
    let stdout = child.stdout.take().ok_or(SignInFailure::Handoff)?;
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = raag::auth::read_login_message(stdout);
        let _ = send.send(result);
    });
    let started = Instant::now();
    loop {
        if shared.attempt.load(Ordering::Acquire) != epoch
            || started.elapsed() > Duration::from_secs(16 * 60)
        {
            drop(child.stdin.take()); // EOF requests graceful teardown of the native browser.
            let deadline = Instant::now() + Duration::from_secs(2);
            while child.try_wait()?.is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("Sign-in canceled");
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    if status.code() == Some(2) {
                        return Err(raag::auth::LoginCanceled.into());
                    }
                    return Err(if status.code() == Some(3) {
                        BrowserUnavailable.into()
                    } else {
                        SignInFailure::Handoff.into()
                    });
                }
                let bytes = receive
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|_| SignInFailure::Handoff)?
                    .map_err(|_| SignInFailure::Handoff)?;
                return decode_reply(&bytes);
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    }
}

fn decode_reply(bytes: &[u8]) -> Result<SessionMaterial> {
    match serde_json::from_slice(bytes).map_err(|_| SignInFailure::Handoff)? {
        LoginReply::Failed(failure) => Err(failure.into()),
        LoginReply::Validated(mut material) => {
            material.cookies = CookieJar::new(material.cookies.into_cookies())
                .map_err(|_| SignInFailure::CookieCapture)?;
            if material.account.name.trim().is_empty() || material.account.channel_id.is_empty() {
                return Err(SignInFailure::IncompleteAccount.into());
            }
            Ok(material)
        }
    }
}

fn record_failure(failure: SignInFailure) {
    // One bounded record, fixed categories only. Never serialize an anyhow error chain here.
    let status = match failure {
        SignInFailure::Http(status) => Some(status),
        _ => None,
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diagnostic =
        serde_json::json!({"stage": failure.code(), "http_status": status, "timestamp": timestamp});
    let directory = raag::library::data_dir();
    if std::fs::create_dir_all(&directory).is_ok() {
        let _ = std::fs::write(
            directory.join("sign-in-diagnostic.json"),
            diagnostic.to_string(),
        );
    }
}
