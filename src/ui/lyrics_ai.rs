//! Fork: the pronunciation and translation lines under each lyric line, and
//! their settings (src/lyrics_ai.rs).

use egui::{Color32, FontId, Rect};

use crate::app::App;
use crate::i18n::gettext;
use crate::lyrics_ai::{LANGUAGES, Provider, SecretKey};
use crate::model::Action;
use crate::theme::{self, Palette};

use super::widgets;

/// Draws lyric line `index` with its pronunciation above it: each word's
/// reading centred over that word when the words could be matched, else
/// the whole reading as a small line on top. `None` when there is no
/// pronunciation to show, and the caller draws the line as usual.
#[allow(clippy::too_many_arguments)]
pub(super) fn lyric_line(
    app: &App,
    ui: &mut egui::Ui,
    index: usize,
    text: &str,
    font: FontId,
    reading_size: f32,
    color: Color32,
    sense: egui::Sense,
) -> Option<egui::Response> {
    let uri = app.lyrics_uri.as_deref();
    let settings = &app.settings.lyrics_ai;
    let words = app.lyrics_ai.words(uri, index, settings);
    let (pronunciation, _) = app.lyrics_ai.line(uri, index, settings);
    if words.is_none() && pronunciation.is_none() {
        return None;
    }
    let reading_font = theme::regular(reading_size);
    let reading_color = color.gamma_multiply(0.85);
    let drawn = ui.scope(|ui| match words {
        Some(words) if !crate::bidi::is_rtl(text) => {
            ruby(ui, words, &font, &reading_font, color, reading_color);
        }
        _ => {
            if let Some(pronunciation) = pronunciation {
                line(ui, pronunciation, reading_font.clone(), reading_color);
            }
            line(ui, text, font.clone(), color);
        }
    });
    let response = ui.interact(
        drawn.response.rect,
        ui.id().with(("lyric-line", index)),
        sense,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    Some(response)
}

/// Each word with its reading above it, wrapping word by word.
fn ruby(
    ui: &mut egui::Ui,
    words: &[crate::lyrics_ai::Ruby],
    font: &FontId,
    reading_font: &FontId,
    color: Color32,
    reading_color: Color32,
) {
    let painter = ui.painter().clone();
    let space = painter
        .layout_no_wrap(" ".into(), font.clone(), color)
        .size()
        .x;
    // Every word keeps a reading row, read or not, so words line up.
    let reading_height = painter
        .layout_no_wrap("Ag".into(), reading_font.clone(), reading_color)
        .size()
        .y;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
        for word in words {
            let main = painter.layout_no_wrap(word.text.clone(), font.clone(), color);
            let reading = word.reading.as_ref().map(|reading| {
                painter.layout_no_wrap(reading.clone(), reading_font.clone(), reading_color)
            });
            let width = main
                .size()
                .x
                .max(reading.as_ref().map_or(0.0, |galley| galley.size().x));
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(width, reading_height + main.size().y),
                egui::Sense::hover(),
            );
            if let Some(reading) = reading {
                let at = egui::pos2(rect.center().x - reading.size().x / 2.0, rect.top());
                ui.painter().galley(at, reading, reading_color);
            }
            let at = egui::pos2(
                rect.center().x - main.size().x / 2.0,
                rect.top() + reading_height,
            );
            ui.painter().galley(at, main, color);
            if word.space {
                ui.add_space(space);
            }
        }
    });
}

/// Draws line `index`'s translation below it, in `size` and dimmed from
/// `color`, and returns the rect it covers, so following the song keeps
/// the whole group in view.
pub(super) fn under_line(
    app: &App,
    ui: &mut egui::Ui,
    index: usize,
    size: f32,
    color: Color32,
) -> Option<Rect> {
    let (_, translation) =
        app.lyrics_ai
            .line(app.lyrics_uri.as_deref(), index, &app.settings.lyrics_ai);
    translation.map(|text| line(ui, text, theme::regular(size), color.gamma_multiply(0.7)))
}

fn line(ui: &mut egui::Ui, text: &str, font: FontId, color: Color32) -> Rect {
    let response = if crate::bidi::is_rtl(text) {
        let galley = crate::bidi::layout(
            ui.painter(),
            text,
            font,
            color,
            ui.available_width(),
            usize::MAX,
            None,
        );
        ui.add(egui::Label::new(galley))
    } else {
        ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap())
    };
    response.rect
}

/// Why the extra lines are missing, quietly, above the lyrics, with a way
/// to ask again.
pub(super) fn notice(app: &mut App, ui: &mut egui::Ui, color: Color32) {
    let Some(error) = &app.lyrics_ai.error else {
        return;
    };
    if app.lyrics_ai.uri != app.lyrics_uri {
        return;
    }
    let message = gettext(
        app.locale,
        // Translators: Keep {error} unchanged. It is the original failure detail.
        "Translation unavailable: {error}",
    )
    .replace("{error}", error);
    ui.add(
        egui::Label::new(
            egui::RichText::new(message)
                .font(theme::regular(12.0))
                .color(color),
        )
        .wrap(),
    );
    if theme::link(
        ui,
        gettext(app.locale, "Try again").to_string(),
        theme::medium(12.0),
        color,
    )
    .clicked()
    {
        app.actions.push(Action::RetryLyricsExtras);
    }
    ui.add_space(6.0);
}

const KEY_FIELD: &str = "lyrics-ai-key-field";
const KEYS_ASKED: &str = "lyrics-ai-keys-asked";

/// The settings section's contents. Returns whether a setting changed.
pub(super) fn settings(app: &mut App, ui: &mut egui::Ui, palette: &Palette) -> bool {
    let locale = app.locale;
    let mut changed = false;
    if app.lyrics_ai.keys.is_none()
        && !ui.data(|data| {
            data.get_temp::<bool>(egui::Id::new(KEYS_ASKED))
                .unwrap_or(false)
        })
    {
        ui.data_mut(|data| data.insert_temp(egui::Id::new(KEYS_ASKED), true));
        app.actions.push(Action::LoadAiKeys);
    }
    let ai = &mut app.settings.lyrics_ai;

    widgets::setting_row(
        ui,
        palette,
        &gettext(locale, "Translation"),
        &gettext(locale, "Show what each line means, under it."),
        |ui| {
            if widgets::switch(
                ui,
                palette,
                &gettext(locale, "Translation"),
                &mut ai.translate,
            )
            .changed()
            {
                changed = true;
            }
        },
    );
    widgets::setting_row(
        ui,
        palette,
        &gettext(locale, "Pronunciation"),
        &gettext(
            locale,
            "Show how lines in other scripts are sung, in the Latin alphabet.",
        ),
        |ui| {
            if widgets::switch(
                ui,
                palette,
                &gettext(locale, "Pronunciation"),
                &mut ai.pronounce,
            )
            .changed()
            {
                changed = true;
            }
        },
    );
    widgets::setting_row(ui, palette, &gettext(locale, "Translate into"), "", |ui| {
        let interface = gettext(locale, "Interface language");
        let selected = crate::lyrics_ai::language(&ai.target_language).map_or_else(
            || interface.to_string(),
            |language| language.native.to_string(),
        );
        egui::ComboBox::from_id_salt("lyrics_ai_target")
            .selected_text(selected)
            .width(200.0_f32.min(ui.available_width()))
            .height(1000.0)
            .show_ui(ui, |ui| {
                let choices = std::iter::once(("", interface.as_ref())).chain(
                    LANGUAGES
                        .iter()
                        .map(|language| (language.tag, language.native)),
                );
                for (tag, label) in choices {
                    if ui
                        .selectable_label(ai.target_language == tag, label)
                        .clicked()
                        && ai.target_language != tag
                    {
                        ai.target_language = tag.to_string();
                        changed = true;
                    }
                }
            });
    });
    widgets::setting_row(
        ui,
        palette,
        &gettext(locale, "AI provider"),
        &gettext(
            locale,
            "Each song's lyrics are sent once, with your key, and the answer is kept on this computer.",
        ),
        |_| {},
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for provider in Provider::ALL {
            if theme::soft_button(ui, palette, None, provider.label(), ai.provider == provider)
                .clicked()
                && ai.provider != provider
            {
                ai.provider = provider;
                changed = true;
            }
        }
    });
    ui.add_space(10.0);
    if ai.provider == Provider::OpenAiCompatible {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (name, url) in [
                ("OpenRouter", "https://openrouter.ai/api/v1"),
                ("OpenAI", "https://api.openai.com/v1"),
                ("Groq", "https://api.groq.com/openai/v1"),
            ] {
                if theme::soft_button(ui, palette, None, name, ai.openai_base_url.trim() == url)
                    .clicked()
                {
                    ai.openai_base_url = url.to_string();
                    changed = true;
                }
            }
        });
        ui.add_space(8.0);
        if widgets::labeled_field(
            ui,
            palette,
            locale,
            &gettext(locale, "Base URL"),
            &mut ai.openai_base_url,
            crate::lyrics_ai::DEFAULT_OPENAI_BASE_URL,
            false,
        )
        .changed()
        {
            changed = true;
        }
        ui.add_space(8.0);
    }
    let model = match ai.provider {
        Provider::Gemini => &mut ai.gemini_model,
        Provider::OpenAiCompatible => &mut ai.openai_model,
    };
    let model_hint = match ai.provider {
        Provider::Gemini => crate::lyrics_ai::DEFAULT_GEMINI_MODEL,
        Provider::OpenAiCompatible => crate::lyrics_ai::DEFAULT_OPENAI_MODEL,
    };
    if widgets::labeled_field(
        ui,
        palette,
        locale,
        &gettext(locale, "Model"),
        model,
        model_hint,
        false,
    )
    .changed()
    {
        changed = true;
    }
    ui.add_space(8.0);

    let provider = ai.provider;
    let saved = app.lyrics_ai.keys.is_some_and(|keys| keys.has(provider));
    let field_id = egui::Id::new(KEY_FIELD).with(provider.label());
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
        &gettext(locale, "API key"),
        &mut key,
        &hint,
        true,
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if !key.trim().is_empty()
            && theme::pill_button(ui, palette, &gettext(locale, "Save key"), true).clicked()
        {
            app.actions.push(Action::SaveAiKey {
                provider,
                key: SecretKey(std::mem::take(&mut key)),
            });
        }
        if saved && theme::pill_button(ui, palette, &gettext(locale, "Remove key"), false).clicked()
        {
            app.actions.push(Action::SaveAiKey {
                provider,
                key: SecretKey(String::new()),
            });
        }
    });
    if key.is_empty() {
        ui.data_mut(|data| data.remove::<String>(field_id));
    } else {
        ui.data_mut(|data| data.insert_temp(field_id, key));
    }
    if let Some(error) = &app.lyrics_ai.key_error {
        ui.add_space(6.0);
        theme::text(ui, error, theme::regular(12.5), palette.danger);
    }
    changed
}
