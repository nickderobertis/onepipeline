# `human-items-v1`

A host human-item journal (`human-items/events.ndjson`) the **released**
`onepipeline` 0.107.0 wrote (the `onepipeline-cli-linux-arm64` 0.107.0 npm
package), every event at `schema_version` 1, before actions, resolutions and `by`
carried a `provenance`. It holds three raises — the task `released-done`, the
question `released-open` and the task `released-task`, each owned by session
`owner` and holding nothing — and the completion of `released-done`, taken by a
manager session acting for `user:operator`. `tests/e2e/human_items.rs` copies it
into a world's runs root as it stands; nothing here is edited by hand.
