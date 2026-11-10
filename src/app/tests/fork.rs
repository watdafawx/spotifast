//! Fork: tests of the fork's part of App (src/app/fork.rs).

use super::*;
use crate::fork::{ArtistGenres, GENRES_RETRY};

#[test]
fn playing_genres_are_asked_once_per_artist_and_retried_after_failure() {
    let mut app = test_app("playing-genres");
    app.backend.set_offline(true);
    app.local = LocalState {
        playback: Playback::Playing,
        track: Some(crate::player::LocalTrack {
            uri: "spotify:track:song".into(),
            title: "Song".into(),
            artists: vec![
                ArtistRef {
                    id: Some("a".into()),
                    name: "A".into(),
                    uri: Some("spotify:artist:a".into()),
                },
                ArtistRef {
                    id: Some("b".into()),
                    name: "B".into(),
                    uri: Some("spotify:artist:b".into()),
                },
            ],
            duration_ms: 200_000,
            ..crate::player::LocalTrack::default()
        }),
        ..LocalState::default()
    };

    app.settings.fork.player_bar_genres = false;
    app.request_playing_genres();
    assert!(app.fork.artist_genres.is_empty());

    app.settings.fork.player_bar_genres = true;
    app.request_playing_genres();
    assert_eq!(
        app.fork.artist_genres.get("a"),
        Some(&ArtistGenres::Loading)
    );
    assert_eq!(
        app.fork.artist_genres.get("b"),
        Some(&ArtistGenres::Loading)
    );

    app.handle_backend_events(vec![
        Event::Fork(crate::fork::Event::ArtistGenres {
            id: "a".into(),
            result: Ok(vec!["hip hop".into(), "r&b".into()]),
        }),
        Event::Fork(crate::fork::Event::ArtistGenres {
            id: "b".into(),
            result: Err("MusicBrainz answered 503".into()),
        }),
    ]);
    let now = app.now_playing().unwrap();
    assert_eq!(app.playing_genres(&now), "Hip Hop, R&B");

    // A fresh failure waits; an old one is asked again.
    app.request_playing_genres();
    assert!(matches!(
        app.fork.artist_genres["b"],
        ArtistGenres::Failed(_)
    ));
    app.fork.artist_genres.insert(
        "b".into(),
        ArtistGenres::Failed(Instant::now() - GENRES_RETRY),
    );
    app.request_playing_genres();
    assert_eq!(
        app.fork.artist_genres.get("b"),
        Some(&ArtistGenres::Loading)
    );
    assert!(matches!(
        app.fork.artist_genres["a"],
        ArtistGenres::Known(_)
    ));

    // Saving a Last.fm key asks again for the artists nobody knew.
    app.fork
        .artist_genres
        .insert("b".into(), ArtistGenres::Known(Vec::new()));
    app.fork.lyrics_ai.keys = Some(crate::fork::lyrics_ai::KeysPresent::default());
    app.handle_backend_events(vec![Event::Fork(crate::fork::Event::AiKeys(Ok(
        crate::fork::lyrics_ai::KeysPresent {
            lastfm: true,
            ..Default::default()
        },
    )))]);
    assert!(!app.fork.artist_genres.contains_key("b"));
    assert!(matches!(
        app.fork.artist_genres["a"],
        ArtistGenres::Known(_)
    ));
    app.backend.shutdown();
}

/// Fork: the extra lines arrive after the lyrics and stay with them; an
/// answer for an earlier request is ignored, and turning a line off
/// hides it at once.
#[test]
fn lyrics_extras_fill_in_under_the_lyrics_without_a_second_request() {
    let mut app = test_app("lyrics-extras");
    let uri = "spotify:track:extras".to_string();
    app.lyrics_uri = Some(uri.clone());
    app.lyrics = Loadable::Loaded(Some(crate::lyrics::Lyrics {
        lines: vec![
            crate::lyrics::Line {
                at_ms: Some(0),
                text: "夢ならばどれほどよかったでしょう".into(),
            },
            crate::lyrics::Line {
                at_ms: Some(5_000),
                text: "未だにあなたのことを夢にみる".into(),
            },
        ],
        synced: true,
        instrumental: false,
    }));
    app.settings.fork.lyrics_ai.target_language = "en".into();

    // No key for the chosen provider: nothing is asked.
    app.fork.lyrics_ai.keys = Some(crate::fork::lyrics_ai::KeysPresent::default());
    app.request_lyrics_extras();
    assert_eq!(app.fork.lyrics_ai.id, None);

    app.fork.lyrics_ai.keys = Some(crate::fork::lyrics_ai::KeysPresent {
        gemini: true,
        openai: false,
        lastfm: false,
    });
    app.request_lyrics_extras();
    let id = app
        .fork
        .lyrics_ai
        .id
        .clone()
        .expect("asked once the key is there");
    app.request_lyrics_extras();
    assert_eq!(app.fork.lyrics_ai.id.as_deref(), Some(id.as_str()));

    let extras = crate::fork::lyrics_ai::Extras {
        lines: vec![
            crate::fork::lyrics_ai::Extra {
                pronunciation: Some("yume naraba dorehodo yokatta deshou".into()),
                translation: Some("If only it were a dream".into()),
                words: None,
            },
            crate::fork::lyrics_ai::Extra::default(),
        ],
    };
    app.handle_backend_events(vec![Event::Fork(crate::fork::Event::LyricsExtras {
        id: "stale".into(),
        result: Ok(extras.clone()),
    })]);
    assert_eq!(app.fork.lyrics_ai.extras, None);

    app.lyrics_line_shown = Some(Some(0));
    app.handle_backend_events(vec![Event::Fork(crate::fork::Event::LyricsExtras {
        id: id.clone(),
        result: Ok(extras),
    })]);
    assert_eq!(app.lyrics_line_shown, None, "the sung line is placed again");
    let settings = app.settings.fork.lyrics_ai.clone();
    assert_eq!(
        app.fork.lyrics_ai.line(Some(&uri), 0, &settings),
        (
            Some("yume naraba dorehodo yokatta deshou"),
            Some("If only it were a dream")
        )
    );
    assert_eq!(
        app.fork.lyrics_ai.line(Some(&uri), 1, &settings),
        (None, None)
    );
    assert_eq!(
        app.fork
            .lyrics_ai
            .line(Some("spotify:track:other"), 0, &settings),
        (None, None)
    );

    let mut off = settings.clone();
    off.translate = false;
    assert_eq!(
        app.fork.lyrics_ai.line(Some(&uri), 0, &off),
        (Some("yume naraba dorehodo yokatta deshou"), None)
    );

    // A failure is shown and asked again only on request.
    app.fork.lyrics_ai.id = Some("failing".into());
    app.handle_backend_events(vec![Event::Fork(crate::fork::Event::LyricsExtras {
        id: "failing".into(),
        result: Err(crate::fork::lyrics_ai::Error::Failed("quota".into())),
    })]);
    assert_eq!(app.fork.lyrics_ai.error.as_deref(), Some("quota"));
    assert!(app.fork.lyrics_ai.extras.is_some(), "earlier lines stay");
    app.fork.lyrics_ai.retry();
    assert_eq!(app.fork.lyrics_ai.error, None);
    assert_eq!(app.fork.lyrics_ai.id, None);
}
