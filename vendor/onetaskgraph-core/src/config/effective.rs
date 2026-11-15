//! The configuration as it ended up, with the layer each setting came from.
//!
//! This is what makes precedence something a user can see rather than something the
//! tests alone know. It is built from the merge itself, not reconstructed from the
//! final [`Config`], so it cannot claim a layer the merge did not actually take the
//! value from.

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use onetaskgraph_plugin_api::SourceName;

use crate::secrets::SecretsReport;

use super::Config;
use super::layer::{Merged, Origin, Setting, SettingPath};

/// Every setting this build reads, with its value and where the value came from.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EffectiveConfig {
    /// Every setting, in key order.
    pub settings: Vec<Setting>,
    /// What the credentials file supplied, by name, and which layer answers for each.
    ///
    /// Beside the settings because it is the same question — where does this value
    /// come from — asked of the one kind of value that may never be printed. So the
    /// name and the layer are reported and the value never is.
    pub secrets: SecretsReport,
    /// Every source some layer refines but no layer gives a `plugin`, in name order.
    ///
    /// Its settings are among [`settings`](Self::settings) with the layer each came from;
    /// this says the source they belong to is incomplete, and so not configured here. Left
    /// out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub incomplete_sources: Vec<SourceName>,
}

impl EffectiveConfig {
    /// Combine what the layers set with the built-in values for what they did not.
    ///
    /// A setting nothing set still appears, carrying [`Origin::Default`] and the value
    /// the run will actually use — the point of the verb is to answer "what is this
    /// command going to do", and a silent omission answers it wrongly.
    #[must_use]
    pub fn new(merged: &Merged, config: &Config, secrets: SecretsReport) -> Self {
        let mut settings: Vec<Setting> = merged.values().cloned().collect();

        for (key, value) in [
            ("page_size", Value::from(config.page_size().get())),
            (
                "output",
                serde_json::to_value(config.output()).expect("an output format renders as JSON"),
            ),
            ("interactive", Value::Bool(config.interactive())),
            (
                "default_sources",
                serde_json::to_value(config.selected_sources())
                    .expect("source names render as JSON"),
            ),
        ] {
            let key = SettingPath::parse(key).expect("a literal path with no empty segment");
            if !settings.iter().any(|setting| setting.key == key) {
                settings.push(Setting {
                    key,
                    value,
                    origin: Origin::Default,
                });
            }
        }

        settings.sort_by(|left, right| left.key.cmp(&right.key));
        Self {
            settings,
            secrets,
            incomplete_sources: config.incomplete().iter().cloned().collect(),
        }
    }

    /// The table a person reads, one setting per line.
    ///
    /// Values render as compact JSON rather than bare: this table's whole job is to
    /// answer what a setting *is*, and a bare `50` beside a bare `"50"` would hide the
    /// difference between a number a document set and a string an environment variable
    /// spelled.
    #[must_use]
    pub fn render_text(&self) -> String {
        let key_width = self
            .settings
            .iter()
            .map(|setting| setting.key.to_string().chars().count())
            .max()
            .unwrap_or(0);
        let values: Vec<String> = self
            .settings
            .iter()
            .map(|setting| render_value(&setting.value))
            .collect();
        // Capped: one long list — a whole in-memory fixture, say — would otherwise
        // push the layer column off the far side of a terminal for every other row.
        // A value wider than the cap simply runs on, and its layer follows it.
        let value_width = values
            .iter()
            .map(|value| value.chars().count())
            .filter(|width| *width <= VALUE_COLUMN_CAP)
            .max()
            .unwrap_or(0);

        let mut rendered = String::new();
        for (setting, value) in self.settings.iter().zip(values) {
            rendered.push_str(&format!(
                "{:key_width$}  {:value_width$}  {}\n",
                setting.key.to_string(),
                value,
                setting.origin
            ));
        }

        if !self.incomplete_sources.is_empty() {
            rendered.push_str(
                "\nincomplete sources  no layer declares a plugin, so these are not configured here\n",
            );
            for name in &self.incomplete_sources {
                rendered.push_str(&format!("  {name}  incomplete\n"));
            }
        }

        rendered.push_str(&match self.secrets.path.as_ref() {
            Some(path) => format!("\nsecrets file  {}\n", path.display()),
            None => "\nsecrets file  none — neither XDG_CONFIG_HOME nor HOME is set\n".to_owned(),
        });
        if self.secrets.variables.is_empty() {
            rendered.push_str("  (it defines no variables, or is not there)\n");
        }
        let name_width = self
            .secrets
            .variables
            .iter()
            .map(|credential| credential.variable.as_str().chars().count())
            .max()
            .unwrap_or(0);
        for credential in &self.secrets.variables {
            rendered.push_str(&format!(
                "  {:name_width$}  resolved from the {}\n",
                credential.variable, credential.resolved_from
            ));
        }
        rendered
    }
}

/// How wide the value column grows before a value is left to run on.
const VALUE_COLUMN_CAP: usize = 44;

/// One value as compact JSON.
fn render_value(value: &Value) -> String {
    serde_json::to_string(value).expect("a value that was deserialized from JSON re-renders")
}
