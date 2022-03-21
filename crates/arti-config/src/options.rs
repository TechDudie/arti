//! Handling for arti's configuration formats.

use derive_builder::Builder;
use serde::Deserialize;
use tor_config::ConfigBuildError;

/// Default options to use for our configuration.
//
// TODO should this be in `arti::cfg` ?
pub const ARTI_DEFAULTS: &str = concat!(include_str!("./arti_defaults.toml"),);

/// Structure to hold our application configuration options
#[derive(Deserialize, Debug, Default, Clone, Builder, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[builder(build_fn(error = "ConfigBuildError"))]
#[builder(derive(Deserialize))]
pub struct ApplicationConfig {
    /// If true, we should watch our configuration files for changes, and reload
    /// our configuration when they change.
    ///
    /// Note that this feature may behave in unexpected ways if the path to the
    /// directory holding our configuration files changes its identity (because
    /// an intermediate symlink is changed, because the directory is removed and
    /// recreated, or for some other reason).
    #[serde(default)]
    #[builder(default)]
    watch_configuration: bool,
}

impl ApplicationConfig {
    /// Return true if we're configured to watch for configuration changes.
    pub fn watch_configuration(&self) -> bool {
        self.watch_configuration
    }
}
