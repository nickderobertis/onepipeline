//! The compile-time registry of plugin kinds.
//!
//! Every plugin this build compiled is named here. The two that reach a network —
//! `github-projects` and `linear` — are each behind a cargo feature of this crate that no
//! default enables, because a host that wants only a local store must not compile an HTTP
//! and TLS stack it never runs. A kind this crate could register but this build left out is
//! still known by name, so a configuration naming it is refused with the feature that
//! enables it rather than as a plugin nobody has heard of.

use std::{fmt, str::FromStr};

use onetaskgraph_plugin_api::SourcePlugin;
use serde::{Deserialize, Serialize};

/// One of the plugin kinds this build has.
///
/// A [`SourceConfig`](crate::SourceConfig) holds one of these rather than the string a
/// document spelled, so a configuration naming a plugin nothing answers to cannot exist
/// past [`Config::from_document`](crate::Config::from_document). Resolution therefore has
/// no "what if the registry does not have it" branch left to get wrong, and the refusal
/// happens at the one place that can name the offending key.
///
/// Non-exhaustive because which variants exist is a matter of this crate's features, and
/// cargo unifies features across a build: a match in another crate that is exhaustive
/// without `linear` would stop compiling the moment anything else in the graph enabled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
#[non_exhaustive]
pub enum PluginKind {
    /// GitHub Projects.
    #[cfg(feature = "github-projects")]
    GithubProjects,
    /// The in-memory source the journeys are written against.
    InMemory,
    /// Linear.
    #[cfg(feature = "linear")]
    Linear,
    /// A folder of Markdown files.
    LocalMd,
    /// A program of its own, speaking `docs/plugin-protocol.md` over stdio.
    Subprocess,
}

impl PluginKind {
    /// Every kind this build compiled, in the stable order [`registry`] reports them in.
    pub const ALL: [Self; KIND_COUNT] = [
        #[cfg(feature = "github-projects")]
        Self::GithubProjects,
        Self::InMemory,
        #[cfg(feature = "linear")]
        Self::Linear,
        Self::LocalMd,
        Self::Subprocess,
    ];

    /// The name a configuration document's `plugin:` field names this kind by.
    ///
    /// Spelled here rather than read from the plugin so that matching a name costs no
    /// allocation. `every_plugin_kind_names_the_kind_its_own_plugin_reports` is what
    /// keeps the two from drifting.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "github-projects")]
            Self::GithubProjects => "github-projects",
            Self::InMemory => "in-memory",
            #[cfg(feature = "linear")]
            Self::Linear => "linear",
            Self::LocalMd => "local-md",
            Self::Subprocess => "subprocess",
        }
    }

    /// The kind called `name`, or `None` when nothing in this build answers to it.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// This kind's factory.
    ///
    /// Total, which is the point of the type: a `PluginKind` is one of the kinds this build
    /// compiled, so there is no absent-plugin case for a caller to handle or forget.
    #[must_use]
    pub fn plugin(self) -> Box<dyn SourcePlugin> {
        match self {
            #[cfg(feature = "github-projects")]
            Self::GithubProjects => Box::new(onetaskgraph_github_projects::Plugin),
            Self::InMemory => Box::new(onetaskgraph_in_memory::Plugin),
            #[cfg(feature = "linear")]
            Self::Linear => Box::new(onetaskgraph_linear::Plugin),
            Self::LocalMd => Box::new(onetaskgraph_local_md::Plugin),
            Self::Subprocess => Box::new(crate::subprocess::SubprocessPlugin),
        }
    }
}

impl TryFrom<String> for PluginKind {
    type Error = String;

    /// Read a kind this build has, and refuse one it does not — naming what it does have.
    ///
    /// Where a document or a protocol message carries a plugin kind, this is what keeps
    /// "a kind" and "a kind this binary can build" the same thing: an unknown name stops
    /// being representable at the field rather than at a lookup somewhere later.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| match omitted_feature(&value) {
            Some(feature) => format!(
                "the {value:?} plugin is not compiled into this build; enable the \
                 `{feature}` feature of onetaskgraph-core to register it"
            ),
            None => format!(
                "no plugin of this build is called {value:?}; it knows {}",
                plugin_kinds().join(", ")
            ),
        })
    }
}

impl From<PluginKind> for String {
    fn from(value: PluginKind) -> Self {
        value.as_str().to_owned()
    }
}

impl fmt::Display for PluginKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PluginKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}

/// Sizes [`PluginKind::ALL`] from the same `cfg`s that gate its entries, so that table
/// compiles under every feature combination rather than the one a literal length would fit.
const KIND_COUNT: usize =
    3 + cfg!(feature = "github-projects") as usize + cfg!(feature = "linear") as usize;

/// The kinds this crate can register that this build's features left out, each beside the
/// feature that compiles it.
///
/// Spelled as the kind's name rather than as a [`PluginKind`], because a kind that was not
/// compiled has no variant to name it by — which is what keeps [`PluginKind::plugin`] total.
const OMITTED: &[(&str, &str)] = &[
    #[cfg(not(feature = "github-projects"))]
    ("github-projects", "github-projects"),
    #[cfg(not(feature = "linear"))]
    ("linear", "linear"),
];

/// The cargo feature of this crate that would compile the plugin called `kind`, when this
/// build left it out; `None` for a kind this build has or a name no feature answers to.
#[must_use]
pub(crate) fn omitted_feature(kind: &str) -> Option<&'static str> {
    OMITTED
        .iter()
        .find_map(|&(name, feature)| (name == kind).then_some(feature))
}

/// Every plugin kind this build knows, in a stable order.
#[must_use]
pub fn registry() -> Vec<Box<dyn SourcePlugin>> {
    PluginKind::ALL.map(PluginKind::plugin).into()
}

/// The kind names in [`registry`], for help text and error messages.
#[must_use]
pub fn plugin_kinds() -> Vec<&'static str> {
    registry().iter().map(|plugin| plugin.kind()).collect()
}

/// The plugin registered for `kind`, or `None` when nothing answers to that name.
#[must_use]
pub fn plugin_for(kind: &str) -> Option<Box<dyn SourcePlugin>> {
    PluginKind::parse(kind).map(PluginKind::plugin)
}
