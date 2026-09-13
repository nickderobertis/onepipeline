//! The two commands a run fires once when it **ends**: a success hook when every
//! node settled `done`, and a failure hook when it ended any other way.

use crate::cli::DEFAULT_HOOK_TIMEOUT_SECONDS;

/// The hook command a launch names, resolved from its two rungs.
///
/// The **presence** of a rung decides which one answers, as it does for the node
/// validator: the flag beats the launch config even when what it names is blank,
/// and a blank command is this launch saying it has none rather than a
/// fall-through to the rung below. Not resolved against the launch directory — a
/// command may as legitimately be a name on `PATH` as a path, and it runs in that
/// directory anyway.
pub(crate) fn named(flag: Option<&str>, config: Option<&str>) -> Option<String> {
    flag.or(config)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
}

/// The refusal for a hook timeout of zero, by the spelling that carried it.
///
/// One sentence for the flag and the launch-config key, as the write-back's
/// budget has one: a timeout of zero ends every hook before it has begun, so a
/// launch that wrote it asked for something it would not get.
pub(crate) fn refused_zero_timeout(spelling: &str) -> String {
    format!(
        "{spelling} names a hook timeout of zero seconds, which ends every hook before it has \
         begun — give it a positive whole number of seconds, or leave it out to take \
         {DEFAULT_HOOK_TIMEOUT_SECONDS} seconds"
    )
}
