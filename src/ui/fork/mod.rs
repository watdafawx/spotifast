//! Fork: the fork's own views. Upstream's views call into these through
//! one-line hooks; see FORK.md.

use egui::Color32;

use crate::app::App;

pub(crate) mod genres;
pub(crate) mod lyrics_ai;
pub(crate) mod lyrics_fx;
pub(crate) mod lyrics_page;

/// Hook in `keep_room_for_panels`: a collapsed Library needs only its
/// strip's width, not the open Library's least width.
pub(super) fn min_width(app: &App, width: f32, panels: bool) -> f32 {
    if panels && app.settings.sidebar_visible && app.settings.fork.sidebar_collapsed {
        (width - super::SIDEBAR_MIN_WIDTH + super::sidebar::fork::COLLAPSED_WIDTH).round()
    } else {
        width
    }
}

/// Hook in the central panel under the top bar: the native lyrics page, or
/// a mod's page, takes the main area. True when one drew it.
pub(super) fn central(app: &mut App, ui: &mut egui::Ui) -> bool {
    if app.fork.lyrics_page_open {
        lyrics_page::show(app, ui);
        return true;
    }
    #[cfg(feature = "mods")]
    if crate::fork::mods::page_open() && crate::fork::mods::place(ui) {
        return true;
    }
    false
}

/// Hook in `header_wash`: over a see-through page (glass themes) the art's
/// wash fades out. Fading to the page's own colour would stack a second
/// translucent layer on the page and leave a seam where it ends.
pub(super) fn glass_wash(
    palette: &crate::theme::Palette,
    art: Color32,
    strength: f32,
) -> Option<(Color32, Color32)> {
    (palette.window.a() < 255).then(|| (art.gamma_multiply(strength), Color32::TRANSPARENT))
}

/// Hook in `blend` for a see-through base: the mix keeps the base's alpha,
/// so a glass theme's tinted header stays glass.
pub(super) fn glass_blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let [r, g, b, _] = super::blend(base.to_opaque(), tint, amount).to_array();
    Color32::from_rgba_unmultiplied(r, g, b, base.a())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tinted glass header stays glass; an opaque one stays opaque.
    #[test]
    fn blend_keeps_the_base_alpha() {
        let tint = Color32::from_rgb(200, 40, 40);
        let opaque = Color32::from_rgb(10, 20, 30);
        assert_eq!(super::super::blend(opaque, tint, 0.5).a(), 255);
        let glass = Color32::from_rgba_unmultiplied(10, 20, 30, 128);
        let mixed = super::super::blend(glass, tint, 0.5);
        assert_eq!(mixed.a(), 128);
        let [r, g, b, _] = mixed.to_srgba_unmultiplied();
        let [or, og, ob, _] = super::super::blend(opaque, tint, 0.5).to_array();
        assert!(r.abs_diff(or) <= 2 && g.abs_diff(og) <= 2 && b.abs_diff(ob) <= 2);
    }
}
