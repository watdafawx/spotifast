//! Fork: libmpv, loaded at run time, drawing a video into the window.
//!
//! `libmpv-2.dll` is looked for beside the app and then on the search
//! path; without it there is simply no video. mpv streams YouTube through
//! yt-dlp (beside the app, or on PATH), without sound, and draws each
//! frame with its OpenGL render API into a texture of ours inside egui's
//! paint callback, which is then copied under the lyrics. Nothing here
//! blocks: commands are asynchronous and state arrives as events, read
//! without waiting.

use std::ffi::{CString, c_char, c_int, c_void};
use std::num::NonZeroU32;
use std::sync::{Mutex, OnceLock};

use eframe::glow::{self, HasContext as _};

type Handle = *mut c_void;
type RenderContext = *mut c_void;

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct OpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

#[repr(C)]
struct Event {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct EventLogMessage {
    prefix: *const c_char,
    level: *const c_char,
    text: *const c_char,
    log_level: c_int,
}

#[repr(C)]
struct EventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

const RENDER_PARAM_INVALID: c_int = 0;
const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const RENDER_PARAM_OPENGL_FBO: c_int = 3;
const RENDER_PARAM_FLIP_Y: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_LOG_MESSAGE: c_int = 2;
const EVENT_END_FILE: c_int = 7;
const EVENT_PLAYBACK_RESTART: c_int = 21;
const EVENT_PROPERTY_CHANGE: c_int = 22;
const PROPERTY_TIME: u64 = 1;
const PROPERTY_DURATION: u64 = 2;

struct Api {
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    command_async: unsafe extern "C" fn(Handle, u64, *mut *const c_char) -> c_int,
    observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    request_log_messages: unsafe extern "C" fn(Handle, *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut Event,
    terminate_destroy: unsafe extern "C" fn(Handle),
    render_create: unsafe extern "C" fn(*mut RenderContext, Handle, *mut RenderParam) -> c_int,
    render_set_update_callback:
        unsafe extern "C" fn(RenderContext, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    render_render: unsafe extern "C" fn(RenderContext, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(RenderContext),
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// libmpv's functions, or `None` when the library is missing or too old.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("libmpv-2.dll")))
            .filter(|path| path.exists());
        let module = unsafe {
            match &beside {
                Some(path) => LoadLibraryW(wide(&path.to_string_lossy()).as_ptr()),
                None => LoadLibraryW(wide("libmpv-2.dll").as_ptr()),
            }
        };
        if module.is_null() {
            log::info!("lyrics video: libmpv-2.dll not found, so no video backgrounds");
            return None;
        }
        macro_rules! load {
            ($name:literal) => {{
                let symbol = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
                match symbol {
                    // SAFETY: the symbol is libmpv's, with the C signature
                    // its header declares for the field it fills.
                    // Function pointers are all one size; the field's type
                    // picks the signature.
                    Some(symbol) => unsafe { std::mem::transmute_copy(&symbol) },
                    None => {
                        log::warn!("lyrics video: libmpv has no {}", $name);
                        return None;
                    }
                }
            }};
        }
        Some(Api {
            create: load!("mpv_create"),
            initialize: load!("mpv_initialize"),
            set_option_string: load!("mpv_set_option_string"),
            command_async: load!("mpv_command_async"),
            observe_property: load!("mpv_observe_property"),
            request_log_messages: load!("mpv_request_log_messages"),
            wait_event: load!("mpv_wait_event"),
            terminate_destroy: load!("mpv_terminate_destroy"),
            render_create: load!("mpv_render_context_create"),
            render_set_update_callback: load!("mpv_render_context_set_update_callback"),
            render_render: load!("mpv_render_context_render"),
            render_free: load!("mpv_render_context_free"),
        })
    })
    .as_ref()
}

/// Whether video backgrounds can work on this machine.
pub fn available() -> bool {
    api().is_some()
}

/// OpenGL functions for mpv: the driver's through `wglGetProcAddress`,
/// the OpenGL 1.1 core from opengl32.dll.
unsafe extern "C" fn gl_proc(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    static OPENGL: OnceLock<usize> = OnceLock::new();
    let module = *OPENGL
        .get_or_init(|| unsafe { LoadLibraryW(wide("opengl32.dll").as_ptr()) } as usize)
        as windows_sys::Win32::Foundation::HMODULE;
    if module.is_null() {
        return std::ptr::null_mut();
    }
    type WglGetProcAddress = unsafe extern "system" fn(*const u8) -> *mut c_void;
    let wgl = unsafe { GetProcAddress(module, c"wglGetProcAddress".as_ptr().cast()) };
    if let Some(wgl) = wgl {
        // SAFETY: wglGetProcAddress has this signature.
        let wgl: WglGetProcAddress = unsafe { std::mem::transmute(wgl) };
        let found = unsafe { wgl(name.cast()) };
        // 0, 1, 2, 3 and -1 are failures some drivers return.
        if !matches!(found as isize, -1..=3) {
            return found;
        }
    }
    unsafe { GetProcAddress(module, name.cast()) }
        .map_or(std::ptr::null_mut(), |f| f as *mut c_void)
}

/// Asks egui for a frame whenever mpv has a new one.
unsafe extern "C" fn on_update(ctx: *mut c_void) {
    // SAFETY: `ctx` is the boxed context the player owns, freed only after
    // the render context, which stops these calls.
    let ctx = unsafe { &*(ctx as *const egui::Context) };
    ctx.request_repaint();
}

#[derive(Default)]
struct Status {
    /// The video loaded, by YouTube id.
    loaded: Option<String>,
    /// mpv's last message, so a repeated one is logged once.
    last_message: String,
    time: Option<f64>,
    duration: Option<f64>,
    /// A frame of the loaded video is ready to show.
    ready: bool,
    paused: bool,
}

struct Render {
    context: RenderContext,
    framebuffer: Option<glow::NativeFramebuffer>,
    texture: Option<glow::NativeTexture>,
    size: (i32, i32),
}

pub struct Player {
    api: &'static Api,
    handle: Handle,
    status: Mutex<Status>,
    render: Mutex<Option<Render>>,
    repaint: Box<egui::Context>,
}

// SAFETY: libmpv's client API is thread-safe; the render context is only
// used under its mutex, from the thread that owns the OpenGL context.
unsafe impl Send for Player {}
unsafe impl Sync for Player {}

impl Player {
    pub fn new(repaint: &egui::Context) -> Option<Self> {
        let api = api()?;
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return None;
        }
        let set = |name: &str, value: &str| {
            let (name, value) = (CString::new(name).ok()?, CString::new(value).ok()?);
            let result = unsafe { (api.set_option_string)(handle, name.as_ptr(), value.as_ptr()) };
            (result >= 0).then_some(())
        };
        let mut options = vec![
            ("config", "no".to_string()),
            ("terminal", "no".into()),
            ("vo", "libmpv".into()),
            ("hwdec", "auto-safe".into()),
            // Pictures only: the song is Spotify's.
            ("audio", "no".into()),
            ("osc", "no".into()),
            ("input-default-bindings", "no".into()),
            ("input-vo-keyboard", "no".into()),
            ("keep-open", "yes".into()),
            // No looping: a stream that fails would retry without end.
            // A video shorter than the song is started over by the sync.
            // Fill the view, cropping the sides rather than adding bars.
            ("panscan", "1.0".into()),
            ("ytdl", "yes".into()),
            (
                "ytdl-format",
                "bv*[height<=1080]/b[height<=1080]/bv*/b".into(),
            ),
            ("demuxer-max-bytes", "64MiB".into()),
        ];
        if let Some(beside) = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("yt-dlp.exe")))
            .filter(|path| path.exists())
        {
            options.push((
                "script-opts",
                format!("ytdl_hook-ytdl_path={}", beside.display()),
            ));
        }
        for (name, value) in &options {
            if set(name, value).is_none() {
                log::debug!("lyrics video: mpv refused option {name}");
            }
        }
        if unsafe { (api.initialize)(handle) } < 0 {
            unsafe { (api.terminate_destroy)(handle) };
            return None;
        }
        // mpv's own warnings and errors (yt-dlp failing, say) go to our log.
        unsafe { (api.request_log_messages)(handle, c"warn".as_ptr()) };
        for (id, name) in [
            (PROPERTY_TIME, c"time-pos"),
            (PROPERTY_DURATION, c"duration"),
        ] {
            unsafe { (api.observe_property)(handle, id, name.as_ptr(), FORMAT_DOUBLE) };
        }
        Some(Self {
            api,
            handle,
            status: Mutex::new(Status::default()),
            render: Mutex::new(None),
            repaint: Box::new(repaint.clone()),
        })
    }

    fn command(&self, args: &[&str]) {
        let owned: Vec<CString> = args
            .iter()
            .filter_map(|arg| CString::new(*arg).ok())
            .collect();
        let mut pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(std::ptr::null());
        unsafe { (self.api.command_async)(self.handle, 0, pointers.as_mut_ptr()) };
    }

    /// Starts the YouTube video `id` at `start` seconds, paused or not.
    pub fn load(&self, id: &str, start: f64, paused: bool) {
        {
            let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
            if status.loaded.as_deref() == Some(id) {
                return;
            }
            *status = Status {
                loaded: Some(id.to_string()),
                paused,
                ..Status::default()
            };
        }
        let url = format!("https://www.youtube.com/watch?v={id}");
        log::info!("lyrics video: loading {id} at {start:.1}s");
        let options = format!(
            "start={start:.2},pause={}",
            if paused { "yes" } else { "no" }
        );
        self.command(&["loadfile", &url, "replace", "-1", &options]);
    }

    /// The video loaded, by YouTube id.
    pub fn loaded(&self) -> Option<String> {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .loaded
            .clone()
    }

    pub fn stop(&self) {
        self.status.lock().unwrap_or_else(|p| p.into_inner()).loaded = None;
        self.command(&["stop"]);
    }

    pub fn seek(&self, seconds: f64) {
        self.command(&["seek", &format!("{seconds:.3}"), "absolute"]);
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        status.time = Some(seconds);
    }

    pub fn set_paused(&self, paused: bool) {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        if status.paused != paused {
            status.paused = paused;
            drop(status);
            self.command(&["set", "pause", if paused { "yes" } else { "no" }]);
        }
    }

    /// Reads mpv's news without waiting: position, length, first frame.
    pub fn poll(&self) {
        loop {
            let event = unsafe { (self.api.wait_event)(self.handle, 0.0) };
            if event.is_null() {
                return;
            }
            let event = unsafe { &*event };
            let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
            match event.event_id {
                EVENT_NONE | EVENT_SHUTDOWN => return,
                EVENT_PLAYBACK_RESTART => status.ready = true,
                EVENT_END_FILE => status.ready = false,
                EVENT_LOG_MESSAGE if !event.data.is_null() => {
                    let message = unsafe { &*(event.data as *const EventLogMessage) };
                    let text = |pointer: *const c_char| {
                        if pointer.is_null() {
                            String::new()
                        } else {
                            unsafe { std::ffi::CStr::from_ptr(pointer) }
                                .to_string_lossy()
                                .trim_end()
                                .to_string()
                        }
                    };
                    let line = format!(
                        "{} {}: {}",
                        text(message.prefix),
                        text(message.level),
                        text(message.text)
                    );
                    if line != status.last_message {
                        log::warn!("mpv {line}");
                        status.last_message = line;
                    }
                }
                EVENT_PROPERTY_CHANGE if !event.data.is_null() => {
                    let property = unsafe { &*(event.data as *const EventProperty) };
                    let value = (property.format == FORMAT_DOUBLE && !property.data.is_null())
                        .then(|| unsafe { *(property.data as *const f64) });
                    match event.reply_userdata {
                        PROPERTY_TIME => status.time = value,
                        PROPERTY_DURATION => status.duration = value,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Video time, video length, and whether a frame is ready.
    pub fn position(&self) -> (Option<f64>, Option<f64>, bool) {
        let status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        (
            status.time,
            status.duration,
            status.ready && status.loaded.is_some(),
        )
    }

    /// Whether mpv can draw yet. Its render context must exist before a
    /// video loads, or mpv gives up on video output.
    pub fn can_draw(&self) -> bool {
        self.render
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Creates the render context. Runs in egui's paint callback, with the
    /// window's OpenGL context current.
    pub fn prepare(&self) {
        let mut render = self.render.lock().unwrap_or_else(|p| p.into_inner());
        if render.is_none() {
            *render = self.create_render();
        }
    }

    /// Draws the current frame into `info`'s viewport. Runs in egui's paint
    /// callback, with the window's OpenGL context current.
    pub fn paint(&self, gl: &glow::Context, info: &egui::PaintCallbackInfo) {
        let viewport = info.viewport_in_pixels();
        let (w, h) = (viewport.width_px, viewport.height_px);
        if w <= 0 || h <= 0 {
            return;
        }
        let mut render = self.render.lock().unwrap_or_else(|p| p.into_inner());
        if render.is_none() {
            *render = self.create_render();
        }
        let Some(render) = render.as_mut() else {
            return;
        };
        unsafe {
            let previous = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
            if render.size != (w, h) || render.framebuffer.is_none() {
                if let Some(texture) = render.texture.take() {
                    gl.delete_texture(texture);
                }
                if let Some(framebuffer) = render.framebuffer.take() {
                    gl.delete_framebuffer(framebuffer);
                }
                let (Ok(texture), Ok(framebuffer)) = (gl.create_texture(), gl.create_framebuffer())
                else {
                    return;
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    w,
                    h,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                render.texture = Some(texture);
                render.framebuffer = Some(framebuffer);
                render.size = (w, h);
            }
            let Some(framebuffer) = render.framebuffer else {
                return;
            };
            let mut fbo = OpenGlFbo {
                fbo: framebuffer.0.get() as c_int,
                w,
                h,
                internal_format: 0,
            };
            let mut flip: c_int = 1;
            let mut params = [
                RenderParam {
                    kind: RENDER_PARAM_OPENGL_FBO,
                    data: (&mut fbo as *mut OpenGlFbo).cast(),
                },
                RenderParam {
                    kind: RENDER_PARAM_FLIP_Y,
                    data: (&mut flip as *mut c_int).cast(),
                },
                RenderParam {
                    kind: RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            (self.api.render_render)(render.context, params.as_mut_ptr());
            // Copy the frame into the viewport of whatever egui draws to.
            let target = NonZeroU32::new(previous as u32).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, target);
            gl.disable(glow::SCISSOR_TEST);
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.left_px + w,
                viewport.from_bottom_px + h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, target);
        }
    }

    fn create_render(&self) -> Option<Render> {
        let mut init = OpenGlInitParams {
            get_proc_address: gl_proc,
            get_proc_address_ctx: std::ptr::null_mut(),
        };
        let mut params = [
            RenderParam {
                kind: RENDER_PARAM_API_TYPE,
                data: c"opengl".as_ptr().cast_mut().cast(),
            },
            RenderParam {
                kind: RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&mut init as *mut OpenGlInitParams).cast(),
            },
            RenderParam {
                kind: RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let mut context: RenderContext = std::ptr::null_mut();
        let result =
            unsafe { (self.api.render_create)(&mut context, self.handle, params.as_mut_ptr()) };
        if result < 0 || context.is_null() {
            log::warn!("lyrics video: mpv could not draw with OpenGL ({result})");
            return None;
        }
        let repaint = (&*self.repaint as *const egui::Context).cast_mut().cast();
        unsafe { (self.api.render_set_update_callback)(context, Some(on_update), repaint) };
        Some(Render {
            context,
            framebuffer: None,
            texture: None,
            size: (0, 0),
        })
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The render context goes first, with the window's OpenGL context
        // still current on this (the UI) thread; the textures go with the
        // context when the window closes.
        if let Some(render) = self
            .render
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            unsafe { (self.api.render_free)(render.context) };
        }
        unsafe { (self.api.terminate_destroy)(self.handle) };
    }
}
