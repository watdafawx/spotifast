//! Self-update from GitHub releases, through fastframe-update.
//!
//! The previous release's helper installs the first release built on the
//! crate, so what it relies on stays the same: `--version` prints
//! `<command> <version>`, the new app accepts `--update-receipt` and
//! `--update-error` (stripped by [`fastframe_update::intercept`] before the
//! argument parser), and the handoff and receipt files keep their format.

pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig};

/// Fork: builds with mods update from the fork's releases, so an update
/// never swaps them for an upstream build without mods.
#[cfg(feature = "mods")]
const REPOSITORY: &str = "watdafawx/spotifast";
#[cfg(not(feature = "mods"))]
const REPOSITORY: &str = "crmne/spotifast";

/// Fork: the fork's releases are signed with the fork's own key (Release
/// mods workflow, `MODS_UPDATE_SIGNING_KEY` secret), not upstream's.
#[cfg(feature = "mods")]
const PUBLISHER_KEY: &str = include_str!("../assets/update-public-key-mods.hex");
#[cfg(not(feature = "mods"))]
const PUBLISHER_KEY: &str = include_str!("../assets/update-public-key.hex");

pub const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["rocks.spotifast.Spotifast"],
        executable_names: &["Spotifast"],
        legacy_bundle_names: &[],
    },
    // Releases publish an Ed25519 signature for checksums.txt. Require it
    // before downloading an update, using the release workflow's public key.
    publisher_key: Some(PUBLISHER_KEY),
    // Releases build their AppImages before publishing, so the signed
    // checksums.txt lists them (#737): an AppImage replaces itself with the
    // next one.
    appimage: true,
    ..UpdateConfig::new(
        REPOSITORY,
        "Spotifast",
        "spotifast",
        env!("CARGO_PKG_VERSION"),
    )
};

/// An updater on Spotifast's HTTP client, through the configured proxy.
pub fn updater(proxy: &crate::settings::ProxyConfig) -> anyhow::Result<Updater> {
    let builder = crate::http::blocking_builder(proxy).map_err(anyhow::Error::msg)?;
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
    }

    /// Fork: a mods build must check the fork's signature, or every fork
    /// release would be refused.
    #[cfg(feature = "mods")]
    #[test]
    fn mods_builds_trust_the_forks_signing_key() {
        assert_eq!(REPOSITORY, "watdafawx/spotifast");
        assert_eq!(
            CONFIG.publisher_key,
            Some(include_str!("../assets/update-public-key-mods.hex"))
        );
    }

    #[test]
    fn updates_require_the_release_signing_key() {
        assert!(
            CONFIG.publisher_key.is_some(),
            "updates must require a publisher signature"
        );
        CONFIG.validate().unwrap();
    }

    /// An AppImage may replace itself only because the release workflow
    /// builds the AppImages into the signed checksums.txt;
    /// packaging/test-release-names.py checks that side.
    #[test]
    fn appimages_update_themselves() {
        const { assert!(CONFIG.appimage) };
    }
}
