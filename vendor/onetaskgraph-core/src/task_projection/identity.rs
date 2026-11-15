//! Validated producer identities serialize as the fixed v1 string fields.

// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] The repository keeps core domains as submodules of the core crate and its one Nx project, including config, cache and template. These projection-owned identities follow that same boundary; they are not shared routing vocabulary or a second producer contract.

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

macro_rules! identity {
    ($name:ident, $description:literal, $validate:ident) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validate an identity without changing its wire spelling.
            ///
            /// # Errors
            ///
            /// Refuses an identity that does not meet this type's invariants.
            pub fn new(value: impl Into<String>) -> Result<Self, String> {
                let value = value.into();
                $validate(&value)?;
                Ok(Self(value))
            }

            /// The validated wire spelling.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        // The published v1 schema spells these identities as strings. Validation belongs
        // at deserialization without changing that frozen consumer schema.
        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> std::borrow::Cow<'static, str> {
                String::schema_name()
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                String::json_schema(generator)
            }
        }
    };
}

identity!(
    ExternalItemId,
    "A producer's stable external idempotency identity.",
    nonempty
);
identity!(
    SessionId,
    "The identity of the producer session that owns an intent.",
    nonempty
);
identity!(
    RunId,
    "The identity of the producer run, when one owns the item.",
    nonempty
);
identity!(
    PlanId,
    "The identity of the producer plan, when one owns the item.",
    nonempty
);
identity!(
    ProjectionProjectId,
    "A resolved tracker project identity.",
    nonempty
);
identity!(
    ProjectedTaskId,
    "A validated source-qualified tracker task identity.",
    qualified_task
);
identity!(
    TrackerUrl,
    "An absolute HTTPS tracker URL suitable for a clickable card.",
    tracker_url
);

identity!(
    SourceIdentityDigest,
    "The cache's lowercase SHA-256 source identity digest.",
    source_digest
);

identity!(
    HostIdentity,
    "A validated local projection authority host identity.",
    host_identity
);
identity!(
    PayloadDigest,
    "The immutable projection payload and placement digest.",
    source_digest
);
identity!(
    ProjectionOrigin,
    "A stable human task origin naming its host and claim key.",
    projection_origin
);

fn host_identity(value: &str) -> Result<(), String> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        Ok(())
    } else {
        Err("a host identity uses ASCII letters, digits, dots, underscores and hyphens".to_owned())
    }
}

fn projection_origin(value: &str) -> Result<(), String> {
    let mut parts = value.split(':');
    if parts.next() != Some("human-task") {
        return Err("a projection origin starts with human-task".to_owned());
    }
    host_identity(
        parts
            .next()
            .ok_or("an origin requires its authoritative host")?,
    )?;
    source_digest(parts.next().ok_or("an origin requires its claim digest")?)?;
    if parts.next().is_some() {
        return Err("an origin has exactly a purpose, host and claim digest".to_owned());
    }
    Ok(())
}

fn source_digest(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("a source identity must be a lowercase SHA-256 digest".to_owned())
    }
}

identity!(
    ProjectionTitle,
    "A nonblank task title, preserved in its supplied wire spelling.",
    nonblank_title
);

fn nonblank_title(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err("a projected task title must be nonempty".to_owned())
    } else {
        Ok(())
    }
}

fn nonempty(value: &str) -> Result<(), String> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        Err(
            "an identity must be nonempty with no surrounding whitespace or control characters"
                .to_owned(),
        )
    } else {
        Ok(())
    }
}

fn qualified_task(value: &str) -> Result<(), String> {
    nonempty(value)?;
    value
        .parse::<crate::GlobalId>()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn tracker_url(value: &str) -> Result<(), String> {
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("a tracker URL cannot contain whitespace".to_owned());
    }
    let parsed = url::Url::parse(value).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https"
        || !parsed.has_host()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("a tracker URL must be absolute HTTPS without embedded credentials".to_owned());
    }
    Ok(())
}

impl ProjectedTaskId {
    /// The engine's validated qualified identity.
    #[must_use]
    pub fn global_id(&self) -> crate::GlobalId {
        self.as_str()
            .parse()
            .expect("the constructor validated the qualified identity")
    }
}

impl TryFrom<crate::GlobalId> for ProjectedTaskId {
    type Error = String;

    fn try_from(value: crate::GlobalId) -> Result<Self, Self::Error> {
        Self::new(value.to_string())
    }
}

impl ProjectionProjectId {
    /// The existing plugin API's native project identity.
    #[must_use]
    pub fn native_id(&self) -> onetaskgraph_plugin_api::NativeId {
        onetaskgraph_plugin_api::NativeId(self.as_str().to_owned())
    }
}
