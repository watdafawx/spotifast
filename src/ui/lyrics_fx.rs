//! Fork: the lyrics page's optional effects: how lines enter, move and
//! leave, a glow or outline on the sung line, dots through instrumental
//! breaks, and slow colour blobs behind the lyrics when there is no video.

use egui::emath::TSTransform;
use egui::{Color32, Pos2, Rect, Sense, Shape, Vec2, pos2, vec2};

use crate::lyrics_page::{Enter, Leave, Motion};

/// How a line looks `t` (0 to 1) of the way into its entrance, or out of
/// its exit: its opacity, how far it is moved down (in line heights) and
/// its scale, and for a typewriter how much of its text shows.
pub(super) struct Phase {
    pub opacity: f32,
    pub shift: f32,
    pub scale: f32,
    pub reveal: f32,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn entering(enter: Enter, t: f32) -> Phase {
    let e = smooth(t);
    match enter {
        Enter::Fade => Phase {
            opacity: e,
            shift: 0.0,
            scale: 1.0,
            reveal: 1.0,
        },
        Enter::Rise => Phase {
            opacity: e,
            shift: 1.0 - e,
            scale: 1.0,
            reveal: 1.0,
        },
        Enter::Typewriter => Phase {
            opacity: (e * 4.0).min(1.0),
            shift: 0.0,
            scale: 1.0,
            reveal: t.clamp(0.0, 1.0),
        },
        Enter::Zoom => Phase {
            opacity: e,
            shift: 0.0,
            scale: 0.85 + 0.15 * e,
            reveal: 1.0,
        },
    }
}

pub(super) fn leaving(leave: Leave, t: f32) -> Phase {
    // `t` falls from 1 to 0 as the line goes.
    let e = smooth(t);
    match leave {
        Leave::Fade => Phase {
            opacity: e,
            shift: 0.0,
            scale: 1.0,
            reveal: 1.0,
        },
        Leave::DriftUp => Phase {
            opacity: e,
            shift: -(1.0 - e),
            scale: 1.0,
            reveal: 1.0,
        },
        Leave::Zoom => Phase {
            opacity: e,
            shift: 0.0,
            scale: 1.0 + 0.15 * (1.0 - e),
            reveal: 1.0,
        },
    }
}

/// A scale about `centre`, then a move down by `shift`.
pub(super) fn transform(centre: Pos2, shift: f32, scale: f32) -> TSTransform {
    TSTransform::new(centre.to_vec2() * (1.0 - scale) + vec2(0.0, shift), scale)
}

/// The sung line's movement at `time` seconds: a move down in points and
/// an opacity.
pub(super) fn moving(motion: Motion, time: f64, size: f32) -> (f32, f32) {
    let wave = |period: f64| (time * std::f64::consts::TAU / period).sin() as f32;
    match motion {
        Motion::None => (0.0, 1.0),
        Motion::Float => (wave(3.2) * size * 0.08, 1.0),
        Motion::Pulse => (0.0, 0.82 + 0.18 * (wave(1.6) * 0.5 + 0.5)),
    }
}

/// The first `share` of `text`'s characters, for a typewriter.
pub(super) fn typed(text: &str, share: f32) -> &str {
    if share >= 1.0 {
        return text;
    }
    let count = text.chars().count();
    let shown = (count as f32 * share.max(0.0)).ceil() as usize;
    match text.char_indices().nth(shown) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// Paints `galley` again behind where it was drawn at `at`: a soft glow
/// in `glow`, and or a dark outline, into the placeholder `slot`.
pub(super) fn halo(
    painter: &egui::Painter,
    slot: egui::layers::ShapeIdx,
    galley: &std::sync::Arc<egui::Galley>,
    at: Pos2,
    glow: Option<Color32>,
    outline: bool,
) {
    let mut shapes = Vec::new();
    if let Some(glow) = glow {
        for (radius, alpha) in [(6.0, 0.10), (3.5, 0.16), (1.8, 0.22)] {
            for step in 0..8 {
                let angle = step as f32 * std::f32::consts::TAU / 8.0;
                let offset = vec2(angle.cos(), angle.sin()) * radius;
                shapes.push(Shape::galley_with_override_text_color(
                    at + offset,
                    galley.clone(),
                    glow.gamma_multiply(alpha),
                ));
            }
        }
    }
    if outline {
        for step in 0..8 {
            let angle = step as f32 * std::f32::consts::TAU / 8.0;
            let offset = vec2(angle.cos(), angle.sin()) * 1.5;
            shapes.push(Shape::galley_with_override_text_color(
                at + offset,
                galley.clone(),
                Color32::from_black_alpha(190),
            ));
        }
    }
    painter.set(slot, Shape::Vec(shapes));
}

/// Three dots through an instrumental break: they fill one by one as the
/// break runs out, and breathe while it lasts.
pub(super) fn break_dots(
    ui: &mut egui::Ui,
    size: f32,
    color: Color32,
    progress: f32,
    active: bool,
    time: f64,
) -> egui::Response {
    let radius = (size * 0.16).max(4.0);
    let gap = radius * 3.2;
    let (rect, response) = ui.allocate_exact_size(vec2(gap * 3.0, size * 0.9), Sense::click());
    let breathe = if active {
        1.0 + 0.12 * ((time * std::f64::consts::TAU / 1.8).sin() as f32)
    } else {
        1.0
    };
    for dot in 0..3 {
        let centre = pos2(rect.left() + gap * (dot as f32 + 0.5), rect.center().y);
        let lit = if active {
            ((progress * 3.0) - dot as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ui.painter().circle_filled(
            centre,
            radius * breathe,
            color.gamma_multiply(0.35 + 0.65 * lit),
        );
    }
    response
}

/// Soft colour blobs drifting over `rect`, tinted by `colours`.
pub(super) fn blobs(painter: &egui::Painter, rect: Rect, colours: &[Color32], time: f64) {
    if colours.is_empty() {
        return;
    }
    let span = rect.size().max_elem();
    for (index, colour) in colours.iter().enumerate().take(4) {
        let phase = index as f64 * 1.7;
        let x = 0.5 + 0.32 * ((time / (23.0 + index as f64 * 5.0)) + phase).sin();
        let y = 0.5 + 0.30 * ((time / (31.0 + index as f64 * 7.0)) + phase * 1.3).cos();
        let centre = rect.min + Vec2::new(rect.width() * x as f32, rect.height() * y as f32);
        let reach = span * (0.38 + 0.06 * index as f32);
        // A radial falloff from rings of fading colour.
        for ring in (1..=10).rev() {
            let share = ring as f32 / 10.0;
            painter.circle_filled(
                centre,
                reach * share,
                colour.gamma_multiply(0.045 * (1.0 - share * 0.8)),
            );
        }
    }
}
