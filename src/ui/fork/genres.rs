//! Fork: the Genres settings section: the player bar's genre line, and the
//! Last.fm key that fills in genres MusicBrainz lacks (`crate::fork::genres`).

use crate::app::App;
use crate::fork::lyrics_ai::SecretKey;
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme::{self, Palette};

use crate::ui::widgets;

const KEY_FIELD: &str = "genres-lastfm-key-field";
const KEYS_ASKED: &str = "genres-keys-asked";
const KEY_PAGE: &str = "https://www.last.fm/api/account/create";

/// True when a setting changed; the key is saved through an action.
pub(crate) fn settings(app: &mut App, ui: &mut egui::Ui, palette: &Palette) -> bool {
    let locale = app.locale;
    let mut changed = false;
    let title = gettext(locale, "Genres in player bar");
    widgets::setting_row(
        ui,
        palette,
        &title,
        &gettext(
            locale,
            "Show the playing song's genres under the artist names. Spotify no longer lists them, so the artist is looked up on MusicBrainz.",
        ),
        |ui| {
            if widgets::switch(
                ui,
                palette,
                &title,
                &mut app.settings.fork.player_bar_genres,
            )
            .changed()
            {
                changed = true;
            }
        },
    );
    if !app.settings.fork.player_bar_genres {
        return changed;
    }
    if app.fork.lyrics_ai.keys.is_none()
        && !ui.data(|data| {
            data.get_temp::<bool>(egui::Id::new(KEYS_ASKED))
                .unwrap_or(false)
        })
    {
        ui.data_mut(|data| data.insert_temp(egui::Id::new(KEYS_ASKED), true));
        app.actions
            .push(Action::Fork(crate::fork::Action::LoadAiKeys));
    }

    ui.add_space(4.0);
    theme::text(
        ui,
        gettext(
            locale,
            "For artists MusicBrainz does not know, Last.fm's top tags fill in. Only the artist's name is sent, with your key.",
        ),
        theme::regular(12.5),
        palette.secondary,
    );
    ui.add_space(8.0);
    let saved = app.fork.lyrics_ai.keys.is_some_and(|keys| keys.lastfm);
    let field_id = egui::Id::new(KEY_FIELD);
    let mut key = ui
        .data(|data| data.get_temp::<String>(field_id))
        .unwrap_or_default();
    let hint = if saved {
        gettext(locale, "Saved. Paste a new key to replace it.")
    } else {
        gettext(locale, "Paste your API key")
    };
    widgets::labeled_field(
        ui,
        palette,
        locale,
        &gettext(locale, "Last.fm key"),
        &mut key,
        &hint,
        true,
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if !key.trim().is_empty()
            && theme::pill_button(ui, palette, &gettext(locale, "Save key"), true).clicked()
        {
            app.actions
                .push(Action::Fork(crate::fork::Action::SaveLastFmKey(SecretKey(
                    std::mem::take(&mut key),
                ))));
        }
        if saved && theme::pill_button(ui, palette, &gettext(locale, "Remove key"), false).clicked()
        {
            app.actions
                .push(Action::Fork(crate::fork::Action::SaveLastFmKey(SecretKey(
                    String::new(),
                ))));
        }
    });
    if key.is_empty() {
        ui.data_mut(|data| data.remove::<String>(field_id));
    } else {
        ui.data_mut(|data| data.insert_temp(field_id, key));
    }
    ui.add_space(6.0);
    let label = gettext(
        locale,
        // Translators: {service} is where the key comes from, such as Google AI Studio.
        "Get a key from {service}",
    )
    .replace("{service}", "Last.fm");
    if theme::link(ui, label, theme::medium(13.0), palette.accent)
        .on_hover_text(KEY_PAGE)
        .clicked()
    {
        app.actions.push(Action::OpenUrl(KEY_PAGE.to_string()));
    }
    if let Some(error) = &app.fork.lyrics_ai.key_error {
        ui.add_space(6.0);
        theme::text(ui, error, theme::regular(12.5), palette.danger);
    }
    ui.add_space(10.0);
    changed
}
