//! Spotify requests made for mods, on the backend's runtime (fork only).
//!
//! A page never sees a token: it names one of the calls below, the backend
//! makes it with the app's own grants, and only the answer goes back. The
//! list is deliberately short; each entry is something a mod needs that the
//! app can already ask Spotify for.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};

use crate::api::gateway::{ApiGateway, Operation};
use crate::player::Engine;

/// A Spotify request a page may make.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Call {
    /// Spotify's own lyrics for a track, the JSON its clients read from
    /// `color-lyrics`. Needs local playback signed in.
    Lyrics { uri: String },
    /// The signed-in account's profile, as `GET /v1/me` returns it.
    Me,
}

pub struct Request {
    pub id: u64,
    pub call: Call,
    pub reply: Sender<Reply>,
    pub wake: egui::Context,
}

#[derive(Debug, Serialize)]
pub struct Reply {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// An HTTP-like status for a failure, so pages can tell "none" (404)
    /// from "not signed in" (401) from anything else (502).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

pub async fn serve(request: Request, engine: Option<Arc<Engine>>, api: Arc<ApiGateway>) {
    log::info!("Mods: {:?}", request.call);
    let result = match request.call {
        Call::Lyrics { uri } => lyrics(engine, &uri).await,
        Call::Me => me(&api).await,
    };
    let (ok, error, status) = match result {
        Ok(value) => (Some(value), None, None),
        Err((status, error)) => {
            log::info!("Mods: answered {status}: {error}");
            (None, Some(error), Some(status))
        }
    };
    let _ = request.reply.send(Reply {
        id: request.id,
        ok,
        error,
        status,
    });
    request.wake.request_repaint();
}

type Answer = Result<serde_json::Value, (u16, String)>;

async fn lyrics(engine: Option<Arc<Engine>>, uri: &str) -> Answer {
    if !uri.starts_with("spotify:track:") {
        return Err((400, "not a track".into()));
    }
    let engine = engine.ok_or((401, "local playback is not signed in".to_string()))?;
    match engine.lyrics_json(uri).await {
        Ok(Some(json)) => Ok(json),
        Ok(None) => Err((404, "Spotify has no lyrics for this track".into())),
        Err(error) => Err((502, format!("{error:#}"))),
    }
}

async fn me(api: &ApiGateway) -> Answer {
    let client = api
        .client_for(Operation::CanonicalAccount)
        .await
        .map_err(|error| (401, error.to_string()))?;
    let user = client
        .me()
        .await
        .map_err(|error| (502, error.to_string()))?;
    serde_json::to_value(user).map_err(|error| (502, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::NetActivity;

    /// Signed out, every call fails fast with a status the page can act on,
    /// instead of waiting for a grant.
    #[tokio::test]
    async fn signed_out_calls_answer_with_a_status() {
        let api = Arc::new(ApiGateway::new(
            reqwest::Client::new(),
            Arc::new(NetActivity::default()),
        ));
        let (reply, replies) = std::sync::mpsc::channel();
        for (id, call) in [
            (
                1,
                Call::Lyrics {
                    uri: "spotify:track:0VjIjW4GlUZAMYd2vXMi3b".into(),
                },
            ),
            (
                2,
                Call::Lyrics {
                    uri: "spotify:album:x".into(),
                },
            ),
            (3, Call::Me),
        ] {
            let request = Request {
                id,
                call,
                reply: reply.clone(),
                wake: egui::Context::default(),
            };
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                serve(request, None, api.clone()),
            )
            .await
            .expect("a signed-out call must not wait");
        }
        let statuses: Vec<_> = replies
            .try_iter()
            .map(|reply| (reply.id, reply.status))
            .collect();
        assert_eq!(statuses, [(1, Some(401)), (2, Some(400)), (3, Some(401))]);
    }
}
