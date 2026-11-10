//! Fork: the Library collapsed to a strip of covers (#298). Upstream's
//! `show` hands over to the fork's copy at its first line; the strip builds
//! its shelf with a copy of the entries `contents` builds. When upstream
//! changes either, the sync workflow opens an issue to port it (FORK.md).

use super::*;

/// The fork's `show` replaces upstream's. A constant, so upstream's body
/// still compiles and stays easy to diff against.
pub(super) const REPLACES: bool = true;

/// Hook in `contents` after the Library icon: the icon collapses the
/// Library, as in Spotify, keeping the static icon's footprint.
pub(super) fn collapse_button(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let rect = ui.min_rect();
    let label = gettext(app.locale, "Collapse Your Library");
    let response = ui.interact(rect, ui.id().with("collapse-library"), Sense::click());
    if response.hovered() {
        // Upstream draws the icon after 6 points of space.
        let icon = Rect::from_min_size(
            pos2(rect.left() + 6.0, rect.center().y - 11.0),
            Vec2::splat(22.0),
        );
        ui.painter().rect_filled(icon, 0.0, palette.panel);
        Icon::Library.image(palette.text, 22.0).paint_at(ui, icon);
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label)
    });
    theme::focus_ring(ui, &response);
    if response.on_hover_text(label.as_ref()).clicked() {
        app.actions
            .push(Action::Fork(crate::fork::Action::SetLibraryCollapsed(true)));
    }
}

pub(super) fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let expanded_art = has_expanded_art(app);
    let floating_art = app.settings.sidebar_grid && expanded_art;
    // The traffic lights float over the top-left of the sidebar now, so the
    // first nav row has to start below them.
    let top = 12
        + theme::titlebar_inset(ui.ctx()) as i8
        + crate::ui::linux_left_controls_inset(ui.ctx()) as i8;
    let beside = if app.show_queue_panel || app.show_lyrics_panel || app.show_friends_panel {
        theme::SIDE_PANEL_MIN_WIDTH
    } else {
        0.0
    };
    let fit = crate::ui::yielding_panel(
        ui.ctx(),
        "sidebar",
        crate::ui::SIDEBAR_MIN_WIDTH..=600.0,
        app.settings.sidebar_width,
        ui.available_width() - crate::ui::topbar::least_width(ui.ctx()) - beside,
    );
    let panel = egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(app.settings.sidebar_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.panel).inner_margin(Margin {
            left: 12,
            right: 8,
            top,
            bottom: if expanded_art { 0 } else { 8 },
        }));
    // Collapsed, the Library is a strip of covers. Dragging the edge past
    // the strip's width, either way, switches between the two (#298).
    let collapsed_panel = egui::Panel::left("sidebar-collapsed")
        .resizable(true)
        .default_size(COLLAPSED_WIDTH)
        .size_range(COLLAPSED_WIDTH..=COLLAPSED_WIDTH)
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.panel).inner_margin(Margin {
            left: 8,
            right: 8,
            top,
            bottom: 8,
        }));
    let mut is_expanded = !app.settings.fork.sidebar_collapsed;
    let response = egui::Panel::show_switched(
        ui,
        &mut is_expanded,
        collapsed_panel,
        panel,
        |ui, show_expanded| {
            if !show_expanded {
                collapsed_contents(app, ui);
                return;
            }
            let art_rect = expanded_art.then(|| expanded_art_rect(ui));
            if let Some(rect) = art_rect.filter(|_| !floating_art) {
                reserve_expanded_art(ui, rect);
            }
            contents(app, ui, art_rect.filter(|_| floating_art));
            if let Some(rect) = art_rect {
                if floating_art {
                    paint_grid_art_mask(app, ui, rect);
                }
                paint_expanded_art(app, ui, rect);
            }
        },
    );
    if is_expanded == app.settings.fork.sidebar_collapsed {
        app.actions
            .push(Action::Fork(crate::fork::Action::SetLibraryCollapsed(
                !is_expanded,
            )));
    }
    // Only the open Library's width is remembered; the strip and the slide
    // between the two are not a width anyone chose.
    let width = response.response.rect.width();
    if is_expanded
        && !app.settings.fork.sidebar_collapsed
        && width >= crate::ui::SIDEBAR_MIN_WIDTH - 0.5
        && (width - app.settings.sidebar_width).abs() > 1.0
        && crate::ui::panel_width_chosen(ui.ctx(), "sidebar", &fit)
    {
        app.settings.sidebar_width = width;
        app.actions.push(Action::SettingsChanged);
    }
}

/// One Library shelf as both the full sidebar and the collapsed strip show
/// it: its entries in order, and whether more are coming.
struct Shelf {
    entries: Vec<Entry>,
    loading: bool,
    error: Option<String>,
    more_page: Option<Page>,
}

/// Builds `filter`'s shelf, matching `needle`, and makes sure it is loading.
fn shelf(app: &mut App, filter: Filter, sort: LibrarySort, needle: &str) -> Shelf {
    // Make sure the selected shelf is loading.
    match filter {
        Filter::Playlists => {}
        Filter::Albums => {
            if !app.library.albums.loading
                && app.library.albums.error.is_none()
                && (!app.library.albums.loaded_once
                    || (sort != LibrarySort::Library && app.library.albums.can_load_more()))
            {
                app.actions.push(Action::LoadMore(Page::Albums));
            }
        }
        Filter::Artists => {
            if !app.library.artists.loading
                && app.library.artists.error.is_none()
                && (!app.library.artists.loaded_once
                    || (sort != LibrarySort::Library && app.library.artists.can_load_more()))
            {
                app.actions.push(Action::LoadMore(Page::Artists));
            }
        }
        Filter::Podcasts => {
            if !app.library.shows.loading
                && app.library.shows.error.is_none()
                && (!app.library.shows.loaded_once
                    || (sort != LibrarySort::Library && app.library.shows.can_load_more()))
            {
                app.actions.push(Action::LoadMore(Page::Podcasts));
            }
        }
    }

    let locale = app.locale;
    let mut entries: Vec<Entry> = Vec::new();
    let art = EntryArt::of(app);
    let mut loading = false;
    let mut error: Option<String> = None;
    let mut more_page: Option<Page> = None;
    match filter {
        Filter::Playlists => {
            playlist_rows(app, sort, needle, &mut entries);
            match &app.library.playlists {
                Loadable::Loaded(_) => {}
                Loadable::Loading | Loadable::NotLoaded => loading = true,
                Loadable::Failed(message) => error = Some(message.clone()),
            }
        }
        Filter::Albums => {
            for saved in &app.library.albums.items {
                let album = &saved.album;
                if !needle.is_empty()
                    && !album.name.to_lowercase().contains(needle)
                    && !album
                        .artists
                        .iter()
                        .any(|a| a.name.to_lowercase().contains(needle))
                {
                    continue;
                }
                let artists = crate::api::models::join_names(
                    album.artists.iter().map(|artist| artist.name.as_str()),
                );
                entries.push(Entry {
                    image: pick_image(&album.images, art.row).map(str::to_string),
                    grid_image: pick_image(&album.images, art.grid).map(str::to_string),
                    name: album.name.clone(),
                    subtitle: format!("{} • {artists}", app.album_kind_label(album)),
                    grid_subtitle: artists,
                    page: Page::Album(album.id.clone()),
                    uri: album.uri.clone(),
                    round: false,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: None,
                    depth: 0,
                    added_at: saved_time(saved.added_at.as_deref()),
                });
            }
            loading = app.library.albums.loading && app.library.albums.items.is_empty();
            error = app.library.albums.error.clone();
            if app.library.albums.error.is_none() && app.library.albums.can_load_more() {
                more_page = Some(Page::Albums);
            }
        }
        Filter::Artists => {
            for artist in &app.library.artists.items {
                if !needle.is_empty() && !artist.name.to_lowercase().contains(needle) {
                    continue;
                }
                entries.push(Entry {
                    image: pick_image(&artist.images, art.row).map(str::to_string),
                    grid_image: pick_image(&artist.images, art.grid).map(str::to_string),
                    name: artist.name.clone(),
                    subtitle: gettext(locale, "Artist").into_owned(),
                    grid_subtitle: String::new(),
                    page: Page::Artist(artist.id.clone()),
                    uri: artist.uri.clone(),
                    round: true,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: None,
                    depth: 0,
                    added_at: None,
                });
            }
            loading = app.library.artists.loading && app.library.artists.items.is_empty();
            error = app.library.artists.error.clone();
            if app.library.artists.error.is_none() && app.library.artists.can_load_more() {
                more_page = Some(Page::Artists);
            }
        }
        Filter::Podcasts => {
            for saved in &app.library.shows.items {
                let show = &saved.show;
                // Audiobooks arrive as shows, but librespot can't play them.
                if app.audiobook_shows.contains(&show.uri) {
                    continue;
                }
                if !needle.is_empty() && !show.name.to_lowercase().contains(needle) {
                    continue;
                }
                entries.push(Entry {
                    image: pick_image(&show.images, art.row).map(str::to_string),
                    grid_image: pick_image(&show.images, art.grid).map(str::to_string),
                    name: show.name.clone(),
                    // Translators: {publisher} is the podcast's publisher.
                    subtitle: gettext(locale, "Podcast • {publisher}")
                        .replace("{publisher}", &show.publisher),
                    grid_subtitle: show.publisher.clone(),
                    page: Page::Show(show.id.clone()),
                    uri: show.uri.clone(),
                    round: false,
                    liked: false,
                    owned: false,
                    editable: false,
                    playlist_index: None,
                    folder: None,
                    depth: 0,
                    added_at: saved_time(saved.added_at.as_deref()),
                });
            }
            loading = app.library.shows.loading && app.library.shows.items.is_empty();
            error = app.library.shows.error.clone();
            if app.library.shows.error.is_none() && app.library.shows.can_load_more() {
                more_page = Some(Page::Podcasts);
            }
        }
    }

    order_entries(app, filter, sort, &mut entries);
    Shelf {
        entries,
        loading,
        error,
        more_page,
    }
}

/// The collapsed Library's width, as Spotify's.
pub(crate) const COLLAPSED_WIDTH: f32 = 72.0;
const COLLAPSED_TILE: f32 = 48.0;
const COLLAPSED_ROW_HEIGHT: f32 = 56.0;

/// A square button for the collapsed strip, centred, its name on hover.
fn strip_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
    active: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let color = if active || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        icon.image(color, 22.0)
            .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(22.0)));
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label)
    });
    theme::focus_ring(ui, &response);
    response.on_hover_text(label)
}

/// The collapsed Library: the nav, then the current shelf as covers with
/// their names on hover. A cover does what its row does: open on click,
/// play on double click, the same menu, drag, and drop songs on it.
fn collapsed_contents(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let page = app.page().clone();
    let locale = app.locale;
    ui.add_space(4.0);
    if strip_button(
        ui,
        &palette,
        Icon::House,
        &gettext(locale, "Home"),
        page == Page::Home,
    )
    .clicked()
    {
        app.actions.push(Action::Open(Page::Home));
    }
    if strip_button(
        ui,
        &palette,
        Icon::Search,
        &gettext(locale, "Search"),
        page == Page::Search,
    )
    .clicked()
    {
        app.actions.push(Action::FocusSearch);
    }
    ui.add_space(6.0);
    ui.painter().hline(
        ui.max_rect().x_range().shrink(4.0),
        ui.cursor().top(),
        egui::Stroke::new(1.0, palette.outline),
    );
    ui.add_space(6.0);
    if strip_button(
        ui,
        &palette,
        Icon::Library,
        &gettext(locale, "Expand Your Library"),
        false,
    )
    .clicked()
    {
        app.actions
            .push(Action::Fork(crate::fork::Action::SetLibraryCollapsed(
                false,
            )));
    }
    if strip_button(
        ui,
        &palette,
        Icon::Plus,
        &gettext(locale, "Create a playlist"),
        false,
    )
    .clicked()
    {
        app.actions.push(Action::ShowDialog(Dialog::CreatePlaylist {
            name: String::new(),
            public: false,
            add_uris: Vec::new(),
        }));
    }
    ui.add_space(6.0);

    // The shelf the open Library last showed, unfiltered: a search typed
    // there has no field here to show or clear it.
    let filter = ui
        .data(|data| data.get_temp::<Filter>(egui::Id::new("sidebar-filter")))
        .unwrap_or_default();
    let sort = selected_sort(app, filter);
    let Shelf {
        entries,
        loading,
        error,
        more_page,
    } = shelf(app, filter, sort, "");
    let custom_order = filter == Filter::Playlists && sort == LibrarySort::Local;
    let playing_context = app.playing_context_uri();
    let context_playing = app.believed_playing();
    let current_page = app.page().clone();
    crate::autoscroll::show(
        ui,
        egui::ScrollArea::vertical()
            .id_salt("sidebar-strip")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden),
        egui::Vec2b::new(false, true),
        |ui| {
            let dragging_song = egui::DragAndDrop::has_payload_of_type::<DragTrack>(ui.ctx());
            if dragging_song {
                crate::ui::widgets::scroll_during_drag(ui);
            }
            if loading {
                crate::ui::widgets::loading_row(ui, &palette, app.locale);
            }
            if let Some(error) = &error {
                crate::ui::widgets::error_row(ui, app, error, None);
            }
            crate::ui::widgets::virtual_rows(
                ui,
                entries.len(),
                COLLAPSED_ROW_HEIGHT,
                |ui, index| {
                    let entry = &entries[index];
                    let active = entry.folder.is_none() && entry.page == current_page;
                    let playing = context_playing
                        && entry_is_playing_context(entry, playing_context.as_deref());
                    let (_, rect) =
                        ui.allocate_space(vec2(ui.available_width(), COLLAPSED_ROW_HEIGHT));
                    let id = ui.id().with((
                        "library-tile",
                        &entry.uri,
                        entry.liked,
                        entry.folder.as_ref().map(|(id, _, _)| id),
                    ));
                    let response = ui.interact(rect, id, Sense::click_and_drag());
                    let name = if let Some((_, collapsed, _)) = &entry.folder {
                        app.locale.folder_state_label(&entry.name, *collapsed)
                    } else {
                        entry.name.clone()
                    };
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            active,
                            &name,
                        )
                    });
                    if ui.is_rect_visible(rect) {
                        let droppable = entry.liked || entry.editable;
                        let drop_hover = dragging_song && droppable && response.contains_pointer();
                        if active || response.hovered() || drop_hover {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(6),
                                if drop_hover {
                                    palette.accent.gamma_multiply(0.18)
                                } else if active {
                                    palette.surface
                                } else {
                                    palette.surface_hover.gamma_multiply(0.6)
                                },
                            );
                        }
                        let tile =
                            Rect::from_center_size(rect.center(), Vec2::splat(COLLAPSED_TILE));
                        let radius = if entry.round {
                            COLLAPSED_TILE / 2.0
                        } else {
                            6.0
                        };
                        if entry.folder.is_some() {
                            Icon::Library.image(palette.secondary, 24.0).paint_at(
                                ui,
                                Rect::from_center_size(tile.center(), Vec2::splat(24.0)),
                            );
                        } else if entry.liked {
                            liked_cover(ui, tile, 6.0);
                        } else {
                            crate::ui::widgets::paint_cover(
                                ui,
                                &palette,
                                entry.image.as_deref(),
                                tile,
                                radius,
                                if entry.round { Icon::User } else { Icon::Music },
                                Some(app.backend.art()),
                            );
                        }
                        // The playing shelf keeps its mark, here as a ring.
                        if playing {
                            ui.painter().rect_stroke(
                                tile.expand(2.0),
                                CornerRadius::same((radius + 2.0) as u8),
                                egui::Stroke::new(2.0, palette.accent),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if dragging_song && !droppable {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(6),
                                palette.panel.gamma_multiply(0.5),
                            );
                        }
                    }
                    let response = response.on_hover_ui(|ui| {
                        ui.label(egui::RichText::new(&name).strong());
                        if entry.folder.is_none() && !entry.subtitle.is_empty() {
                            ui.label(&entry.subtitle);
                        }
                    });
                    finish_entry_interaction(app, ui, &response, entry, false, custom_order, true);
                },
            );
            if let Some(page) = more_page {
                crate::ui::widgets::load_more_when_near_end(ui, app, page, true);
            }
        },
    );
}
