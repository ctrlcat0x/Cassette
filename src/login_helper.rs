//! Runs before GPUI, in a separate short-lived process with no application IPC on remote pages.
use anyhow::{Context, Result};
use raag::auth::{
    BrowserCookie, CookieJar, LoginReply, SessionMaterial, SignInFailure, completion_url,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::run_return::EventLoopExtRunReturn,
    window::WindowBuilder,
};
use wry::{WebContext, WebViewBuilder};

const CAPTURE_INTERVAL: Duration = Duration::from_millis(750);
pub(crate) const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn run(
    profile: &std::path::Path,
    parent_closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<LoginReply> {
    let mut context = WebContext::new(Some(profile.to_owned()));
    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music · Cassette")
        .with_inner_size(tao::dpi::LogicalSize::new(520., 760.))
        .build(&event_loop)?;
    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_incognito(true)
        .with_devtools(false)
        .with_url("https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F")
        .with_navigation_handler(|url| reqwest::Url::parse(&url).is_ok_and(|u| {
            u.as_str() == "about:blank" || (u.scheme() == "https" && matches!(u.host_str(),
                Some("accounts.google.com" | "music.youtube.com" | "accounts.youtube.com" |
                "consent.youtube.com" | "www.youtube.com" | "youtube.com" | "www.google.com" |
                "myaccount.google.com" | "consent.google.com" | "policies.google.com")))
        }))
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let webview = builder
        .build(&window)
        .context("Could not start the system login browser")?;
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(
            window
                .default_vbox()
                .context("Login requires a GTK container")?,
        )?
    };
    let started = Instant::now();
    let mut next_capture = Instant::now();
    let mut landing = None;
    let mut captured = None;
    let (validated, validation) =
        mpsc::sync_channel::<(Instant, Result<SessionMaterial, SignInFailure>)>(1);
    let mut checking = false;
    let mut gate = ValidationGate::new(Instant::now());
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(next_capture);
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
            return;
        }
        if parent_closed.load(std::sync::atomic::Ordering::Acquire) {
            *control_flow = ControlFlow::Exit;
            return;
        }
        if started.elapsed() >= LOGIN_TIMEOUT {
            let failure = if landing.is_some() {
                SignInFailure::Timeout
            } else {
                SignInFailure::VerificationTimeout
            };
            captured = Some(LoginReply::Failed(failure));
            *control_flow = ControlFlow::Exit;
            return;
        }
        // Check the actual destination even if the document never finishes loading.
        // Cookie APIs run on the event loop, outside WebView callbacks.
        if !matches!(event, Event::MainEventsCleared) || Instant::now() < next_capture {
            return;
        }
        let now = Instant::now();
        next_capture = now + CAPTURE_INTERVAL;
        *control_flow = ControlFlow::WaitUntil(next_capture);
        let url = webview.url().unwrap_or_default();
        let on_music = completion_url(&url);
        if on_music {
            if landing.is_none() {
                landing = Some(now);
                gate = ValidationGate::new(now);
                window.set_title("Finishing YouTube Music sign-in · Cassette");
            }
        } else if landing.take().is_some() {
            window.set_title("Sign in to YouTube Music · Cassette");
        }
        if let Ok((checked_landing, result)) = validation.try_recv() {
            checking = false;
            if landing != Some(checked_landing) {
                return;
            }
            match result {
                Ok(material) => {
                    captured = Some(LoginReply::Validated(material));
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                Err(error) if error.retryable() && gate.attempts < 3 => {
                    window.set_title("Finishing YouTube Music sign-in · Cassette");
                }
                Err(error) => {
                    captured = Some(LoginReply::Failed(error));
                    *control_flow = ControlFlow::Exit;
                    return;
                }
            }
        }
        if !checking && let Some(landed_at) = landing {
            if now.duration_since(landed_at) >= Duration::from_secs(60) {
                captured = Some(LoginReply::Failed(SignInFailure::CookieCapture));
                *control_flow = ControlFlow::Exit;
                return;
            }
            if let Ok(cookies) = webview.cookies() {
                let cookies: Vec<_> = cookies
                    .into_iter()
                    .filter(|c| {
                        raag::auth::youtube_domain(c.domain().unwrap_or("").trim_start_matches('.'))
                    })
                    .map(|c| BrowserCookie {
                        name: c.name().into(),
                        value: c.value().into(),
                        domain: c
                            .domain()
                            .unwrap_or("")
                            .trim_start_matches('.')
                            .to_ascii_lowercase(),
                        path: c.path().unwrap_or("/").into(),
                        secure: c.secure().unwrap_or(false),
                        http_only: c.http_only().unwrap_or(false),
                        expires: c.expires_datetime().map(|date| date.unix_timestamp()),
                    })
                    .collect();
                if let Ok(jar) = CookieJar::new(cookies) {
                    // Unrelated cookie rotation should not postpone account validation.
                    let fingerprint = raag::cache::key(jar.auth_value().unwrap_or_default());
                    if gate.ready(&fingerprint, Instant::now()) {
                        checking = true;
                        window.set_title("Checking YouTube Music account · Cassette");
                        let validated = validated.clone();
                        // Native cookie APIs stay on this event loop; HTTP validation never does.
                        std::thread::spawn(move || {
                            let result = raag::catalog::Catalog::validate(jar, None).map_err(|e| {
                                e.downcast_ref::<SignInFailure>()
                                    .copied()
                                    .unwrap_or(SignInFailure::InvalidResponse)
                            });
                            let _ = validated.send((landed_at, result));
                        });
                    }
                }
            }
        }
    });
    drop(webview);
    drop(window);
    drop(context);
    if let Some(reply) = captured {
        Ok(reply)
    } else {
        Err(raag::auth::LoginCanceled.into())
    }
}

/// Require cookies to settle, and cap account requests even when the page keeps emitting events.
struct ValidationGate {
    fingerprint: String,
    stable_since: Instant,
    next: Instant,
    attempts: u8,
}
impl ValidationGate {
    fn new(now: Instant) -> Self {
        Self {
            fingerprint: String::new(),
            stable_since: now,
            next: now,
            attempts: 0,
        }
    }
    fn ready(&mut self, fingerprint: &str, now: Instant) -> bool {
        if self.fingerprint != fingerprint {
            self.fingerprint = fingerprint.into();
            self.stable_since = now;
        }
        if now.duration_since(self.stable_since) < Duration::from_millis(1500)
            || now < self.next
            || self.attempts >= 3
        {
            return false;
        }
        self.attempts += 1;
        self.next = now + Duration::from_secs(2);
        true
    }
}

pub fn main() -> i32 {
    use std::io::{Read, Write};
    let Some(profile) = std::env::args_os().nth(2) else {
        return 2;
    };
    // EOF is also delivered when the parent crashes; no orphan login window survives it.
    let parent_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = parent_closed.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read(&mut [0u8; 1]);
        signal.store(true, std::sync::atomic::Ordering::Release);
    });
    match run(std::path::Path::new(&profile), parent_closed) {
        Ok(reply) => {
            // stdout is an inherited private anonymous pipe, never a file/argument/renderer message.
            let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&reply).unwrap_or_default());
            let mut output = std::io::stdout().lock();
            if bytes.len() as u64 <= raag::auth::SESSION_LIMIT
                && output
                    .write_all(&(bytes.len() as u32).to_le_bytes())
                    .is_ok()
                && output.write_all(&bytes).is_ok()
                && output.flush().is_ok()
            {
                0
            } else {
                1
            }
        }
        Err(error) if error.is::<raag::auth::LoginCanceled>() => 2,
        Err(_) => 3, // Do not emit remote URLs, page content or cookies to stderr.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_waits_for_stable_credentials_and_bounds_retries() {
        let start = Instant::now();
        let mut gate = ValidationGate::new(start);
        assert!(!gate.ready("first", start));
        assert!(!gate.ready("updated", start + CAPTURE_INTERVAL));
        assert!(!gate.ready("updated", start + 2 * CAPTURE_INTERVAL));
        let settled = start + 3 * CAPTURE_INTERVAL;
        assert!(gate.ready("updated", settled));
        assert!(!gate.ready("updated", settled + CAPTURE_INTERVAL));
        assert!(gate.ready("updated", settled + Duration::from_secs(2)));
        assert!(gate.ready("updated", settled + Duration::from_secs(4)));
        assert!(!gate.ready("updated", settled + Duration::from_secs(6)));
    }

    #[test]
    fn account_challenges_are_not_treated_as_music_landings() {
        assert!(!completion_url(
            "https://accounts.google.com/v3/signin/challenge"
        ));
        assert!(!completion_url("https://music.youtube.com.example.org/"));
        assert!(!completion_url("https://music.youtube.com:8443/"));
        assert!(completion_url("https://music.youtube.com/"));
    }
}
