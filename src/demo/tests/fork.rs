//! Fork: tests of the fork's views in the demo.

use super::*;

/// Collapsed, the Library is a strip of covers named for screen readers
/// and on hover, with the way back; open, its icon collapses it (#298).
#[test]
fn the_collapsed_library_is_a_strip_of_named_covers() {
    let frame = |app: &mut App, ctx: &egui::Context| {
        let mut last = None;
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| app.frame_ui(ui),
            );
            output.textures_delta.clear();
            last = Some(output);
        }
        last.unwrap().platform_output.accesskit_update.unwrap()
    };
    let bounds = |tree: &egui::accesskit::TreeUpdate, label: &str| {
        tree.nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .and_then(|(_, node)| node.bounds())
    };

    let (ctx, mut app) = accessible_app("library-collapsed");
    app.settings.fork.sidebar_collapsed = true;
    let tree = frame(&mut app, &ctx);
    assert!(bounds(&tree, "Expand Your Library").is_some());
    let liked = bounds(&tree, "Liked Songs").expect("the Liked Songs cover");
    assert!(
        liked.x1 <= f64::from(crate::ui::sidebar::fork::COLLAPSED_WIDTH),
        "the cover stays in the strip, ending at {}",
        liked.x1
    );
    assert!(bounds(&tree, "Search Your Library").is_none());
    app.backend.shutdown();

    let (ctx, mut app) = accessible_app("library-open");
    let tree = frame(&mut app, &ctx);
    assert!(bounds(&tree, "Collapse Your Library").is_some());
    assert!(bounds(&tree, "Expand Your Library").is_none());
    app.backend.shutdown();
}

/// The player bar shows up to three of the playing artists' genres,
/// capitalised, under the artist names; none when Spotify has none or
/// the setting is off.
#[test]
fn the_player_bar_shows_the_playing_artists_genres() {
    use crate::fork::ArtistGenres;
    use crate::player::{LocalState, LocalTrack, Playback};

    let (ctx, mut app) = accessible_app("player-bar-genres");
    app.local = LocalState {
        playback: Playback::Playing,
        track: Some(LocalTrack {
            uri: "spotify:track:genres".into(),
            title: "Song".into(),
            artists: vec![
                ArtistRef {
                    id: Some("lead".into()),
                    name: "Lead".into(),
                    uri: Some("spotify:artist:lead".into()),
                },
                ArtistRef {
                    id: Some("guest".into()),
                    name: "Guest".into(),
                    uri: Some("spotify:artist:guest".into()),
                },
            ],
            duration_ms: 200_000,
            ..LocalTrack::default()
        }),
        ..LocalState::default()
    };
    app.fork.artist_genres.insert(
        "lead".into(),
        ArtistGenres::Known(vec!["pop".into(), "latin pop".into()]),
    );
    app.fork.artist_genres.insert(
        "guest".into(),
        ArtistGenres::Known(vec![
            "Latin Pop".into(),
            "reggaeton".into(),
            "urbano latino".into(),
        ]),
    );
    let view = crate::ui::player_bar::show;
    let shown = |ctx: &egui::Context, app: &mut App| {
        view_frame(ctx, app, vec![], view);
        view_frame(ctx, app, vec![], view)
    };
    let text = shown(&ctx, &mut app);
    let line = text
        .iter()
        .find(|(label, _)| label == "Pop, Latin Pop, Reggaeton")
        .unwrap_or_else(|| panic!("{text:?}"))
        .1;
    let artist = text.iter().find(|(label, _)| label == "Lead").unwrap().1;
    assert!(line.top() >= artist.bottom() - 1.0, "{line:?} {artist:?}");

    // Spotify returns no genres for many artists: no line at all.
    app.fork
        .artist_genres
        .insert("lead".into(), ArtistGenres::Known(Vec::new()));
    app.fork
        .artist_genres
        .insert("guest".into(), ArtistGenres::Loading);
    let text = shown(&ctx, &mut app);
    assert!(
        !text
            .iter()
            .any(|(label, _)| label.contains("Pop") || label.contains("Reggaeton")),
        "{text:?}"
    );

    app.fork.artist_genres.insert(
        "guest".into(),
        ArtistGenres::Known(vec!["reggaeton".into()]),
    );
    assert!(
        shown(&ctx, &mut app)
            .iter()
            .any(|(label, _)| label == "Reggaeton")
    );
    app.settings.fork.player_bar_genres = false;
    assert!(
        !shown(&ctx, &mut app)
            .iter()
            .any(|(label, _)| label == "Reggaeton")
    );
    app.backend.shutdown();
}

/// The waveform seek bar draws a bar for each few points of the song,
/// fewer while only part has been heard, and seeks as the plain bar does.
#[cfg(feature = "demo")]
#[test]
fn the_waveform_seek_bar_draws_the_song_and_seeks() {
    let (ctx, mut app) = accessible_app("seek-waveform");
    let draw = |app: &mut App, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                events,
                ..Default::default()
            },
            |ui| crate::ui::player_bar::show(app, ui),
        );
        output.textures_delta.clear();
        output
    };
    // The bars are one untextured mesh of a rectangle each; the bars
    // still unheard sit at the floor, as tall as each other.
    let bar_heights = |output: &egui::FullOutput| -> Vec<f32> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh)
                    if mesh.texture_id == egui::TextureId::default()
                        && mesh.vertices.len() >= 400 =>
                {
                    Some(
                        mesh.vertices
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|quad| quad[2].pos.y - quad[0].pos.y)
                            .collect(),
                    )
                }
                _ => None,
            })
            .next()
            .unwrap_or_default()
    };

    // #given a song heard before
    apply_flags(&mut app, None, Some("seek-waveform"));
    draw(&mut app, vec![]);
    let whole = bar_heights(&draw(&mut app, vec![]));
    assert!(whole.len() > 100, "{} bars", whole.len());
    let quietest = whole.iter().copied().fold(f32::MAX, f32::min);
    let tallest = whole.iter().copied().fold(0.0, f32::max);
    assert!(tallest > quietest * 1.5, "the loud parts stand out");
    // The floor every unheard step is drawn at.
    let floor = tallest * crate::fork::seekwave::FLOOR + 0.01;
    assert!(whole.iter().all(|&height| height > floor), "{whole:?}");

    // #when only its first part has been heard
    apply_flags(&mut app, None, Some("seek-waveform-filling"));
    let filling = bar_heights(&draw(&mut app, vec![]));
    assert_eq!(filling.len(), whole.len());
    let ahead = filling[filling.len() * 3 / 4];
    assert!(ahead <= floor, "unheard is flat: {ahead}");
    assert!(filling[filling.len() / 3] > floor * 2.0, "heard is drawn");

    // #when its middle is clicked
    app.actions.clear();
    let middle = egui::pos2(640.0, 800.0 - crate::theme::PLAYER_BAR_HEIGHT / 2.0 + 23.0);
    draw(
        &mut app,
        pointer_click(middle, egui::PointerButton::Primary),
    );
    let duration = app.local.track.as_ref().unwrap().duration_ms;
    let seek = app.actions.iter().find_map(|action| match action {
        Action::Seek(position) => Some(*position),
        _ => None,
    });
    let seek = seek.expect("a click seeks");
    assert!(
        seek.abs_diff(duration / 2) < duration / 50,
        "{seek} of {duration}"
    );

    // #when the setting is off the plain bar is back
    app.settings.fork.seek_waveform = false;
    assert!(bar_heights(&draw(&mut app, vec![])).is_empty());
    app.backend.shutdown();
}
