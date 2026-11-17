//! The environment read as a configuration layer.
//!
//! This is a *layer*, not a set of per-command overrides: it parses into the same
//! shape a document parses into and is appended after the documents, so a setting
//! reached this way flows through every verb without any verb knowing it exists.

use crate::Environment;

use super::layer::{Layer, Origin, Setting, SettingPath, value_from_text};
use super::{ConfigError, SECRETS_FILE_VARIABLE};

/// What every configuration variable's name begins with.
pub const ENVIRONMENT_PREFIX: &str = "ONETASKGRAPH_";

/// Variables that begin with [`ENVIRONMENT_PREFIX`] and are *not* settings.
///
/// `ONETASKGRAPH_SECRETS_FILE` points at the credentials file, which is read before
/// sources are resolved and is nowhere in the configuration document. Without this
/// list it would decode to a setting called `secrets_file` and be refused as an
/// unknown field — turning a documented variable into an error.
///
/// [`BIN_VARIABLE`] is the same case from outside: onepipeline names the plan-store binary
/// it drives in `ONETASKGRAPH_BIN` and passes its environment on to that binary, where it
/// would decode to a setting called `bin` and refuse every command. It says which binary
/// to run — settled before this binary starts — so no command's answer depends on it.
///
/// [`HOST_VARIABLE`] names the host this process runs on, which a source's
/// `task_projection` is compared with; it is a fact about the machine rather than a setting.
const RESERVED: &[&str] = &[SECRETS_FILE_VARIABLE, BIN_VARIABLE, HOST_VARIABLE];

/// The variable naming the host this process runs on, which is not a setting.
pub const HOST_VARIABLE: &str = "ONETASKGRAPH_HOST";

/// The variable a caller names this binary's own path in, which is not a setting.
const BIN_VARIABLE: &str = "ONETASKGRAPH_BIN";

/// A whole namespace under [`ENVIRONMENT_PREFIX`] that holds no settings at all.
///
/// `onetaskgraph-live` spells this repository's own live-test variables here:
/// `ONETASKGRAPH_LIVE_REQUIRED` says a live session is expected rather than optional,
/// and `ONETASKGRAPH_LIVE_SEAT_DIR` says where that session's seat goes. Neither is a
/// setting, and `.github/workflows/ci.yml` exports the first of them on the very step
/// that runs the SDK generator and the journeys — so without this reservation every
/// invocation of the binary on that step is refused for an unknown field called
/// `live_required`. That is the failure [`RESERVED`] exists to prevent, one namespace
/// wider.
///
/// A namespace rather than those two names, because the reservation has to hold for the
/// next variable that lane adds; `scripts/check-live-lane.sh` fails when a live variable
/// falls outside it, so the two cannot drift apart.
const RESERVED_NAMESPACE: &str = "ONETASKGRAPH_LIVE_";

/// The separator between path segments in a variable name.
const SEGMENT_SEPARATOR: &str = "__";

/// Whether a prefixed variable names something this layer must leave alone.
fn reserved(variable: &str) -> bool {
    RESERVED.contains(&variable) || variable.starts_with(RESERVED_NAMESPACE)
}

/// Every `ONETASKGRAPH_`-prefixed variable, as one configuration layer.
///
/// The rule, and its inverse. A variable's name is [`ENVIRONMENT_PREFIX`] followed by
/// the setting's path, each segment upper-cased with `-` replaced by `_`, segments
/// joined by [`SEGMENT_SEPARATOR`]. Decoding lower-cases each segment, and turns `_`
/// back into `-` in exactly one position: the segment naming a source, immediately
/// after `sources`. That is the only place a `-` can occur, because a
/// [`SourceName`](onetaskgraph_plugin_api::SourceName) may not contain `_` while
/// every other key in the document is `snake_case` — which is what makes the forward
/// mapping injective and this inverse exact rather than a guess.
///
/// # Errors
///
/// Returns [`ConfigError::Setting`] when a variable's name decodes to no path at all,
/// as `ONETASKGRAPH_` and `ONETASKGRAPH_SOURCES__` do, and when one of these variables
/// holds a value that is not valid Unicode — a setting this build cannot read is
/// refused by name rather than quietly left unset. A variable in [`RESERVED`] or under
/// [`RESERVED_NAMESPACE`] is neither, on either count: it is not a setting, so this
/// layer does not decode it and does not refuse it for a value it never reads.
pub fn layer(environment: &Environment) -> Result<Layer, ConfigError> {
    for variable in environment.unusable() {
        if variable.starts_with(ENVIRONMENT_PREFIX) && !reserved(variable) {
            return Err(ConfigError::setting(
                variable,
                "this variable's value is not valid Unicode, so it cannot be read as a \
                 setting",
                "export it again with a value this shell and this process agree on, or \
                 unset it and set the setting in a configuration document instead.",
            ));
        }
    }

    let mut settings = Vec::new();
    for (variable, raw) in environment.iter() {
        let Some(encoded) = variable.strip_prefix(ENVIRONMENT_PREFIX) else {
            continue;
        };
        if reserved(variable) {
            continue;
        }
        settings.push(Setting {
            key: path_from(encoded, variable)?,
            value: value_from_text(raw),
            origin: Origin::Environment {
                variable: variable.to_owned(),
            },
        });
    }
    Ok(Layer::new(settings))
}

/// Decode one variable name's suffix into the setting path it addresses.
fn path_from(encoded: &str, variable: &str) -> Result<SettingPath, ConfigError> {
    let segments: Vec<String> = encoded
        .split(SEGMENT_SEPARATOR)
        .enumerate()
        .map(|(index, segment)| decode_segment(segment, index, encoded))
        .collect();
    SettingPath::new(segments, variable)
}

/// One segment, lower-cased, with `_` restored to `-` where a source name sits.
fn decode_segment(segment: &str, index: usize, encoded: &str) -> String {
    let lowered = segment.to_ascii_lowercase();
    if index == 1 && encoded.starts_with("SOURCES__") {
        lowered.replace('_', "-")
    } else {
        lowered
    }
}

/// The variable that sets `key`, for a message that tells a user what to export.
#[must_use]
pub fn variable_for(key: &SettingPath) -> String {
    let segments: Vec<String> = key
        .segments()
        .iter()
        .map(|segment| segment.to_ascii_uppercase().replace('-', "_"))
        .collect();
    format!("{ENVIRONMENT_PREFIX}{}", segments.join(SEGMENT_SEPARATOR))
}
