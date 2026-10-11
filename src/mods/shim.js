// A Spicetify-shaped API over the Spotifast bridge (fork only).
//
// The app sends the playing item through window.__spotifast.receive(); the
// page sends commands with window.ipc.postMessage(). Only what mods use is
// here; grow it per mod, keeping Spicetify's names and shapes.
(() => {
  if (window.Spicetify) return;
  const listeners = {};
  let state = null;
  let receivedAt = 0;

  const send = (message) => window.ipc.postMessage(JSON.stringify(message));
  const emit = (type, data) => {
    for (const listener of listeners[type] || []) {
      try {
        listener({ type, data });
      } catch (error) {
        console.error(error);
      }
    }
  };
  const progress = () => {
    if (!state) return 0;
    const ran = state.playing ? performance.now() - receivedAt : 0;
    return Math.min(state.duration_ms, Math.round(state.position_ms + ran));
  };
  const images = (url) => (url ? [{ url, label: "large" }, { url, label: "xlarge" }] : []);
  const playerData = (now) => {
    if (!now) return null;
    const artists = now.artists.map((a) => ({ name: a.name, uri: a.uri || "" }));
    return {
      item: {
        uri: now.uri,
        name: now.title,
        type: now.uri.split(":")[1] || "track",
        duration: { milliseconds: now.duration_ms },
        artists,
        album: { name: now.album, uri: now.album_uri || "", images: images(now.art_url) },
        images: images(now.art_url),
        metadata: {
          title: now.title,
          artist_name: artists.map((a) => a.name).join(", "),
          artist_uri: artists[0]?.uri || "",
          album_title: now.album,
          album_uri: now.album_uri || "",
          image_url: now.art_url || "",
          image_large_url: now.art_url || "",
          image_xlarge_url: now.art_url || "",
          duration: String(now.duration_ms),
        },
      },
      isPaused: !now.playing,
      isBuffering: false,
      duration: now.duration_ms,
      position: now.position_ms,
      positionAsOfTimestamp: now.position_ms,
      timestamp: Date.now(),
      shuffle: now.shuffle,
      repeat: { off: 0, context: 1, track: 2 }[now.repeat] ?? 0,
    };
  };

  const Player = {
    data: null,
    addEventListener(type, listener) {
      (listeners[type] ||= []).push(listener);
    },
    removeEventListener(type, listener) {
      listeners[type] = (listeners[type] || []).filter((l) => l !== listener);
    },
    getProgress: progress,
    getProgressPercent: () => (state?.duration_ms ? progress() / state.duration_ms : 0),
    getDuration: () => state?.duration_ms ?? 0,
    isPlaying: () => !!state?.playing,
    togglePlay: () => send({ cmd: "toggle_play" }),
    play: () => state && !state.playing && send({ cmd: "toggle_play" }),
    pause: () => state?.playing && send({ cmd: "toggle_play" }),
    next: () => send({ cmd: "next" }),
    back: () => send({ cmd: "previous" }),
    // Spicetify takes milliseconds, or a fraction of the track up to 1.
    seek(position) {
      const ms = position <= 1 ? position * (state?.duration_ms ?? 0) : position;
      send({ cmd: "seek", ms: Math.max(0, Math.round(ms)) });
    },
    getVolume: () => (state?.volume_percent ?? 100) / 100,
    setVolume: (volume) =>
      send({ cmd: "volume", percent: Math.round(Math.min(1, Math.max(0, volume)) * 100) }),
    getShuffle: () => !!state?.shuffle,
    setShuffle: (on) => send({ cmd: "shuffle", on: !!on }),
    toggleShuffle: () => send({ cmd: "shuffle", on: !state?.shuffle }),
    getRepeat: () => Player.data?.repeat ?? 0,
  };

  const LocalStorage = {
    get: (key) => localStorage.getItem(key),
    set: (key, value) => localStorage.setItem(key, value),
    remove: (key) => localStorage.removeItem(key),
    clear: () => localStorage.clear(),
  };

  window.Spicetify = { Player, LocalStorage };

  window.__spotifast = {
    receive(message) {
      const before = state;
      state = message.state;
      receivedAt = performance.now();
      Player.data = playerData(state);
      if (before?.uri !== state?.uri) emit("songchange", Player.data);
      if (before?.playing !== state?.playing) emit("onplaypause", Player.data);
    },
  };

  setInterval(() => state?.playing && emit("onprogress", progress()), 100);
  const ready = () => send({ cmd: "ready" });
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", ready);
  } else {
    ready();
  }
})();
