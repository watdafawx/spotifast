//! The mods window: a webview in its own process (fork only).
//!
//! Lines on stdin are state from the app, handed to the page's
//! `window.__spotifast.receive`. Messages the page posts with
//! `window.ipc.postMessage` go to stdout, one JSON object per line. The page
//! and its files come from the mods folder through the `mods` scheme; with
//! no `index.html` there, the built-in loader runs the Spicetify custom app
//! it finds, or shows the demo page when there is none.

use std::borrow::Cow;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::raw_window_handle::{RawWindowHandle, Win32WindowHandle};
use winit::window::{Window, WindowId};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{WebContext, WebView, WebViewBuilder};

const SHIM: &str = include_str!("shim.js");
const LOADER: &str = include_str!("loader.html");
const DEMO: &str = include_str!("demo.html");
/// Settings for the page's storage, carried over from another client (such
/// as Spotify with Spicetify): applied by the loader once, then deleted.
const IMPORT_FILE: &str = "localstorage-import.json";
/// Libraries the loader gives mods, served from the binary (see
/// `vendor/README.md`), so nothing needs a CDN.
const VENDOR: &[(&str, &[u8])] = &[
    (
        "vendor/react.production.min.js",
        include_bytes!("vendor/react.production.min.js"),
    ),
    (
        "vendor/react-dom.production.min.js",
        include_bytes!("vendor/react-dom.production.min.js"),
    ),
    (
        "vendor/mousetrap.min.js",
        include_bytes!("vendor/mousetrap.min.js"),
    ),
];

/// Where a page served by the `mods` scheme lives. WebView2 and Android
/// reach custom schemes through `https://<scheme>.localhost`.
#[cfg(any(windows, target_os = "android"))]
const START_URL: &str = "https://mods.localhost/index.html";
#[cfg(not(any(windows, target_os = "android")))]
const START_URL: &str = "mods://localhost/index.html";

pub struct Args {
    dir: PathBuf,
    data: PathBuf,
    /// The app's window (an HWND), to live inside instead of on our own.
    parent: Option<isize>,
}

impl Args {
    pub fn parse() -> Option<Self> {
        if !std::env::args().any(|arg| arg == "--mods-child") {
            return None;
        }
        let mut dir = None;
        let mut data = None;
        let mut parent = None;
        let mut all = std::env::args().skip(1);
        while let Some(arg) = all.next() {
            match arg.as_str() {
                "--mods-dir" => dir = all.next().map(PathBuf::from),
                "--mods-data" => data = all.next().map(PathBuf::from),
                "--mods-parent" => parent = all.next().and_then(|hwnd| hwnd.parse().ok()),
                _ => {}
            }
        }
        Some(Self {
            dir: dir?,
            data: data?,
            parent,
        })
    }
}

enum Incoming {
    /// A line from the app: state or a reply.
    State(String),
    /// The page loaded and wants the latest state.
    Ready,
    /// The loader applied `localstorage-import.json`; it is not needed again.
    Imported,
    /// The app closed our stdin.
    Closed,
}

struct Mods {
    args: Args,
    proxy: EventLoopProxy<Incoming>,
    context: WebContext,
    window: Option<Window>,
    webview: Option<WebView>,
    last: Option<String>,
    /// The newest placement not yet applied; a resize sends one a frame and
    /// only the last of a batch is worth a WebView2 relayout.
    pending_place: Option<Option<super::Placement>>,
}

pub fn run(args: Args) -> i32 {
    let event_loop = match EventLoop::<Incoming>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            report(&format!("no event loop: {error}"));
            return 1;
        }
    };
    let proxy = event_loop.create_proxy();
    let stdin_proxy = proxy.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if stdin_proxy.send_event(Incoming::State(line)).is_err() {
                return;
            }
        }
        let _ = stdin_proxy.send_event(Incoming::Closed);
    });
    let mut mods = Mods {
        context: WebContext::new(Some(args.data.clone())),
        args,
        proxy,
        window: None,
        webview: None,
        last: None,
        pending_place: None,
    };
    match event_loop.run_app(&mut mods) {
        Ok(()) => 0,
        Err(error) => {
            report(&format!("{error}"));
            1
        }
    }
}

impl ApplicationHandler<Incoming> for Mods {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attributes = Window::default_attributes()
            .with_title("Spotifast mods")
            .with_inner_size(LogicalSize::new(960.0, 720.0));
        // Inside the app's window: no frame, hidden until the app says where.
        if let Some(parent) = self.args.parent.and_then(std::num::NonZeroIsize::new) {
            let parent = RawWindowHandle::Win32(Win32WindowHandle::new(parent));
            // SAFETY: the app passed its own live top-level window, and it
            // outlives this process: closing it closes our stdin, and we exit.
            attributes = unsafe { attributes.with_parent_window(Some(parent)) }
                .with_decorations(false)
                .with_visible(false);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                report(&format!("no window: {error}"));
                event_loop.exit();
                return;
            }
        };
        let dir = self.args.dir.clone();
        let proxy = self.proxy.clone();
        let builder = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_initialization_script(SHIM)
            .with_ipc_handler(move |request: Request<String>| forward(request.body(), &proxy))
            .with_custom_protocol("mods".into(), move |_id, request| serve(&dir, &request))
            .with_devtools(true)
            .with_url(START_URL);
        // A secure origin, so the page gets the APIs browsers keep for https.
        #[cfg(windows)]
        let builder = wry::WebViewBuilderExtWindows::with_https_scheme(builder, true);
        let built = builder.build(&window);
        match built {
            Ok(webview) => self.webview = Some(webview),
            Err(error) => {
                report(&format!("no webview: {error}"));
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Incoming) {
        let line = match event {
            Incoming::State(line) if line.starts_with(r#"{"place""#) => {
                #[derive(serde::Deserialize)]
                struct Line {
                    place: Option<super::Placement>,
                }
                if let Ok(Line { place }) = serde_json::from_str(&line) {
                    self.pending_place = Some(place);
                }
                return;
            }
            // A state line is kept for a page that loads later; a reply is
            // for the page that asked.
            Incoming::State(line) if line.starts_with(r#"{"state""#) => {
                self.last = Some(line);
                self.last.clone()
            }
            Incoming::State(line) => Some(line),
            Incoming::Ready => self.last.clone(),
            Incoming::Imported => {
                let _ = std::fs::remove_file(self.args.dir.join(IMPORT_FILE));
                return;
            }
            Incoming::Closed => {
                event_loop.exit();
                return;
            }
        };
        if let (Some(webview), Some(line)) = (&self.webview, line) {
            // The line is JSON written by the app, so it is a JS expression.
            let script = format!("window.__spotifast && window.__spotifast.receive({line})");
            let _ = webview.evaluate_script(&script);
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(place) = self.pending_place.take() {
            self.place(place);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        // A child window is the webview's whole surface; keep it filled.
        if let (WindowEvent::Resized(size), Some(webview)) = (&event, &self.webview) {
            let _ = webview.set_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(size.width, size.height).into(),
            });
        }
        // Closed, or destroyed with the app's window it lived in.
        if let WindowEvent::CloseRequested | WindowEvent::Destroyed = event {
            event_loop.exit();
        }
    }
}

impl Mods {
    /// Moves the window to where the app reserved room for it, or hides it.
    fn place(&self, place: Option<super::Placement>) {
        let Some(window) = &self.window else { return };
        match place {
            Some(at) => {
                window.set_outer_position(PhysicalPosition::new(at.x, at.y));
                let _ = window.request_inner_size(PhysicalSize::new(at.width, at.height));
                if window.is_visible() != Some(true) {
                    window.set_visible(true);
                    if let Some(webview) = &self.webview {
                        let _ = webview.set_visible(true);
                    }
                }
            }
            None => {
                // Hiding the window alone leaves WebView2 drawing (a video
                // background, animations) for no one; told it is hidden, it
                // stops rendering and throttles timers. The page and its
                // state stay, so it comes back at once.
                if let Some(webview) = &self.webview {
                    let _ = webview.set_visible(false);
                }
                window.set_visible(false);
            }
        }
    }
}

/// Tells the app's log about a failure here; a GUI build has no stderr.
fn report(message: &str) {
    let line = serde_json::json!({ "cmd": "log", "level": "error", "message": message });
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// Passes a page's message on to the app as one compact JSON line; answers
/// `ready` here, since only this process holds the latest state.
fn forward(body: &str, proxy: &EventLoopProxy<Incoming>) {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    if message["cmd"] == "ready" {
        let _ = proxy.send_event(Incoming::Ready);
        return;
    }
    if message["cmd"] == "imported" {
        let _ = proxy.send_event(Incoming::Imported);
        return;
    }
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

/// Serves a file from the mods folder, refusing any path that leaves it.
fn serve(dir: &Path, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let relative = Path::new(path);
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return status(StatusCode::FORBIDDEN);
    }
    if let Some((_, body)) = VENDOR.iter().find(|(name, _)| *name == path) {
        return Response::builder()
            .header(CONTENT_TYPE, "text/javascript; charset=utf-8")
            .body(Cow::Borrowed(*body))
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR));
    }
    let body = match std::fs::read(dir.join(relative)) {
        Ok(body) => Cow::Owned(body),
        Err(_) if path == "index.html" => Cow::Borrowed(LOADER.as_bytes()),
        Err(_) if path == "demo.html" => Cow::Borrowed(DEMO.as_bytes()),
        Err(_) if path == "apps.json" => Cow::Owned(apps(dir)),
        Err(_) => return status(StatusCode::NOT_FOUND),
    };
    let kind = match relative.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    };
    Response::builder()
        .header(CONTENT_TYPE, kind)
        .body(body)
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

/// The Spicetify custom apps in the mods folder: each folder with a
/// `manifest.json`, by name.
fn apps(dir: &Path) -> Vec<u8> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("manifest.json").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    serde_json::to_vec(&names).unwrap_or_default()
}

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::new(Cow::Borrowed(&[][..]));
    *response.status_mut() = code;
    response
}
