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
/// Fork page: how far the sung line has got, to light its words up to there.
#[derive(Clone, Debug)]
pub(super) struct WordFill {
    /// The estimate: how far through the line, 0 to 1.
    pub progress: f32,
    pub sung: Color32,
    pub unsung: Color32,
    /// Each character's time when Unison timed the line word by word.
    pub times: Option<std::sync::Arc<Vec<(u32, u32)>>>,
    /// The song's position (its lyrics offset applied), for `times`.
    pub position: u32,
}

impl WordFill {
    /// The colours of `segments`, pieces of `text` in order: from their
    /// characters' real times when the line has them, else estimated.
    fn colours_for<S: AsRef<str>>(&self, text: &str, segments: &[S]) -> Vec<Color32> {
        if let Some(times) = &self.times
            && times.len() == text.chars().count()
        {
            let mut from = 0usize;
            let mut found = Vec::with_capacity(segments.len());
            for segment in segments {
                let segment = segment.as_ref();
                let Some(byte) = text[from..].find(segment) else {
                    found.clear();
                    break;
                };
                let first = text[..from + byte].chars().count();
                let count = segment.chars().count().max(1);
                found.push((first, first + count - 1));
                from += byte + segment.len();
            }
            if found.len() == segments.len() {
                return found
                    .into_iter()
                    .map(|(first, last)| {
                        let start = times[first].0;
                        let end = times[last.min(times.len() - 1)].1.max(start + 1);
                        let t = (self.position.saturating_sub(start)) as f32 / (end - start) as f32;
                        super::lyrics::blend(self.unsung, self.sung, t.clamp(0.0, 1.0))
                    })
                    .collect();
            }
        }
        self.colours(&crate::lyrics_page::word_ends(segments))
    }

    /// The colours of words ending at `ends` (shares of the line).
    fn colours(&self, ends: &[f32]) -> Vec<Color32> {
        let mut start = 0.0;
        ends.iter()
            .map(|end| {
                let colour = if self.progress >= *end {
                    self.sung
                } else if self.progress <= start {
                    self.unsung
                } else {
                    let t = (self.progress - start) / (end - start).max(0.001);
                    super::lyrics::blend(self.unsung, self.sung, t)
                };
                start = *end;
                colour
            })
            .collect()
    }
}

/// `text` laid out with each word coloured by how far the singing has got.
pub(super) fn filled_galley(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    wrap_width: f32,
    fill: WordFill,
) -> std::sync::Arc<egui::Galley> {
    let words: Vec<&str> = text.split_inclusive(char::is_whitespace).collect();
    let colours = fill.colours_for(text, &words);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    for (word, colour) in words.iter().zip(colours) {
        job.append(
            word,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: colour,
                ..Default::default()
            },
        );
    }
    painter.layout_job(job)
}

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
    fill: Option<WordFill>,
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
            ruby(
                ui,
                words,
                &font,
                &reading_font,
                color,
                (settings.pronounce, settings.translate),
                fill.as_ref().map(|fill| (fill, text)),
            );
        }
        _ => {
            if let Some(pronunciation) = pronunciation {
                line(ui, pronunciation, reading_font.clone(), reading_color);
            }
            match fill.filter(|_| !crate::bidi::is_rtl(text)) {
                Some(fill) => {
                    let galley =
                        filled_galley(ui.painter(), text, font.clone(), ui.available_width(), fill);
                    ui.add(egui::Label::new(galley));
                }
                None => {
                    line(ui, text, font.clone(), color);
                }
            }
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

/// Each word with its reading above it and its meaning below, wrapping
/// word by word. `shown` is (readings, meanings) as the settings want them.
fn ruby(
    ui: &mut egui::Ui,
    words: &[crate::lyrics_ai::Ruby],
    font: &FontId,
    reading_font: &FontId,
    color: Color32,
    shown: (bool, bool),
    fill: Option<(&WordFill, &str)>,
) {
    let fills = fill.map(|(fill, text)| {
        let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        fill.colours_for(text, &texts)
    });
    let reading_color = color.gamma_multiply(0.85);
    let gloss_color = color.gamma_multiply(0.6);
    let painter = ui.painter().clone();
    let space = painter
        .layout_no_wrap(" ".into(), font.clone(), color)
        .size()
        .x;
    let row = painter
        .layout_no_wrap("Ag".into(), reading_font.clone(), reading_color)
        .size()
        .y;
    // Every word keeps the line's reading and meaning rows, read or not,
    // so the words line up.
    let readings = shown.0 && words.iter().any(|word| word.reading.is_some());
    let glosses = shown.1 && words.iter().any(|word| word.gloss.is_some());
    let reading_height = if readings { row } else { 0.0 };
    let gloss_height = if glosses { row } else { 0.0 };
    // Fork page: a line that fits on one row sits in the middle when the
    // page centres its lines.
    let centred = ui.layout().cross_align == egui::Align::Center;
    let measured: f32 = if centred {
        words
            .iter()
            .map(|word| {
                let main = painter
                    .layout_no_wrap(word.text.clone(), font.clone(), color)
                    .size()
                    .x;
                let small = |text: &Option<String>, on: bool| {
                    text.as_ref().filter(|_| on).map_or(0.0, |text| {
                        painter
                            .layout_no_wrap(text.clone(), reading_font.clone(), color)
                            .size()
                            .x
                            + row * 0.5
                    })
                };
                main.max(small(&word.reading, readings))
                    .max(small(&word.gloss, glosses))
                    + if word.space { space } else { 0.0 }
            })
            .sum()
    } else {
        0.0
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
        if centred && measured < ui.available_width() {
            ui.add_space((ui.available_width() - measured) / 2.0);
        }
        for (number, word) in words.iter().enumerate() {
            let color = fills
                .as_ref()
                .and_then(|fills| fills.get(number).copied())
                .unwrap_or(color);
            let main = painter.layout_no_wrap(word.text.clone(), font.clone(), color);
            let small = |text: &Option<String>, on: bool, color: Color32| {
                text.as_ref()
                    .filter(|_| on)
                    .map(|text| painter.layout_no_wrap(text.clone(), reading_font.clone(), color))
            };
            let reading = small(&word.reading, readings, reading_color);
            let gloss = small(&word.gloss, glosses, gloss_color);
            // Readings wider than their word keep a little room on each
            // side, so neighbours never run together.
            let room = |galley: &Option<std::sync::Arc<egui::Galley>>| {
                galley
                    .as_ref()
                    .map_or(0.0, |galley| galley.size().x + row * 0.5)
            };
            let width = main.size().x.max(room(&reading)).max(room(&gloss));
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(width, reading_height + main.size().y + gloss_height),
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
            let main_height = main.size().y;
            ui.painter().galley(at, main, color);
            if let Some(gloss) = gloss {
                let at = egui::pos2(
                    rect.center().x - gloss.size().x / 2.0,
                    rect.top() + reading_height + main_height,
                );
                ui.painter().galley(at, gloss, gloss_color);
            }
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
    #[cfg(feature = "mods")]
    widgets::setting_row(
        ui,
        palette,
        &gettext(locale, "ivLyrics (web mod)"),
        &gettext(
            locale,
            "The old ivLyrics page in a webview, beside the native lyrics page. Uses about a gigabyte of memory while open.",
        ),
        |ui| {
            if widgets::switch(
                ui,
                palette,
                &gettext(locale, "ivLyrics (web mod)"),
                &mut app.settings.mods_enabled,
            )
            .changed()
            {
                changed = true;
            }
        },
    );
    let available = crate::lyrics_video::available();
    let video_note = if available {
        gettext(
            locale,
            "A community-picked music video behind full-screen lyrics, in time with the song.",
        )
    } else {
        gettext(
            locale,
            "Needs libmpv-2.dll and yt-dlp.exe beside Spotifast or on the system path.",
        )
    };
    widgets::setting_row(
        ui,
        palette,
        &gettext(locale, "Video background"),
        &video_note,
        |ui| {
            ui.add_enabled_ui(available, |ui| {
                if widgets::switch(
                    ui,
                    palette,
                    &gettext(locale, "Video background"),
                    &mut app.settings.lyrics_video,
                )
                .changed()
                {
                    changed = true;
                }
            });
        },
    );
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
        changed |= target_picker(app, ui);
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
    changed |= provider_and_key(app, ui, palette);
    changed
}

/// The target language's dropdown. True when it changed.
pub(super) fn target_picker(app: &mut App, ui: &mut egui::Ui) -> bool {
    let locale = app.locale;
    let ai = &mut app.settings.lyrics_ai;
    let mut changed = false;
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
    changed
}

/// The provider, its model and endpoint, and the API key. True when a
/// setting changed (a key is saved through an action).
pub(super) fn provider_and_key(app: &mut App, ui: &mut egui::Ui, palette: &Palette) -> bool {
    let locale = app.locale;
    let ai = &mut app.settings.lyrics_ai;
    let mut changed = false;
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
    // Where to get a key for the provider chosen.
    if let Some((name, url)) = crate::lyrics_ai::key_page(
        app.settings.lyrics_ai.provider,
        &app.settings.lyrics_ai.openai_base_url,
    ) {
        ui.add_space(6.0);
        let label = gettext(
            locale,
            // Translators: {service} is where the key comes from, such as Google AI Studio.
            "Get a key from {service}",
        )
        .replace("{service}", name);
        if theme::link(ui, label, theme::medium(13.0), palette.accent)
            .on_hover_text(url)
            .clicked()
        {
            app.actions.push(Action::OpenUrl(url.to_string()));
        }
    }
    if let Some(error) = &app.lyrics_ai.key_error {
        ui.add_space(6.0);
        theme::text(ui, error, theme::regular(12.5), palette.danger);
    }
    changed
}
