//! Fork: the fork's copies of the lyrics views. Upstream's `contents`,
//! `fullscreen_contents` and `background` hand over to these at their
//! first line, so upstream's own bodies are unused. When upstream changes
//! those functions, the sync workflow opens an issue to port the change
//! here (FORK.md).

use super::*;

/// The fork's views replace upstream's. A constant, so upstream's bodies
/// still compile and stay easy to diff against.
pub(super) const REPLACES: bool = true;

/// Hook in `fullscreen` under the header: with the mod's page open, full
/// screen shows the mod, below the header, which keeps the way out. True
/// when it drew it.
pub(super) fn mods_page(app: &mut App, ui: &mut egui::Ui, rect: Rect, top: f32) -> bool {
    #[cfg(feature = "mods")]
    if crate::fork::mods::page_open() {
        let region = rect.shrink2(vec2(24.0, 0.0)).with_min_y(rect.top() + top);
        let mut content = ui.new_child(UiBuilder::new().max_rect(region));
        fullscreen_header(app, &mut content);
        content.add_space(12.0);
        return crate::fork::mods::place(&mut content);
    }
    let _ = (app, ui, rect, top);
    false
}

/// Scrolls so the middle of `line` sits `SUNG_LINE_AT` of the way down the
/// visible lyrics; `at` is `SUNG_LINE_AT` or the lyrics page's own.
fn show_sung_line_at(
    ui: &egui::Ui,
    line: Rect,
    animation: Option<egui::style::ScrollAnimation>,
    at: f32,
) {
    let above = (ui.clip_rect().height() * at - line.height() / 2.0).max(0.0);
    let target = Rect::from_min_max(pos2(line.left(), line.top() - above), line.max);
    match animation {
        Some(animation) => ui.scroll_to_rect_animation(target, Some(Align::Min), animation),
        None => ui.scroll_to_rect(target, Some(Align::Min)),
    }
}

/// Upstream's private `blend`, for the fork's views.
pub(crate) fn blend(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    super::blend(from, to, t)
}

/// Fork's copy of upstream's `contents`, the side panel's lyrics, with
/// the lyrics offset, pronunciation and translation.
pub(super) fn contents(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let Some(now) = app.now_playing() else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            if !app.network_reachable {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Spotifast will reconnect automatically when your connection returns.",
                    ),
                );
            } else {
                widgets::loading_row(ui, &palette, app.locale);
            }
            return;
        }
        Loadable::Failed(error) => {
            if !app.network_reachable {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Spotifast will reconnect automatically when your connection returns.",
                    ),
                );
                return;
            }
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.secondary);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(app.locale, "Try again"), false).clicked()
            {
                app.request_lyrics();
            }
            return;
        }
        Loadable::Loaded(None) => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Music,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics.clone(),
    };

    // Fork: the song's own lyrics offset (src/fork/lyrics_page.rs).
    let active = lyrics.active_line(
        app.settings
            .fork
            .lyrics_page
            .position(&now.uri, now.position_ms),
    );
    let follow = app.lyrics_following && app.lyrics_line_shown != Some(active);
    // The line being sung is bold and in the accent colour; every other
    // line is quiet, regular text, the same before and after it has been
    // sung. A line takes 220 ms to light up or fade, as in omarchy-lyrics.
    let quiet = palette.text.gamma_multiply(0.45);
    let scroll = crate::autoscroll::show(
        ui,
        egui::ScrollArea::vertical()
            .id_salt("lyrics-scroll")
            .auto_shrink([false, false]),
        egui::Vec2b::new(false, true),
        |ui| {
            // Before the first line there is nothing to highlight, so the
            // panel sits at the top rather than wherever it was left.
            if follow && lyrics.synced && active.is_none() {
                let top = ui.cursor().min;
                ui.scroll_to_rect(
                    egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
                    Some(Align::Min),
                );
            }
            ui.add_space(12.0);
            // Fork: why the translation is missing, if it is.
            crate::ui::fork::lyrics_ai::notice(app, ui, palette.secondary);
            for (index, line) in lyrics.lines.iter().enumerate() {
                let is_active = active == Some(index);
                let lit = ui.ctx().animate_bool_with_time(
                    egui::Id::new("lyric-line").with(index),
                    is_active,
                    crate::motion::seconds(ui.ctx(), LIGHT_UP_SECONDS),
                );
                let color = blend(quiet, palette.accent, lit);
                let font = if lit > 0.5 {
                    theme::bold(LINE_SIZE)
                } else {
                    theme::regular(LINE_SIZE)
                };
                // A timed line with no words is the band playing on.
                let text = if line.text.is_empty() && lyrics.synced {
                    "\u{266a}"
                } else {
                    line.text.as_str()
                };
                let sense = if lyrics.synced {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                // Fork: with its pronunciation above, word by word.
                let ruby = crate::ui::fork::lyrics_ai::lyric_line(
                    app,
                    ui,
                    index,
                    text,
                    font.clone(),
                    13.0,
                    color,
                    sense,
                    None,
                );
                let response = if let Some(response) = ruby {
                    response
                } else if crate::bidi::is_rtl(text) {
                    let galley = crate::bidi::layout(
                        ui.painter(),
                        text,
                        font,
                        color,
                        ui.available_width(),
                        usize::MAX,
                        None,
                    );
                    ui.add(egui::Label::new(galley).sense(sense))
                } else {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).font(font).color(color))
                            .sense(sense),
                    )
                };
                crate::autoscroll::row(ui, &response);
                // Fork: pronunciation and translation under the line.
                let rect = crate::ui::fork::lyrics_ai::under_line(app, ui, index, 14.0, color)
                    .map_or(response.rect, |extra| response.rect.union(extra));
                if lyrics.synced
                    && response.clicked()
                    && let Some(at_ms) = line.at_ms
                {
                    app.actions.push(Action::Seek(at_ms));
                    app.lyrics_following = true;
                }
                if is_active && follow {
                    show_sung_line_at(ui, rect, None, SUNG_LINE_AT);
                }
                ui.add_space(LINE_GAP);
            }
            // Words without timing can only be followed by the clock: sit
            // at the part of the text the song is probably at.
            if app.lyrics_following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(now.position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect(
                    egui::Rect::from_min_max(
                        egui::pos2(content.left(), y),
                        egui::pos2(content.right(), y + 1.0),
                    ),
                    Some(Align::Center),
                );
            }
            // Room for the last line to rise to where a sung line sits.
            ui.add_space((ui.clip_rect().height() * (1.0 - SUNG_LINE_AT)).max(60.0));
        },
    );
    crate::autoscroll::lyrics(ui, scroll.id);
    // Scrolling by hand means the reader wants to look elsewhere; the
    // Follow button in the header picks the song back up.
    if ui.rect_contains_pointer(scroll.inner_rect)
        && ui.input(|input| input.smooth_scroll_delta.y != 0.0)
    {
        app.lyrics_following = false;
    }
    app.lyrics_line_shown = Some(active);
}

/// Fork: the video, or the song's softened cover, filling `rect`; true
/// when it was the video.
pub(crate) fn backdrop(app: &mut App, ui: &egui::Ui, rect: Rect) -> bool {
    let art = app
        .now_playing()
        .and_then(|now| preferred_backdrop_art(now.art_small, now.art_url));
    let painter = ui.painter().with_clip_rect(rect);
    // Fork: the song's music video in place of its cover, once a frame of
    // it is ready; the cover stays until then, so nothing goes black.
    let video = crate::fork::lyrics_video::paint(app, &painter, rect);
    if video {
    } else if let Some(texture) =
        app.lyrics_backdrop
            .texture(ui.ctx(), app.backend.art(), art.as_deref())
    {
        painter.image(
            texture.id(),
            rect,
            cover_uv(rect.size(), texture.size_vec2()),
            Color32::from_gray(180),
        );
    }
    video
}

/// Fork's copy of upstream's `background`, the music video in place
/// of the cover.
pub(super) fn background(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    theme::apply_local(ui, &theme::Palette::dark());
    #[cfg(feature = "milkdrop")]
    if milkdrop_backdrop(app, ui, rect) {
        return;
    }
    let video = backdrop(app, ui, rect);
    let painter = ui.painter().with_clip_rect(rect);
    // A moving video is busier than a blurred cover; darken it more so the
    // words stay readable.
    painter.rect_filled(
        rect,
        0.0,
        Color32::from_black_alpha(if video { 170 } else { 120 }),
    );
    widgets::paint_vertical_gradient(
        ui,
        rect,
        Color32::from_black_alpha(0),
        Color32::from_black_alpha(95),
    );
}

/// Fork's copy of upstream's `fullscreen_contents`, in the lyrics
/// page's `look` (full screen uses `Look::FULLSCREEN`).
pub(crate) fn fullscreen_contents(
    app: &mut App,
    ui: &mut egui::Ui,
    look: crate::fork::lyrics_page::Look,
) {
    let centred = look.centred;
    let palette = theme::Palette::dark();
    let Some(now) = app.now_playing() else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            widgets::loading_row(ui, &palette, app.locale);
            return;
        }
        Loadable::Failed(error) => {
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.text);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(app.locale, "Try again"), false).clicked()
            {
                app.actions.push(Action::RetryLyrics);
            }
            return;
        }
        Loadable::Loaded(None) => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Music,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics.clone(),
    };

    // Fork: the song's own lyrics offset (src/fork/lyrics_page.rs).
    let offset = app.settings.fork.lyrics_page.offset(&now.uri);
    let position = app
        .settings
        .fork
        .lyrics_page
        .position(&now.uri, now.position_ms);
    let active = lyrics.active_line(position);
    // A Follow click resets the remembered line after drawing. Other frames
    // record the shown line before any line-click action restores following.
    if !app
        .actions
        .iter()
        .any(|action| matches!(action, Action::FollowLyrics))
    {
        app.actions.push(Action::LyricsLineShown(active));
    }
    let viewport = ui.available_rect_before_wrap();
    let manual_scroll = ui.rect_contains_pointer(viewport)
        && ui.input(|input| {
            input.smooth_scroll_delta.y != 0.0
                || (input.pointer.primary_down() && input.pointer.delta().y != 0.0)
        });
    let following = look.follow && app.lyrics_following && !manual_scroll;
    let follow = following && app.lyrics_line_shown != Some(active);
    let animation = crate::motion::scroll(ui.ctx(), 0.45);
    let size = (ui.available_width() * 0.046).clamp(28.0, 42.0) * look.scale;
    // The line being sung takes the theme's accent (or, on the lyrics page,
    // the style's colour), though the view is always dark, and grows; each
    // line wraps for its full size all along, so growing can't rewrap its
    // words. A line takes 300 ms to light up or fade.
    // Fork page: Accent takes the cover's colour, lifted to read on a dark
    // backdrop; a theme's own accent can be near white.
    let bright = if look.style == crate::fork::lyrics_page::LineStyle::Accent {
        let art = now.art_small.clone().or_else(|| now.art_url.clone());
        app.tint_for(art.as_deref())
            .map_or(app.palette.accent, |tint| blend(tint, Color32::WHITE, 0.3))
    } else {
        app.palette.accent
    };
    let starts: Vec<Option<u32>> = lyrics.lines.iter().map(|line| line.at_ms).collect();
    // Real word times, when Unison has the song timed word by word.
    app.match_word_timing();
    let timings = if app.fork.word_timing.uri.as_deref() == Some(now.uri.as_str()) {
        app.fork.word_timing.matched.clone()
    } else {
        Vec::new()
    };
    let fill_for = |index: usize, quiet: Color32| {
        (look.word_highlight && lyrics.synced && active == Some(index)).then(|| {
            crate::ui::fork::lyrics_ai::WordFill {
                progress: crate::fork::lyrics_page::line_progress(&starts, index, position),
                sung: bright,
                unsung: quiet,
                times: timings.get(index).cloned().flatten(),
                position,
            }
        })
    };
    if look.motion != crate::fork::lyrics_page::Motion::None && now.playing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    if look.word_highlight && now.playing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    // A moving picture asks more of the quiet lines than a still one; the
    // lyrics page sets its own (Other lines).
    let quiet = palette
        .text
        .gamma_multiply(if app.milkdrop_behind_lyrics() {
            look.others.max(0.62)
        } else {
            look.others
        });
    if look.single_line && lyrics.synced {
        single_line(
            app, ui, &lyrics, active, size, look, bright, quiet, &fill_for,
        );
        return;
    }
    ui.spacing_mut().scroll.fade.strength = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(("fullscreen-lyrics-scroll", &now.uri))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Before the first line there is nothing to highlight, so the
            // panel sits at the top rather than wherever it was left.
            if follow && lyrics.synced && active.is_none() {
                let top = ui.cursor().min;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
                    Some(Align::Min),
                    animation,
                );
            }
            // Room for the first line to sit where a sung line sits, and for
            // the last to rise to it.
            let (padding, below) = if lyrics.synced {
                (
                    (viewport.height() * look.sung_at - size).max(12.0),
                    viewport.height() * (1.0 - look.sung_at),
                )
            } else {
                (12.0, 60.0)
            };
            ui.add_space(padding);
            // Fork: why the translation is missing, if it is.
            crate::ui::fork::lyrics_ai::notice(app, ui, palette.text.gamma_multiply(0.6));
            for (index, line) in lyrics.lines.iter().enumerate() {
                let is_active = active == Some(index);
                let lit = ui.ctx().animate_bool_with_time(
                    egui::Id::new("lyric-line").with(("fullscreen", &now.uri, index)),
                    is_active,
                    crate::motion::seconds(ui.ctx(), 0.3),
                );
                // Fade: lines quieten the further they are from the sung one.
                let quiet = match (look.style, active) {
                    (crate::fork::lyrics_page::LineStyle::Fade, Some(sung)) => {
                        let distance = sung.abs_diff(index) as f32;
                        quiet.gamma_multiply((1.0 - distance * 0.3).max(0.08))
                    }
                    _ => quiet,
                };
                let color = if lyrics.synced {
                    blend(quiet, bright, lit)
                } else {
                    palette.text
                };
                let fill = fill_for(index, quiet);
                let grown = if lyrics.synced {
                    1.0 + (SUNG_LINE_GROWTH - 1.0) * lit
                } else {
                    1.0
                };
                let wrap = ui.available_width() / SUNG_LINE_GROWTH * grown;
                let font = theme::bold(size * grown);
                // A timed line with no words is the band playing on.
                let text = if line.text.is_empty() && lyrics.synced {
                    "\u{266a}"
                } else {
                    line.text.as_str()
                };
                // Untimed words have nowhere to jump to, but a click on them
                // is still on a line, so it must not reach the backdrop.
                let sense = Sense::click();
                // Fork page: an instrumental break as dots.
                let dots = look.break_dots && lyrics.synced && line.text.is_empty();
                let galley = match fill.clone().filter(|_| !crate::bidi::is_rtl(text)) {
                    Some(fill) => crate::ui::fork::lyrics_ai::filled_galley(
                        ui.painter(),
                        text,
                        font.clone(),
                        wrap,
                        fill,
                    ),
                    None => crate::bidi::layout(
                        ui.painter(),
                        text,
                        font.clone(),
                        color,
                        wrap,
                        usize::MAX,
                        None,
                    ),
                };
                let center = ui.cursor().top() + galley.size().y * 0.5;
                let edge = ((center - viewport.top()).min(viewport.bottom() - center)
                    / (size * 2.0))
                    .clamp(0.0, 1.0);
                let time = ui.input(|input| input.time);
                let (float, pulse) = if is_active {
                    crate::ui::fork::lyrics_fx::moving(look.motion, time, size)
                } else {
                    (0.0, 1.0)
                };
                let (response, extra) = ui
                    .scope(|ui| {
                        ui.multiply_opacity(edge * edge * (3.0 - 2.0 * edge) * pulse);
                        let draw = |ui: &mut egui::Ui| {
                            if dots {
                                let progress = crate::fork::lyrics_page::line_progress(
                                    &starts, index, position,
                                );
                                let response = crate::ui::fork::lyrics_fx::break_dots(
                                    ui, size, color, progress, is_active, time,
                                );
                                return (response, None);
                            }
                            // Fork page: the translation as the line, the
                            // original small above it.
                            if look.translation_first
                                && let Some(drawn) = translation_first(
                                    app,
                                    ui,
                                    index,
                                    text,
                                    size,
                                    color,
                                    sense,
                                    fill.clone(),
                                )
                            {
                                return drawn;
                            }
                            // Fork: with its pronunciation above, word by word.
                            let response = crate::ui::fork::lyrics_ai::lyric_line(
                                app,
                                ui,
                                index,
                                text,
                                font.clone(),
                                (size * 0.5).max(15.0),
                                color,
                                sense,
                                fill.clone(),
                            )
                            .unwrap_or_else(|| {
                                // Fork page: glow and outline behind the words.
                                let slot = ui.painter().add(egui::Shape::Noop);
                                let response =
                                    ui.add(egui::Label::new(galley.clone()).sense(sense));
                                if look.glow || look.outline {
                                    crate::ui::fork::lyrics_fx::halo(
                                        ui.painter(),
                                        slot,
                                        &galley,
                                        response.rect.min,
                                        (look.glow && is_active).then_some(bright),
                                        look.outline,
                                    );
                                }
                                response
                            });
                            // Fork: pronunciation and translation under the line.
                            let extra = crate::ui::fork::lyrics_ai::under_line(
                                app,
                                ui,
                                index,
                                (size * 0.5).max(15.0),
                                color,
                            );
                            (response, extra)
                        };
                        let transform =
                            egui::emath::TSTransform::from_translation(egui::vec2(0.0, float));
                        ui.with_visual_transform(transform, |ui| {
                            if centred {
                                ui.vertical_centered(draw).inner
                            } else {
                                draw(ui)
                            }
                        })
                        .inner
                    })
                    .inner;
                let rect = extra.map_or(response.rect, |extra| response.rect.union(extra));
                if lyrics.synced
                    && response.clicked()
                    && let Some(at_ms) = line.at_ms
                {
                    let at_ms = u32::try_from((i64::from(at_ms) - i64::from(offset)).max(0))
                        .unwrap_or(at_ms);
                    app.actions.push(Action::Seek(at_ms));
                    app.actions.push(Action::FollowLyrics);
                }
                if is_active && follow {
                    show_sung_line_at(ui, rect, Some(animation), look.sung_at);
                }
                // Fork page: translated lines are groups; give them room.
                ui.add_space(if look.translation_first { 40.0 } else { 27.0 });
            }
            // Words without timing can only be followed by the clock: sit
            // at the part of the text the song is probably at.
            if following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(now.position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_max(
                        egui::pos2(content.left(), y),
                        egui::pos2(content.right(), y + 1.0),
                    ),
                    Some(Align::Center),
                    animation,
                );
            }
            ui.add_space(below.max(60.0));
        });
    // Scrolling by hand means the reader wants to look elsewhere; the
    // Follow button in the header picks the song back up.
    if manual_scroll && app.lyrics_following {
        app.actions.push(Action::PauseLyricsFollow);
    }
    if now.playing
        && lyrics.synced
        && let Some(next) = lyrics
            .lines
            .iter()
            .filter_map(|line| line.at_ms)
            .find(|at| *at > position)
    {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(u64::from(next - position)));
    }
}

/// Fork page: a translated line with the translation in the lyrics' own
/// font and size, the original (with its readings and meanings) smaller
/// above. `None` for a line without a translation, drawn as usual.
#[allow(clippy::too_many_arguments)]
fn translation_first(
    app: &App,
    ui: &mut egui::Ui,
    index: usize,
    text: &str,
    size: f32,
    color: Color32,
    sense: Sense,
    fill: Option<crate::ui::fork::lyrics_ai::WordFill>,
) -> Option<(egui::Response, Option<Rect>)> {
    let (_, translation) = app.fork.lyrics_ai.line(
        app.lyrics_uri.as_deref(),
        index,
        &app.settings.fork.lyrics_ai,
    );
    let translation = translation?.to_string();
    let small = theme::bold((size * 0.55).max(16.0));
    let original = crate::ui::fork::lyrics_ai::lyric_line(
        app,
        ui,
        index,
        text,
        small.clone(),
        (size * 0.38).max(12.0),
        color.gamma_multiply(0.8),
        sense,
        fill.clone(),
    )
    .unwrap_or_else(|| {
        let galley = match fill.filter(|_| !crate::bidi::is_rtl(text)) {
            Some(fill) => crate::ui::fork::lyrics_ai::filled_galley(
                ui.painter(),
                text,
                small.clone(),
                ui.available_width(),
                fill,
            ),
            None => crate::bidi::layout(
                ui.painter(),
                text,
                small.clone(),
                color.gamma_multiply(0.8),
                ui.available_width(),
                usize::MAX,
                None,
            ),
        };
        ui.add(egui::Label::new(galley).sense(sense))
    });
    ui.add_space(4.0);
    let galley = crate::bidi::layout(
        ui.painter(),
        &translation,
        theme::bold(size),
        color,
        ui.available_width(),
        usize::MAX,
        None,
    );
    let main = ui.add(egui::Label::new(galley).sense(sense));
    Some((original.union(main.clone()), Some(main.rect)))
}

/// Fork page: only the sung line shows, in the middle; the one before
/// leaves and the sung one enters, as the page's look says.
#[allow(clippy::too_many_arguments)]
fn single_line(
    app: &mut App,
    ui: &mut egui::Ui,
    lyrics: &crate::lyrics::Lyrics,
    active: Option<usize>,
    size: f32,
    look: crate::fork::lyrics_page::Look,
    bright: Color32,
    quiet: Color32,
    fill_for: &dyn Fn(usize, Color32) -> Option<crate::ui::fork::lyrics_ai::WordFill>,
) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let uri = app.lyrics_uri.clone().unwrap_or_default();
    let shown = active.unwrap_or(0);
    let time = ui.input(|input| input.time);
    let lines = shown.saturating_sub(1)..=(shown + 1).min(lyrics.lines.len().saturating_sub(1));
    for index in lines {
        let Some(line) = lyrics.lines.get(index) else {
            continue;
        };
        let current = index == shown;
        let t = ui.ctx().animate_bool_with_time(
            egui::Id::new("single-lyric-line").with((&uri, index)),
            current,
            crate::motion::seconds(ui.ctx(), 0.45),
        );
        if t <= 0.01 {
            continue;
        }
        let phase = if current {
            crate::ui::fork::lyrics_fx::entering(look.enter, t)
        } else {
            crate::ui::fork::lyrics_fx::leaving(look.leave, t)
        };
        let height_id = egui::Id::new("single-lyric-height").with((&uri, index));
        let height = ui
            .data(|data| data.get_temp::<f32>(height_id))
            .unwrap_or(size * 1.6);
        let top = rect.center().y - height / 2.0;
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    pos2(rect.left(), top),
                    pos2(rect.right(), top + height + size * 6.0),
                ))
                .layout(Layout::top_down(if look.centred {
                    Align::Center
                } else {
                    Align::Min
                })),
        );
        child.set_clip_rect(rect);
        let (float, pulse) = if current && active.is_some() {
            crate::ui::fork::lyrics_fx::moving(look.motion, time, size)
        } else {
            (0.0, 1.0)
        };
        child.multiply_opacity(phase.opacity * pulse);
        let centre = pos2(rect.center().x, top + height / 2.0);
        let transform = crate::ui::fork::lyrics_fx::transform(
            centre,
            phase.shift * size * 1.6 + float,
            phase.scale,
        );
        let color = if current && active.is_some() {
            bright
        } else {
            quiet
        };
        let fill = fill_for(index, quiet);
        child.with_visual_transform(transform, |ui| {
            let font = theme::bold(size * 1.15);
            if line.text.is_empty() && look.break_dots {
                let progress = crate::fork::lyrics_page::line_progress(
                    &starts_of(lyrics),
                    index,
                    now_position(app),
                );
                crate::ui::fork::lyrics_fx::break_dots(ui, size, color, progress, current, time);
            } else {
                let text = if line.text.is_empty() {
                    "\u{266a}"
                } else {
                    crate::ui::fork::lyrics_fx::typed(&line.text, phase.reveal)
                };
                // The translation as the line, as in the scrolling view.
                if look.translation_first
                    && phase.reveal >= 1.0
                    && translation_first(
                        app,
                        ui,
                        index,
                        text,
                        size * 1.15,
                        color,
                        Sense::hover(),
                        fill.clone(),
                    )
                    .is_some()
                {
                    return;
                }
                let shown_line = (phase.reveal >= 1.0)
                    .then(|| {
                        crate::ui::fork::lyrics_ai::lyric_line(
                            app,
                            ui,
                            index,
                            text,
                            font.clone(),
                            (size * 0.55).max(15.0),
                            color,
                            Sense::hover(),
                            fill.clone(),
                        )
                    })
                    .flatten();
                if shown_line.is_none() {
                    let slot = ui.painter().add(egui::Shape::Noop);
                    let galley = match fill.clone().filter(|_| !crate::bidi::is_rtl(text)) {
                        Some(fill) => crate::ui::fork::lyrics_ai::filled_galley(
                            ui.painter(),
                            text,
                            font,
                            ui.available_width(),
                            fill,
                        ),
                        None => crate::bidi::layout(
                            ui.painter(),
                            text,
                            font,
                            color,
                            ui.available_width(),
                            usize::MAX,
                            None,
                        ),
                    };
                    let response = ui.add(egui::Label::new(galley.clone()));
                    crate::ui::fork::lyrics_fx::halo(
                        ui.painter(),
                        slot,
                        &galley,
                        response.rect.min,
                        (look.glow && current).then_some(bright),
                        look.outline,
                    );
                }
                crate::ui::fork::lyrics_ai::under_line(
                    app,
                    ui,
                    index,
                    (size * 0.55).max(15.0),
                    color,
                );
            }
        });
        let drawn = child.min_rect().height();
        ui.data_mut(|data| data.insert_temp(height_id, drawn));
    }
    if look.motion != crate::fork::lyrics_page::Motion::None {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// Every line's start, for the progress through a line.
fn starts_of(lyrics: &crate::lyrics::Lyrics) -> Vec<Option<u32>> {
    lyrics.lines.iter().map(|line| line.at_ms).collect()
}

/// The playing position with the song's lyrics offset applied.
fn now_position(app: &App) -> u32 {
    app.now_playing().map_or(0, |now| {
        app.settings
            .fork
            .lyrics_page
            .position(&now.uri, now.position_ms)
    })
}
