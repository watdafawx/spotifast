//! Fork: pronunciation and translation lines under the lyrics, from an AI
//! provider the listener brings their own key for.
//!
//! One request per song carries every distinct line and asks for strict
//! JSON back: for each line its pronunciation in the Latin alphabet and its
//! translation into the target language. The answer is cached on disk next
//! to the lyrics, keyed by the track, the target language and the words, so
//! playing a song again asks nothing.
//!
//! Gemini and any OpenAI-compatible endpoint (OpenAI, OpenRouter, Groq) are
//! supported. The key lives in the platform credential store
//! ([`crate::credentials`]) and never in settings or logs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// A translation costs the listener money; keep it as long as the lyrics
/// could plausibly stay the same.
const CACHE_LIFETIME: Duration = Duration::from_secs(180 * 24 * 60 * 60);
/// A whole song in one answer takes a while on slower models.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "google/gemini-2.5-flash";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Gemini,
    /// OpenAI's chat completions API, as OpenAI, OpenRouter and Groq serve it.
    OpenAiCompatible,
}

impl Provider {
    pub const ALL: [Self; 2] = [Self::Gemini, Self::OpenAiCompatible];

    pub fn label(self) -> &'static str {
        match self {
            Self::Gemini => "Gemini",
            Self::OpenAiCompatible => "OpenAI-compatible",
        }
    }
}

/// What the listener chose. Stored in `settings.json` under `lyrics_ai`;
/// older files without it get these defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    pub translate: bool,
    pub pronounce: bool,
    /// A tag from [`LANGUAGES`]; empty follows the interface language.
    pub target_language: String,
    pub provider: Provider,
    pub gemini_model: String,
    pub openai_base_url: String,
    pub openai_model: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            translate: true,
            pronounce: true,
            target_language: String::new(),
            provider: Provider::Gemini,
            gemini_model: DEFAULT_GEMINI_MODEL.into(),
            openai_base_url: DEFAULT_OPENAI_BASE_URL.into(),
            openai_model: DEFAULT_OPENAI_MODEL.into(),
        }
    }
}

impl AiSettings {
    pub fn model(&self) -> &str {
        match self.provider {
            Provider::Gemini => self.gemini_model.trim(),
            Provider::OpenAiCompatible => self.openai_model.trim(),
        }
    }

    pub fn base_url(&self) -> &str {
        match self.provider {
            Provider::Gemini => GEMINI_BASE_URL,
            Provider::OpenAiCompatible => self.openai_base_url.trim(),
        }
    }

    /// The language translations are written in.
    pub fn target(&self, interface: crate::i18n::Locale) -> &'static Language {
        language(&self.target_language)
            .unwrap_or_else(|| language(interface.tag()).unwrap_or(&LANGUAGES[0]))
    }
}

/// Where to get a key for `provider` at `base_url`: the service's name and
/// its key page. `None` for an OpenAI-compatible endpoint we do not know.
pub fn key_page(provider: Provider, base_url: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        Provider::Gemini => Some(("Google AI Studio", "https://aistudio.google.com/apikey")),
        Provider::OpenAiCompatible => {
            let host = base_url.to_ascii_lowercase();
            [
                (
                    "openrouter.ai",
                    "OpenRouter",
                    "https://openrouter.ai/settings/keys",
                ),
                (
                    "api.openai.com",
                    "OpenAI",
                    "https://platform.openai.com/api-keys",
                ),
                ("api.groq.com", "Groq", "https://console.groq.com/keys"),
            ]
            .into_iter()
            .find(|(domain, _, _)| host.contains(domain))
            .map(|(_, name, url)| (name, url))
        }
    }
}

/// The listener's keys, one per provider. Deliberately no Debug: they are
/// usable secrets.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Keys {
    #[serde(default)]
    pub gemini: String,
    #[serde(default)]
    pub openai: String,
    /// Last.fm's API key, for genres MusicBrainz lacks (`crate::genres`).
    /// Kept with the AI keys so it shares their credential store entry.
    #[serde(default)]
    pub lastfm: String,
}

impl Keys {
    pub fn get(&self, provider: Provider) -> &str {
        match provider {
            Provider::Gemini => self.gemini.trim(),
            Provider::OpenAiCompatible => self.openai.trim(),
        }
    }

    pub fn set(&mut self, provider: Provider, key: String) {
        match provider {
            Provider::Gemini => self.gemini = key,
            Provider::OpenAiCompatible => self.openai = key,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.gemini.trim().is_empty()
            && self.openai.trim().is_empty()
            && self.lastfm.trim().is_empty()
    }

    /// Sets one key; an empty key removes it.
    pub fn set_for(&mut self, which: KeyFor, key: String) {
        match which {
            KeyFor::Ai(provider) => self.set(provider, key),
            KeyFor::LastFm => self.lastfm = key,
        }
    }

    /// Which providers have a key, for the settings page.
    pub fn present(&self) -> KeysPresent {
        KeysPresent {
            gemini: !self.gemini.trim().is_empty(),
            openai: !self.openai.trim().is_empty(),
            lastfm: !self.lastfm.trim().is_empty(),
        }
    }
}

/// Which saved key a change is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyFor {
    Ai(Provider),
    /// Last.fm, for genres (`crate::genres`).
    LastFm,
}

/// Which keys are saved, without the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeysPresent {
    pub gemini: bool,
    pub openai: bool,
    pub lastfm: bool,
}

impl KeysPresent {
    pub fn has(self, provider: Provider) -> bool {
        match provider {
            Provider::Gemini => self.gemini,
            Provider::OpenAiCompatible => self.openai,
        }
    }
}

/// A key on its way from the settings page to the credential store. Its
/// Debug never shows the key, so logging an `Action` cannot leak it.
#[derive(Clone, PartialEq)]
pub struct SecretKey(pub String);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

// ---- languages -------------------------------------------------------------

pub struct Language {
    pub tag: &'static str,
    /// The name the prompt uses.
    pub english: &'static str,
    /// The name the picker shows, never translated.
    pub native: &'static str,
}

impl Language {
    /// The ISO 639-1 code a model reports the lyrics' language in.
    pub fn base(&self) -> &'static str {
        self.tag.split('-').next().unwrap_or(self.tag)
    }
}

pub const LANGUAGES: &[Language] = &[
    Language {
        tag: "en",
        english: "English",
        native: "English",
    },
    Language {
        tag: "ko",
        english: "Korean",
        native: "한국어",
    },
    Language {
        tag: "ja",
        english: "Japanese",
        native: "日本語",
    },
    Language {
        tag: "zh-Hans",
        english: "Simplified Chinese",
        native: "简体中文",
    },
    Language {
        tag: "zh-Hant",
        english: "Traditional Chinese",
        native: "繁體中文",
    },
    Language {
        tag: "es",
        english: "Spanish",
        native: "Español",
    },
    Language {
        tag: "fr",
        english: "French",
        native: "Français",
    },
    Language {
        tag: "de",
        english: "German",
        native: "Deutsch",
    },
    Language {
        tag: "it",
        english: "Italian",
        native: "Italiano",
    },
    Language {
        tag: "pt-BR",
        english: "Brazilian Portuguese",
        native: "Português (Brasil)",
    },
    Language {
        tag: "pt-PT",
        english: "European Portuguese",
        native: "Português (Portugal)",
    },
    Language {
        tag: "nl",
        english: "Dutch",
        native: "Nederlands",
    },
    Language {
        tag: "sv",
        english: "Swedish",
        native: "Svenska",
    },
    Language {
        tag: "pl",
        english: "Polish",
        native: "Polski",
    },
    Language {
        tag: "ru",
        english: "Russian",
        native: "Русский",
    },
    Language {
        tag: "uk",
        english: "Ukrainian",
        native: "Українська",
    },
    Language {
        tag: "tr",
        english: "Turkish",
        native: "Türkçe",
    },
    Language {
        tag: "id",
        english: "Indonesian",
        native: "Bahasa Indonesia",
    },
    Language {
        tag: "vi",
        english: "Vietnamese",
        native: "Tiếng Việt",
    },
    Language {
        tag: "th",
        english: "Thai",
        native: "ไทย",
    },
    Language {
        tag: "ar",
        english: "Arabic",
        native: "العربية",
    },
    Language {
        tag: "hi",
        english: "Hindi",
        native: "हिन्दी",
    },
];

/// The language a tag names: exactly, or by its first part ("de-DE").
pub fn language(tag: &str) -> Option<&'static Language> {
    let tag = tag.trim();
    if tag.is_empty() {
        return None;
    }
    LANGUAGES
        .iter()
        .find(|language| language.tag.eq_ignore_ascii_case(tag))
        .or_else(|| {
            let base = tag.split('-').next()?;
            LANGUAGES
                .iter()
                .find(|language| language.tag.eq_ignore_ascii_case(base))
        })
}

// ---- scripts ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Scripts {
    latin: usize,
    kana: usize,
    hangul: usize,
    han: usize,
    cyrillic: usize,
    other: usize,
    /// Letters only Ukrainian writes in Cyrillic.
    ukrainian: usize,
}

impl Scripts {
    fn of<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut scripts = Self::default();
        for c in lines.into_iter().flat_map(str::chars) {
            if !c.is_alphabetic() {
                continue;
            }
            match u32::from(c) {
                0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF => scripts.latin += 1,
                0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => scripts.kana += 1,
                0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => scripts.hangul += 1,
                0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF => {
                    scripts.han += 1;
                }
                0x400..=0x4FF => {
                    scripts.cyrillic += 1;
                    if matches!(c, 'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ') {
                        scripts.ukrainian += 1;
                    }
                }
                _ => scripts.other += 1,
            }
        }
        scripts
    }

    fn letters(&self) -> usize {
        self.latin + self.kana + self.hangul + self.han + self.cyrillic + self.other
    }

    fn non_latin(&self) -> usize {
        self.letters() - self.latin
    }

    /// At least half the letters are in these.
    fn mostly(&self, count: usize) -> bool {
        count > 0 && count * 2 >= self.letters()
    }
}

/// Words common enough in each Latin-script language to tell them apart in
/// a song's worth of text.
const STOPWORDS: &[(&str, &[&str])] = &[
    (
        "en",
        &[
            "the", "and", "you", "i", "me", "my", "to", "is", "it", "in", "of", "that", "your",
            "don't", "i'm", "we", "be", "on", "for", "with", "all", "can't", "what",
        ],
    ),
    (
        "es",
        &[
            "el", "la", "que", "y", "en", "no", "me", "mi", "tu", "te", "yo", "es", "por", "con",
            "lo", "una", "se", "como", "pero", "más",
        ],
    ),
    (
        "fr",
        &[
            "le", "la", "les", "je", "tu", "et", "que", "pas", "un", "une", "est", "mon", "ma",
            "moi", "toi", "des", "dans", "pour", "qui", "ne", "c'est", "j'ai",
        ],
    ),
    (
        "de",
        &[
            "ich", "du", "und", "die", "der", "das", "nicht", "ist", "mich", "dich", "ein", "eine",
            "mit", "zu", "wir", "es", "mein", "auf", "sie", "nur",
        ],
    ),
    (
        "it",
        &[
            "il", "che", "di", "e", "non", "mi", "ti", "io", "sono", "un", "una", "per", "con",
            "ma", "nel", "del", "si", "come", "più",
        ],
    ),
    (
        "pt",
        &[
            "o", "que", "de", "não", "eu", "você", "me", "meu", "um", "uma", "com", "em", "se",
            "te", "é", "do", "da", "mais", "minha",
        ],
    ),
    (
        "nl",
        &[
            "ik", "je", "de", "het", "een", "en", "niet", "van", "dat", "mijn", "op", "met", "wat",
            "we", "zijn", "jij", "ook", "maar",
        ],
    ),
    (
        "sv",
        &[
            "jag", "du", "och", "att", "det", "är", "inte", "en", "på", "som", "med", "min", "mig",
            "dig", "vi", "har", "för",
        ],
    ),
    (
        "pl",
        &[
            "nie", "się", "w", "to", "na", "że", "ja", "ty", "mnie", "jest", "jak", "z", "do",
            "mi", "co", "tak", "już",
        ],
    ),
    (
        "tr",
        &[
            "ve", "bir", "bu", "ben", "sen", "ne", "için", "gibi", "çok", "beni", "seni", "var",
            "yok", "değil", "ama",
        ],
    ),
    (
        "id",
        &[
            "aku", "kamu", "yang", "dan", "di", "ini", "itu", "tak", "tidak", "ku", "dia", "kita",
            "akan", "untuk", "dengan", "ada", "cinta",
        ],
    ),
    (
        "vi",
        &[
            "anh", "em", "và", "là", "không", "của", "một", "có", "cho", "những", "người", "yêu",
            "tôi", "mình", "đã",
        ],
    ),
];

/// The Latin-script language the words most look like, when one clearly
/// leads.
fn latin_language<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    let mut words = 0usize;
    let mut hits = vec![0usize; STOPWORDS.len()];
    for line in lines {
        for word in line
            .split(|c: char| !(c.is_alphabetic() || c == '\'' || c == '’'))
            .filter(|word| !word.is_empty())
        {
            let word = word.to_lowercase().replace('’', "'");
            words += 1;
            for (index, (_, list)) in STOPWORDS.iter().enumerate() {
                if list.contains(&word.as_str()) {
                    hits[index] += 1;
                }
            }
        }
    }
    if words == 0 {
        return None;
    }
    let mut ranked: Vec<(usize, &str)> = hits
        .iter()
        .zip(STOPWORDS)
        .map(|(hits, (tag, _))| (*hits, *tag))
        .collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let (best, tag) = ranked[0];
    let second = ranked.get(1).map_or(0, |entry| entry.0);
    // A fifth of the words are the language's commonest, and it leads the
    // next language by half again.
    (best * 5 >= words && best * 2 >= second * 3).then_some(tag)
}

/// Whether the words are already in `target`, so a translation would only
/// repeat them.
fn already_in(lines: &[&str], target: &Language) -> bool {
    let scripts = Scripts::of(lines.iter().copied());
    if scripts.letters() == 0 {
        return true;
    }
    match target.base() {
        "ja" => scripts.kana > 0 && scripts.mostly(scripts.kana + scripts.han),
        "ko" => scripts.mostly(scripts.hangul),
        "zh" => scripts.kana == 0 && scripts.hangul == 0 && scripts.mostly(scripts.han),
        "ru" => scripts.ukrainian == 0 && scripts.mostly(scripts.cyrillic),
        "uk" => scripts.ukrainian > 0 && scripts.mostly(scripts.cyrillic),
        base => {
            scripts.latin * 10 >= scripts.letters() * 9
                && latin_language(lines.iter().copied()) == Some(base)
        }
    }
}

/// What to ask for: pronunciation unless the words are already in the
/// Latin alphabet, translation unless they are already in the target
/// language. Both are asked for whichever switch is on, so turning the
/// other on later shows it from the same answer instead of asking again;
/// the switches only hide lines. `None` when both are off or there is
/// nothing to ask.
pub fn wanted(lines: &[&str], settings: &AiSettings, target: &Language) -> Option<Wanted> {
    if !settings.pronounce && !settings.translate {
        return None;
    }
    let scripts = Scripts::of(lines.iter().copied());
    let wanted = Wanted {
        pronunciation: scripts.non_latin() >= 5,
        translation: !already_in(lines, target),
    };
    (wanted.pronunciation || wanted.translation).then_some(wanted)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Wanted {
    pub pronunciation: bool,
    pub translation: bool,
}

// ---- app state -------------------------------------------------------------

/// What the app holds for the lyrics on show.
#[derive(Default)]
pub struct State {
    /// The track the lines below belong to.
    pub uri: Option<String>,
    /// The request the lines answer or are waited for.
    pub id: Option<String>,
    pub extras: Option<Extras>,
    /// Why the last request failed; shown quietly under the header.
    pub error: Option<String>,
    /// The last request found no key, so a new key asks again.
    pub unanswered: bool,
    /// Which keys are saved, once the backend has said.
    pub keys: Option<KeysPresent>,
    /// Why a key could not be stored or read.
    pub key_error: Option<String>,
}

impl State {
    /// The pronunciation and translation to draw under line `index` of the
    /// lyrics for `uri`, as far as the settings still want them: turning
    /// one off hides it at once, before any new answer.
    pub fn line(
        &self,
        uri: Option<&str>,
        index: usize,
        settings: &AiSettings,
    ) -> (Option<&str>, Option<&str>) {
        if uri.is_none() || self.uri.as_deref() != uri {
            return (None, None);
        }
        let Some(extra) = self.extras.as_ref().and_then(|extras| extras.line(index)) else {
            return (None, None);
        };
        (
            extra
                .pronunciation
                .as_deref()
                .filter(|_| settings.pronounce),
            extra.translation.as_deref().filter(|_| settings.translate),
        )
    }

    /// Line `index` cut into words with their readings, when pronunciation
    /// is on and the words could be matched to the line.
    pub fn words(&self, uri: Option<&str>, index: usize, settings: &AiSettings) -> Option<&[Ruby]> {
        if !(settings.pronounce || settings.translate)
            || uri.is_none()
            || self.uri.as_deref() != uri
        {
            return None;
        }
        self.extras
            .as_ref()
            .and_then(|extras| extras.line(index))
            .and_then(|extra| extra.words.as_deref())
    }

    /// Forget the last answer's failure so the next chance asks again.
    pub fn retry(&mut self) {
        if self.error.is_some() || self.unanswered {
            self.id = None;
            self.error = None;
            self.unanswered = false;
        }
    }
}

// ---- requests --------------------------------------------------------------

/// The lines under one lyric line. Either may be missing: a line already
/// in the Latin alphabet has no pronunciation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronunciation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    /// The line cut into words, each with its reading, so the reading can
    /// sit above its own word. `None` when the pieces could not be matched
    /// to the line; the whole-line pronunciation is shown then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<Ruby>>,
}

/// One piece of a lyric line and how it is pronounced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ruby {
    pub text: String,
    /// `None` for a piece already in the Latin alphabet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<String>,
    /// What the piece means, in the target language; `None` for a word
    /// already in it, or punctuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gloss: Option<String>,
    /// The line has a space after this piece.
    #[serde(default)]
    pub space: bool,
}

/// Matches a model's `words` to the line they came from: every piece must
/// be found in order, and only spaces and punctuation may lie between
/// them (kept as pieces without a reading). Unread pieces with spaces in
/// them are cut into words, so a long English hook can still wrap.
pub fn align(line: &str, words: &[Ruby]) -> Option<Vec<Ruby>> {
    fn push_plain(out: &mut Vec<Ruby>, text: &str) {
        let mut pieces = text.split_whitespace().peekable();
        if text.starts_with(char::is_whitespace)
            && let Some(last) = out.last_mut()
        {
            last.space = true;
        }
        while let Some(piece) = pieces.next() {
            out.push(Ruby {
                text: piece.to_string(),
                reading: None,
                gloss: None,
                space: pieces.peek().is_some(),
            });
        }
        if text.ends_with(char::is_whitespace)
            && let Some(last) = out.last_mut()
        {
            last.space = true;
        }
    }
    let mut out: Vec<Ruby> = Vec::new();
    let mut rest = line.trim();
    for word in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        let at = rest.find(text)?;
        let skipped = &rest[..at];
        if skipped.chars().any(char::is_alphanumeric) {
            return None;
        }
        push_plain(&mut out, skipped);
        let kept = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty() && !same_words(value, text))
                .map(str::to_string)
        };
        let (reading, gloss) = (kept(&word.reading), kept(&word.gloss));
        if reading.is_some() || gloss.is_some() {
            out.push(Ruby {
                text: text.to_string(),
                reading,
                gloss,
                space: false,
            });
        } else {
            push_plain(&mut out, text);
        }
        rest = &rest[at + text.len()..];
    }
    if rest.chars().any(char::is_alphanumeric) {
        return None;
    }
    push_plain(&mut out, rest);
    let out = merge_split_words(out);
    out.iter()
        .any(|piece| piece.reading.is_some() || piece.gloss.is_some())
        .then_some(out)
}

/// Joins neighbouring pieces that share one meaning: a model sometimes
/// cuts a word into characters and repeats its meaning on each (想 起,
/// "remember" twice), and the word reads better whole, its meaning once.
pub fn merge_split_words(pieces: Vec<Ruby>) -> Vec<Ruby> {
    let mut out: Vec<Ruby> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        if let Some(last) = out.last_mut()
            && !last.space
            && last.gloss.is_some()
            && last.gloss == piece.gloss
            && last.reading.is_some() == piece.reading.is_some()
        {
            last.text.push_str(&piece.text);
            if let (Some(held), Some(more)) = (&mut last.reading, &piece.reading) {
                held.push(' ');
                held.push_str(more);
            }
            last.space = piece.space;
            continue;
        }
        out.push(piece);
    }
    out
}

fn is_han(c: char) -> bool {
    matches!(
        u32::from(c),
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF
    )
}

/// Pairs a Chinese line with its pinyin character by character, for an
/// answer whose `words` do not match the line (a model may copy them in
/// the other script, simplified for traditional). Only for lines of Han
/// characters and punctuation, where one syllable is one character.
fn align_by_characters(line: &str, pronunciation: &str) -> Option<Vec<Ruby>> {
    if line.chars().any(|c| c.is_alphanumeric() && !is_han(c)) {
        return None;
    }
    let syllables: Vec<&str> = pronunciation
        .split_whitespace()
        .map(|syllable| syllable.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|syllable| !syllable.is_empty())
        .collect();
    if syllables.len() != line.chars().filter(|c| is_han(*c)).count() {
        return None;
    }
    let mut syllables = syllables.into_iter();
    let words: Vec<Ruby> = line
        .chars()
        .filter(|c| is_han(*c))
        .map(|c| Ruby {
            text: c.to_string(),
            reading: syllables.next().map(str::to_string),
            gloss: None,
            space: false,
        })
        .collect();
    align(line, &words)
}

/// Pairs a line and its pronunciation word by word, for an answer without
/// usable `words`: Korean and most other spaced scripts romanize one word
/// to one word.
fn align_by_spaces(line: &str, pronunciation: &str) -> Option<Vec<Ruby>> {
    let originals: Vec<&str> = line.split_whitespace().collect();
    let readings: Vec<&str> = pronunciation.split_whitespace().collect();
    if originals.len() != readings.len() || originals.len() < 2 {
        return None;
    }
    let words: Vec<Ruby> = originals
        .iter()
        .zip(&readings)
        .map(|(text, reading)| Ruby {
            text: text.to_string(),
            reading: Some(reading.to_string()),
            gloss: None,
            space: false,
        })
        .collect();
    align(line, &words)
}

/// One [`Extra`] per lyric line, in the lyrics' order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extras {
    pub lines: Vec<Extra>,
}

impl Extras {
    pub fn line(&self, index: usize) -> Option<&Extra> {
        self.lines
            .get(index)
            .filter(|extra| extra.pronunciation.is_some() || extra.translation.is_some())
    }
}

/// Everything one song's request needs except the key, which the backend
/// adds from the credential store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub uri: String,
    pub title: String,
    pub artist: String,
    /// Every lyric line, empty ones included, so indices match the lyrics.
    pub lines: Vec<String>,
    pub target: &'static str,
    pub wanted: Wanted,
    pub provider: Provider,
    pub model: String,
    pub base_url: String,
}

impl Request {
    /// What the answer depends on, so the app can tell a stale answer and
    /// the cache can find a fresh one: the words and the target language.
    /// The track, provider and model are left out, so the same lyrics on a
    /// single and its album, or asked of another provider, are answered
    /// once.
    pub fn id(&self) -> String {
        let mut hasher = Sha1::new();
        hasher.update(self.target.as_bytes());
        hasher.update([0]);
        hasher.update([
            u8::from(self.wanted.pronunciation),
            u8::from(self.wanted.translation),
        ]);
        for line in &self.lines {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The distinct lines worth sending, in first-sung order: a chorus is
    /// translated once.
    fn distinct(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        self.lines
            .iter()
            .map(|line| line.trim())
            .filter(|line| sendable(line) && seen.insert(*line))
            .collect()
    }
}

fn sendable(line: &str) -> bool {
    line.chars().any(char::is_alphanumeric)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No key for the chosen provider: the feature is simply not set up.
    NoKey,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoKey => f.write_str("no API key"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

fn prompt(request: &Request, distinct: &[&str]) -> (String, String) {
    let target = language(request.target).unwrap_or(&LANGUAGES[0]);
    let system = "You add pronunciation and translation lines to song lyrics for a music \
                  player. You answer with one JSON object and nothing else."
        .to_string();
    let mut user = String::new();
    if !request.title.is_empty() {
        user.push_str(&format!("Song: \"{}\"", request.title));
        if !request.artist.is_empty() {
            user.push_str(&format!(" by {}", request.artist));
        }
        user.push_str("\n\n");
    }
    user.push_str("Lyrics, one numbered line each:\n");
    for (index, line) in distinct.iter().enumerate() {
        user.push_str(&format!("{}\t{}\n", index + 1, line));
    }
    let mut fields = Vec::new();
    if request.wanted.pronunciation {
        fields.push("\"pronunciation\": string or null");
    }
    if request.wanted.translation {
        fields.push("\"translation\": string or null");
    }
    fields.push("\"words\": [[original, reading or null, meaning or null], ...]");
    user.push_str(&format!(
        "\nAnswer with this JSON: {{\"language\": ISO 639-1 code of the lyrics' main \
         language, \"lines\": [{{\"i\": line number, {}}}]}}, with exactly one entry for \
         each of the {} numbered lines, in order.\n",
        fields.join(", "),
        distinct.len()
    ));
    if request.wanted.pronunciation {
        user.push_str(
            "- pronunciation: how the line is sung, written in the Latin alphabet with the \
             standard romanization: Hepburn for Japanese (by the reading actually sung), \
             Revised Romanization for Korean, Hanyu Pinyin with tone marks for Chinese, the \
             usual transliteration for any other script. Keep words already in the Latin \
             alphabet as they are. null for a line written entirely in the Latin alphabet.\n",
        );
    }
    user.push_str(&format!(
        "- words: the same line cut into its words, in order, each as [original, reading, \
         meaning]. original is copied exactly from the line, so the originals rebuild it. \
         Keep the pieces short, one word each, never a whole phrase: Chinese in words of one \
         to three characters, Japanese in single words with their particles apart and kanji \
         kept with their okurigana. {} {}\n",
        if request.wanted.pronunciation {
            "reading is that word's part of the pronunciation (spaces between the syllables \
             of a Chinese reading), or null for a word already in the Latin alphabet."
        } else {
            "reading is always null."
        },
        if request.wanted.translation {
            format!(
                "meaning is that word's meaning in {} in one to three words, as it is meant in \
                 this line, or null for punctuation, sounds such as \"oh\", and words already \
                 in {}.",
                target.english, target.english
            )
        } else {
            "meaning is always null.".to_string()
        }
    ));
    if request.wanted.translation {
        user.push_str(&format!(
            "- translation: the line's meaning in natural {} ({}), read in the context of the \
             whole song. Never merge or split lines. null for a line already in {}.\n",
            target.english, target.native, target.english
        ));
    }
    user.push_str("Keep the line numbers exact. No commentary, no Markdown.");
    (system, user)
}

/// Asks the provider for the song's extra lines, or answers from the cache.
pub async fn fetch(
    http: &reqwest::Client,
    cache_dir: &Path,
    request: &Request,
    keys: &Keys,
) -> Result<Extras, Error> {
    let path = cache_path(cache_dir, request);
    if let Some(cached) = read_cache(&path, request.lines.len()) {
        log::debug!("lyrics extras for {}: from the cache", request.uri);
        return Ok(cached);
    }
    let key = keys.get(request.provider);
    if key.is_empty() {
        return Err(Error::NoKey);
    }
    let distinct = request.distinct();
    if distinct.is_empty() {
        return Ok(Extras {
            lines: vec![Extra::default(); request.lines.len()],
        });
    }
    let (system, user) = prompt(request, &distinct);
    log::info!(
        "lyrics extras for {}: asking {} ({}) about {} lines",
        request.uri,
        request.provider.label(),
        request.model,
        distinct.len()
    );
    let text = match request.provider {
        Provider::Gemini => gemini(http, request, key, &system, &user).await,
        Provider::OpenAiCompatible => openai(http, request, key, &system, &user).await,
    }
    .map_err(|message| Error::Failed(scrub(&message, key)))?;
    let parsed = parse(&text, distinct.len()).map_err(|error| {
        // Lyrics and the model's words only, never the key.
        let head: String = text.chars().take(300).collect();
        let tail: String = {
            let mut tail: Vec<char> = text.chars().rev().take(300).collect();
            tail.reverse();
            tail.into_iter().collect()
        };
        log::debug!(
            "lyrics extras answer unreadable ({} chars): {head:?} … {tail:?}",
            text.chars().count()
        );
        Error::Failed(error)
    })?;
    let extras = spread(request, &distinct, parsed);
    write_cache(&path, &extras);
    Ok(extras)
}

/// Never let a key a provider echoes reach the interface or the log.
fn scrub(message: &str, key: &str) -> String {
    let message: String = message.chars().take(300).collect();
    if key.len() >= 8 {
        message.replace(key, "…")
    } else {
        message
    }
}

async fn gemini(
    http: &reqwest::Client,
    request: &Request,
    key: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let model = request.model.trim().trim_start_matches("models/");
    let mut generation = serde_json::json!({
        "responseMimeType": "application/json",
        "temperature": 0.2,
    });
    // Thinking adds a long wait and nothing a transliteration needs; the
    // Flash models can turn it off.
    if model.starts_with("gemini-2.5-flash") {
        generation["thinkingConfig"] = serde_json::json!({ "thinkingBudget": 0 });
    }
    let body = serde_json::json!({
        "systemInstruction": { "parts": [{ "text": system }] },
        "contents": [{ "role": "user", "parts": [{ "text": user }] }],
        "generationConfig": generation,
    });
    let url = format!(
        "{}/models/{}:generateContent",
        request.base_url.trim_end_matches('/'),
        model
    );
    // The key goes in a header, never the URL, so no request log shows it.
    let response = http
        .post(url)
        .header("x-goog-api-key", key)
        .timeout(REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("cannot reach Gemini: {}", error.without_url()))?;
    let json = answer(response, "Gemini").await?;
    let parts = json
        .pointer("/candidates/0/content/parts")
        .and_then(|parts| parts.as_array())
        .ok_or_else(|| {
            let reason = json
                .pointer("/promptFeedback/blockReason")
                .or_else(|| json.pointer("/candidates/0/finishReason"))
                .and_then(|value| value.as_str())
                .unwrap_or("no answer");
            format!("Gemini gave no lyrics ({reason})")
        })?;
    // Thought parts, when a model sends them, are not the answer.
    if let Some(reason) = json
        .pointer("/candidates/0/finishReason")
        .and_then(|value| value.as_str())
        .filter(|reason| *reason != "STOP")
    {
        log::debug!("Gemini stopped early: {reason}");
    }
    Ok(parts
        .iter()
        .filter(|part| part.get("thought").and_then(|value| value.as_bool()) != Some(true))
        .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
        .collect())
}

async fn openai(
    http: &reqwest::Client,
    request: &Request,
    key: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let url = format!(
        "{}/chat/completions",
        request.base_url.trim_end_matches('/')
    );
    let mut body = serde_json::json!({
        "model": request.model.trim(),
        "temperature": 0.2,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let mut retried = false;
    loop {
        let response = http
            .post(&url)
            .bearer_auth(key)
            .timeout(REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("cannot reach {url}: {}", error.without_url()))?;
        // Some models behind a compatible endpoint refuse JSON mode; the
        // prompt asks for JSON anyway, so ask once more without it.
        if response.status() == reqwest::StatusCode::BAD_REQUEST && !retried {
            retried = true;
            if let Some(fields) = body.as_object_mut() {
                fields.remove("response_format");
            }
            continue;
        }
        let json = answer(response, "the AI provider").await?;
        return json
            .pointer("/choices/0/message/content")
            .and_then(|content| content.as_str())
            .map(str::to_string)
            .ok_or_else(|| "the AI provider gave no lyrics".to_string());
    }
}

/// The JSON body of a successful answer, or the provider's own message.
async fn answer(response: reqwest::Response, who: &str) -> Result<serde_json::Value, String> {
    let status = response.status();
    let json: Option<serde_json::Value> = response.json().await.ok();
    if status.is_success() {
        return json.ok_or_else(|| format!("unexpected answer from {who}"));
    }
    let detail = json
        .as_ref()
        .and_then(|json| {
            json.pointer("/error/message")
                .or_else(|| json.pointer("/0/error/message"))
                .or_else(|| json.get("message"))
        })
        .and_then(|message| message.as_str())
        .map(|message| format!(": {message}"))
        .unwrap_or_default();
    Err(format!("{who} answered {status}{detail}"))
}

// ---- answers ---------------------------------------------------------------

/// A model's answer, one entry per distinct line sent.
#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    /// The lyrics' language as the model judged it.
    pub language: Option<String>,
    pub lines: Vec<Extra>,
}

/// Reads a model's JSON answer for `count` lines. Tolerates what models do
/// to JSON in practice: Markdown fences, words around it, a bare array, and
/// numbered entries out of order or missing.
pub fn parse(text: &str, count: usize) -> Result<Parsed, String> {
    let value = extract_json(text).ok_or_else(|| "the AI answer was not JSON".to_string())?;
    let (language, items) = match &value {
        serde_json::Value::Array(items) => (None, items.as_slice()),
        serde_json::Value::Object(fields) => (
            fields
                .get("language")
                .and_then(|value| value.as_str())
                .map(|code| code.trim().to_lowercase())
                .filter(|code| !code.is_empty()),
            fields
                .get("lines")
                .and_then(|lines| lines.as_array())
                .map(Vec::as_slice)
                .ok_or_else(|| "the AI answer had no lines".to_string())?,
        ),
        _ => return Err("the AI answer had no lines".into()),
    };
    let mut lines = vec![Extra::default(); count];
    let mut filled = 0;
    for (position, item) in items.iter().enumerate() {
        let index = ["i", "index", "line"]
            .iter()
            .find_map(|name| item.get(*name).and_then(number))
            .and_then(|number| number.checked_sub(1))
            .unwrap_or(position);
        let Some(slot) = lines.get_mut(index) else {
            continue;
        };
        let field = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| item.get(*name).and_then(|value| value.as_str()))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        slot.pronunciation = field(&["pronunciation", "romanization", "phonetic"]);
        slot.translation = field(&["translation"]);
        slot.words = item
            .get("words")
            .and_then(|words| words.as_array())
            .map(|words| words.iter().filter_map(ruby_piece).collect());
        filled += 1;
    }
    if filled == 0 && count > 0 {
        return Err("the AI answer had no lines".into());
    }
    Ok(Parsed { language, lines })
}

/// `["원문", "reading"]`, or `{"text": .., "reading": ..}` from a model
/// that prefers objects.
fn ruby_piece(value: &serde_json::Value) -> Option<Ruby> {
    let (text, reading, gloss) = match value {
        serde_json::Value::Array(parts) => (parts.first()?, parts.get(1), parts.get(2)),
        serde_json::Value::Object(fields) => (
            fields.get("text").or_else(|| fields.get("original"))?,
            fields
                .get("reading")
                .or_else(|| fields.get("pronunciation")),
            fields.get("meaning").or_else(|| fields.get("gloss")),
        ),
        _ => return None,
    };
    let string = |value: Option<&serde_json::Value>| {
        value.and_then(|value| value.as_str()).map(str::to_string)
    };
    Some(Ruby {
        text: text.as_str()?.to_string(),
        reading: string(reading),
        gloss: string(gloss),
        space: false,
    })
}

fn number(value: &serde_json::Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn extract_json(text: &str) -> Option<serde_json::Value> {
    let text = text.trim();
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    // The outermost object or array, wherever the model put it.
    let start = text.find(['{', '['])?;
    let close = if text[start..].starts_with('{') {
        '}'
    } else {
        ']'
    };
    let end = text.rfind(close)?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// Gives every lyric line the answer for its words, drops lines that only
/// repeat the original, and drops every translation when the model says
/// a Latin-script song is already in the target language. A song with
/// lines in another script (K-pop with English hooks) keeps them: the model
/// may name the hooks' language, and the other lines still need theirs.
fn spread(request: &Request, distinct: &[&str], parsed: Parsed) -> Extras {
    let target = language(request.target).map_or(request.target, Language::base);
    let same_language = !request.wanted.pronunciation
        && parsed
            .language
            .as_deref()
            .is_some_and(|code| code.split('-').next() == Some(target));
    let by_text: std::collections::HashMap<&str, &Extra> =
        distinct.iter().copied().zip(&parsed.lines).collect();
    let lines = request
        .lines
        .iter()
        .map(|line| {
            let original = line.trim();
            let Some(extra) = by_text.get(original) else {
                return Extra::default();
            };
            let differs =
                |text: &Option<String>| text.clone().filter(|text| !same_words(text, original));
            let pronunciation =
                differs(&extra.pronunciation).filter(|_| request.wanted.pronunciation);
            let glosses = request.wanted.translation && !same_language;
            let words = extra
                .words
                .as_deref()
                .and_then(|words| {
                    let words: Vec<Ruby> = words
                        .iter()
                        .map(|word| Ruby {
                            reading: word
                                .reading
                                .clone()
                                .filter(|_| request.wanted.pronunciation),
                            gloss: word.gloss.clone().filter(|_| glosses),
                            ..word.clone()
                        })
                        .collect();
                    align(original, &words)
                })
                .or_else(|| {
                    let pronunciation = pronunciation.as_deref()?;
                    align_by_spaces(original, pronunciation)
                        .or_else(|| align_by_characters(original, pronunciation))
                });
            Extra {
                pronunciation,
                translation: differs(&extra.translation).filter(|_| glosses),
                words,
            }
        })
        .collect();
    Extras { lines }
}

fn same_words(a: &str, b: &str) -> bool {
    let squash = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    squash(a) == squash(b)
}

// ---- cache -----------------------------------------------------------------

/// 2: answers for mixed-script songs no longer lose their translations.
/// 3: readings word by word.
/// 4: shorter pieces, so readings do not run together.
/// 5: Chinese lines whose pieces do not match pair character by character.
/// 6: each word's meaning under it.
const CACHE_VERSION: u32 = 6;

#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    extras: Extras,
}

/// The current format's folder. Answers in an older format can never be
/// read again, so the first look at the cache in a run removes them.
pub fn cache_dir(lyrics_cache: &Path) -> PathBuf {
    let root = lyrics_cache.join("extras");
    let current = root.join(format!("v{CACHE_VERSION}"));
    static PRUNED: std::sync::Once = std::sync::Once::new();
    PRUNED.call_once(|| {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == current {
                continue;
            }
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
        }
    });
    current
}

fn cache_path(cache_dir: &Path, request: &Request) -> PathBuf {
    cache_dir.join(format!("{}.json", request.id()))
}

fn read_cache(path: &Path, line_count: usize) -> Option<Extras> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    if modified.elapsed().unwrap_or(CACHE_LIFETIME) >= CACHE_LIFETIME {
        return None;
    }
    let cached: Cached = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if cached.version != CACHE_VERSION || cached.extras.lines.len() != line_count {
        return None;
    }
    let mut extras = cached.extras;
    for line in &mut extras.lines {
        if let Some(words) = line.words.take() {
            line.words = Some(merge_split_words(words));
        }
    }
    Some(extras)
}

fn write_cache(path: &Path, extras: &Extras) {
    let Ok(bytes) = serde_json::to_vec(&Cached {
        version: CACHE_VERSION,
        extras: extras.clone(),
    }) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = fastframe_file::write(path, bytes) {
        log::debug!("lyrics extras cache write failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(lines: &[&str], wanted: Wanted) -> Request {
        Request {
            uri: "spotify:track:abc".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            lines: lines.iter().map(|line| line.to_string()).collect(),
            target: "en",
            wanted,
            provider: Provider::Gemini,
            model: DEFAULT_GEMINI_MODEL.into(),
            base_url: GEMINI_BASE_URL.into(),
        }
    }

    const BOTH: Wanted = Wanted {
        pronunciation: true,
        translation: true,
    };

    #[test]
    fn a_plain_answer_parses_line_by_line() {
        let parsed = parse(
            r#"{"language":"ja","lines":[
                {"i":1,"pronunciation":"yume naraba","translation":"If it were a dream"},
                {"i":2,"pronunciation":"sayonara","translation":null}]}"#,
            2,
        )
        .unwrap();
        assert_eq!(parsed.language.as_deref(), Some("ja"));
        assert_eq!(
            parsed.lines[0].pronunciation.as_deref(),
            Some("yume naraba")
        );
        assert_eq!(
            parsed.lines[0].translation.as_deref(),
            Some("If it were a dream")
        );
        assert_eq!(parsed.lines[1].translation, None);
    }

    #[test]
    fn fences_words_and_bare_arrays_are_tolerated() {
        let fenced = "Here you go:\n```json\n[{\"pronunciation\":\"saranghae\",\"translation\":\"I love you\"}]\n```";
        let parsed = parse(fenced, 1).unwrap();
        assert_eq!(parsed.language, None);
        assert_eq!(parsed.lines[0].pronunciation.as_deref(), Some("saranghae"));
    }

    #[test]
    fn numbered_entries_land_on_their_line_whatever_the_order() {
        let parsed = parse(
            r#"{"lines":[{"i":"3","translation":"c"},{"i":1,"translation":"a"},{"i":9,"translation":"x"}]}"#,
            3,
        )
        .unwrap();
        let translations: Vec<_> = parsed
            .lines
            .iter()
            .map(|line| line.translation.as_deref())
            .collect();
        assert_eq!(translations, vec![Some("a"), None, Some("c")]);
    }

    #[test]
    fn an_answer_without_lines_is_an_error() {
        assert!(parse("I can't help with that.", 2).is_err());
        assert!(parse(r#"{"language":"ja"}"#, 2).is_err());
        assert!(parse(r#"{"lines":[]}"#, 2).is_err());
    }

    #[test]
    fn repeated_lines_are_sent_once_and_answered_everywhere() {
        let request = request(&["사랑해", "", "보고 싶어", "사랑해"], BOTH);
        let distinct = request.distinct();
        assert_eq!(distinct, vec!["사랑해", "보고 싶어"]);
        let parsed = parse(
            r#"{"language":"ko","lines":[
                {"i":1,"pronunciation":"saranghae","translation":"I love you"},
                {"i":2,"pronunciation":"bogo sipeo","translation":"I miss you"}]}"#,
            2,
        )
        .unwrap();
        let extras = spread(&request, &distinct, parsed);
        assert_eq!(extras.lines.len(), 4);
        assert_eq!(extras.lines[3].pronunciation.as_deref(), Some("saranghae"));
        assert!(extras.line(1).is_none(), "an empty line gets nothing");
    }

    #[test]
    fn echoes_of_the_original_and_same_language_translations_are_dropped() {
        let request = request(&["Hello 世界"], BOTH);
        let distinct = request.distinct();
        let parsed = Parsed {
            language: Some("en".into()),
            lines: vec![Extra {
                pronunciation: Some("hello SHÌJIÈ".into()),
                translation: Some("hello, 世界!".into()),
                words: None,
            }],
        };
        let extras = spread(&request, &distinct, parsed);
        assert!(extras.lines[0].pronunciation.is_some());
        assert_eq!(extras.lines[0].translation, None);
    }

    fn piece(text: &str, reading: Option<&str>) -> Ruby {
        Ruby {
            text: text.into(),
            reading: reading.map(Into::into),
            gloss: None,
            space: false,
        }
    }

    fn shown(pieces: &[Ruby]) -> Vec<(String, Option<String>, bool)> {
        pieces
            .iter()
            .map(|piece| (piece.text.clone(), piece.reading.clone(), piece.space))
            .collect()
    }

    #[test]
    fn readings_land_on_their_words_with_punctuation_and_spaces_kept() {
        let words = [
            piece("놀아보자", Some("noraboja")),
            piece("얼쑤", Some("eolssu")),
            piece("얼쑤", Some("eolssu")),
        ];
        let aligned = align("놀아보자 얼쑤 (얼쑤!)", &words).unwrap();
        let r = |text: &str| Some(text.to_string());
        assert_eq!(
            shown(&aligned),
            vec![
                ("놀아보자".into(), r("noraboja"), true),
                ("얼쑤".into(), r("eolssu"), true),
                ("(".into(), None, false),
                ("얼쑤".into(), r("eolssu"), false),
                ("!)".into(), None, false),
            ]
        );
    }

    #[test]
    fn unread_words_split_so_english_hooks_can_wrap() {
        let words = [
            piece("Let's go", None),
            piece("삐걱", Some("ppigeok")),
            piece("lose control", Some("lose control")),
        ];
        let aligned = align("Let's go 삐걱 lose control", &words).unwrap();
        let texts: Vec<_> = aligned.iter().map(|piece| piece.text.as_str()).collect();
        assert_eq!(texts, vec!["Let's", "go", "삐걱", "lose", "control"]);
        assert!(aligned[1].space && aligned[2].space && aligned[3].space);
        assert_eq!(
            aligned[3].reading, None,
            "a reading equal to the word is dropped"
        );
    }

    #[test]
    fn unspaced_scripts_align_and_bad_pieces_are_refused() {
        let aligned = align(
            "夢ならば",
            &[piece("夢", Some("yume")), piece("ならば", Some("naraba"))],
        )
        .unwrap();
        assert_eq!(aligned.len(), 2);
        assert!(!aligned[0].space);
        // A word the line does not have, or words skipped over.
        assert_eq!(align("夢ならば", &[piece("雨", Some("ame"))]), None);
        assert_eq!(align("夢ならば", &[piece("ならば", Some("naraba"))]), None);
        // Nothing to read: plain text is drawn as before.
        assert_eq!(align("Sit back", &[piece("Sit back", None)]), None);
    }

    #[test]
    fn chinese_pairs_character_by_character_when_its_words_do_not_match() {
        let aligned =
            align_by_characters("沒想到 失去的勇氣？", "méi xiǎng dào shī qù de yǒng qì?").unwrap();
        let texts: Vec<_> = aligned.iter().map(|piece| piece.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["沒", "想", "到", "失", "去", "的", "勇", "氣", "？"]
        );
        assert_eq!(aligned[7].reading.as_deref(), Some("qì"));
        assert!(aligned[2].space);
        // A syllable short, or a line with other letters in it: no guess.
        assert_eq!(align_by_characters("沒想到", "méi xiǎng"), None);
        assert_eq!(align_by_characters("夢ならば", "yu me na ra ba"), None);
        assert_eq!(
            align_by_characters("沒想到 yeah", "méi xiǎng dào yeah"),
            None
        );
    }

    #[test]
    fn each_word_carries_its_meaning_under_it() {
        let parsed = parse(
            r#"{"language":"zh","lines":[{"i":1,"pronunciation":"dàn piān piān","translation":"But unfortunately",
                "words":[["但","dàn","but"],["偏偏","piān piān","unfortunately"]]}]}"#,
            1,
        )
        .unwrap();
        let request = request(&["但偏偏"], BOTH);
        let extras = spread(&request, &request.distinct(), parsed);
        let words = extras.lines[0].words.as_ref().unwrap();
        assert_eq!(words[1].reading.as_deref(), Some("piān piān"));
        assert_eq!(words[1].gloss.as_deref(), Some("unfortunately"));

        // Translation off: the words keep their readings and lose meanings.
        let parsed = parse(
            r#"{"lines":[{"i":1,"pronunciation":"dàn piān piān","words":[["但","dàn","but"],["偏偏","piān piān","unfortunately"]]}]}"#,
            1,
        )
        .unwrap();
        let readings_only = super::tests::request(
            &["但偏偏"],
            Wanted {
                pronunciation: true,
                translation: false,
            },
        );
        let extras = spread(&readings_only, &readings_only.distinct(), parsed);
        let words = extras.lines[0].words.as_ref().unwrap();
        assert_eq!(words[0].gloss, None);
        assert_eq!(words[0].reading.as_deref(), Some("dàn"));

        // A Latin-script song gets meanings alone.
        let spanish = super::tests::request(
            &["te quiero"],
            Wanted {
                pronunciation: false,
                translation: true,
            },
        );
        let parsed = parse(
            r#"{"lines":[{"i":1,"translation":"I love you","words":[["te",null,"you"],["quiero",null,"I want"]]}]}"#,
            1,
        )
        .unwrap();
        let extras = spread(&spanish, &spanish.distinct(), parsed);
        let words = extras.lines[0].words.as_ref().unwrap();
        assert_eq!(words[1].gloss.as_deref(), Some("I want"));
        assert!(words[0].space);
    }

    #[test]
    fn a_word_cut_into_characters_is_joined_under_one_meaning() {
        let gloss = |text: &str, reading: &str, gloss: &str| Ruby {
            text: text.into(),
            reading: Some(reading.into()),
            gloss: Some(gloss.into()),
            space: false,
        };
        let merged = merge_split_words(vec![
            gloss("想", "xiǎng", "remember"),
            gloss("起", "qǐ", "remember"),
            gloss("花", "huā", "flower"),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "想起");
        assert_eq!(merged[0].reading.as_deref(), Some("xiǎng qǐ"));
        assert_eq!(merged[0].gloss.as_deref(), Some("remember"));
        // Separate words that happen to mean the same stay apart.
        let mut apart = gloss("走", "zǒu", "go");
        apart.space = true;
        assert_eq!(
            merge_split_words(vec![apart, gloss("走", "zǒu", "go")]).len(),
            2
        );
    }

    #[test]
    fn a_spaced_pronunciation_pairs_word_by_word_without_words() {
        let aligned = align_by_spaces("까마디 까만 이", "kkamadi kkaman i").unwrap();
        assert_eq!(aligned[1].reading.as_deref(), Some("kkaman"));
        assert_eq!(align_by_spaces("까마디 까만", "kkamadi kkaman i"), None);
        let parsed = parse(
            r#"{"lines":[{"i":1,"pronunciation":"bureul jipyeora","words":[["불을","bureul"],["지펴라","jipyeora"]]}]}"#,
            1,
        )
        .unwrap();
        let request = request(&["불을 지펴라"], BOTH);
        let extras = spread(&request, &request.distinct(), parsed);
        let words = extras.lines[0].words.as_ref().unwrap();
        assert_eq!(words[0].reading.as_deref(), Some("bureul"));
    }

    #[test]
    fn a_mixed_script_song_keeps_translations_whatever_language_the_model_names() {
        let request = request(&["불을 지펴라", "Sit back relax"], BOTH);
        let distinct = request.distinct();
        let parsed = parse(
            r#"{"language":"en","lines":[
                {"i":1,"pronunciation":"bureul jipyeora","translation":"Light the fire"},
                {"i":2,"pronunciation":null,"translation":null}]}"#,
            2,
        )
        .unwrap();
        let extras = spread(&request, &distinct, parsed);
        assert_eq!(
            extras.lines[0].translation.as_deref(),
            Some("Light the fire")
        );
        assert_eq!(extras.lines[1], Extra::default());

        // A Latin-script song the model calls English, for an English
        // reader, gets no translations.
        let latin = super::tests::request(
            &["Ich liebe dich"],
            Wanted {
                pronunciation: false,
                translation: true,
            },
        );
        let distinct = latin.distinct();
        let parsed = Parsed {
            language: Some("en".into()),
            lines: vec![Extra {
                pronunciation: None,
                translation: Some("I love you".into()),
                words: None,
            }],
        };
        assert_eq!(spread(&latin, &distinct, parsed).lines[0].translation, None);
    }

    #[test]
    fn pronunciation_is_skipped_for_latin_lyrics_and_translation_for_the_target_language() {
        let english = [
            "I don't know what you want from me",
            "And all the things you said",
        ];
        let settings = AiSettings::default();
        let en = language("en").unwrap();
        assert_eq!(wanted(&english, &settings, en), None);
        let es = language("es").unwrap();
        assert_eq!(
            wanted(&english, &settings, es),
            Some(Wanted {
                pronunciation: false,
                translation: true
            })
        );
        let japanese = [
            "夢ならばどれほどよかったでしょう",
            "未だにあなたのことを夢にみる",
        ];
        assert_eq!(wanted(&japanese, &settings, en), Some(BOTH));
        assert_eq!(
            wanted(&japanese, &settings, language("ja").unwrap()),
            Some(Wanted {
                pronunciation: true,
                translation: false
            })
        );
        let korean = ["사랑해 너를", "보고 싶어"];
        assert_eq!(
            wanted(&korean, &settings, language("ko").unwrap()),
            Some(Wanted {
                pronunciation: true,
                translation: false
            })
        );
        let off = AiSettings {
            pronounce: false,
            translate: false,
            ..AiSettings::default()
        };
        assert_eq!(wanted(&japanese, &off, en), None);
        // One switch on asks for both, so the other shows later without a
        // second request.
        let translation_only = AiSettings {
            pronounce: false,
            ..AiSettings::default()
        };
        assert_eq!(wanted(&japanese, &translation_only, en), Some(BOTH));
    }

    #[test]
    fn the_interface_language_is_the_default_target() {
        let settings = AiSettings::default();
        assert_eq!(settings.target(crate::i18n::Locale::German).tag, "de");
        assert_eq!(
            settings.target(crate::i18n::Locale::PortugueseBrazil).tag,
            "pt-BR"
        );
        let chosen = AiSettings {
            target_language: "ko".into(),
            ..AiSettings::default()
        };
        assert_eq!(chosen.target(crate::i18n::Locale::German).tag, "ko");
    }

    #[test]
    fn the_cache_answers_the_same_song_and_language_only() {
        let dir = std::env::temp_dir().join(format!(
            "spotifast-extras-{}-{}",
            std::process::id(),
            line!()
        ));
        let request = request(&["사랑해", "보고 싶어"], BOTH);
        let extras = Extras {
            lines: vec![
                Extra {
                    pronunciation: Some("saranghae".into()),
                    translation: Some("I love you".into()),
                    words: None,
                },
                Extra::default(),
            ],
        };
        let path = cache_path(&dir, &request);
        assert_eq!(read_cache(&path, 2), None);
        write_cache(&path, &extras);
        assert_eq!(read_cache(&path, 2), Some(extras.clone()));
        // A different line count means the lyrics changed under the cache.
        assert_eq!(read_cache(&path, 3), None);

        // Another language or other words are another entry.
        let mut korean_target = request.clone();
        korean_target.target = "ko";
        assert_ne!(cache_path(&dir, &korean_target), path);
        let mut other_words = request.clone();
        other_words.lines[1] = "다른 말".into();
        assert_ne!(cache_path(&dir, &other_words), path);
        // The same words on another track (a single and its album) too.
        let mut other_track = request.clone();
        other_track.uri = "spotify:track:single".into();
        assert_eq!(cache_path(&dir, &other_track), path);
        // Another provider's answer for the same words serves.
        let mut other_provider = request.clone();
        other_provider.provider = Provider::OpenAiCompatible;
        assert_eq!(cache_path(&dir, &other_provider), path);

        // A cached answer is used without a key or a network.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let fetched = runtime.block_on(fetch(
            &reqwest::Client::new(),
            &dir,
            &request,
            &Keys::default(),
        ));
        assert_eq!(fetched, Ok(extras));
        // Nothing cached and no key: not set up, and nothing is sent.
        let fetched = runtime.block_on(fetch(
            &reqwest::Client::new(),
            &dir,
            &korean_target,
            &Keys::default(),
        ));
        assert_eq!(fetched, Err(Error::NoKey));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_prompt_numbers_distinct_lines_and_asks_only_for_what_is_wanted() {
        let request = request(
            &["사랑해", "사랑해", "보고 싶어"],
            Wanted {
                pronunciation: true,
                translation: false,
            },
        );
        let (_, user) = prompt(&request, &request.distinct());
        assert!(user.contains("1\t사랑해\n2\t보고 싶어\n"));
        assert!(user.contains("\"pronunciation\""));
        assert!(!user.contains("\"translation\""));
    }

    #[test]
    fn each_provider_links_to_its_key_page() {
        assert_eq!(
            key_page(Provider::Gemini, "").map(|page| page.1),
            Some("https://aistudio.google.com/apikey")
        );
        let page = |url| key_page(Provider::OpenAiCompatible, url).map(|page| page.0);
        assert_eq!(page("https://openrouter.ai/api/v1"), Some("OpenRouter"));
        assert_eq!(page("https://api.openai.com/v1"), Some("OpenAI"));
        assert_eq!(page("https://api.groq.com/openai/v1"), Some("Groq"));
        assert_eq!(page("http://localhost:1234/v1"), None);
    }

    #[test]
    fn keys_never_reach_messages_or_debug_output() {
        let key = "AIzaSyExampleExampleExample";
        assert_eq!(
            scrub(&format!("bad key {key} rejected"), key),
            "bad key … rejected"
        );
        assert_eq!(format!("{:?}", SecretKey(key.into())), "SecretKey(..)");
    }
}
