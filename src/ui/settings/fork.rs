//! Fork: the fork's rows and sections on the Settings page.

use super::*;

/// Hook in the Appearance section: the waveform seek bar's switch. True
/// when it changed.
pub(super) fn appearance(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &Palette,
    needle: &str,
    appearance: &str,
) -> bool {
    let locale = app.locale;
    let seek_waveform = gettext(locale, "Waveform seek bar");
    let row = RowText::new(
        seek_waveform.clone(),
        gettext(
            locale,
            "Draw the seek bar as the song's waveform. It fills in as a song first plays and is kept for next time.",
        ),
    );
    let mut changed = false;
    filtered_row(ui, palette, needle, appearance, &row, |ui| {
        changed = widgets::switch(
            ui,
            palette,
            &seek_waveform,
            &mut app.settings.fork.seek_waveform,
        )
        .changed();
    });
    changed
}

/// Hook before the mini player section: the Lyrics extras and Genres
/// sections. True when a setting changed.
pub(super) fn sections(
    app: &mut App,
    ui: &mut egui::Ui,
    palette: &Palette,
    needle: &str,
    any_visible: &mut bool,
) -> bool {
    let locale = app.locale;
    let mut changed = false;

    // Pronunciation and translation under the lyrics.
    let lyrics_ai = gettext(locale, "Lyrics extras");
    let lyrics_ai_rows = [
        RowText::new(
            gettext(locale, "Translation, pronunciation, AI provider and key"),
            gettext(
                locale,
                "Lines under the lyrics from Gemini or an OpenAI-compatible service, with your own key.",
            ),
        ),
        RowText::new(
            gettext(locale, "Video background"),
            gettext(locale, "Music videos behind full-screen lyrics."),
        ),
    ];
    if section_matches(needle, &lyrics_ai, &lyrics_ai_rows) {
        *any_visible = true;
        ui.push_id("lyrics-ai-settings", |ui| {
            section(ui, palette, &lyrics_ai, |ui| {
                if crate::ui::fork::lyrics_ai::settings(app, ui, palette) {
                    changed = true;
                }
            });
        });
    }

    // The player bar's genre line and where its genres come from.
    let genres = gettext(locale, "Genres");
    let genre_rows = [
        RowText::new(
            gettext(locale, "Genres in player bar"),
            gettext(
                locale,
                "Show the playing song's genres under the artist names.",
            ),
        ),
        RowText::new(
            gettext(locale, "Last.fm key"),
            gettext(
                locale,
                "Genres MusicBrainz lacks, from Last.fm with your own key.",
            ),
        ),
    ];
    if section_matches(needle, &genres, &genre_rows) {
        *any_visible = true;
        ui.push_id("genre-settings", |ui| {
            section(ui, palette, &genres, |ui| {
                if crate::ui::fork::genres::settings(app, ui, palette) {
                    changed = true;
                }
            });
        });
    }
    changed
}

/// Hook in the About section: where updates come from. Builds with mods
/// read the fork's releases, not upstream's.
pub(super) fn updates_link(app: &mut App, ui: &mut egui::Ui, palette: &Palette) {
    ui.add_space(8.0);
    let repository = crate::updates::repository();
    let from = gettext(
        app.locale,
        // Translators: {repository} is the GitHub repository, such as owner/name.
        "Updates come from {repository}",
    )
    .replace("{repository}", repository);
    let url = format!("https://github.com/{repository}/releases");
    if theme::link(ui, from, theme::regular(13.0), palette.secondary)
        .on_hover_text(&url)
        .clicked()
    {
        app.actions.push(Action::OpenUrl(url));
    }
}
