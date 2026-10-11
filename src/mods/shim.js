// A Spicetify-shaped API over the Spotifast bridge (fork only).
//
// The app sends the playing item through window.__spotifast.receive(); the
// page sends commands with window.ipc.postMessage(). Only what mods use is
// here, keeping Spicetify's names and shapes. Calls that need Spotify's own
// signed-in client go through the app (src/mods/proxy.rs) where it can
// answer them, and reject otherwise so mods take their fallbacks.
(() => {
  if (window.Spicetify) return;
  const listeners = {};
  let state = null;
  let receivedAt = 0;

  const send = (message) => window.ipc.postMessage(JSON.stringify(message));
  // A Spotify request made by the app with its own grants; the page never
  // holds a token. Answered by a {"reply": …} line with the same id.
  const pending = new Map();
  let nextId = 1;
  const request = (call) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(Object.assign(new Error("Spotifast did not answer"), { status: 504 }));
      }, 30000);
      pending.set(id, { resolve, reject, timer });
      send({ cmd: "request", id, call });
    });
  const settle = ({ id, ok, error, status }) => {
    const waiting = pending.get(id);
    if (!waiting) return;
    pending.delete(id);
    clearTimeout(waiting.timer);
    if (error === undefined) waiting.resolve(ok);
    else waiting.reject(Object.assign(new Error(error), { status }));
  };
  // A track's 128-bit gid, as hex, in Spotify's 22-character base62.
  const BASE62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
  const gidToBase62 = (hex) => {
    let n = BigInt(`0x${hex}`);
    let id = "";
    for (let i = 0; i < 22; i++) {
      id = BASE62[Number(n % 62n)] + id;
      n /= 62n;
    }
    return id;
  };
  // The Spotify URLs the app answers, for CosmosAsync and RequestBuilder
  // alike; anything else is not available.
  const spotifyCall = (url) => {
    const { hostname, pathname } = new URL(url);
    if (hostname === "spclient.wg.spotify.com") {
      const lyrics = pathname.match(/^\/color-lyrics\/v2\/track\/([A-Za-z0-9]+)/);
      if (lyrics) return { call: { kind: "lyrics", uri: `spotify:track:${lyrics[1]}` } };
      const metadata = pathname.match(/^\/metadata\/4\/track\/([0-9a-fA-F]{32})$/);
      if (metadata) {
        return { call: { kind: "isrc", uri: `spotify:track:${gidToBase62(metadata[1])}` }, shape: (isrc) => ({ isrc }) };
      }
    }
    if (hostname === "api.spotify.com" && pathname === "/v1/me") return { call: { kind: "me" } };
    throw Object.assign(new Error(`${hostname}${pathname} is not available in Spotifast`), { status: 501 });
  };
  const spotifyGet = async (url) => {
    const { call, shape } = spotifyCall(url);
    const answer = await request(call);
    return shape ? shape(answer) : answer;
  };
  const cosmosGet = spotifyGet;
  // Platform.RequestBuilder: build().withHost().withPath()….send(), which
  // answers with a status rather than throwing, as Spotify's does.
  const RequestBuilder = {
    build() {
      const target = { host: "", path: "", query: {} };
      const builder = {
        withHost: (host) => ((target.host = host), builder),
        withPath: (path) => ((target.path = path), builder),
        withEndpointIdentifier: () => builder,
        withQueryParameters: (query) => (Object.assign(target.query, query), builder),
        withoutMarket: () => builder,
        async send() {
          try {
            return { status: 200, body: await spotifyGet(target.host + target.path) };
          } catch (error) {
            return { status: error.status || 502, body: null };
          }
        },
      };
      return builder;
    },
  };
  // Album colours come from the cover colour the app sends with the state,
  // shaped like the GraphQL answers Spotify's client gives.
  const colorWaiters = new Set();
  const coverColor = (isCurrent) =>
    new Promise((resolve, reject) => {
      const fail = () => reject(Object.assign(new Error("No cover colour for that item"), { status: 404 }));
      if (!isCurrent) return fail();
      if (state.color) return resolve(state.color);
      const uri = state.uri;
      const done = () => {
        clearTimeout(timer);
        colorWaiters.delete(waiter);
      };
      const waiter = (now) => {
        if (now?.uri !== uri) {
          done();
          fail();
        } else if (now.color) {
          done();
          resolve(now.color);
        }
      };
      const timer = setTimeout(() => {
        done();
        fail();
      }, 5000);
      colorWaiters.add(waiter);
    });
  const rgba = (hex, scale) => {
    const n = Number.parseInt(hex.slice(1), 16);
    const channel = (shift) => (((n >> shift) & 255) / 255) * scale;
    return { red: channel(16), green: channel(8), blue: channel(0), alpha: 1 };
  };
  const GraphQL = {
    Definitions: {
      fetchExtractedColorForTrackEntity: { name: "fetchExtractedColorForTrackEntity" },
      getDynamicColorsByUris: { name: "getDynamicColorsByUris" },
    },
    async Request(definition, variables = {}) {
      switch (definition?.name) {
        case "fetchExtractedColorForTrackEntity": {
          const hex = await coverColor(variables.uri === state?.uri);
          const extractedColors = { colorDark: { hex }, colorLight: { hex }, colorRaw: { hex } };
          return { data: { trackUnion: { albumOfTrack: { coverArt: { extractedColors } } } } };
        }
        case "getDynamicColorsByUris": {
          const hex = await coverColor(variables.imageUris?.[0] === state?.art_url);
          // Darker steps for higher contrast, as ivLyrics' own fallback does.
          const at = (scale) => ({ backgroundBase: rgba(hex, scale) });
          return { data: { getDynamicColorsByUris: [{ minContrast: at(1), highContrast: at(0.72), higherContrast: at(0.48) }] } };
        }
      }
      throw Object.assign(new Error(`GraphQL ${definition?.name} is not available in Spotifast`), { status: 501 });
    },
  };
  const urisOf = (arg) => (Array.isArray(arg) ? arg : arg?.uris ?? [arg]).map(String);

  const unavailable = (what) => () => Promise.reject(new Error(`${what} is not available in Spotifast`));

  // Page errors go to the app's log, so a broken mod can be diagnosed.
  const log = (level, parts) => {
    const message = parts
      .map((part) => (part instanceof Error ? `${part.message}\n${part.stack}` : typeof part === "string" ? part : safeJson(part)))
      .join(" ")
      .slice(0, 2000);
    send({ cmd: "log", level, message });
  };
  const safeJson = (value) => {
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  };
  const consoleError = console.error.bind(console);
  console.error = (...parts) => {
    consoleError(...parts);
    log("error", parts);
  };
  addEventListener("error", (event) => log("error", [event.error || event.message, `at ${event.filename}:${event.lineno}`]));
  addEventListener("unhandledrejection", (event) => log("error", ["unhandled rejection:", event.reason]));

  // Player
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
    const artists = now.artists.map((a) => ({ name: a.name, uri: a.uri || "", type: "artist" }));
    return {
      item: {
        uri: now.uri,
        uid: now.uri,
        name: now.title,
        type: now.uri.split(":")[1] || "track",
        duration: { milliseconds: now.duration_ms },
        duration_ms: now.duration_ms,
        artists,
        album: { name: now.album, uri: now.album_uri || "", images: images(now.art_url) },
        images: images(now.art_url),
        isLocal: false,
        metadata: {
          title: now.title,
          artist_name: artists.map((a) => a.name).join(", "),
          artist_uri: artists[0]?.uri || "",
          album_title: now.album,
          album_uri: now.album_uri || "",
          image_url: now.art_url || "",
          image_small_url: now.art_url || "",
          image_large_url: now.art_url || "",
          image_xlarge_url: now.art_url || "",
          duration: String(now.duration_ms),
          duration_ms: String(now.duration_ms),
        },
      },
      context: { uri: "", url: "", metadata: {} },
      isPaused: !now.playing,
      isBuffering: false,
      duration: now.duration_ms,
      position: now.position_ms,
      positionAsOfTimestamp: now.position_ms,
      timestamp: Date.now(),
      shuffle: now.shuffle,
      repeat: { off: 0, context: 1, track: 2 }[now.repeat] ?? 0,
      volume: now.volume_percent / 100,
    };
  };

  const Player = {
    data: null,
    // Spotify's internal player; mods reach for its queue events.
    origin: { _events: { addListener() {}, removeListener() {} } },
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
    setRepeat: () => {},
    playUri: unavailable("Player.playUri"),
  };

  // Storage: Spicetify.LocalStorage is plain strings; Platform's is JSON.
  const LocalStorage = {
    get: (key) => localStorage.getItem(key),
    set: (key, value) => localStorage.setItem(key, value),
    remove: (key) => localStorage.removeItem(key),
    clear: () => localStorage.clear(),
  };
  const LocalStorageAPI = {
    namespace: "spotifast",
    getItem(key) {
      const value = localStorage.getItem(`${this.namespace}:${key}`);
      try {
        return value === null ? null : JSON.parse(value);
      } catch {
        return value;
      }
    },
    setItem(key, value) {
      localStorage.setItem(`${this.namespace}:${key}`, JSON.stringify(value));
    },
    clearItem(key) {
      localStorage.removeItem(`${this.namespace}:${key}`);
    },
  };

  // Navigation inside the mods window: one app, its own paths.
  const historyListeners = new Set();
  const History = {
    location: { pathname: "/", search: "", hash: "", state: {} },
    entries: [],
    push(to, pushState) {
      const next = typeof to === "string" ? { pathname: to } : to;
      History.entries.push(History.location);
      History.location = { pathname: "/", search: "", hash: "", state: pushState ?? {}, ...next };
      historyListeners.forEach((listener) => listener(History.location, "PUSH"));
    },
    replace(to) {
      History.location = { ...History.location, ...(typeof to === "string" ? { pathname: to } : to) };
      historyListeners.forEach((listener) => listener(History.location, "REPLACE"));
    },
    goBack() {
      if (!History.entries.length) return;
      History.location = History.entries.pop();
      historyListeners.forEach((listener) => listener(History.location, "POP"));
    },
    listen(listener) {
      historyListeners.add(listener);
      return () => historyListeners.delete(listener);
    },
  };

  // Where Spotify shows a mod's buttons (profile menu, playbar), the mods
  // window shows a toolbar instead; see the loader.
  const controls = new Set();
  const changed = () => window.dispatchEvent(new Event("spotifast:controls"));
  class Control {
    constructor(label, icon, onClick, disabled = false, active = false, registerOnCreate = true) {
      Object.assign(this, { label, icon, onClick, disabled, active });
      if (registerOnCreate === true) this.register();
    }
    register() {
      controls.add(this);
      changed();
    }
    deregister() {
      controls.delete(this);
      changed();
    }
    setState(state) {
      Object.assign(this, state);
      changed();
    }
  }
  class MenuItem extends Control {
    // Spicetify.Menu.Item(name, isEnabled, onClick, icon)
    constructor(name, isEnabled, onClick, icon) {
      super(name, icon, onClick, false, isEnabled, false);
    }
  }

  const URI = {
    fromString(uri) {
      const [, type = "", id = ""] = String(uri).split(":");
      return { type, id, toURI: () => uri, toString: () => uri };
    },
    isTrack: (uri) => String(uri).startsWith("spotify:track:"),
    isEpisode: (uri) => String(uri).startsWith("spotify:episode:"),
  };

  // Spicetify's icons are the inner markup of a 16×16 SVG.
  const SVGIcons = {
    play: '<path d="M3 1.7v12.6L14 8 3 1.7z"/>',
    pause: '<path d="M3 2h3v12H3zm7 0h3v12h-3z"/>',
    check: '<path d="M13.3 3.3 6 10.6 2.7 7.3 1.3 8.7 6 13.4l8.7-8.7z"/>',
    heart: '<path d="M8 14s-6-3.8-6-8a3.5 3.5 0 0 1 6-2.5A3.5 3.5 0 0 1 14 6c0 4.2-6 8-6 8z"/>',
    repeat: '<path d="M4 4h8v2l3-3-3-3v2H2v6h2zm8 8H4v-2l-3 3 3 3v-2h10V8h-2z"/>',
    shuffle: '<path d="M11 2v2H9.5L4 12H1v2h4l5.5-8H11v2l3-3zM1 4h4l1.5 2.2-1.2 1.7L4 6H1zm9 6.2 1 1.8h0v-2l3 3-3 3v-2H9.5l-1.2-1.7z"/>',
    search: '<path d="M7 1a6 6 0 1 0 3.7 10.7l3.6 3.6 1.4-1.4-3.6-3.6A6 6 0 0 0 7 1zm0 2a4 4 0 1 1 0 8 4 4 0 0 1 0-8z"/>',
  };

  window.Spicetify = {
    Player,
    LocalStorage,
    Platform: {
      History,
      LocalStorageAPI,
      ClipboardAPI: { copy: (text) => navigator.clipboard.writeText(String(text)) },
      Session: { username: "", accessToken: "" },
      PlaybackAPI: { _isLocal: false },
      PlayerAPI: {
        get _state() {
          return Player.data;
        },
      },
      UserAPI: { getUser: async () => ({ username: "", displayName: "" }) },
      // Liked Songs, as the app knows them; only the playing song is known here.
      LibraryAPI: {
        contains: async (...args) => urisOf(args.length > 1 ? args : args[0]).map((uri) => uri === state?.uri && state.saved === true),
        add: async (arg) => urisOf(arg).forEach((uri) => send({ cmd: "set_saved", uri, saved: true })),
        remove: async (arg) => urisOf(arg).forEach((uri) => send({ cmd: "set_saved", uri, saved: false })),
      },
      PlaylistAPI: { add: unavailable("PlaylistAPI.add") },
      RootlistAPI: {},
      RequestBuilder,
    },
    CosmosAsync: { get: cosmosGet, post: unavailable("CosmosAsync.post"), put: unavailable("CosmosAsync.put"), del: unavailable("CosmosAsync.del") },
    GraphQL,
    getAudioData: unavailable("getAudioData"),
    Locale: { getLocale: () => (navigator.language || "en").split("-")[0] },
    Config: { version: "2.40.0", current_theme: "", color_scheme: "", username: "" },
    Queue: { nextTracks: [], prevTracks: [] },
    URI,
    SVGIcons,
    Menu: { Item: MenuItem },
    Playbar: { Button: Control, Widget: Control },
    // The loader puts these libraries on window before any mod runs.
    get React() {
      return window.React;
    },
    get ReactDOM() {
      return window.ReactDOM;
    },
    get Mousetrap() {
      return window.Mousetrap;
    },
    _controls: controls,
  };

  window.__spotifast = {
    receive(message) {
      if (message.reply) {
        settle(message.reply);
        return;
      }
      const before = state;
      state = message.state;
      receivedAt = performance.now();
      Player.data = playerData(state);
      colorWaiters.forEach((waiter) => waiter(state));
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
