//! The usage domain driven from *outside* the crate, the way a consumer reaches
//! it: `oneharness-core` is published so sibling tools (and this repo's own
//! `usage` probe, landing next) can depend on the engine, so its public API is a
//! real boundary — not an implementation detail.
//!
//! These tests therefore start from what a probe actually captures — a whole
//! JSONL stream, a whole JSON-RPC exchange, an HTTP response body — rather than
//! a pre-unwrapped payload, and they finish at the JSON a downstream consumer
//! reads. Anything a consumer needs that is not `pub`, and any envelope step a
//! consumer would have to reinvent, fails here.

use oneharness_core::domain::config::VariantName;
use oneharness_core::domain::usage::{
    claude_control_response, normalize_timestamp, parse_claude_get_usage, parse_codex_rate_limits,
    parse_copilot_user, parse_cursor_about, AuthMode, IdentitySelector, ParsedUsage, QuotaAmount,
    QuotaCounters, QuotaUnit, ResetCredit, ResetCreditStatus, ResetCredits, ResetCreditsUnknown,
    UnavailableReason, UnknownReason, UsageAvailability, UsageIdentity, UsageReport, UsageWindow,
    UsedPercent, UtcInstant, WindowDuration, WindowUsage, SCHEMA_VERSION,
};
use serde_json::Value;

/// What `claude -p --input-format stream-json --output-format stream-json`
/// writes to stdout when a probe sends one `get_usage` control request: the
/// payload arrives on one line among several, so a consumer must find it.
const CLAUDE_STREAM: &str = r#"{"type":"system","subtype":"init","apiKeySource":"none","claude_code_version":"2.1.220"}
{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"session":{"total_cost_usd":0,"total_api_duration_ms":0,"total_duration_ms":607,"total_lines_added":0,"total_lines_removed":0,"model_usage":{}},"subscription_type":"max","rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":42,"resets_at":"2026-07-29T18:30:00.123456+00:00","limit_dollars":null,"used_dollars":null,"remaining_dollars":null},"seven_day":{"utilization":61,"resets_at":"2026-08-02T09:00:00.000000-04:00","limit_dollars":null,"used_dollars":null,"remaining_dollars":null},"seven_day_opus":null,"tangelo":null,"amber_ladder":null,"limits":[{"kind":"session","group":"session","percent":42,"severity":"normal","resets_at":"2026-07-29T18:30:00.123456+00:00","scope":null,"is_active":false},{"kind":"weekly_all","group":"weekly","percent":61,"severity":"normal","resets_at":"2026-08-02T13:00:00.000000+00:00","scope":null,"is_active":true},{"kind":"weekly_scoped","group":"weekly","percent":17,"severity":"normal","resets_at":"2026-08-02T13:00:00.000000+00:00","scope":{"model":{"id":null,"display_name":"Opus 5"},"surface":null},"is_active":false}],"member_dashboard_available":true},"behaviors":null}}}"#;

/// What a probe reads back from `codex app-server --stdio`: the `initialize`
/// reply first, then the rate-limit reply, so a consumer must match on id.
const CODEX_EXCHANGE: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"userAgent":"codex-cli/0.145.0"}}
{"jsonrpc":"2.0","id":2,"result":{"rateLimits":{"limitId":"codex","limitName":null,"primary":{"usedPercent":31,"windowDurationMins":10080,"resetsAt":1785000000},"secondary":null,"credits":{"hasCredits":true,"unlimited":false,"balance":"12.34"},"individualLimit":null,"spendControlReached":false,"planType":"pro","rateLimitReachedType":null},"rateLimitsByLimitId":{"codex":{"limitId":"codex","limitName":null,"primary":{"usedPercent":31,"windowDurationMins":10080,"resetsAt":1785000000},"secondary":null,"credits":null,"individualLimit":null,"spendControlReached":null,"planType":"pro","rateLimitReachedType":null},"limit_model_x":{"limitId":"limit_model_x","limitName":"GPT-5.3 Codex","primary":{"usedPercent":12,"windowDurationMins":10080,"resetsAt":1785000000},"secondary":null,"credits":null,"individualLimit":null,"spendControlReached":null,"planType":"pro","rateLimitReachedType":null}},"rateLimitResetCredits":{"availableCount":0,"credits":[]}}}"#;

/// A `rateLimitResetCredits` summary shaped from a real three-credit answer,
/// with its credit ids elided: one expiring credit fully described, one that
/// does not expire with no display text, one expiring sooner.
fn three_credit_summary() -> Value {
    serde_json::json!({
        "availableCount": 3,
        "credits": [
            {
                "id": "RateLimitResetCredit_…a",
                "resetType": "codexRateLimits",
                "status": "available",
                "grantedAt": 1_790_200_000,
                "expiresAt": 1_792_800_000,
                "title": "Full reset",
                "description": "Thanks for using Codex! You've been granted one free rate limit reset."
            },
            {
                "id": "RateLimitResetCredit_…b",
                "resetType": "codexRateLimits",
                "status": "available",
                "grantedAt": 1_790_200_000,
                "expiresAt": null,
                "title": null,
                "description": null
            },
            {
                "id": "RateLimitResetCredit_…c",
                "resetType": "codexRateLimits",
                "status": "available",
                "grantedAt": 1_790_000_000,
                "expiresAt": 1_791_000_000,
                "title": "Full reset",
                "description": null
            }
        ]
    })
}

/// codex's rate-limit reply with its `rateLimitResetCredits` replaced —
/// `None` removes the key, as a codex predating it answers.
fn codex_with_credits(summary: Option<Value>) -> Value {
    let mut response = codex_response(CODEX_EXCHANGE, 2);
    let result = response["result"].as_object_mut().expect("a result object");
    match summary {
        Some(summary) => result.insert("rateLimitResetCredits".to_string(), summary),
        None => result.remove("rateLimitResetCredits"),
    };
    response
}

/// The `GET /copilot_internal/user` response body.
const COPILOT_BODY: &str = r#"{"copilot_plan":"individual","access_type_sku":"monthly_subscriber_quota","quota_reset_date":"2026-08-01","quota_reset_date_utc":"2026-08-01T00:00:00.000Z","token_based_billing":true,"quota_snapshots":{"chat":{"unlimited":true,"percent_remaining":100.0,"has_quota":true,"entitlement":0,"remaining":0,"credits_used":0,"overage_permitted":false,"quota_reset_at":0},"premium_interactions":{"unlimited":false,"percent_remaining":0.0,"has_quota":false,"entitlement":1500,"credits_used":13518,"remaining":-12019,"overage_permitted":false,"quota_reset_at":0}}}"#;

/// The canonical observation instant every report in this suite is stamped with.
/// A consumer reaches one only through [`UtcInstant`], so a test cannot build a
/// report whose `observed_at` claims something the contract does not allow.
fn observed_at() -> UtcInstant {
    "2026-07-29T12:00:00Z"
        .parse()
        .expect("a canonical RFC 3339 UTC instant")
}

fn lines(stream: &str) -> Vec<Value> {
    stream
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The envelope walk a consumer performs on Claude's stream.
fn claude_payload(stream: &str) -> Value {
    lines(stream)
        .iter()
        .find_map(|line| claude_control_response(line).cloned())
        .expect("the stream carries a successful get_usage response")
}

/// The envelope walk a consumer performs on codex's exchange.
fn codex_response(exchange: &str, id: u64) -> Value {
    lines(exchange)
        .into_iter()
        .find(|line| line.get("id").and_then(Value::as_u64) == Some(id))
        .expect("the exchange carries that response id")
}

fn used_percent(availability: &UsageAvailability, id: &str) -> f64 {
    let window = availability
        .windows()
        .iter()
        .find(|window| window.id == id)
        .unwrap_or_else(|| panic!("no window {id}"));
    match window.usage {
        WindowUsage::Metered { used_percent, .. } => used_percent.get(),
        WindowUsage::Unlimited => panic!("{id} is unlimited"),
    }
}

#[test]
fn a_consumer_reads_claude_headroom_from_a_captured_stream() {
    let parsed = parse_claude_get_usage(&claude_payload(CLAUDE_STREAM));

    assert_eq!(parsed.auth_mode, AuthMode::Subscription);
    assert_eq!(parsed.plan.as_deref(), Some("max"));
    assert_eq!(used_percent(&parsed.availability, "five_hour"), 42.0);
    assert_eq!(used_percent(&parsed.availability, "seven_day"), 61.0);

    let seven_day = parsed
        .availability
        .windows()
        .iter()
        .find(|window| window.id == "seven_day")
        .expect("the weekly window");
    assert_eq!(
        seven_day.resets_at.as_ref().map(UtcInstant::as_str),
        Some("2026-08-02T13:00:00Z"),
        "the -04:00 offset the harness sent must arrive as absolute UTC"
    );
    assert_eq!(seven_day.duration.seconds(), Some(604_800));
    assert_eq!(seven_day.is_binding, Some(true));

    assert!(
        !parsed.availability.windows().iter().any(|window| [
            "seven_day_opus",
            "tangelo",
            "amber_ladder"
        ]
        .contains(&window.id.as_str())),
        "null windows must not reach a consumer at all"
    );

    // The per-model weekly limit has no named `rate_limits` key of its own, so
    // the flat `limits[]` array is the only place a consumer can find it.
    let scoped = parsed
        .availability
        .windows()
        .iter()
        .find(|window| window.id == "weekly_scoped/Opus 5")
        .expect("the model-scoped weekly window");
    assert_eq!(
        used_percent(&parsed.availability, "weekly_scoped/Opus 5"),
        17.0
    );
    assert_eq!(scoped.scope.as_deref(), Some("Opus 5"));
    assert_eq!(scoped.duration.seconds(), Some(604_800));
}

#[test]
fn a_consumer_reads_codex_headroom_from_a_captured_app_server_exchange() {
    let parsed = parse_codex_rate_limits(&codex_response(CODEX_EXCHANGE, 2));

    assert_eq!(parsed.auth_mode, AuthMode::Subscription);
    assert_eq!(
        parsed.plan.as_deref(),
        Some("pro"),
        "codex's plan vocabulary reaches the consumer verbatim, unmerged with Claude's"
    );
    assert_eq!(used_percent(&parsed.availability, "codex/primary"), 31.0);

    let primary = &parsed.availability.windows()[0];
    assert_eq!(
        primary.resets_at.as_ref().map(UtcInstant::as_str),
        Some("2026-07-25T17:20:00Z"),
        "epoch seconds must arrive as absolute UTC, like every other harness"
    );
    assert_eq!(primary.duration.seconds(), Some(604_800));

    // The exchange carries the main bucket, a per-model bucket, and the
    // top-level single-bucket mirror of the first. A consumer sees two windows:
    // one per bucket, with the mirror deduplicated and the null secondaries
    // omitted rather than reported at 0% used.
    assert_eq!(
        parsed
            .availability
            .windows()
            .iter()
            .map(|window| window.id.as_str())
            .collect::<Vec<_>>(),
        vec!["codex/primary", "limit_model_x/primary"]
    );
    let per_model = &parsed.availability.windows()[1];
    assert_eq!(
        per_model.label.as_deref(),
        Some("GPT-5.3 Codex"),
        "codex labels its per-model bucket, and that name must reach the consumer"
    );
    assert_eq!(
        used_percent(&parsed.availability, "limit_model_x/primary"),
        12.0
    );
}

#[test]
fn a_consumer_reads_copilot_headroom_from_the_response_body() {
    let body: Value = serde_json::from_str(COPILOT_BODY).expect("a JSON body");
    let parsed = parse_copilot_user(&body);

    assert_eq!(parsed.plan.as_deref(), Some("individual"));
    assert_eq!(
        used_percent(&parsed.availability, "premium_interactions"),
        100.0
    );

    let chat = parsed
        .availability
        .windows()
        .iter()
        .find(|window| window.id == "chat")
        .expect("the chat quota");
    assert_eq!(
        chat.usage,
        WindowUsage::Unlimited,
        "an unlimited quota must reach a consumer with no counters to misread as full headroom"
    );

    let premium = parsed
        .availability
        .windows()
        .iter()
        .find(|window| window.id == "premium_interactions")
        .expect("the premium quota");
    let WindowUsage::Metered {
        counters: Some(counters),
        ..
    } = &premium.usage
    else {
        panic!("the metered quota carries counters");
    };
    assert!(
        counters.blocked(),
        "a consumer must be able to see exhausted-and-blocked without parsing prose"
    );
}

#[test]
fn an_api_key_identity_offers_a_consumer_no_percentage_to_render() {
    let codex = parse_codex_rate_limits(&serde_json::json!({
        "id": 4,
        "error": {"code": -32600, "message": "chatgpt authentication required to read rate limits"}
    }));
    assert_eq!(codex.auth_mode, AuthMode::ApiKey);
    assert_eq!(
        codex.availability,
        UsageAvailability::Unavailable {
            reason: UnavailableReason::ApiKeyAuth
        }
    );

    let claude = parse_claude_get_usage(&serde_json::json!({
        "subscription_type": null, "rate_limits_available": false,
        "rate_limits": null, "behaviors": null
    }));
    assert_eq!(claude.auth_mode, AuthMode::ApiKey);
    assert_eq!(
        claude.availability,
        UsageAvailability::Unavailable {
            reason: UnavailableReason::ApiKeyAuth
        }
    );

    for parsed in [&codex, &claude] {
        assert!(parsed.availability.windows().is_empty());
    }
}

/// The degradation paths, which matter more than the happy ones: a consumer
/// deciding whether to launch a long run must never read "0% used" out of a
/// probe that simply did not work.
#[test]
fn a_degraded_probe_reaches_a_consumer_as_unknown_or_unavailable_never_as_headroom() {
    // A wedged app-server: a reply carrying neither a result nor an error.
    let wedged = parse_codex_rate_limits(&serde_json::json!({"jsonrpc": "2.0", "id": 2}));
    assert!(matches!(
        wedged.availability,
        UsageAvailability::Unknown { .. }
    ));

    // A failure whose message this version does not recognize stays unknown
    // rather than becoming an assumed absence of headroom.
    let surprising = parse_codex_rate_limits(&serde_json::json!({
        "id": 2, "error": {"code": -32603, "message": "internal error"}
    }));
    assert!(matches!(
        surprising.availability,
        UsageAvailability::Unknown { .. }
    ));

    // No stored credential is a *different* answer from API-key auth, and both
    // are affirmative rather than unknown.
    let logged_out = parse_codex_rate_limits(&serde_json::json!({
        "id": 2,
        "error": {"code": -32600, "message": "codex account authentication required to read rate limits"}
    }));
    assert_eq!(
        logged_out.availability,
        UsageAvailability::Unavailable {
            reason: UnavailableReason::NotLoggedIn
        }
    );

    // A Copilot body with no quota block at all. The endpoint is undocumented
    // internal, so a vanished quota surface is drift — nothing was learned —
    // rather than an affirmative "this account has no quota window".
    let bodiless = parse_copilot_user(&serde_json::json!({"copilot_plan": "individual"}));
    assert!(matches!(
        bodiless.availability,
        UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { .. }
        }
    ));

    // A harness that reports a reset instant a consumer cannot trust: the
    // window still reports its usage, but with no reset rather than a guessed
    // one. (`2026-08-01T00:00:00` has no offset, so its instant is ambiguous.)
    assert_eq!(normalize_timestamp("2026-08-01T00:00:00"), None);
    let unparseable_reset = parse_copilot_user(&serde_json::json!({
        "copilot_plan": "individual",
        "quota_reset_date_utc": "2026-08-01T00:00:00",
        "token_based_billing": true,
        "quota_snapshots": {"premium_interactions": {
            "unlimited": false, "percent_remaining": 40.0, "has_quota": true,
            "entitlement": 1500, "credits_used": 900, "remaining": 600,
            "overage_permitted": true
        }}
    }));
    let window = &unparseable_reset.availability.windows()[0];
    assert_eq!(window.resets_at, None);
    assert!(matches!(window.usage, WindowUsage::Metered { .. }));

    // Not one of these degraded answers exposes a percentage a renderer could
    // draw as an empty bar.
    for parsed in [&wedged, &surprising, &logged_out, &bodiless] {
        assert!(
            parsed.availability.windows().is_empty(),
            "a degraded probe must offer no window at all"
        );
        let json = serde_json::to_string(&UsageIdentity::new(
            "codex",
            IdentitySelector::Ambient,
            parsed.clone(),
        ))
        .expect("the identity serializes");
        assert!(
            !json.contains("used_percent"),
            "no percentage may reach the wire for a degraded probe: {json}"
        );
    }
}

/// A successful JSON-RPC reply is not by itself evidence about an account. The
/// codex app-server answers `account/rateLimits/read` with a result whose whole
/// rate-limit surface is `rateLimits` (schema-required) plus the optional
/// `rateLimitsByLimitId`; a result carrying neither says nothing, and "nothing"
/// must not reach a consumer as the affirmative account state
/// `subscription` + `no_windows_reported`.
#[test]
fn a_codex_result_with_no_rate_limit_surface_is_never_an_account_state() {
    for result in [
        // Answered, empty: the shape a stripped or partially-built payload has.
        serde_json::json!({}),
        // Answered with the keys renamed — the drift a checked-in schema diff
        // catches at build time and a live app-server can still hand a consumer.
        serde_json::json!({
            "rate_limits": {"limitId": "codex", "primary": {"usedPercent": 31}},
            "rateLimitResetCredits": {"availableCount": 0}
        }),
    ] {
        let parsed = parse_codex_rate_limits(&serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "result": result
        }));

        assert!(
            matches!(
                parsed.availability,
                UsageAvailability::Unknown {
                    reason: UnknownReason::ProbeFailed { .. }
                }
            ),
            "{result} carries no rate-limit surface, so it is drift rather than \
             an answer: got {:?}",
            parsed.availability
        );
        assert_ne!(
            parsed.auth_mode,
            AuthMode::Subscription,
            "{result} proves nothing about how this identity authenticates"
        );

        let json = serde_json::to_string(&UsageIdentity::new(
            "codex",
            IdentitySelector::Ambient,
            parsed,
        ))
        .expect("the identity serializes");
        assert!(
            !json.contains("no_windows_reported"),
            "an empty result must not publish an affirmative account state: {json}"
        );
        assert!(!json.contains("used_percent"), "{json}");
    }
}

/// A field whose *type* contradicts the contract carries no information about
/// a subscription, so it can never be read as one of the account states a
/// consumer acts on. This is the silent-false-headroom failure: a wrong answer
/// looks exactly like a right one, where drift at least says so.
#[test]
fn a_wrong_typed_field_reaches_a_consumer_as_drift_never_as_an_account_state() {
    // Cursor's tier is contracted as a string or null, and null is what means
    // "no stored login" — so a value of any other type must not inherit that
    // answer for every Cursor user at once.
    for tier in [
        serde_json::json!(42),
        serde_json::json!(true),
        serde_json::json!(["Team"]),
        serde_json::json!({"name": "Team"}),
    ] {
        let parsed = parse_cursor_about(&serde_json::json!({
            "cliVersion": "2026.07.23-e383d2b",
            "subscriptionTier": tier,
            "userEmail": "someone@example.com"
        }));

        let UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { message },
        } = &parsed.availability
        else {
            panic!(
                "a `subscriptionTier` of {tier} is drift, got {:?}",
                parsed.availability
            );
        };
        assert!(message.contains("subscriptionTier"), "{message}");
        assert_eq!(parsed.auth_mode, AuthMode::Unknown);
        assert_eq!(parsed.plan, None);
    }

    // Copilot's `unlimited` gates whether its counters mean anything at all.
    // The snapshot below is otherwise a picture of health — 100% remaining, a
    // full entitlement — which is exactly what must not be published from a
    // payload that failed to parse.
    let malformed = parse_copilot_user(&serde_json::json!({
        "copilot_plan": "individual",
        "token_based_billing": true,
        "quota_snapshots": {"premium_interactions": {
            "unlimited": "false", "percent_remaining": 100.0, "has_quota": true,
            "entitlement": 1500, "credits_used": 0, "remaining": 1500,
            "overage_permitted": true
        }}
    }));

    let UsageAvailability::Unknown {
        reason: UnknownReason::ProbeFailed { message },
    } = &malformed.availability
    else {
        panic!("got {:?}", malformed.availability);
    };
    assert!(message.contains("unlimited"), "{message}");
    assert!(
        malformed.availability.windows().is_empty(),
        "no percentage may survive a snapshot whose gate could not be read"
    );
}

/// Claude's payload is explicitly experimental with no schema to diff, and every
/// affirmative state its parser publishes rests on a field's *absence*: a missing
/// `subscription_type` means API key, a missing `rate_limits_available` means no
/// headroom. So the drift guard cannot be a separate step a caller has to
/// remember — `parse_claude_get_usage` is public, and a sibling tool reaching it
/// directly must not be able to obtain a confident account state from a payload
/// whose shape moved.
#[test]
fn claudes_public_parser_never_reads_a_drifted_payload_as_an_account_state() {
    let observed = claude_payload(CLAUDE_STREAM);

    // The auth-mode discriminator, renamed. Its absence reads as API-key auth,
    // which is affirmatively "this identity has no plan headroom" — the exact
    // confident wrong answer a planner would act on.
    let mut renamed = observed.clone();
    let fields = renamed.as_object_mut().expect("an object");
    fields.remove("subscription_type");
    fields.insert("plan_type".to_string(), serde_json::json!("max"));
    renamed["rate_limits_available"] = serde_json::json!(false);
    renamed["rate_limits"] = Value::Null;

    // The same discriminator, wrong-typed: contracted as a plan string or null.
    let mut wrong_typed = observed.clone();
    wrong_typed["subscription_type"] = serde_json::json!(7);

    // The window surface moved wholesale under an affirmative availability flag,
    // so the parser finds nothing and would report "no windows reported".
    let mut moved_windows = observed.clone();
    moved_windows["rate_limits"] = serde_json::json!({
        "renamed_five_hour": {"pct_used": 42},
        "limits": [{"kind": "brand_new_kind", "percent": 42}]
    });

    // The availability flag itself, vanished: read with an absence-means-false
    // default, so a rename turns every subscriber into "no headroom" at once.
    let mut vanished_flag = observed.clone();
    vanished_flag
        .as_object_mut()
        .expect("an object")
        .remove("rate_limits_available");

    for drifted in [renamed, wrong_typed, moved_windows, vanished_flag] {
        let parsed = parse_claude_get_usage(&drifted);

        assert!(
            matches!(
                parsed.availability,
                UsageAvailability::Unknown {
                    reason: UnknownReason::ProbeFailed { .. }
                }
            ),
            "a drifted payload must say so, not answer: got {:?} from {drifted}",
            parsed.availability
        );
        assert_eq!(
            parsed.auth_mode,
            AuthMode::Unknown,
            "a payload that could not be read says nothing about auth mode"
        );
        assert_eq!(parsed.plan, None);
        assert!(
            parsed.availability.windows().is_empty(),
            "no percentage may survive a payload whose shape was not recognized"
        );
    }
}

/// The counters' deliberate asymmetry: `remaining` is genuinely negative for an
/// account past its ceiling and that deficit is the signal, while a negative
/// entitlement or consumption is a payload nobody could read.
#[test]
fn an_over_ceiling_deficit_survives_while_a_negative_entitlement_drops_the_counters() {
    let body: Value = serde_json::from_str(COPILOT_BODY).expect("a JSON body");
    let observed = parse_copilot_user(&body);
    let WindowUsage::Metered {
        counters: Some(counters),
        ..
    } = &premium(&observed).usage
    else {
        panic!("the observed capture carries counters");
    };
    assert_eq!(
        counters.remaining, -12019,
        "spending past the ceiling is a real deficit, reported as reported"
    );
    assert_eq!(counters.entitlement.get(), 1500);
    assert_eq!(counters.used.get(), 13518);

    let mut impossible = body.clone();
    impossible["quota_snapshots"]["premium_interactions"]["entitlement"] = serde_json::json!(-1500);
    let parsed = parse_copilot_user(&impossible);
    let WindowUsage::Metered {
        used_percent,
        counters,
    } = &premium(&parsed).usage
    else {
        panic!("the percentage is validated separately and still stands");
    };
    assert_eq!(used_percent.get(), 100.0);
    assert_eq!(
        *counters, None,
        "a negative entitlement is unreadable, never a quantity to clamp"
    );

    // Nor can one be deserialized back out of a stored report.
    assert!(serde_json::from_str::<QuotaAmount>("-1").is_err());
    assert_eq!(
        serde_json::from_str::<QuotaAmount>("0")
            .expect("zero is a real entitlement")
            .get(),
        0
    );
}

/// The `premium_interactions` window of a parsed Copilot payload.
fn premium(parsed: &ParsedUsage) -> &oneharness_core::domain::usage::UsageWindow {
    parsed
        .availability
        .windows()
        .iter()
        .find(|window| window.id == "premium_interactions")
        .expect("the premium quota")
}

#[test]
fn the_report_json_is_what_a_downstream_consumer_reads() {
    let report = UsageReport::new(
        observed_at(),
        vec![
            UsageIdentity::new(
                "claude-code",
                IdentitySelector::EnvPath {
                    env: "CLAUDE_CONFIG_DIR".to_string(),
                    path: "/home/u/.claude".to_string(),
                },
                parse_claude_get_usage(&claude_payload(CLAUDE_STREAM)),
            ),
            UsageIdentity::new(
                "codex",
                IdentitySelector::EnvPath {
                    env: "CODEX_HOME".to_string(),
                    path: "/home/u/.codex".to_string(),
                },
                parse_codex_rate_limits(&codex_response(CODEX_EXCHANGE, 2)),
            ),
            UsageIdentity::new(
                "copilot",
                IdentitySelector::EnvSecret {
                    env: "GH_TOKEN".to_string(),
                },
                parse_copilot_user(&serde_json::from_str(COPILOT_BODY).expect("a JSON body")),
            ),
        ],
    );

    // Read it back as a consumer in any language would: through the JSON.
    let json: Value =
        serde_json::from_str(&serde_json::to_string(&report).expect("the report serializes"))
            .expect("valid JSON");

    assert_eq!(json["schema_version"], SCHEMA_VERSION);
    assert_eq!(json["observed_at"], "2026-07-29T12:00:00Z");

    let claude = &json["identities"][0];
    assert_eq!(claude["harness"], "claude-code");
    assert_eq!(claude["selector"]["kind"], "env_path");
    assert_eq!(claude["plan"], "max");
    assert_eq!(claude["availability"]["state"], "available");
    let five_hour = &claude["availability"]["windows"][0];
    assert_eq!(five_hour["id"], "five_hour");
    assert_eq!(five_hour["usage"]["kind"], "metered");
    assert_eq!(five_hour["usage"]["used_percent"], 42.0);
    assert_eq!(five_hour["window_seconds_source"], "inferred_from_id");
    assert_eq!(five_hour["window_seconds"], 18_000);

    let codex_primary = &json["identities"][1]["availability"]["windows"][0];
    assert_eq!(
        codex_primary["window_seconds_source"], "reported",
        "a consumer must be able to tell a stated window length from a derived one"
    );

    let copilot = &json["identities"][2];
    assert_eq!(copilot["plan"], "individual");
    assert_eq!(
        copilot["selector"].get("path"),
        None,
        "a secret-valued selector must never carry its credential into the report"
    );
    let chat = &copilot["availability"]["windows"][0];
    assert_eq!(chat["id"], "chat");
    assert_eq!(chat["usage"]["kind"], "unlimited");
    assert_eq!(
        chat["usage"].get("used_percent"),
        None,
        "an unlimited quota has no percentage a consumer could draw as a full bar"
    );
    assert_eq!(
        chat["window_seconds_source"], "unknown",
        "a calendar month reports no fixed length rather than a default one"
    );

    let round_tripped: UsageReport =
        serde_json::from_value(json).expect("the contract deserializes back");
    assert_eq!(round_tripped, report);
}

/// The bytes a consumer of the v0.1 contract sees.
const GOLDEN: &str = include_str!("../../../tests/fixtures/usage-report-v01.json");

/// Every shape the v0.1 contract can take, in one report: a plan window whose
/// length is derived from its key, one whose length the harness stated, one
/// whose length cannot be established at all, a model-scoped window, an
/// unlimited quota, a metered quota with counters, an affirmative unavailable,
/// an unknown, and reset credits listed (expiring and not), known by count
/// only, unanswered and unread. If a shape is not here, the golden does not pin
/// it.
fn golden_report() -> UsageReport {
    let mut claude = claude_payload(CLAUDE_STREAM);
    // A codename key carrying real data. Every one observed so far was null,
    // but the key set is open by contract, and this is the forward-compatible
    // shape a consumer must handle: an opaque window with no derivable
    // duration, no label, no reset, no scope, and no binding flag — i.e. the
    // window that exercises every omission at once.
    claude["rate_limits"]["nimbus_quill"] = serde_json::json!({
        "utilization": 4,
        "resets_at": null,
        "limit_dollars": null, "used_dollars": null, "remaining_dollars": null
    });

    UsageReport::new(
        observed_at(),
        vec![
            UsageIdentity::new(
                "claude-code",
                IdentitySelector::EnvPath {
                    env: "CLAUDE_CONFIG_DIR".to_string(),
                    path: "/home/u/.claude".to_string(),
                },
                parse_claude_get_usage(&claude),
            ),
            UsageIdentity::new(
                "codex",
                IdentitySelector::EnvPath {
                    env: "CODEX_HOME".to_string(),
                    path: "/home/u/.codex".to_string(),
                },
                parse_codex_rate_limits(&codex_with_credits(Some(three_credit_summary()))),
            ),
            UsageIdentity::new(
                "copilot",
                IdentitySelector::EnvSecret {
                    env: "GH_TOKEN".to_string(),
                },
                parse_copilot_user(&serde_json::from_str(COPILOT_BODY).expect("a JSON body")),
            ),
            UsageIdentity::new(
                "codex",
                IdentitySelector::EnvPath {
                    env: "CODEX_HOME".to_string(),
                    path: "/home/u/.codex-apikey".to_string(),
                },
                parse_codex_rate_limits(&serde_json::json!({
                    "id": 4,
                    "error": {
                        "code": -32600,
                        "message": "chatgpt authentication required to read rate limits"
                    }
                })),
            ),
            // A second subscription of the same harness, selected by a named
            // variant: the entry a consumer must be able to tell apart from the
            // first even though both are `claude-code`.
            UsageIdentity::new(
                "claude-code",
                IdentitySelector::EnvPath {
                    env: "CLAUDE_CONFIG_DIR".to_string(),
                    path: "/home/u/.claude-work".to_string(),
                },
                ParsedUsage::unknown(UnknownReason::ProbeFailed {
                    message: "claude-code's `get_usage` control request did not answer within 60s"
                        .to_string(),
                }),
            )
            .with_variant(Some("work".parse().expect("a declarable variant name"))),
            // Plan tier only: a real plan name with an affirmative "no reader".
            UsageIdentity::new(
                "cursor",
                IdentitySelector::Ambient,
                parse_cursor_about(&serde_json::json!({"subscriptionTier": "Team"})),
            ),
            // The two affirmative "cannot report headroom" answers, which a
            // consumer must be able to tell apart from each other and from
            // `unknown`.
            UsageIdentity::new(
                "opencode",
                IdentitySelector::Ambient,
                unavailable(UnavailableReason::NoPlanQuota),
            ),
            UsageIdentity::new(
                "qwen",
                IdentitySelector::Ambient,
                unavailable(UnavailableReason::NoHeadroomReader),
            ),
            // A harness that is simply not installed here.
            UsageIdentity::new(
                "crush",
                IdentitySelector::Ambient,
                ParsedUsage::unknown(UnknownReason::BinaryMissing {
                    bin: "crush".to_string(),
                }),
            ),
            UsageIdentity::new(
                "goose",
                IdentitySelector::Ambient,
                ParsedUsage::unknown(UnknownReason::Unprobed),
            ),
            // Reset credits known only by count: `credits` is omitted.
            UsageIdentity::new(
                "codex",
                IdentitySelector::EnvPath {
                    env: "CODEX_HOME".to_string(),
                    path: "/home/u/.codex-count".to_string(),
                },
                parse_codex_rate_limits(&codex_with_credits(Some(
                    serde_json::json!({"availableCount": 2, "credits": null}),
                ))),
            ),
            // A codex answering without a credits summary, and one whose
            // summary could not be read: both still report their windows.
            UsageIdentity::new(
                "codex",
                IdentitySelector::EnvPath {
                    env: "CODEX_HOME".to_string(),
                    path: "/home/u/.codex-old".to_string(),
                },
                parse_codex_rate_limits(&codex_with_credits(None)),
            ),
            UsageIdentity::new(
                "codex",
                IdentitySelector::EnvPath {
                    env: "CODEX_HOME".to_string(),
                    path: "/home/u/.codex-odd".to_string(),
                },
                parse_codex_rate_limits(&codex_with_credits(Some(
                    serde_json::json!({"availableCount": -1}),
                ))),
            ),
        ],
    )
}

/// An affirmative "no headroom to report", as the command layer builds it.
fn unavailable(reason: UnavailableReason) -> ParsedUsage {
    ParsedUsage {
        auth_mode: AuthMode::Unknown,
        plan: None,
        availability: UsageAvailability::Unavailable { reason },
        reset_credits: ResetCredits::Unknown {
            reason: ResetCreditsUnknown::NoReader,
        },
    }
}

#[test]
fn the_v0_1_report_serializes_to_the_checked_in_golden() {
    let actual = serde_json::to_string(&golden_report()).expect("the report serializes");

    assert_eq!(
        actual.as_bytes(),
        GOLDEN.trim_end().as_bytes(),
        "the v0.1 usage contract changed; if that is deliberate, bump SCHEMA_VERSION \
         and update tests/fixtures/usage-report-v01.json in the same change"
    );
}

#[test]
fn the_v0_1_golden_deserializes_and_round_trips_through_the_public_api() {
    let parsed: UsageReport =
        serde_json::from_str(GOLDEN).expect("the golden deserializes with the current types");

    assert_eq!(parsed, golden_report());
    assert_eq!(
        serde_json::to_string(&parsed).expect("re-serializes"),
        GOLDEN.trim_end(),
        "reading the contract and writing it back must be byte-stable"
    );

    // An absent optional deserializes as absent, not as some default value.
    let opaque = parsed.identities[0]
        .availability
        .windows()
        .iter()
        .find(|window| window.id == "nimbus_quill")
        .expect("the opaque codename window");
    assert_eq!(opaque.label, None);
    assert_eq!(opaque.resets_at, None);
    assert_eq!(opaque.scope, None);
    assert_eq!(opaque.is_binding, None);
    assert_eq!(opaque.duration.seconds(), None);
    assert_eq!(
        opaque.usage,
        WindowUsage::Metered {
            used_percent: UsedPercent::new(4.0).expect("a valid percentage"),
            counters: None
        }
    );
    assert_eq!(parsed.identities[3].plan, None);
}

/// The golden with one envelope field replaced, as a consumer's own JSON would
/// arrive: a report written by a different build, or by hand.
fn golden_with(field: &str, value: Value) -> String {
    let mut json: Value = serde_json::from_str(GOLDEN).expect("valid JSON");
    json[field] = value;
    json.to_string()
}

/// A `schema_version` this build does not implement must be refused, not read as
/// the version it does. `oneharness-core` is published, so a sibling tool linked
/// against v0.1 will one day be handed a v0.2 report — and reinterpreting it
/// would hand back a confident headroom figure computed from a shape that no
/// longer means the same thing, the exact silent-wrong-answer this report exists
/// to prevent.
#[test]
fn a_report_claiming_an_unsupported_schema_version_is_refused() {
    for version in ["0.2", "1.0", "0.10", "", "0.1-rc1"] {
        let json = golden_with("schema_version", serde_json::json!(version));
        let message = match serde_json::from_str::<UsageReport>(&json) {
            Ok(report) => panic!(
                "schema_version `{version}` must not deserialize, \
                 got a report claiming `{}`",
                report.schema_version
            ),
            Err(error) => error.to_string(),
        };
        assert!(
            message.contains("schema_version") && message.contains(SCHEMA_VERSION),
            "the refusal must name the field and the version this build reads: {message}"
        );
    }

    let current = serde_json::from_str::<UsageReport>(&golden_with(
        "schema_version",
        serde_json::json!(SCHEMA_VERSION),
    ))
    .expect("the supported version still deserializes");
    assert_eq!(current.schema_version.as_str(), SCHEMA_VERSION);

    // The construction half: a caller of the published crate cannot claim another
    // version either, because the field's type has exactly one value. Which is
    // why the refusal above is the only way a wrong version can be *observed*.
    let built = UsageReport::new(observed_at(), one_unprobed_identity());
    assert_eq!(built.schema_version.as_str(), SCHEMA_VERSION);
    assert_eq!(built.schema_version.to_string(), SCHEMA_VERSION);
    assert_eq!(
        serde_json::to_value(&built).expect("the report serializes")["schema_version"],
        serde_json::json!(SCHEMA_VERSION),
        "and it reaches the wire as the version string a consumer branches on"
    );
}

/// Text that is not an RFC 3339 UTC instant. Shared by every boundary test
/// below, because a timestamp field is validated at three of them — deserializing
/// a consumer's JSON, parsing a harness payload, and public construction — and one
/// list is what keeps those three from drifting apart on what they accept.
const NOT_UTC_INSTANTS: &[&str] = &[
    "2026-07-29T12:00:00",       // no offset at all: an ambiguous local time
    "2026-07-29",                // a date, not an instant
    "2026-07-29T12:00:00.Z",     // an empty sub-second fraction
    "2026-13-01T00:00:00Z",      // a month that does not exist
    "2026-07-29T25:00:00Z",      // an hour that does not exist
    "2026-07-29T08:00:00-04:00", // a real instant, but not UTC
    "2026-07-29T17:30:00+05:30",
    "yesterday afternoon",
    "",
];

/// Every spelling of the golden's own observation instant that means exactly the
/// same moment in UTC, so each is canonicalized rather than refused.
const EQUIVALENT_UTC_SPELLINGS: &[&str] = &[
    "2026-07-29T12:00:00Z",
    "2026-07-29T12:00:00+00:00",
    "2026-07-29T12:00:00.500Z",
    "2026-07-29T12:00:00-0000",
];

/// `observed_at` documents an RFC 3339 UTC instant, and every consumer that
/// renders "usage as of ..." reads it literally. An unparseable one, or one whose
/// offset is not UTC, is therefore refused rather than stored — a `+05:30`
/// instant in a field read as UTC is wrong by hours with nothing to signal it.
#[test]
fn a_report_whose_observed_at_is_not_rfc3339_utc_is_refused() {
    for malformed in NOT_UTC_INSTANTS {
        let json = golden_with("observed_at", serde_json::json!(malformed));
        let message = match serde_json::from_str::<UsageReport>(&json) {
            Ok(report) => panic!(
                "`{malformed}` must not deserialize, got observed_at `{}`",
                report.observed_at
            ),
            Err(error) => error.to_string(),
        };
        assert!(
            message.contains("observed_at"),
            "the refusal must name the field: {message}"
        );
    }

    // An equivalent UTC spelling is the same instant, so it is canonicalized
    // rather than refused — the field still means exactly what it documents.
    for equivalent in EQUIVALENT_UTC_SPELLINGS {
        let parsed: UsageReport =
            serde_json::from_str(&golden_with("observed_at", serde_json::json!(equivalent)))
                .unwrap_or_else(|error| panic!("`{equivalent}` is RFC 3339 UTC: {error}"));
        assert_eq!(
            parsed.observed_at.as_str(),
            "2026-07-29T12:00:00Z",
            "`{equivalent}` must arrive as the canonical UTC spelling"
        );
    }
}

/// The boundary the test above cannot reach: a sibling tool linked against this
/// published crate *builds* reports rather than reading them. While `observed_at`
/// was a plain `String`, such a caller could stamp one with `"yesterday
/// afternoon"` or a `+05:30` instant and publish a report whose own documentation
/// was false — the same confidently-wrong answer the deserialize check exists to
/// stop, arriving from the other side. [`UtcInstant`] is the only door in, so this
/// is what it must refuse and what it must accept.
#[test]
fn a_caller_cannot_construct_a_report_whose_observed_at_is_not_rfc3339_utc() {
    for malformed in NOT_UTC_INSTANTS {
        let error = malformed
            .parse::<UtcInstant>()
            .expect_err("`{malformed}` is not an RFC 3339 UTC instant");
        assert!(
            error.to_string().contains("RFC 3339 UTC instant"),
            "the refusal must say what is required, got: {error}"
        );
    }

    // An equivalent spelling is accepted and canonicalized on the way in, so the
    // built report reads back byte-identically to a deserialized one.
    for equivalent in EQUIVALENT_UTC_SPELLINGS {
        let instant: UtcInstant = equivalent
            .parse()
            .unwrap_or_else(|error| panic!("`{equivalent}` is RFC 3339 UTC: {error}"));
        let json = serde_json::to_value(UsageReport::new(instant, one_unprobed_identity()))
            .expect("the report serializes");
        assert_eq!(
            json["observed_at"], "2026-07-29T12:00:00Z",
            "`{equivalent}` must reach the wire as the canonical UTC spelling"
        );
    }
}

/// The single identity a report built for a boundary test carries, so the report
/// is still a whole one while the envelope is what is under test.
fn one_unprobed_identity() -> Vec<UsageIdentity> {
    vec![UsageIdentity::new(
        "goose",
        IdentitySelector::Ambient,
        ParsedUsage::unknown(UnknownReason::Unprobed),
    )]
}

/// The golden with one field replaced on its first window — the shape a consumer's
/// own JSON takes when it carries a reset or a window length this build must judge.
fn golden_window_with(fields: &[(&str, Value)]) -> String {
    let mut json: Value = serde_json::from_str(GOLDEN).expect("valid JSON");
    let window = &mut json["identities"][0]["availability"]["windows"][0];
    for (field, value) in fields {
        window[*field] = value.clone();
    }
    json.to_string()
}

/// A window's `resets_at` is read exactly as literally as `observed_at`: a
/// planner deciding whether to wait for a reset reads it as an absolute UTC
/// instant. So the same rule holds at the same three boundaries — and a
/// harness's own non-UTC offset is *converted* on the way in rather than
/// refused, which is the one asymmetry [`normalize_timestamp`] exists for.
#[test]
fn a_window_reset_is_a_utc_instant_or_absent_never_arbitrary_text() {
    for refused in NOT_UTC_INSTANTS
        .iter()
        .copied()
        // Also refused: text that is instant-shaped but carries a terminal escape.
        // It cannot be flattened into something safe, because a flattened
        // timestamp is no longer a timestamp.
        .chain(["2026-07-29T18:30:00Z\u{1b}[2K", "never"])
    {
        let json = golden_window_with(&[("resets_at", serde_json::json!(refused))]);
        let message = match serde_json::from_str::<UsageReport>(&json) {
            Ok(report) => panic!(
                "a window reset of `{refused:?}` must not deserialize, got `{:?}`",
                report.identities[0].availability.windows()[0].resets_at
            ),
            Err(error) => error.to_string(),
        };
        assert!(
            message.contains("RFC 3339 UTC instant"),
            "the refusal must say what is required: {message}"
        );
        assert!(
            !message.chars().any(char::is_control),
            "the refusal quotes the value back, so it must not smuggle an escape: {message:?}"
        );
        assert!(
            refused.parse::<UtcInstant>().is_err(),
            "public construction must refuse `{refused:?}` too, not just the wire"
        );
    }

    // The converting entry point, for the offsets a harness really reports.
    assert_eq!(
        normalize_timestamp("2026-08-02T09:00:00-04:00")
            .as_ref()
            .map(UtcInstant::as_str),
        Some("2026-08-02T13:00:00Z"),
        "a harness's own offset is converted, which is what makes the field UTC"
    );
}

/// A window zero seconds long is not a window. The parsers already degraded a
/// non-positive `windowDurationMins` to [`WindowDuration::Unknown`], but the
/// variants exposed a raw integer — so a caller of the published crate, or a
/// report written by one, could still claim `window_seconds: 0`. A consumer
/// dividing a remaining quota by that window's length would divide by zero;
/// `Unknown` is the state it already handles.
#[test]
fn a_window_length_of_zero_seconds_is_not_representable() {
    for source in ["reported", "inferred_from_id"] {
        let json = golden_window_with(&[
            ("window_seconds_source", serde_json::json!(source)),
            ("window_seconds", serde_json::json!(0)),
        ]);
        assert!(
            serde_json::from_str::<UsageReport>(&json).is_err(),
            "a `{source}` window of 0 seconds must not deserialize"
        );
    }

    for zero in [
        WindowDuration::reported(0),
        WindowDuration::inferred_from_id(0),
    ] {
        assert_eq!(
            zero,
            WindowDuration::Unknown,
            "a caller constructing a zero length gets the honest 'not established'"
        );
        assert_eq!(zero.seconds(), None);
    }

    // A real length is untouched by any of that.
    assert_eq!(WindowDuration::reported(600).seconds(), Some(600));
    assert_eq!(
        WindowDuration::inferred_from_id(18_000).seconds(),
        Some(18_000)
    );
}

/// The two figures that look like errors and are not, kept representable while
/// their neighbours were tightened: consumption *past* a harness ceiling is
/// exactly when a consumer needs the real percentage, and an over-ceiling
/// account's `remaining` is a genuine deficit. Both are built the way a sibling
/// tool would and read back the way a consumer would.
#[test]
fn an_overage_percentage_and_a_deficit_survive_construction_and_the_wire() {
    let window = UsageWindow {
        id: "premium_interactions".to_string(),
        label: None,
        usage: WindowUsage::Metered {
            used_percent: UsedPercent::new(140.0).expect("an overage is a real reading"),
            counters: Some(QuotaCounters {
                entitlement: QuotaAmount::new(1_500).expect("non-negative"),
                used: QuotaAmount::new(2_100).expect("non-negative"),
                remaining: -600,
                has_quota: false,
                overage_permitted: false,
                unit: QuotaUnit::AiCredits,
            }),
        },
        duration: WindowDuration::reported(600),
        resets_at: Some("2026-08-01T00:00:00Z".parse().expect("a canonical instant")),
        scope: None,
        is_binding: Some(true),
    };
    let report = UsageReport::new(
        observed_at(),
        vec![UsageIdentity::new(
            "copilot",
            IdentitySelector::EnvSecret {
                env: "GH_TOKEN".to_string(),
            },
            ParsedUsage {
                auth_mode: AuthMode::Subscription,
                plan: Some("individual".to_string()),
                availability: UsageAvailability::from_windows(vec![window]),
                reset_credits: ResetCredits::Unknown {
                    reason: ResetCreditsUnknown::NoReader,
                },
            },
        )],
    );

    let text = serde_json::to_string(&report).expect("the report serializes");
    let read_back: UsageReport = serde_json::from_str(&text).expect("a consumer reads it back");

    assert_eq!(read_back, report);
    let WindowUsage::Metered {
        used_percent,
        counters: Some(counters),
    } = &read_back.identities[0].availability.windows()[0].usage
    else {
        panic!("the metered window survives the round trip: {text}");
    };
    assert_eq!(
        used_percent.get(),
        140.0,
        "an overage must not be clamped to 100: {text}"
    );
    assert_eq!(
        counters.remaining, -600,
        "a deficit past the ceiling is the signal, not an error to zero out: {text}"
    );
}

#[test]
fn identity_attribution_is_validated_when_read_from_the_wire() {
    let valid: Value = serde_json::from_str(GOLDEN).expect("the golden is JSON");

    for (field, invalid) in [
        ("harness", ""),
        ("harness", "unknown-harness"),
        ("variant", ""),
        ("variant", "not.a.variant"),
    ] {
        let mut report = valid.clone();
        report["identities"][0][field] = Value::String(invalid.to_string());
        assert!(
            serde_json::from_value::<UsageReport>(report).is_err(),
            "an identity with {field} `{invalid}` must not cross the wire boundary"
        );
    }

    for index in [0, 1] {
        let mut report = valid.clone();
        report["identities"][index]["selector"]["env"] =
            Value::String("NOT-AN-ENV-NAME".to_string());
        assert!(
            serde_json::from_value::<UsageReport>(report).is_err(),
            "an invalid environment name in identity {index} must not cross the wire boundary"
        );
    }
}

/// The construction half of the rule above. A published crate that accepts an
/// attribution through one door and refuses the same one through the other
/// hands a sibling tool a report it cannot read back — and the attribution is
/// the whole point of the field, since it is what tells two subscriptions of one
/// harness apart.
///
/// A variant reaches the field only as a [`VariantName`], so the refusal is the
/// same validator's, whichever door the value came through.
#[test]
fn identity_attribution_is_validated_when_built_in_process() {
    let valid: Value = serde_json::from_str(GOLDEN).expect("the golden is JSON");

    // "" names no declared variant at all; "not.a.variant" is a name no config
    // could have declared.
    for invalid in ["", "not.a.variant"] {
        let refused = invalid
            .parse::<VariantName>()
            .expect_err("the constructor path refuses the name outright")
            .to_string();

        let mut report = valid.clone();
        report["identities"][0]["variant"] = Value::String(invalid.to_string());
        let from_wire = serde_json::from_value::<UsageReport>(report)
            .expect_err("the wire path refuses it too")
            .to_string();

        assert!(
            from_wire.contains(&refused),
            "both doors must refuse `{invalid}` for the same stated reason, \
             got `{from_wire}` on the wire and `{refused}` in process"
        );
    }

    // And the attribution a caller *can* build is one this crate reads back.
    let attributed = UsageIdentity::new(
        "claude-code",
        IdentitySelector::Ambient,
        ParsedUsage::unknown(UnknownReason::Unprobed),
    )
    .with_variant(Some("work".parse().expect("a declarable variant name")));
    let report = UsageReport::new(observed_at(), vec![attributed]);
    let text = serde_json::to_string(&report).expect("the report serializes");
    let read_back: UsageReport =
        serde_json::from_str(&text).expect("a constructed report reads back");

    assert_eq!(read_back, report);
    assert_eq!(
        read_back.identities[0]
            .variant
            .as_ref()
            .map(VariantName::as_str),
        Some("work"),
        "the variant reaches the wire as the plain string a run report carries: {text}"
    );
}

/// The omissions are the point: a consumer distinguishes "the harness did not
/// report this" from a value, and a JSON `null` is neither.
#[test]
fn absent_optionals_are_omitted_from_the_wire_rather_than_written_as_null() {
    let json: Value = serde_json::from_str(GOLDEN).expect("valid JSON");

    assert!(
        !GOLDEN.contains(":null"),
        "no field may reach the wire as null: {GOLDEN}"
    );

    let opaque = json["identities"][0]["availability"]["windows"]
        .as_array()
        .expect("windows")
        .iter()
        .find(|window| window["id"] == "nimbus_quill")
        .expect("the opaque codename window");
    for absent in [
        "label",
        "resets_at",
        "scope",
        "is_binding",
        "window_seconds",
    ] {
        assert_eq!(
            opaque.get(absent),
            None,
            "{absent} must be absent, not null, on a window that has none"
        );
    }
    assert_eq!(
        opaque["window_seconds_source"], "unknown",
        "the source discriminator stays, so a consumer can tell \
         'no length reported' from 'length not yet read'"
    );
    assert_eq!(
        opaque["usage"].get("counters"),
        None,
        "a window with no counter set omits it rather than writing null"
    );

    // The shapes that *do* carry these fields still carry them.
    let scoped = json["identities"][0]["availability"]["windows"]
        .as_array()
        .expect("windows")
        .iter()
        .find(|window| window["id"] == "weekly_scoped/Opus 5")
        .expect("the model-scoped window");
    assert_eq!(scoped["scope"], "Opus 5");
    assert_eq!(scoped["is_binding"], false);
    assert_eq!(scoped["label"], "Opus 5");

    assert_eq!(
        json["identities"][3].get("plan"),
        None,
        "an API-key identity has no plan, and says so by omission"
    );
    assert_eq!(json["identities"][0]["plan"], "max");
}

/// Rewrites `tests/fixtures/usage-report-v01.json` from [`golden_report`], then
/// asserts the file it left behind is one a consumer can actually read back.
/// Ignored by default — run with `--ignored` after a deliberate contract change,
/// then read the diff before committing it. Regenerating without checking the
/// result would let a broken serialization author its own passing golden.
#[test]
#[ignore = "regenerates the checked-in golden; run deliberately"]
fn regenerating_the_golden_leaves_a_readable_contract() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/usage-report-v01.json"
    );
    let json = serde_json::to_string(&golden_report()).expect("the report serializes");
    std::fs::write(path, format!("{json}\n")).expect("the fixture is writable");

    let written = std::fs::read_to_string(path).expect("the fixture reads back");
    let parsed: UsageReport =
        serde_json::from_str(&written).expect("the regenerated golden deserializes");
    assert_eq!(parsed, golden_report(), "the golden must round-trip");
    assert_eq!(
        parsed.schema_version.as_str(),
        SCHEMA_VERSION,
        "a regenerated golden that no longer matches SCHEMA_VERSION means a bump was missed"
    );
}

/// codex's own generated contract for `account/rateLimits/read`, snapshotted
/// from the installed CLI. `scripts/check-codex-usage-schema.sh` diffs this
/// against a fresh generation when codex is installed; the test below is the
/// hermetic half, and it is the one that fails when a *field the parser reads*
/// disappears rather than merely when the file changes.
const CODEX_SCHEMA: &str = include_str!("../../../tests/fixtures/codex-rate-limits.schema.json");

/// Does `schema` define `property` on `definition` (or at the top level when
/// `definition` is `None`)?
fn declares(schema: &Value, definition: Option<&str>, property: &str) -> bool {
    let object = match definition {
        Some(name) => &schema["definitions"][name],
        None => schema,
    };
    object["properties"].get(property).is_some()
}

#[test]
fn codex_schema_snapshot_still_declares_every_field_the_parser_reads() {
    // The app-server is experimental, so this is the drift alarm for the exact
    // set of names `parse_codex_rate_limits` walks. A rename upstream would
    // otherwise turn a real reading into "no windows reported" — a confident
    // answer built from a shape that no longer exists.
    let schema: Value = serde_json::from_str(CODEX_SCHEMA).expect("the snapshot is JSON");

    for property in ["rateLimits", "rateLimitsByLimitId"] {
        assert!(
            declares(&schema, None, property),
            "the response no longer declares `{property}`"
        );
    }
    for property in ["limitId", "limitName", "primary", "secondary", "planType"] {
        assert!(
            declares(&schema, Some("RateLimitSnapshot"), property),
            "RateLimitSnapshot no longer declares `{property}`"
        );
    }
    for property in ["usedPercent", "windowDurationMins", "resetsAt"] {
        assert!(
            declares(&schema, Some("RateLimitWindow"), property),
            "RateLimitWindow no longer declares `{property}`"
        );
    }

    // `usedPercent` is the one required window field, which is why a window
    // without a usable percentage is dropped rather than defaulted to zero.
    assert_eq!(
        schema["definitions"]["RateLimitWindow"]["required"],
        serde_json::json!(["usedPercent"])
    );
    // The reset-credits summary: a missing name here would turn a real count
    // into `not_reported` for every account at once.
    assert!(
        declares(&schema, None, "rateLimitResetCredits"),
        "the response no longer declares `rateLimitResetCredits`"
    );
    for property in ["availableCount", "credits"] {
        assert!(
            declares(&schema, Some("RateLimitResetCreditsSummary"), property),
            "RateLimitResetCreditsSummary no longer declares `{property}`"
        );
    }
    assert_eq!(
        schema["definitions"]["RateLimitResetCreditsSummary"]["required"],
        serde_json::json!(["availableCount"]),
        "`availableCount` is what an absent one is read as malformed against"
    );
    for property in [
        "id",
        "resetType",
        "status",
        "grantedAt",
        "expiresAt",
        "title",
        "description",
    ] {
        assert!(
            declares(&schema, Some("RateLimitResetCredit"), property),
            "RateLimitResetCredit no longer declares `{property}`"
        );
    }
    assert_eq!(
        schema["definitions"]["RateLimitResetCredit"]["required"],
        serde_json::json!(["grantedAt", "id", "resetType", "status"]),
        "the parser refuses a row missing exactly these"
    );
    // The two words the text view treats as expiry evidence must still be
    // values codex can send.
    for (definition, value) in [
        ("RateLimitResetCreditStatus", "available"),
        ("RateLimitResetType", "codexRateLimits"),
    ] {
        assert!(
            schema["definitions"][definition]["enum"]
                .as_array()
                .is_some_and(|values| values.iter().any(|v| v == value)),
            "{definition} no longer carries `{value}`"
        );
    }

    // The plan vocabulary is kept verbatim rather than mapped onto Claude's, so
    // the enum only has to remain a string — but it must remain present.
    assert!(
        schema["definitions"]["PlanType"]["enum"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "pro")),
        "PlanType is no longer the enum the `plan` field is read from"
    );
}

#[test]
fn a_payload_shaped_like_the_snapshot_parses_into_headroom() {
    // Ties the two halves together: the fields asserted above are the ones an
    // actual reading flows through, so the snapshot cannot drift into being a
    // check on names nothing consumes.
    let parsed = parse_codex_rate_limits(&codex_response(CODEX_EXCHANGE, 2));

    assert!(matches!(
        parsed.availability,
        UsageAvailability::Available { .. }
    ));
    assert_eq!(parsed.plan.as_deref(), Some("pro"));
    assert_eq!(used_percent(&parsed.availability, "codex/primary"), 31.0);
}

fn codex_credits(response: &Value) -> ResetCredits {
    parse_codex_rate_limits(response).reset_credits
}

fn listed(credits: &ResetCredits) -> (u64, Option<&[ResetCredit]>) {
    match credits {
        ResetCredits::Reported {
            available_count,
            credits,
        } => (*available_count, credits.as_deref()),
        ResetCredits::Unknown { reason } => panic!("expected reported credits, got {reason:?}"),
    }
}

#[test]
fn a_payload_shaped_like_the_snapshot_parses_into_reported_credits() {
    let credits = codex_credits(&codex_with_credits(Some(three_credit_summary())));

    let (count, rows) = listed(&credits);
    assert_eq!(count, 3);
    let rows = rows.expect("the credits were listed");
    assert_eq!(
        rows[0],
        ResetCredit {
            id: "RateLimitResetCredit_…a".to_string(),
            status: ResetCreditStatus::Available,
            reset_type: "codexRateLimits".to_string().into(),
            granted_at: UtcInstant::from_epoch(1_790_200_000),
            expires_at: Some(UtcInstant::from_epoch(1_792_800_000)),
            title: Some("Full reset".to_string()),
            description: Some(
                "Thanks for using Codex! You've been granted one free rate limit reset."
                    .to_string()
            ),
        }
    );
    assert_eq!(
        rows[1].expires_at, None,
        "`expiresAt: null` does not expire"
    );
    assert_eq!(
        (rows[1].title.as_deref(), rows[1].description.as_deref()),
        (None, None)
    );

    // Through the JSON a consumer reads: absent, never null.
    let json = serde_json::to_value(&credits).expect("serializes");
    assert_eq!(json["state"], "reported");
    assert_eq!(json["available_count"], 3);
    assert_eq!(json["credits"][0]["granted_at"], "2026-09-23T21:46:40Z");
    assert_eq!(json["credits"][0]["expires_at"], "2026-10-24T00:00:00Z");
    for omitted in ["expires_at", "title", "description"] {
        assert_eq!(json["credits"][1].get(omitted), None, "{omitted}: {json}");
    }
}

#[test]
fn an_answer_without_a_credits_summary_is_not_reported_never_zero() {
    for summary in [None, Some(Value::Null)] {
        let parsed = parse_codex_rate_limits(&codex_with_credits(summary.clone()));
        assert_eq!(
            parsed.reset_credits,
            ResetCredits::Unknown {
                reason: ResetCreditsUnknown::NotReported
            },
            "summary {summary:?}"
        );
        assert!(matches!(
            parsed.availability,
            UsageAvailability::Available { .. }
        ));
    }
}

#[test]
fn a_count_without_details_keeps_the_count_and_omits_the_list() {
    let credits = codex_credits(&codex_with_credits(Some(
        serde_json::json!({"availableCount": 4, "credits": null}),
    )));
    assert_eq!(listed(&credits), (4, None));
    let json = serde_json::to_value(&credits).expect("serializes");
    assert_eq!(
        json,
        serde_json::json!({"state": "reported", "available_count": 4})
    );

    // Absent `credits` is the same answer as `null`.
    let absent = codex_credits(&codex_with_credits(Some(
        serde_json::json!({"availableCount": 4}),
    )));
    assert_eq!(absent, credits);
}

#[test]
fn a_capped_list_keeps_the_harness_count_as_the_total() {
    let mut summary = three_credit_summary();
    summary["availableCount"] = 7.into();
    let credits = codex_credits(&codex_with_credits(Some(summary)));
    let (count, rows) = listed(&credits);
    assert_eq!(
        count, 7,
        "the count is the harness's, never the list's length"
    );
    assert_eq!(rows.map(<[ResetCredit]>::len), Some(3));

    let none = codex_credits(&codex_with_credits(Some(
        serde_json::json!({"availableCount": 0, "credits": []}),
    )));
    assert_eq!(
        listed(&none),
        (0, Some(&[][..])),
        "no credits is a reported zero"
    );
}

#[test]
fn an_unseen_status_or_reset_type_is_reported_verbatim_and_the_probe_succeeds() {
    let mut summary = three_credit_summary();
    summary["credits"][0]["status"] = "frozen".into();
    summary["credits"][1]["resetType"] = "claudeWeekly".into();
    let parsed = parse_codex_rate_limits(&codex_with_credits(Some(summary)));

    assert!(matches!(
        parsed.availability,
        UsageAvailability::Available { .. }
    ));
    let (_, rows) = listed(&parsed.reset_credits);
    let rows = rows.expect("listed");
    assert_eq!(rows[0].status.as_str(), "frozen");
    assert_eq!(rows[1].reset_type.as_str(), "claudeWeekly");
    assert!(matches!(rows[0].status, ResetCreditStatus::Unrecognized(_)));

    assert!(!rows[0].is_recognized_available("codex"));
    assert!(!rows[1].is_recognized_available("codex"));
    assert!(rows[2].is_recognized_available("codex"));
    assert!(
        !rows[2].is_recognized_available("claude-code"),
        "a reset type is recognized per harness"
    );
}

#[test]
fn a_malformed_summary_is_unreadable_and_leaves_the_reading_untouched() {
    let baseline = parse_codex_rate_limits(&codex_with_credits(None));
    let row = |field: &str, value: Value| {
        let mut summary = three_credit_summary();
        summary["credits"][1][field] = value;
        summary
    };
    let without = |field: &str| {
        let mut summary = three_credit_summary();
        summary["credits"][1]
            .as_object_mut()
            .expect("a row")
            .remove(field);
        summary
    };
    let cases = [
        (serde_json::json!([]), "rather than an object"),
        (serde_json::json!({"credits": []}), "no `availableCount`"),
        (serde_json::json!({"availableCount": -1}), "negative"),
        (serde_json::json!({"availableCount": 1.5}), "fractional"),
        (serde_json::json!({"availableCount": "3"}), "a string"),
        (
            serde_json::json!({"availableCount": 1, "credits": "x"}),
            "array or null",
        ),
        (
            serde_json::json!({"availableCount": 1, "credits": [7]}),
            "credits[0]` is a number",
        ),
        (without("id"), "credits[1]` carries no `id`"),
        (without("status"), "no `status`"),
        (without("resetType"), "no `resetType`"),
        (without("grantedAt"), "no `grantedAt`"),
        (
            row("grantedAt", "yesterday".into()),
            "`grantedAt` that is a string",
        ),
        (
            row("expiresAt", 1.5.into()),
            "`expiresAt` that is a fractional",
        ),
        (
            row("expiresAt", i64::MAX.into()),
            "outside the four-digit years",
        ),
        (row("title", 3.into()), "`title` that is a number"),
        (row("status", Value::Null), "`status` that is null"),
    ];
    for (summary, expected) in cases {
        let parsed = parse_codex_rate_limits(&codex_with_credits(Some(summary.clone())));
        match &parsed.reset_credits {
            ResetCredits::Unknown {
                reason: ResetCreditsUnknown::Unreadable { message },
            } => assert!(
                message.contains(expected),
                "{summary}: `{message}` should say {expected:?}"
            ),
            other => panic!("{summary} parsed as {other:?}, not unreadable"),
        }
        assert_eq!(parsed.availability, baseline.availability, "{summary}");
        assert_eq!(
            (parsed.auth_mode, &parsed.plan),
            (baseline.auth_mode, &baseline.plan)
        );
    }
}

#[test]
fn a_codex_identity_with_no_rate_limit_answer_has_no_answer_to_read_credits_from() {
    let no_answer = ResetCredits::Unknown {
        reason: ResetCreditsUnknown::NoAnswer,
    };
    for message in [
        "chatgpt authentication required to read rate limits",
        "codex account authentication required to read rate limits",
        "backend unreachable",
    ] {
        let parsed = parse_codex_rate_limits(&serde_json::json!({
            "id": 2, "error": {"code": -32600, "message": message}
        }));
        assert_eq!(parsed.reset_credits, no_answer, "{message}");
    }
    // A probe that failed or never ran: nothing was read.
    for parsed in [
        ParsedUsage::unknown(UnknownReason::binary_missing("codex")),
        parse_codex_rate_limits(&serde_json::json!({"id": 2})),
    ] {
        assert_eq!(parsed.reset_credits, no_answer);
    }
}

#[test]
fn a_harness_with_no_credits_reader_says_so_whatever_its_probe_did() {
    // A failed claude probe is not a missing answer from a credits reader:
    // there is no reader, and the identity says that rather than `no_answer`.
    for (harness, parsed) in [
        ("claude-code", ParsedUsage::unknown(UnknownReason::Unprobed)),
        (
            "claude-code",
            parse_claude_get_usage(&claude_payload(CLAUDE_STREAM)),
        ),
        (
            "copilot",
            parse_copilot_user(&serde_json::from_str(COPILOT_BODY).expect("a JSON body")),
        ),
        (
            "cursor",
            parse_cursor_about(&serde_json::json!({"subscriptionTier": "Team"})),
        ),
        ("goose", unavailable(UnavailableReason::NoPlanQuota)),
    ] {
        let identity = UsageIdentity::new(harness, IdentitySelector::Ambient, parsed);
        assert_eq!(
            identity.reset_credits,
            ResetCredits::Unknown {
                reason: ResetCreditsUnknown::NoReader
            },
            "{harness}"
        );
    }
}

#[test]
fn a_v0_1_report_written_before_reset_credits_reads_them_as_no_reader() {
    let mut json: Value = serde_json::from_str(GOLDEN).expect("valid JSON");
    for identity in json["identities"].as_array_mut().expect("identities") {
        identity
            .as_object_mut()
            .expect("an identity")
            .remove("reset_credits");
    }
    let report: UsageReport = serde_json::from_value(json).expect("the older report deserializes");
    for identity in &report.identities {
        assert_eq!(
            identity.reset_credits,
            ResetCredits::Unknown {
                reason: ResetCreditsUnknown::NoReader
            },
            "{}",
            identity.harness
        );
    }
}

#[test]
fn control_characters_in_credit_strings_are_flattened_on_parse_and_on_read() {
    let mut summary = three_credit_summary();
    summary["credits"][0]["id"] = "id\u{1b}[2J".into();
    summary["credits"][0]["status"] = "avail\u{7}able".into();
    summary["credits"][0]["resetType"] = "codex\rRateLimits".into();
    summary["credits"][0]["title"] = "Full\u{1b}[31m reset".into();
    summary["credits"][0]["description"] = "line\nbreak".into();
    let parsed = UsageIdentity::new(
        "codex",
        IdentitySelector::Ambient,
        parse_codex_rate_limits(&codex_with_credits(Some(summary))),
    );

    let mut wire = serde_json::to_value(&parsed).expect("serializes");
    // The same escapes arriving in a report this crate never parsed.
    wire["reset_credits"]["credits"][1]["id"] = "b\u{9b}31m".into();
    wire["reset_credits"]["credits"][1]["title"] = "t\u{8}".into();
    let read: UsageIdentity = serde_json::from_value(wire).expect("deserializes");
    let unreadable: UsageIdentity = serde_json::from_value(serde_json::json!({
        "harness": "codex",
        "selector": {"kind": "ambient"},
        "auth_mode": "unknown",
        "availability": {"state": "unknown", "reason": {"kind": "unprobed"}},
        "reset_credits": {
            "state": "unknown",
            "reason": {"kind": "unreadable", "message": "bad\u{1b}[2J summary"}
        }
    }))
    .expect("deserializes");

    for identity in [&parsed, &read, &unreadable] {
        let text = serde_json::to_string(&identity.reset_credits).expect("serializes");
        let decoded: Value = serde_json::from_str(&text).expect("JSON");
        let mut strings = Vec::new();
        collect_strings(&decoded, &mut strings);
        for string in strings {
            assert!(
                !string.chars().any(char::is_control),
                "`{string:?}` kept a control character"
            );
        }
    }
    let (_, rows) = listed(&parsed.reset_credits);
    assert_eq!(
        rows.expect("listed")[0].title.as_deref(),
        Some("Full [31m reset")
    );
}

fn collect_strings(value: &Value, into: &mut Vec<String>) {
    match value {
        Value::String(text) => into.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, into)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, into)),
        _ => {}
    }
}

/// The instants `scripts/e2e-lib.sh`'s live phase holds a credit's timestamps
/// to, shared with `scripts/check-usage-enforce.sh` so the two validators
/// cannot disagree: every canonical one parses and re-spells as itself, and
/// every invalid one is refused.
#[test]
fn the_shared_utc_instant_matrix_matches_the_parser() {
    let matrix: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/utc-instant-matrix.json"
    ))
    .expect("the matrix is JSON");
    let cases = |key: &str| -> Vec<String> {
        matrix[key]
            .as_array()
            .expect("a list")
            .iter()
            .map(|case| case.as_str().expect("a string").to_string())
            .collect()
    };
    for canonical in cases("canonical") {
        let instant: UtcInstant = canonical.parse().expect(&canonical);
        assert_eq!(instant.as_str(), canonical);
    }
    for invalid in cases("invalid") {
        assert!(invalid.parse::<UtcInstant>().is_err(), "{invalid} parsed");
    }
}
