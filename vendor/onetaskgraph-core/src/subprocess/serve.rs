//! The plugin's half of the protocol, for a source this build already has.
//!
//! This is the reference implementation of the other side of
//! `docs/plugin-protocol.md`, and it exists for two reasons. It is what the
//! `onetaskgraph-source` program runs, so any registered plugin can be hosted in a child
//! process without being rewritten. And it is what makes the journeys real: the shared
//! fixture table configures a source through it, so every journey in the suite runs a
//! second time over a genuine pipe to a genuine second process rather than over a double
//! standing in for one.
//!
//! It answers strictly in order, which §1.1 names as the simpler correct choice, and it
//! never crashes on a bad line from the other side (§6.3).

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use onetaskgraph_plugin_api::{
    ContradictedItem, Document, ItemWrite, MetadataRecord, NativeId, Page, Project, SecretResolver,
    SourceError, SourceName, Status, StatusCategory, StatusContradiction, Task, TaskDetailRead,
    TaskSource, TaskUpdateOutcome,
};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{Value, json};

use super::connection::{Line, MAX_LINE, read_line};
use super::wire::{
    AddCommentParams, CheckWriteParams, CommentCopyParams, CommentsParams, ContentParams,
    ContradictionsResult, DeleteCommentParams, DeleteParams, DeliveredByParams, DependencyParams,
    DocumentDir, DocumentQueryParams, EditCommentParams, HandshakePluginKind, HoldingsParams,
    HoldingsResult, IdParams, InitializeParams, InitializeResult, LabelParams, MetadataParams,
    PROTOCOL_VERSION, PriorityParams, ProjectQueryParams, ProjectWriteParams, Request, Response,
    ServedWriteParams, StatusParams, StoredStatusParams, StoredStatusResult, TaskDetailParams,
    TaskQueryParams, UpdateParams, after_the_first_vocabulary, knows_every_category, vocabulary,
};
use crate::config::rebased;
use crate::registry::PluginKind;

/// What this reference host needs in the `config` the handshake hands it.
///
/// The protocol says `config` is "this source's `config:` block, verbatim" and says
/// nothing about its contents, because they are the plugin's own business. This host's
/// business is to run one of *this build's* registered plugins, so its settings name
/// which one and hand over that plugin's block untouched.
#[derive(Debug, Clone, Deserialize)]
struct HostedSettings {
    /// The registered plugin kind to build.
    kind: PluginKind,
    /// That plugin's own `config:` block.
    #[serde(default)]
    config: Value,
}

/// The source one connection hosts, and what the engine said it can be told.
struct Hosted {
    /// The built source.
    source: Box<dyn TaskSource>,
    /// Whether the engine's `initialize` listed every status category this build knows
    /// (§3.5). An engine that did not is told a category added after the first vocabulary
    /// as `unknown`, keeping its name.
    engine_knows_every_category: bool,
}

/// Serve one connection until the engine closes its input.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when this process can no longer read its
/// input or write its output. Everything else — an unusable configuration, a request this
/// version has no method for, a line that is not JSON — is answered on the wire or
/// reported on standard error, because a plugin that exits on a bad line takes every
/// other in-flight request with it (§6.3).
pub async fn serve(input: impl BufRead, output: impl Write) -> std::io::Result<()> {
    serve_kind(input, output, None).await
}

/// Serve one connection as the registered plugin `kind`.
///
/// Unlike [`serve`], the initialize request's `config` is handed directly to that plugin;
/// the process command has already selected the kind it hosts.
pub async fn serve_plugin(
    input: impl BufRead,
    output: impl Write,
    kind: PluginKind,
) -> std::io::Result<()> {
    serve_kind(input, output, Some(kind)).await
}

async fn serve_kind(
    mut input: impl BufRead,
    mut output: impl Write,
    kind: Option<PluginKind>,
) -> std::io::Result<()> {
    let mut source: Option<Hosted> = None;
    loop {
        let line = match read_line(&mut input) {
            Line::Read(line) => line,
            Line::Ended => return Ok(()),
            Line::Failed(error) => return Err(error),
            // Nothing after an unterminated line can be framed — the rest of it would be
            // read as further requests it is not — so this side says why and stops rather
            // than answering questions nobody asked. The engine sees the closed stream.
            Line::TooLong => {
                eprintln!(
                    "onetaskgraph-source: a request ran past {MAX_LINE} bytes without \
                     ending its line; closing the connection"
                );
                return Ok(());
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Some(id) = addressed(&line) else {
            eprintln!("onetaskgraph-source: ignoring a line with no request id: {line}");
            continue;
        };
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => answer(&mut source, request, kind).await,
            Err(error) => Response::failed(
                id,
                SourceError::Malformed {
                    message: format!("that is not a request envelope: {error}"),
                },
            ),
        };
        let finished = ended_the_connection(&response);
        writeln!(
            output,
            "{}",
            // A response is built from contract types that all serialize.
            serde_json::to_string(&response).expect("a response is plain data")
        )?;
        output.flush()?;
        if finished {
            return Ok(());
        }
    }
}

/// The `id` a line is addressed with, if it has one at all.
///
/// Read from the raw JSON rather than from a parsed [`Request`] because §6.3 turns on
/// exactly this difference: a request this side cannot otherwise understand is *answered*
/// when an id can be associated with it, and only reported on standard error when one
/// cannot.
fn addressed(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

/// Whether this response is a refusal §6.2 says the plugin exits after.
fn ended_the_connection(response: &Response) -> bool {
    matches!(
        response.error.as_ref(),
        Some(SourceError::Config { message }) if message.starts_with(VERSION_REFUSAL)
    )
}

/// The opening of the one message §6.2 spells out, so the refusal and the exit that
/// follows it cannot drift apart.
const VERSION_REFUSAL: &str = "protocol version ";

/// Answer one well-formed request.
async fn answer(
    source: &mut Option<Hosted>,
    request: Request,
    kind: Option<PluginKind>,
) -> Response {
    let Request { id, method, params } = request;
    if method == "initialize" {
        return match source {
            Some(_) => Response::failed(
                id,
                SourceError::Malformed {
                    message: "this connection was already initialized".to_owned(),
                },
            ),
            None => initialize(source, id, params, kind).await,
        };
    }
    let Some(built) = source.as_ref() else {
        return Response::failed(
            id,
            SourceError::Malformed {
                message: format!("{method} arrived before the handshake"),
            },
        );
    };
    match dispatch(
        built.source.as_ref(),
        built.engine_knows_every_category,
        &method,
        params,
    )
    .await
    {
        Ok(result) => Response::ok(id, result),
        Err(error) => Response::failed(id, error),
    }
}

/// The handshake (§3), including the version refusal §6.2 spells out.
async fn initialize(
    source: &mut Option<Hosted>,
    id: String,
    params: Value,
    kind: Option<PluginKind>,
) -> Response {
    let params: InitializeParams = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(error) => {
            return Response::failed(
                id,
                SourceError::Config {
                    message: format!("that is not an initialize request: {error}"),
                },
            );
        }
    };
    if params.protocol_version != PROTOCOL_VERSION {
        return Response::failed(
            id,
            SourceError::Config {
                message: format!(
                    "{VERSION_REFUSAL}{} is not supported by this plugin; it speaks \
                     version {PROTOCOL_VERSION}",
                    params.protocol_version
                ),
            },
        );
    }
    match build(&params, kind) {
        Ok(built) => {
            let kind = match HandshakePluginKind::new(built.kind()) {
                Ok(kind) => kind,
                Err(error) => {
                    return Response::failed(
                        id,
                        SourceError::Malformed {
                            message: format!("the hosted plugin reported an invalid kind: {error}"),
                        },
                    );
                }
            };
            let result = InitializeResult {
                protocol_version: Some(PROTOCOL_VERSION),
                kind,
                capabilities: built.capabilities(),
                writes: Some(built.writes()),
                // Asked once, here, for the same reason `writes` is: the engine is then
                // never sent a method this plugin would only have to decline.
                meters: matches!(built.metering().await, Ok(Some(_))),
                statuses: Some(vocabulary()),
                // This host serves this build's own plugins, whose every one implements both
                // narrow writes or refuses one by name — which is an answer, not a method the
                // engine should not have sent.
                task_updates: true,
                // On the same terms: every plugin of this build sets one metadata key or
                // refuses to in the contract's own words.
                metadata_updates: true,
                // And again: every plugin of this build replaces a task's content or refuses
                // to in the contract's own words.
                content_updates: true,
                // And the targeted update: every plugin of this build applies one, through its
                // own override or the contract's default.
                targeted_updates: true,
                // And the end of a command: this host forwards it to whichever plugin it
                // serves, which drops what it holds or holds nothing and does nothing.
                ends_commands: true,
                // And a copied comment: every plugin of this build stores its origin natively
                // or through the contract's default marker.
                comment_copies: true,
                // And a task read beside a page of its comments, which this host answers with
                // the plugin's own detail read — one read of the two, where the plugin makes one.
                task_details: true,
                // And a write checked before it is sent, which every plugin of this build answers
                // through its own check or the contract's default.
                write_checks: true,
                // And a project's status set on its own, which every plugin of this build writes
                // or refuses in the contract's own words.
                project_status: true,
                // And what a backend stores of what it is written, which every plugin of this
                // build declares through its own rewrite or the contract's identity, and says
                // after a write where its backend answered with it.
                stored_forms: true,
            };
            *source = Some(Hosted {
                source: built,
                engine_knows_every_category: knows_every_category(params.statuses.as_deref()),
            });
            Response::ok(
                id,
                serde_json::to_value(&result).expect("a result is plain data"),
            )
        }
        Err(error) => Response::failed(id, error),
    }
}

/// Build the registered plugin these settings name.
fn build(
    params: &InitializeParams,
    selected: Option<PluginKind>,
) -> Result<Box<dyn TaskSource>, SourceError> {
    let (kind, config) = match selected {
        Some(kind) => (kind, &params.config),
        None => {
            let settings: HostedSettings = serde_json::from_value(params.config.clone()).map_err(
                |error| SourceError::Config {
                    message: format!(
                        "this host serves a plugin of this build, and its settings must name one \
                         as {{\"kind\": …, \"config\": …}}: {error}"
                    ),
                },
            )?;
            return build_plugin(params, settings.kind, &settings.config);
        }
    };
    build_plugin(params, kind, config)
}

fn build_plugin(
    params: &InitializeParams,
    kind: PluginKind,
    config: &Value,
) -> Result<Box<dyn TaskSource>, SourceError> {
    let name = SourceName::new(params.source_name.clone())?;
    let plugin = kind.plugin();
    let config = measured_from_document(
        params.document_dir.as_ref(),
        plugin.document_relative_paths(),
        config,
    );
    plugin.build(&name, &config, &Handshake(&params.secrets))
}

/// `config` with every relative path at one of `fields` measured from `document_dir`.
///
/// This is the in-process rule of `crate::config`'s `relative` module carried across the
/// seam (§3): the engine keeps a `subprocess` source's settings opaque and rebases
/// nothing inside them, so the hosted plugin's own declaration decides which fields are
/// paths, and a plugin declaring none has nothing resolved. With no `document_dir` the
/// block is handed over untouched, and a relative path goes on meaning the working
/// directory — which is what the engine sends when the block came from the environment
/// or a flag rather than from a document.
fn measured_from_document(
    document_dir: Option<&DocumentDir>,
    fields: &[&str],
    config: &Value,
) -> Value {
    let mut config = config.clone();
    // Absolute by construction: a relative one was refused as the handshake was read.
    let Some(directory) = document_dir.map(DocumentDir::as_path) else {
        return config;
    };
    for field in fields {
        let Some(value) = field
            .split('.')
            .try_fold(&mut config, |cursor, segment| cursor.get_mut(segment))
        else {
            continue;
        };
        let Some(rebased) = value.as_str().and_then(|raw| rebased(directory, raw)) else {
            continue;
        };
        // Both halves arrived as JSON strings, so the path they join into is one too.
        let rebased = rebased
            .into_os_string()
            .into_string()
            .expect("a path joined from two strings is a string");
        *value = Value::String(rebased);
    }
    config
}

/// The credentials the handshake forwarded, and nothing else.
///
/// §3.1 is the whole of this type: a plugin must not read credentials from its own
/// process environment, because doing so makes it work on a host where the engine's own
/// resolution would have failed — and that difference is exactly what `config show`
/// reports and a user relies on.
struct Handshake<'a>(&'a BTreeMap<String, String>);

impl SecretResolver for Handshake<'_> {
    fn get(&self, var: &str) -> Option<SecretString> {
        self.0
            .get(var)
            .map(|value| SecretString::from(value.clone()))
    }
}

/// A status as an engine that knows `known` categories may be told it (§3.5).
fn told(status: Status, known: bool) -> Status {
    if known || !after_the_first_vocabulary(status.category) {
        return status;
    }
    Status {
        category: StatusCategory::Unknown,
        name: status.name,
    }
}

fn told_task(task: Task, known: bool) -> Task {
    Task {
        status: told(task.status.clone(), known),
        ..task
    }
}

fn told_project(project: Project, known: bool) -> Project {
    Project {
        status: told(project.status.clone(), known),
        ..project
    }
}

/// One method call against the built source (§4).
///
/// `known` is whether the engine listed every status category this build knows; every
/// status this answers with passes through [`told`] on its way out.
async fn dispatch(
    source: &dyn TaskSource,
    known: bool,
    method: &str,
    params: Value,
) -> Result<Value, SourceError> {
    match method {
        "health" => encode(source.health().await?),
        "get_task" => {
            let params: IdParams = decode(method, params)?;
            let task = source.get_task(&params.id).await?;
            encode(json!({ "task": task.map(|task| told_task(task, known)) }))
        }
        "get_task_detail" => {
            let params: TaskDetailParams = decode(method, params)?;
            let read = one_detail(
                &params.id,
                source
                    .get_task_details(std::slice::from_ref(&params.id), Some(&params.comments))
                    .await,
            )?;
            let Some(read) = read else {
                return encode(json!({ "task": null, "page": null }));
            };
            let task = told_task(read.task, known);
            match read.comments {
                Some(Ok(page)) => encode(json!({ "task": task, "page": page })),
                // The task was read and its comments were refused: both are the answer.
                Some(Err(error)) => encode(json!({ "task": task, "comments_error": error })),
                None => encode(json!({ "task": task, "page": null })),
            }
        }
        "get_project" => {
            let params: IdParams = decode(method, params)?;
            let project = source.get_project(&params.id).await?;
            encode(json!({ "project": project.map(|project| told_project(project, known)) }))
        }
        "query_tasks" => {
            let params: TaskQueryParams = decode(method, params)?;
            let page = source.query_tasks(&params.query, &params.page).await?;
            encode(Page {
                items: page
                    .items
                    .into_iter()
                    .map(|task| told_task(task, known))
                    .collect(),
                next: page.next,
            })
        }
        "query_projects" => {
            let params: ProjectQueryParams = decode(method, params)?;
            let page = source.query_projects(&params.query, &params.page).await?;
            encode(Page {
                items: page
                    .items
                    .into_iter()
                    .map(|project| told_project(project, known))
                    .collect(),
                next: page.next,
            })
        }
        "set_task_status" => {
            let params: StatusParams = decode(method, params)?;
            let status = source.set_task_status(&params.id, params.category).await?;
            encode(json!({ "status": status.map(|status| told(status, known)) }))
        }
        "set_project_status" => {
            let params: StatusParams = decode(method, params)?;
            let status = source
                .set_project_status(&params.id, params.category)
                .await?;
            // §4.17a: the result is the `Status` itself, or `null`, with no wrapping member.
            encode(status.map(|status| told(status, known)))
        }
        "check_task_write" => {
            let params: CheckWriteParams = decode(method, params)?;
            source.check_task_write(&params.write).await?;
            encode(json!({}))
        }
        "status_contradictions" => {
            let items = source.status_contradictions().await?;
            encode(ContradictionsResult {
                items: items
                    .into_iter()
                    .map(|contradiction| StatusContradiction {
                        item: match contradiction.item {
                            ContradictedItem::Task(task) => {
                                ContradictedItem::Task(told_task(task, known))
                            }
                            ContradictedItem::Project(project) => {
                                ContradictedItem::Project(told_project(project, known))
                            }
                        },
                        ..contradiction
                    })
                    .collect(),
            })
        }
        "issue_holdings" => {
            let params: HoldingsParams = decode(method, params)?;
            let holdings = source.issue_holdings(&params.issues).await?;
            encode(HoldingsResult { holdings })
        }
        "set_task_priority" => {
            let params: PriorityParams = decode(method, params)?;
            let priority = source
                .set_task_priority(&params.id, params.priority)
                .await?;
            encode(json!({ "priority": priority }))
        }
        "set_task_content" => {
            let params: ContentParams = decode(method, params)?;
            let written = source.set_task_content(&params.id, &params.content).await?;
            encode(json!({ "id": written.map(|()| params.id) }))
        }
        "update_task" => {
            let params: UpdateParams = decode(method, params)?;
            let outcome = source.update_task(&params.id, &params.update).await?;
            encode(json!({ "outcome": outcome.map(|outcome| TaskUpdateOutcome {
                task: told_task(outcome.task, known),
                ..outcome
            }) }))
        }
        "set_task_metadata" => {
            let params: MetadataParams = decode(method, params)?;
            let task = source
                .set_task_metadata(params.id(), params.key(), params.value())
                .await?;
            encode(json!({ "task": task.map(|task| told_task(task, known)) }))
        }
        "set_project_metadata" => {
            let params: MetadataParams = decode(method, params)?;
            let project = source
                .set_project_metadata(params.id(), params.key(), params.value())
                .await?;
            encode(json!({ "project": project.map(|project| told_project(project, known)) }))
        }
        "set_document_metadata" => {
            let params: MetadataParams = decode(method, params)?;
            let document = source
                .set_document_metadata(params.id(), params.key(), params.value())
                .await?;
            encode(json!({ "document": document }))
        }
        "set_delivered_by" => {
            let params: DeliveredByParams = decode(method, params)?;
            let held = source
                .set_delivered_by(&params.id, &params.delivered_by)
                .await?;
            encode(json!({ "delivered_by": held.map(|()| params.delivered_by) }))
        }
        "labels" => {
            let params: LabelParams = decode(method, params)?;
            encode(source.labels(&params.page).await?)
        }
        "task_dependencies" => {
            let params: DependencyParams = decode(method, params)?;
            encode(
                source
                    .task_dependencies(&params.id, params.direction, &params.page)
                    .await?,
            )
        }
        "project_dependencies" => {
            let params: DependencyParams = decode(method, params)?;
            encode(
                source
                    .project_dependencies(&params.id, params.direction, &params.page)
                    .await?,
            )
        }
        "write_task" => {
            let params: ServedWriteParams<Task> = decode(method, params)?;
            forget(source, MetadataRecord::Task, &params.write);
            match params.assets {
                Some(assets) => {
                    let written = source
                        .write_task_with_assets(&params.write, None, &assets)
                        .await?;
                    let id = written.id.clone();
                    stored(source, MetadataRecord::Task, &id, encode(written)?)
                }
                None => {
                    let id = source.write_task(&params.write).await?;
                    stored(source, MetadataRecord::Task, &id, json!({ "id": id }))
                }
            }
        }
        "write_project" => {
            let params: ProjectWriteParams = decode(method, params)?;
            forget(source, MetadataRecord::Project, &params.write);
            let id = source.write_project(&params.write).await?;
            stored(source, MetadataRecord::Project, &id, json!({ "id": id }))
        }
        "stored_status" => {
            let params: StoredStatusParams = decode(method, params)?;
            let status = source.stored_status(params.kind, &params.status).await?;
            encode(StoredStatusResult {
                status: told(status, known),
            })
        }
        "delete_task" => {
            let params: DeleteParams = decode(method, params)?;
            source.delete_task(&params.id).await?;
            encode(json!({}))
        }
        "delete_project" => {
            let params: DeleteParams = decode(method, params)?;
            source.delete_project(&params.id).await?;
            encode(json!({}))
        }
        "get_document" => {
            let params: IdParams = decode(method, params)?;
            encode(json!({ "document": source.get_document(&params.id).await? }))
        }
        "query_documents" => {
            let params: DocumentQueryParams = decode(method, params)?;
            encode(source.query_documents(&params.query, &params.page).await?)
        }
        "write_document" => {
            let params: ServedWriteParams<Document> = decode(method, params)?;
            forget(source, MetadataRecord::Document, &params.write);
            match params.assets {
                Some(assets) => {
                    let written = source
                        .write_document_with_assets(&params.write, None, &assets)
                        .await?;
                    let id = written.id.clone();
                    stored(source, MetadataRecord::Document, &id, encode(written)?)
                }
                None => {
                    let id = source.write_document(&params.write).await?;
                    stored(source, MetadataRecord::Document, &id, json!({ "id": id }))
                }
            }
        }
        "delete_document" => {
            let params: DeleteParams = decode(method, params)?;
            source.delete_document(&params.id).await?;
            encode(json!({}))
        }
        "task_comments" => {
            let params: CommentsParams = decode(method, params)?;
            encode(json!({ "page": source.task_comments(&params.task, &params.page).await? }))
        }
        "add_comment" => {
            let params: AddCommentParams = decode(method, params)?;
            encode(json!({ "comment": source.add_comment(&params.task, &params.comment).await? }))
        }
        "edit_comment" => {
            let params: EditCommentParams = decode(method, params)?;
            encode(json!({
                "comment": source
                    .edit_comment(&params.task, &params.comment, &params.body)
                    .await?
            }))
        }
        "write_comment_copy" => {
            let params: CommentCopyParams = decode(method, params)?;
            // The origin's task decides which copies a later copy owns, edits and deletes, so
            // one naming no task is refused before anything is written rather than stored and
            // only found out when it is read back.
            params.copy.origin.qualified()?;
            encode(json!({
                "comment": source
                    .write_comment_copy(&params.task, params.existing.as_ref(), &params.copy)
                    .await?
            }))
        }
        "delete_comment" => {
            let params: DeleteCommentParams = decode(method, params)?;
            encode(json!({
                "deleted": source.delete_comment(&params.task, &params.comment).await?
            }))
        }
        "metering" => encode(json!({ "metering": source.metering().await? })),
        "end_command" => {
            // It takes no parameters, but §1 still makes them an object: anything else is
            // refused before the source drops anything, and members in one are ignored (§2.1).
            let _: serde_json::Map<String, Value> = decode(method, params)?;
            source.end_command().await?;
            encode(json!({}))
        }
        other => Err(SourceError::Malformed {
            message: format!("protocol version {PROTOCOL_VERSION} has no method called {other:?}"),
        }),
    }
}

/// The one answer a hosted source owes a detail read of `id` alone: refused as malformed when
/// it gave any other number, or answered about another task, rather than sent on as the
/// answer about `id`.
fn one_detail(
    id: &NativeId,
    mut answers: Vec<Result<Option<TaskDetailRead>, SourceError>>,
) -> Result<Option<TaskDetailRead>, SourceError> {
    if answers.len() != 1 {
        return Err(SourceError::Malformed {
            message: format!(
                "the hosted source answered {} details for one read of task {id}",
                answers.len()
            ),
        });
    }
    let read = answers.pop().expect("exactly one answer")?;
    if let Some(read) = &read
        && read.task.id != *id
    {
        return Err(SourceError::Malformed {
            message: format!(
                "the hosted source answered a read of task {id} with task {}",
                read.task.id
            ),
        });
    }
    Ok(read)
}

/// One method's parameters, or the reason they could not be read.
fn decode<T: for<'de> Deserialize<'de>>(method: &str, params: Value) -> Result<T, SourceError> {
    serde_json::from_value(params).map_err(|error| SourceError::Malformed {
        message: format!("the parameters of {method} are not the shape it takes: {error}"),
    })
}

/// Let go of what the hosted source still holds from an earlier write of the item `write`
/// updates, so the content a write's result carries is that write's own (§4.9).
fn forget<T>(source: &dyn TaskSource, record: MetadataRecord, write: &ItemWrite<T>) {
    if let Some(target) = &write.target {
        let _ = source.stored_content(record, target);
    }
}

/// A content write's `result`, carrying as `stored_content` what the hosted source's backend
/// answered it stores for the item `id`, where it answered with it (§4.9).
fn stored(
    source: &dyn TaskSource,
    record: MetadataRecord,
    id: &NativeId,
    mut result: Value,
) -> Result<Value, SourceError> {
    if let (Some(content), Some(members)) =
        (source.stored_content(record, id), result.as_object_mut())
    {
        members.insert("stored_content".to_owned(), Value::String(content));
    }
    Ok(result)
}

/// One method's result as the value the envelope carries.
fn encode<T: serde::Serialize>(value: T) -> Result<Value, SourceError> {
    serde_json::to_value(value).map_err(|error| SourceError::Malformed {
        message: format!("this source returned data that will not serialize: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> TaskDetailRead {
        TaskDetailRead {
            task: serde_json::from_value(json!({"id": id, "title": "Alpha",
                "status": {"category": "todo", "name": "Todo"}, "labels": []}))
            .expect("a task"),
            comments: None,
        }
    }

    #[test]
    fn a_detail_read_of_one_task_is_held_to_one_answer_about_that_task() {
        let id = NativeId("T-1".to_owned());
        assert!(
            matches!(one_detail(&id, vec![Ok(Some(task("T-1")))]), Ok(Some(read))
            if read.task.id == id)
        );
        assert!(matches!(one_detail(&id, vec![Ok(None)]), Ok(None)));
        for (answers, named) in [
            (Vec::new(), "answered 0 details"),
            (vec![Ok(None), Ok(Some(task("T-1")))], "answered 2 details"),
            (vec![Ok(Some(task("T-2")))], "with task T-2"),
        ] {
            assert!(
                matches!(one_detail(&id, answers), Err(SourceError::Malformed { message })
                    if message.contains(named)),
                "{named}"
            );
        }
    }
}
