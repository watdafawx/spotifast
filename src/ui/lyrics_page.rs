//! Fork: the lyrics page, which takes over the main area as ivLyrics'
//! page does in Spotify: the music video or cover behind, blurred and
//! dimmed to taste, the lyrics with their extra lines over it, a rail of
//! controls at the right, and the song's sync offset at the bottom left.

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, UiBuilder, pos2, vec2};

use crate::app::App;
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Icon, Palette};

/// Which floating panel of the rail is open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Appearance,
    Video,
    Ai,
}

const VIDEO_FIELD: &str = "lyrics-page-video-link";

const PANEL_ID: &str = "lyrics-page-panel";

/// The player-bar button that opens and closes the page.
pub fn button(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let open = app.lyrics_page_open;
    if theme::icon_button(
        ui,
        Icon::Tv,
        18.0,
        if open {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Lyrics page"),
    )
    .clicked()
    {
        app.actions.push(Action::ToggleLyricsPage);
    }
}

/// Draws the page into the rest of the main area. Called after the top
/// bar while the page is open.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, egui::Sense::hover());
    let dark = Palette::dark();
    let mut page = ui.new_child(UiBuilder::new().max_rect(rect));
    theme::apply_local(&mut page, &dark);
    page.set_clip_rect(rect);

    // The backdrop: the video, or the cover, then the listener's dimming.
    let video = super::lyrics::backdrop(app, &page, rect);
    if !video && app.settings.lyrics_page.blobs {
        // Slow colour blobs from the cover's colour over the cover.
        let art = app
            .now_playing()
            .and_then(|now| now.art_small.or(now.art_url));
        let tint = app.tint_for(art.as_deref()).unwrap_or(app.palette.accent);
        let colours = [
            tint,
            super::lyrics::blend(tint, Color32::WHITE, 0.25),
            super::lyrics::blend(tint, Color32::from_rgb(40, 0, 80), 0.4),
            super::lyrics::blend(tint, Color32::from_rgb(0, 60, 120), 0.4),
        ];
        let time = page.input(|input| input.time);
        super::lyrics_fx::blobs(page.painter(), rect, &colours, time);
        page.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    let dim = app.settings.lyrics_page.dim.clamp(0.0, 0.9);
    page.painter()
        .rect_filled(rect, 0.0, Color32::from_black_alpha((dim * 255.0) as u8));

    // The lyrics, in a column that leaves room for the rail.
    let rail_width = 56.0;
    // An open panel docks at the right, full height, and the lyrics and
    // rail move left to make room.
    let docked = ui
        .data(|data| data.get_temp::<Option<Panel>>(egui::Id::new(PANEL_ID)))
        .flatten()
        .is_some();
    // The panel slides in and out; the lyrics and rail move with it.
    let slide = ui.ctx().animate_bool_with_time(
        egui::Id::new("lyrics-page-dock"),
        docked,
        crate::motion::seconds(ui.ctx(), 0.28),
    );
    let slide = slide * slide * (3.0 - 2.0 * slide);
    let full = rect;
    let edge = full.right() - PANEL_WIDTH * slide;
    let dock = Rect::from_min_max(
        pos2(edge, full.top()),
        pos2(edge + PANEL_WIDTH, full.bottom()),
    );
    let rect = Rect::from_min_max(rect.min, pos2(edge, rect.bottom()));
    let column = (rect.width() - rail_width * 2.0 - 48.0).clamp(240.0, 1100.0);
    let lyrics_rect = Rect::from_min_max(
        pos2(rect.center().x - column / 2.0, rect.top() + 16.0),
        pos2(rect.center().x + column / 2.0, rect.bottom() - 56.0),
    );
    let mut lyrics = page.new_child(UiBuilder::new().max_rect(lyrics_rect));
    let look = app.settings.lyrics_page.look();
    super::lyrics::page_contents(app, &mut lyrics, look);

    rail(app, &mut page, rect, edge, dock, video);
    offset_bar(app, &mut page, rect);
    if crate::lyrics_video::loading(app) {
        // The video is on its way; the cover holds the place till then.
        let spot = Rect::from_min_size(
            pos2(rect.right() - 170.0, rect.top() + 14.0),
            vec2(150.0, 28.0),
        );
        let mut corner = page.new_child(
            UiBuilder::new()
                .max_rect(spot)
                .layout(Layout::right_to_left(Align::Center)),
        );
        Frame::new()
            .fill(Color32::from_black_alpha(140))
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::symmetric(10, 4))
            .show(&mut corner, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0).color(dark.text));
                    theme::text(
                        ui,
                        gettext(app.locale, "Loading video"),
                        theme::regular(12.0),
                        dark.text,
                    );
                });
            });
    }
}

/// How wide an open panel is, docked at the right of the page.
const PANEL_WIDTH: f32 = 360.0;
/// The panel and the rail share one surface, so the rail reads as the
/// panel's edge.
const SURFACE: Color32 = Color32::from_black_alpha(205);
/// The panel last open, kept while it slides away.
const SHOWN_ID: &str = "lyrics-page-panel-shown";

/// The rail of controls, fixed to the right edge of the page or of the open
/// panel, and the panel itself, sliding in from the right. `edge` is where
/// the rail meets the panel; `dock` is where the panel is drawn now.
fn rail(app: &mut App, ui: &mut egui::Ui, page: Rect, edge: f32, dock: Rect, video_shown: bool) {
    let palette = Palette::dark();
    let locale = app.locale;
    let open = ui.data(|data| {
        data.get_temp::<Option<Panel>>(egui::Id::new(PANEL_ID))
            .flatten()
    });
    let labels = app.settings.lyrics_page.rail_labels;
    let video = crate::lyrics_video::available();
    let count = if video { 11.0 } else { 9.0 };
    let width = if labels { 210.0 } else { 48.0 };
    let row = if labels { 36.0 } else { 40.0 };
    let height = count * row + 20.0;
    let rail = Rect::from_min_size(
        pos2(edge - width, page.center().y - height / 2.0),
        vec2(width, height),
    );
    let mut changed = false;
    let mut next_panel = open;
    let mut column = ui.new_child(UiBuilder::new().max_rect(rail).layout(Layout::top_down(
        if labels { Align::Min } else { Align::Center },
    )));
    let flip = |panel: Panel| {
        if open == Some(panel) {
            None
        } else {
            Some(panel)
        }
    };
    // Rounded only on the side away from the edge it sits on.
    let radius = theme::RADIUS + 4;
    Frame::new()
        .fill(SURFACE)
        .corner_radius(CornerRadius {
            nw: radius,
            sw: radius,
            ne: 0,
            se: 0,
        })
        .inner_margin(Margin::symmetric(6, 10))
        .show(&mut column, |ui| {
            ui.set_width(width - 12.0);
            ui.spacing_mut().item_spacing.y = if labels { 2.0 } else { 6.0 };
            // An icon, or with labels shown, a row like the sidebar's. A
            // control whose panel is open is marked like a selected tab.
            let button = |ui: &mut egui::Ui, icon: Icon, on: bool, label: &str| {
                if labels {
                    label_row(ui, &palette, icon, label, on).clicked()
                } else {
                    icon_tab(ui, &palette, icon, label, on).clicked()
                }
            };
            let divider = |ui: &mut egui::Ui| {
                ui.add_space(2.0);
                let (line, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().hline(
                    line.x_range().shrink(6.0),
                    line.center().y,
                    (1.0, Color32::from_white_alpha(28)),
                );
                ui.add_space(2.0);
            };
            // Quick switches.
            if button(
                ui,
                Icon::LocateFixed,
                app.settings.lyrics_page.follow,
                &gettext(locale, "Follow the song"),
            ) {
                app.settings.lyrics_page.follow = !app.settings.lyrics_page.follow;
                app.actions.push(Action::FollowLyrics);
                changed = true;
            }
            if button(
                ui,
                Icon::Globe,
                app.settings.lyrics_ai.translate,
                &gettext(locale, "Translation"),
            ) {
                app.settings.lyrics_ai.translate = !app.settings.lyrics_ai.translate;
                changed = true;
            }
            if button(
                ui,
                Icon::Mic,
                app.settings.lyrics_ai.pronounce,
                &gettext(locale, "Pronunciation"),
            ) {
                app.settings.lyrics_ai.pronounce = !app.settings.lyrics_ai.pronounce;
                changed = true;
            }
            if video
                && button(
                    ui,
                    Icon::ListVideo,
                    app.settings.lyrics_video,
                    &gettext(locale, "Video background"),
                )
            {
                app.settings.lyrics_video = !app.settings.lyrics_video;
                changed = true;
            }
            if button(
                ui,
                Icon::LayoutList,
                app.settings.lyrics_page.centred,
                &gettext(locale, "Centre the lines"),
            ) {
                app.settings.lyrics_page.centred = !app.settings.lyrics_page.centred;
                changed = true;
            }
            divider(ui);
            // Panels.
            if button(
                ui,
                Icon::Palette,
                open == Some(Panel::Appearance),
                &gettext(locale, "Appearance"),
            ) {
                next_panel = flip(Panel::Appearance);
            }
            if button(
                ui,
                Icon::Sparkles,
                open == Some(Panel::Ai),
                &gettext(locale, "AI options"),
            ) {
                next_panel = flip(Panel::Ai);
            }
            if video
                && button(
                    ui,
                    Icon::Pencil,
                    open == Some(Panel::Video),
                    &gettext(locale, "Change video"),
                )
            {
                next_panel = flip(Panel::Video);
            }
            divider(ui);
            if button(
                ui,
                Icon::Expand,
                false,
                &gettext(locale, "Full screen lyrics"),
            ) {
                app.actions.push(Action::SetLyricsFullscreen(true));
            }
            let (icon, label) = if labels {
                (Icon::ChevronRight, gettext(locale, "Hide labels"))
            } else {
                (Icon::ChevronLeft, gettext(locale, "Show labels"))
            };
            if button(ui, icon, false, &label) {
                app.settings.lyrics_page.rail_labels = !labels;
                changed = true;
            }
        });

    // The panel: the one open, or the one sliding away.
    let shown_id = egui::Id::new(SHOWN_ID);
    if let Some(panel) = next_panel.or(open) {
        ui.data_mut(|data| data.insert_temp(shown_id, panel));
    }
    let shown = ui.data(|data| data.get_temp::<Panel>(shown_id));
    if let Some(which) = shown
        // Only while some of it is on screen.
        && dock.left() < ui.clip_rect().right() - 1.0
    {
        let mut area = ui.new_child(UiBuilder::new().max_rect(dock));
        area.set_clip_rect(dock.intersect(ui.clip_rect()));
        Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin {
                left: 20,
                right: 18,
                top: 16,
                bottom: 16,
            })
            .show(&mut area, |ui| {
                ui.set_width(dock.width() - 38.0);
                ui.set_min_height(dock.height() - 32.0);
                let title = match which {
                    Panel::Appearance => gettext(locale, "Appearance"),
                    Panel::Ai => gettext(locale, "AI options"),
                    Panel::Video => gettext(locale, "Change video"),
                };
                ui.horizontal(|ui| {
                    theme::text(ui, title, theme::bold(18.0), palette.text);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(
                            ui,
                            Icon::X,
                            16.0,
                            palette.secondary,
                            palette.text,
                            &gettext(locale, "Close"),
                        )
                        .clicked()
                        {
                            next_panel = None;
                        }
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .id_salt(("lyrics-page-panel", which as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Room at the right for the scroll bar.
                        ui.set_width(ui.available_width() - 14.0);
                        match which {
                            Panel::Appearance => {
                                if appearance(app, ui, &palette, video_shown) {
                                    changed = true;
                                }
                            }
                            Panel::Ai => {
                                if ai_panel(app, ui, &palette) {
                                    changed = true;
                                }
                            }
                            Panel::Video => {
                                if change_video(app, ui, &palette) {
                                    next_panel = None;
                                }
                            }
                        }
                    });
            });
    }
    // A click outside the panel and the rail closes the panel; one in a
    // dropdown the panel opened does not.
    if next_panel.is_some() && !egui::Popup::is_any_open(ui.ctx()) {
        let outside = ui.input(|input| {
            input.pointer.primary_clicked()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|at| !dock.contains(at) && !rail.contains(at))
        });
        if outside {
            next_panel = None;
        }
    }
    ui.data_mut(|data| data.insert_temp(egui::Id::new(PANEL_ID), next_panel));
    if changed {
        app.actions.push(Action::SettingsChanged);
    }
}

/// A rail icon; the one whose panel is open carries a marker on its edge.
fn icon_tab(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    on: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if response.hovered() || on {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(theme::RADIUS_SMALL),
                Color32::from_white_alpha(if on { 22 } else { 14 }),
            );
        }
        let color = if on {
            palette.accent
        } else if response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        icon.image(color, 18.0)
            .paint_at(ui, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)));
    }
    let response = response.on_hover_text(label);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, label)
    });
    theme::focus_ring(ui, &response);
    response
}

/// A small grey heading for a group of settings.
fn heading(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    // A line above each group, so the panel reads in sections.
    ui.add_space(8.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        (1.0, Color32::from_white_alpha(24)),
    );
    ui.add_space(10.0);
    theme::text(
        ui,
        text.to_uppercase(),
        theme::semibold(11.0),
        palette.secondary,
    );
    ui.add_space(4.0);
}

/// A label with a switch at the right of the panel.
fn switch_row(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        theme::text(ui, label, theme::regular(14.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = super::widgets::switch(ui, palette, label, value).changed();
        });
    });
    ui.add_space(1.0);
    changed
}

/// A label, a slider and its value, on one line.
fn slider_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    percent: bool,
) -> bool {
    let mut shown = if percent { *value * 100.0 } else { *value };
    let range = if percent {
        (*range.start() * 100.0)..=(*range.end() * 100.0)
    } else {
        range
    };
    let changed = ui
        .horizontal(|ui| {
            let (label_rect, _) = ui.allocate_exact_size(vec2(96.0, 22.0), egui::Sense::hover());
            ui.painter().text(
                label_rect.left_center(),
                egui::Align2::LEFT_CENTER,
                label,
                theme::regular(14.0),
                palette.text,
            );
            ui.spacing_mut().slider_width = (ui.available_width() - 64.0).max(60.0);
            ui.add(
                egui::Slider::new(&mut shown, range)
                    .suffix(if percent { " %" } else { "" })
                    .integer(),
            )
            .changed()
        })
        .inner;
    if changed {
        *value = if percent { shown / 100.0 } else { shown };
    }
    ui.add_space(2.0);
    changed
}

/// A row of choices; the index of the one picked, if a new one was.
fn choice_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    options: &[(String, bool)],
) -> Option<usize> {
    theme::text(ui, label, theme::regular(14.0), palette.text);
    ui.add_space(2.0);
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (index, (name, on)) in options.iter().enumerate() {
            if theme::soft_button(ui, palette, None, name, *on).clicked() && !on {
                picked = Some(index);
            }
        }
    });
    ui.add_space(4.0);
    picked
}

/// The Appearance panel, in groups. True when a setting changed.
fn appearance(app: &mut App, ui: &mut egui::Ui, palette: &Palette, video_shown: bool) -> bool {
    use crate::lyrics_page::{Enter, Leave, LineStyle, Motion};
    let locale = app.locale;
    let page = &mut app.settings.lyrics_page;
    let mut changed = false;

    heading(ui, palette, &gettext(locale, "Background"));
    if video_shown || crate::lyrics_video::available() {
        changed |= slider_row(
            ui,
            palette,
            &gettext(locale, "Video blur"),
            &mut page.blur,
            0.0..=40.0,
            false,
        );
    }
    changed |= slider_row(
        ui,
        palette,
        &gettext(locale, "Dimming"),
        &mut page.dim,
        0.0..=0.9,
        true,
    );
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Moving colours without a video"),
        &mut page.blobs,
    );

    heading(ui, palette, &gettext(locale, "Text"));
    changed |= slider_row(
        ui,
        palette,
        &gettext(locale, "Size"),
        &mut page.text_scale,
        0.6..=1.8,
        true,
    );
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Translation first"),
        &mut page.translation_first,
    );
    changed |= switch_row(ui, palette, &gettext(locale, "Outline"), &mut page.outline);

    heading(ui, palette, &gettext(locale, "Sung line"));
    let styles: Vec<(String, bool)> = LineStyle::ALL
        .iter()
        .map(|style| {
            let name = match style {
                LineStyle::Classic => gettext(locale, "Classic"),
                LineStyle::Accent => gettext(locale, "Accent"),
                LineStyle::Fade => gettext(locale, "Fade"),
            };
            (name.to_string(), page.style == *style)
        })
        .collect();
    if let Some(index) = choice_row(ui, palette, &gettext(locale, "Style"), &styles) {
        page.style = LineStyle::ALL[index];
        changed = true;
    }
    let motions: Vec<(String, bool)> = Motion::ALL
        .iter()
        .zip([
            gettext(locale, "Still"),
            gettext(locale, "Float"),
            gettext(locale, "Pulse"),
        ])
        .map(|(motion, name)| (name.to_string(), page.motion == *motion))
        .collect();
    if let Some(index) = choice_row(ui, palette, &gettext(locale, "Movement"), &motions) {
        page.motion = Motion::ALL[index];
        changed = true;
    }
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Light up word by word"),
        &mut page.word_highlight,
    );
    changed |= switch_row(ui, palette, &gettext(locale, "Glow"), &mut page.glow);
    changed |= slider_row(
        ui,
        palette,
        &gettext(locale, "Other lines"),
        &mut page.others,
        0.1..=1.0,
        true,
    );

    heading(ui, palette, &gettext(locale, "Single-line view"));
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Only the sung line"),
        &mut page.single_line,
    );
    let enters: Vec<(String, bool)> = Enter::ALL
        .iter()
        .zip([
            gettext(locale, "Fade"),
            gettext(locale, "Rise"),
            gettext(locale, "Typewriter"),
            gettext(locale, "Zoom"),
        ])
        .map(|(enter, name)| (name.to_string(), page.enter == *enter))
        .collect();
    if let Some(index) = choice_row(ui, palette, &gettext(locale, "Lines enter"), &enters) {
        page.enter = Enter::ALL[index];
        changed = true;
    }
    let leaves: Vec<(String, bool)> = Leave::ALL
        .iter()
        .zip([
            gettext(locale, "Fade"),
            gettext(locale, "Drift up"),
            gettext(locale, "Zoom"),
        ])
        .map(|(leave, name)| (name.to_string(), page.leave == *leave))
        .collect();
    if let Some(index) = choice_row(ui, palette, &gettext(locale, "Lines leave"), &leaves) {
        page.leave = Leave::ALL[index];
        changed = true;
    }

    heading(ui, palette, &gettext(locale, "Breaks"));
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Dots through instrumental breaks"),
        &mut page.break_dots,
    );
    changed
}

/// The AI options panel, in groups. True when a setting changed.
fn ai_panel(app: &mut App, ui: &mut egui::Ui, palette: &Palette) -> bool {
    let locale = app.locale;
    let mut changed = false;
    heading(ui, palette, &gettext(locale, "Lines"));
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Translation"),
        &mut app.settings.lyrics_ai.translate,
    );
    changed |= switch_row(
        ui,
        palette,
        &gettext(locale, "Pronunciation"),
        &mut app.settings.lyrics_ai.pronounce,
    );
    ui.horizontal(|ui| {
        theme::text(
            ui,
            gettext(locale, "Translate into"),
            theme::regular(14.0),
            palette.text,
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed |= super::lyrics_ai::target_picker(app, ui);
        });
    });

    heading(ui, palette, &gettext(locale, "Provider"));
    ui.add(
        egui::Label::new(
            egui::RichText::new(gettext(
                locale,
                "Each song's lyrics are sent once, with your key, and the answer is kept on this computer.",
            ))
            .font(theme::regular(12.0))
            .color(palette.secondary),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    changed |= super::lyrics_ai::provider_and_key(app, ui, palette);

    heading(ui, palette, &gettext(locale, "Video"));
    if crate::lyrics_video::available() {
        changed |= switch_row(
            ui,
            palette,
            &gettext(locale, "Video background"),
            &mut app.settings.lyrics_video,
        );
    }
    #[cfg(feature = "mods")]
    {
        changed |= switch_row(
            ui,
            palette,
            &gettext(locale, "ivLyrics (web mod)"),
            &mut app.settings.mods_enabled,
        );
    }
    changed
}

/// The Change video panel. True when a choice was made, to close it.
fn change_video(app: &mut App, ui: &mut egui::Ui, palette: &Palette) -> bool {
    let locale = app.locale;
    let field_id = egui::Id::new(VIDEO_FIELD);
    let mut link = ui
        .data(|data| data.get_temp::<String>(field_id))
        .unwrap_or_default();
    let mut done = false;
    super::widgets::labeled_field(
        ui,
        palette,
        locale,
        &gettext(locale, "YouTube link for this song"),
        &mut link,
        "https://youtube.com/watch?v=...",
        false,
    );
    ui.add_space(10.0);
    let id = crate::lyrics_video::youtube_id(&link);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if ui
            .add_enabled_ui(id.is_some(), |ui| {
                theme::pill_button(ui, palette, &gettext(locale, "Use"), true)
            })
            .inner
            .clicked()
            && let Some(id) = id
        {
            app.actions.push(Action::ChooseLyricsVideo(
                crate::lyrics_video::Choice::Video(id),
            ));
            link.clear();
            done = true;
        }
        if theme::pill_button(ui, palette, &gettext(locale, "Find again"), false).clicked() {
            app.actions.push(Action::ChooseLyricsVideo(
                crate::lyrics_video::Choice::Automatic,
            ));
            done = true;
        }
        if theme::pill_button(ui, palette, &gettext(locale, "No video"), false).clicked() {
            app.actions.push(Action::ChooseLyricsVideo(
                crate::lyrics_video::Choice::Nothing,
            ));
            done = true;
        }
    });
    ui.add_space(10.0);
    theme::text(
        ui,
        gettext(locale, "Your pick is kept for this song for a year."),
        theme::regular(12.0),
        palette.secondary,
    );
    ui.data_mut(|data| data.insert_temp(field_id, link));
    done
}

/// A rail row with its label: the sidebar's look, smaller. An option that
/// is on shows its icon in the accent colour.
fn label_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    on: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(theme::RADIUS_SMALL),
                Color32::from_white_alpha(14),
            );
        }
        let text = if on || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        let icon_rect =
            Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(18.0, 18.0));
        icon.image(if on { palette.accent } else { text }, 18.0)
            .paint_at(ui, icon_rect);
        ui.painter().text(
            pos2(rect.left() + 38.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            theme::medium(14.0),
            text,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, label)
    });
    theme::focus_ring(ui, &response);
    response
}

/// The song's sync offset, as ivLyrics has it: nudge the lyrics earlier
/// or later, and back to none.
fn offset_bar(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let Some(uri) = app.lyrics_uri.clone() else {
        return;
    };
    let palette = Palette::dark();
    let offset = app.settings.lyrics_page.offset(&uri);
    let bar = Rect::from_min_size(
        pos2(rect.left() + 16.0, rect.bottom() - 48.0),
        vec2(460.0, 36.0),
    );
    let mut area = ui.new_child(
        UiBuilder::new()
            .max_rect(bar)
            .layout(Layout::left_to_right(Align::Center)),
    );
    Frame::new()
        .fill(Color32::from_black_alpha(140))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(10, 4))
        .show(&mut area, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // Folded into a clock until wanted; it shows the song's offset
            // when there is one.
            let open_id = egui::Id::new("lyrics-page-offset-open");
            let open = ui
                .data(|data| data.get_temp::<bool>(open_id))
                .unwrap_or(false);
            if theme::icon_button(
                ui,
                Icon::Clock,
                18.0,
                if offset != 0 {
                    palette.accent
                } else {
                    palette.text
                },
                palette.text,
                &gettext(app.locale, "Lyrics offset"),
            )
            .clicked()
            {
                ui.data_mut(|data| data.insert_temp(open_id, !open));
            }
            if !open {
                if offset != 0 {
                    let shown = if offset > 0 {
                        format!("+{offset}ms")
                    } else {
                        format!("{offset}ms")
                    };
                    theme::text(ui, shown, theme::medium(12.0), palette.text);
                }
                return;
            }
            let mut nudge = None;
            for delta in [-100, -50, -10] {
                if theme::soft_button(ui, &palette, None, &format!("{delta}ms"), false).clicked() {
                    nudge = Some(delta);
                }
            }
            let shown = if offset > 0 {
                format!("+{offset}ms")
            } else {
                format!("{offset}ms")
            };
            if theme::soft_button(ui, &palette, None, &shown, offset != 0)
                .on_hover_text(gettext(
                    app.locale,
                    "Lyrics offset for this song; click to reset",
                ))
                .clicked()
            {
                nudge = Some(0);
            }
            for delta in [10, 50, 100] {
                if theme::soft_button(ui, &palette, None, &format!("+{delta}ms"), false).clicked() {
                    nudge = Some(delta);
                }
            }
            if let Some(delta) = nudge {
                app.settings.lyrics_page.nudge(&uri, delta);
                app.lyrics_line_shown = None;
                app.actions.push(Action::SettingsChanged);
            }
        });
}
