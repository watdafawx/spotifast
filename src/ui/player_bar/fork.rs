//! Fork: the player bar's waveform seek bar, genre line and the fork's
//! buttons.

use super::*;
use crate::ui::widgets::fork::painted_slider;

/// The waveform seek bar: its height, and each mirrored bar's width and the
/// gap after it.
const WAVE_SEEK_HEIGHT: f32 = 26.0;
const WAVE_SEEK_BAR: f32 = 2.0;
const WAVE_SEEK_GAP: f32 = 1.0;
/// How strongly the part still to play shows, and steps not yet heard.
const WAVE_SEEK_AHEAD_ALPHA: f32 = 0.32;
const WAVE_SEEK_UNKNOWN_ALPHA: f32 = 0.16;

/// Hook in `transport` after the seek bar's rect: the waveform seek bar is
/// taller, and follows the playing song's waveform.
pub(super) fn seek_rect(app: &mut App, now: Option<&NowPlaying>, rect: Rect) -> Rect {
    let waveform = app.settings.fork.seek_waveform;
    app.winamp.tap.seek_wave.set_enabled(waveform);
    if !waveform {
        return rect;
    }
    if let Some(now) = now {
        let wave = &app.winamp.tap.seek_wave;
        wave.follow(&now.uri, now.duration_ms, now.position_ms);
        wave.refresh(&now.uri, &mut app.fork.seek_wave_view);
    }
    Rect::from_center_size(rect.center(), vec2(rect.width(), WAVE_SEEK_HEIGHT))
}

/// Hook in `transport` before the seek bar: stands in for `thin_slider`,
/// drawing the waveform instead when that setting is on.
pub(super) fn seek_slider(
    app: &App,
    duration_ms: u32,
) -> impl FnOnce(&mut egui::Ui, &theme::Palette, egui::Id, &str, f32, f32, Option<f32>) -> SliderEvent + '_
{
    move |ui, palette, id, label, value, width, wheel_step| {
        if !app.settings.fork.seek_waveform {
            return thin_slider(ui, palette, id, label, value, width, wheel_step);
        }
        let levels = app.fork.seek_wave_view.levels.as_ref();
        let pointer = ui.ctx().pointer_latest_pos();
        painted_slider(
            ui,
            id,
            label,
            value,
            vec2(width, WAVE_SEEK_HEIGHT),
            wheel_step,
            |painter, rect, shown, active| {
                seek_waveform(
                    painter,
                    rect,
                    shown,
                    active,
                    SeekPointer {
                        pos: pointer,
                        duration_ms,
                    },
                    levels,
                    palette,
                );
            },
        )
    }
}

/// The seek bar as the song's waveform: thin bars mirrored about the
/// middle, as tall as the song is loud there, bright up to `shown` and
/// grey after it. Steps not yet heard stay at the floor, dimmer still.
/// Where the pointer is, and the song's length for the time under it.
struct SeekPointer {
    pos: Option<egui::Pos2>,
    duration_ms: u32,
}

/// Hovered or dragged, a playhead in the accent follows the pointer, with
/// the time there in a label above it.
fn seek_waveform(
    painter: &egui::Painter,
    rect: Rect,
    shown: f32,
    active: bool,
    pointer: SeekPointer,
    levels: Option<&crate::fork::seekwave::Levels>,
    palette: &theme::Palette,
) {
    let pitch = WAVE_SEEK_BAR + WAVE_SEEK_GAP;
    let bars = ((rect.width() + WAVE_SEEK_GAP) / pitch).floor().max(1.0) as usize;
    let heights = levels.map_or_else(
        || vec![None; bars],
        |levels| crate::fork::seekwave::bar_heights(levels, bars),
    );
    let bright = if palette.dark {
        Color32::WHITE
    } else {
        palette.text
    };
    let head = rect.left() + rect.width() * shown.clamp(0.0, 1.0);
    let mut mesh = egui::Mesh::default();
    for (index, height) in heights.iter().enumerate() {
        let left = rect.left() + index as f32 * pitch;
        let played = left + WAVE_SEEK_BAR / 2.0 <= head;
        let (height, colour) = match height {
            Some(height) => (
                *height,
                if played {
                    bright
                } else {
                    bright.gamma_multiply(WAVE_SEEK_AHEAD_ALPHA)
                },
            ),
            None => (
                crate::fork::seekwave::FLOOR,
                bright.gamma_multiply(if played {
                    WAVE_SEEK_AHEAD_ALPHA
                } else {
                    WAVE_SEEK_UNKNOWN_ALPHA
                }),
            ),
        };
        let half = (height * rect.height() / 2.0).max(1.0);
        let bar = Rect::from_min_max(
            pos2(left, rect.center().y - half),
            pos2(left + WAVE_SEEK_BAR, rect.center().y + half),
        );
        mesh.add_colored_rect(bar, colour);
    }
    painter.add(egui::Shape::mesh(mesh));
    if !active {
        return;
    }
    // The pointer counts while it is over the bar or dragging it; focus from
    // the keyboard leaves it wherever it is.
    let pressed = painter.ctx().input(|input| input.pointer.primary_down());
    let duration_ms = pointer.duration_ms;
    let Some(pointer) = pointer
        .pos
        .filter(|pointer| pressed || rect.expand(6.0).contains(*pointer))
    else {
        // Focused from the keyboard: mark where the song is.
        painter.vline(head, rect.y_range(), egui::Stroke::new(2.0, palette.accent));
        return;
    };
    let x = pointer.x.clamp(rect.left(), rect.right());
    painter.vline(x, rect.y_range(), egui::Stroke::new(2.0, palette.accent));
    if duration_ms == 0 {
        return;
    }
    let fraction = (x - rect.left()) / rect.width().max(1.0);
    let time = crate::util::format_duration_ms((fraction * duration_ms as f32) as u32);
    // Above everything, so the transport's buttons never cover it.
    let label = painter.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("seek-wave-time"),
    ));
    let galley = label.layout_no_wrap(time, theme::regular(11.0), palette.text);
    let size = galley.size() + vec2(10.0, 4.0);
    let left = (x - size.x / 2.0).clamp(rect.left(), (rect.right() - size.x).max(rect.left()));
    let bubble = Rect::from_min_size(pos2(left, rect.top() - size.y - 4.0), size);
    label.rect_filled(bubble, 4.0, palette.surface.gamma_multiply(0.92));
    label.galley(bubble.min + vec2(5.0, 2.0), galley, palette.text);
}

/// The genre line's font, a step below the artist names.
fn genre_font() -> egui::FontId {
    theme::regular(11.0)
}

/// Hook in `now_playing_block`: the text block is as wide as the genre
/// line too.
pub(super) fn genre_width(
    ui: &egui::Ui,
    genres: &str,
    width: f32,
    text_width: f32,
    palette: &theme::Palette,
) -> f32 {
    if genres.is_empty() {
        return width;
    }
    let genre = ui
        .painter()
        .layout_no_wrap(genres.to_owned(), genre_font(), palette.dim)
        .size()
        .x;
    width.max(genre.min(text_width))
}

/// Hook in `now_playing_block`: room for the genre line, the text block
/// still centred.
pub(super) fn genre_rect(rect: Rect, genres: &str) -> Rect {
    if genres.is_empty() {
        rect
    } else {
        rect.expand2(vec2(0.0, 7.0))
    }
}

/// Hook in `now_playing_block` after the artists: a dim third line of the
/// artists' genres, when any are known. It drags the song like the rest.
pub(super) fn genre_line(
    ui: &mut egui::Ui,
    genres: &str,
    palette: &theme::Palette,
    sense: egui::Sense,
    responses: &mut Vec<egui::Response>,
) {
    if !genres.is_empty() {
        responses.push(theme::text(ui, genres, genre_font(), palette.dim).interact(sense));
    }
}

/// Hook at the end of `extras`: the mod's page, and the native lyrics page,
/// left of upstream's buttons. Each gives way in a narrow window before it
/// crowds the transport, before upstream's do.
pub(super) fn extras_buttons(app: &mut App, ui: &mut egui::Ui, transport_right: f32) {
    let room = |ui: &egui::Ui| {
        let left =
            ui.min_rect().left() - ui.spacing().item_spacing.x - (18.0 + theme::ICON_BUTTON_PAD);
        left - transport_right >= TRANSPORT_GAP
    };
    #[cfg(feature = "mods")]
    if room(ui) {
        crate::fork::mods::page_button(app, ui);
    }
    if room(ui) {
        crate::ui::fork::lyrics_page::button(app, ui);
    }
}
