//! The mods window: a webview in its own process (fork only).
//!
//! Lines on stdin are state from the app, handed to the page's
//! `window.__spotifast.receive`. Messages the page posts with
//! `window.ipc.postMessage` go to stdout, one JSON object per line. The page
//! and its files come from the mods folder through the `mods` scheme; with
//! no `index.html` there, a built-in demo page stands in.

use std::borrow::Cow;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{WebContext, WebView, WebViewBuilder};

const SHIM: &str = include_str!("shim.js");
const DEMO: &str = include_str!("demo.html");

/// Where a page served by the `mods` scheme lives. WebView2 and Android
/// reach custom schemes through `https://<scheme>.localhost`.
#[cfg(any(windows, target_os = "android"))]
const START_URL: &str = "https://mods.localhost/index.html";
#[cfg(not(any(windows, target_os = "android")))]
const START_URL: &str = "mods://localhost/index.html";

pub struct Args {
    dir: PathBuf,
    data: PathBuf,
}

impl Args {
    pub fn parse() -> Option<Self> {
        if !std::env::args().any(|arg| arg == "--mods-child") {
            return None;
        }
        let mut dir = None;
        let mut data = None;
        let mut all = std::env::args().skip(1);
        while let Some(arg) = all.next() {
            match arg.as_str() {
                "--mods-dir" => dir = all.next().map(PathBuf::from),
                "--mods-data" => data = all.next().map(PathBuf::from),
                _ => {}
            }
        }
        Some(Self {
            dir: dir?,
            data: data?,
        })
    }
}

enum Incoming {
    /// A line of state from the app.
    State(String),
    /// The page loaded and wants the latest state.
    Ready,
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
}

pub fn run(args: Args) -> i32 {
    let event_loop = match EventLoop::<Incoming>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("mods: no event loop: {error}");
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
    };
    match event_loop.run_app(&mut mods) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("mods: {error}");
            1
        }
    }
}

impl ApplicationHandler<Incoming> for Mods {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Spotifast mods")
            .with_inner_size(LogicalSize::new(960.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("mods: no window: {error}");
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
                eprintln!("mods: no webview: {error}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Incoming) {
        match event {
            Incoming::State(line) => self.last = Some(line),
            Incoming::Ready => {}
            Incoming::Closed => {
                event_loop.exit();
                return;
            }
        }
        if let (Some(webview), Some(line)) = (&self.webview, &self.last) {
            // The line is JSON written by the app, so it is a JS expression.
            let script = format!("window.__spotifast && window.__spotifast.receive({line})");
            let _ = webview.evaluate_script(&script);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
        }
    }
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
    let body = match std::fs::read(dir.join(relative)) {
        Ok(body) => Cow::Owned(body),
        Err(_) if path == "index.html" => Cow::Borrowed(DEMO.as_bytes()),
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

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::new(Cow::Borrowed(&[][..]));
    *response.status_mut() = code;
    response
}
