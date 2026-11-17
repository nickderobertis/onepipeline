//! Walking one source's stream, and merging the streams into the caller's page.
//!
//! # What "transient" means here, exactly
//!
//! A walk holds the caller's page plus **one** page of the source it is walking, and
//! nothing else. It never writes to a file, a database, an index or a value that
//! outlives the call: [`Fetched`] is a local, it is consumed by the merge below, and
//! what survives the response is the caller's page and an opaque resume state. That is
//! the whole of the mechanism behind "no work data outside a plugin" — the engine
//! re-asks rather than remembers, which is also why the same query asked twice reaches
//! the source twice.

use std::future::Future;

use onetaskgraph_plugin_api::{Cursor, Page, SourceError, SourceName};

use super::resume::{Owed, Resume, StreamKind, StreamState};

/// One surviving row, with the state that would deliver it first.
pub(crate) struct Row<T> {
    pub item: T,
    /// Where a walk resumes in order to hand *this* row back first, which is what makes a
    /// page boundary expressible in the middle of a source page the engine narrowed.
    pub resume: Resume,
}

/// What one walk of one stream produced.
pub(crate) struct Fetched<T> {
    /// In the source's own order, and narrowed already: what the engine compensated for
    /// never reaches the merge.
    pub rows: Vec<Row<T>>,
    /// The cursor after the last page pulled, or `None` when the stream is exhausted.
    pub after: Option<Cursor>,
}

/// One source's stream, ready to merge.
pub(crate) struct Stream<T> {
    pub source: SourceName,
    /// A source contributes more than one stream to `search --kind both`, so the pair is
    /// the key a resume state is stored under rather than the source alone.
    pub kind: StreamKind,
    pub fetched: Fetched<T>,
}

/// Walk one stream until the caller's page could be filled or the source runs out.
///
/// `page_size` is what each request asks the source for, and it is what bounds the
/// memory this holds: one page of it, plus at most `budget` rows on the way to the
/// caller. A caller pushing every predicate down asks for exactly what it needs; a
/// caller compensating asks for the source's ceiling, because it cannot know how many
/// rows of a page will survive.
///
/// # Errors
///
/// Returns whatever the source returned, and [`SourceError::Malformed`] for a source
/// that hands back the cursor it was given — which would otherwise be an endless walk.
pub(crate) async fn walk<T, K, F, Fut>(
    start: &Resume,
    budget: u32,
    page_size: u32,
    mut keep: K,
    mut fetch: F,
) -> Result<Fetched<T>, SourceError>
where
    K: FnMut(&T) -> bool,
    F: FnMut(Option<Cursor>, u32) -> Fut,
    Fut: Future<Output = Result<Page<T>, SourceError>>,
{
    let mut rows: Vec<Row<T>> = Vec::new();
    let mut cursor = start.cursor.clone();
    let mut skip = start.skip;

    let asked_for = page_size.max(1);
    loop {
        let asked = cursor.clone();
        let page = fetch(asked.clone(), asked_for).await?;
        fits(page.items.len(), asked_for)?;
        unrepeated(
            page.next.as_ref(),
            asked.as_ref(),
            "one of its streams was being walked",
        )?;

        let mut ordinal = 0u32;
        for item in page.items {
            if !keep(&item) {
                continue;
            }
            let resume = Resume {
                cursor: asked.clone(),
                skip: ordinal,
            };
            ordinal = ordinal.saturating_add(1);
            // Rows this walk already handed back on an earlier page. Counted among the
            // survivors, because surviving rows are what the resume state counts.
            if skip > 0 {
                skip -= 1;
                continue;
            }
            rows.push(Row { item, resume });
        }

        cursor = page.next;
        if cursor.is_none() || rows.len() as u64 >= u64::from(budget) {
            break;
        }
    }

    Ok(Fetched {
        rows,
        after: cursor,
    })
}

/// Refuse a page larger than the one that was asked for.
///
/// "A source may return fewer, never more" is the contract's own rule, and until now it
/// was a rule the engine trusted rather than checked. A source is external code — a
/// subprocess-hosted plugin is somebody else's program — and the one thing an over-long
/// page breaks is the bound this whole file exists to hold: one source page plus the
/// caller's, and nothing else, ever in memory at once.
///
/// Refused rather than truncated. Dropping the excess would narrow a result set silently,
/// which is the one failure mode the engine's compensation cannot repair, and it would
/// hide a plugin defect from the person who could fix it.
///
/// # Errors
///
/// Returns [`SourceError::Malformed`] when `returned` exceeds `asked_for`.
pub(crate) fn fits(returned: usize, asked_for: u32) -> Result<(), SourceError> {
    if returned as u64 <= u64::from(asked_for) {
        return Ok(());
    }
    Err(SourceError::Malformed {
        message: format!(
            "the source returned {returned} rows for a page of at most {asked_for}; a \
             source may return fewer than it was asked for and never more"
        ),
    })
}

/// Refuse a source that answers a cursor with the cursor it was given.
///
/// A source handed back its own cursor is a source the loop that asked would ask again
/// with the same cursor, for ever: the command never ends, and from outside it is
/// indistinguishable from one still working. Saying so names the defect instead.
///
/// **One pair, and no more — which is why it is not called `advances`.** A source cycling
/// through two cursors passes every time, and catching that needs memory a walk here does
/// not keep. It is caught one level up instead, as a page token handed back unchanged to
/// the copy's walk of a project's members.
///
/// One implementation for every pagination loop in this engine, because two spellings of
/// one refusal drift apart. `scan` names what was being walked. Generic over what the
/// loop pages by: a source [`Cursor`], or this engine's own page token where a copy walks
/// a verb of it — the same defect one level up.
///
/// # Errors
///
/// Returns [`SourceError::Malformed`] when `returned` is `Some` and equal to `asked`. A
/// `returned` of `None` ends the walk, so there is no cursor to have been repeated.
pub(crate) fn unrepeated<T: PartialEq>(
    returned: Option<&T>,
    asked: Option<&T>,
    scan: &str,
) -> Result<(), SourceError> {
    if returned.is_none() || returned != asked {
        return Ok(());
    }
    Err(SourceError::Malformed {
        message: format!(
            "the source returned the cursor it was given while {scan}, so the walk would \
             never end"
        ),
    })
}

/// Interleave the streams into one page, and say where each stream picks up.
///
/// **The order across sources is round-robin**: one row from each selected source in
/// configured-name order, then the next from each, until the page is full or every
/// stream is spent. Within a source the source's own order is preserved exactly.
///
/// Round-robin rather than one source after another because the alternative makes every
/// source but the first useless work on every page: with source-major order a page
/// filled entirely by the first source still costs a call to each of the others, which
/// against a hosted source is a request spent on rows nobody will see. Interleaving
/// means every row fetched is a row that can be shown.
///
/// The rounds continue **across page boundaries**, which is why `resume_first` exists: a
/// page of three rows over two streams leaves the second stream's turn owed, and a next
/// page that began its rounds at the first stream again would hand back two of that
/// stream's rows in a row. The walk would still return every row exactly once, but the
/// sequence it returned them in would depend on the page size the caller happened to
/// choose — so `--limit 3` and `--limit 4` would interleave the same rows differently.
///
/// Each stream is walked strictly forwards, so a walk to exhaustion returns every row
/// exactly once whatever the page size, it returns them in the same order whatever the
/// page size, and repeating a walk returns the same sequence.
pub(crate) fn merge<T>(
    streams: Vec<Stream<T>>,
    budget: u32,
    resume_first: Option<&Owed>,
) -> (Vec<(SourceName, T)>, Vec<StreamState>, Option<Owed>) {
    let count = streams.len();
    let mut sources: Vec<SourceName> = Vec::with_capacity(count);
    let mut kinds: Vec<StreamKind> = Vec::with_capacity(count);
    let mut afters: Vec<Option<Cursor>> = Vec::with_capacity(count);
    // Each row behind an `Option` so the interleave can move one out by position
    // without disturbing the ones after it.
    let mut rows: Vec<Vec<Option<Row<T>>>> = Vec::with_capacity(count);
    for stream in streams {
        sources.push(stream.source);
        kinds.push(stream.kind);
        afters.push(stream.fetched.after);
        rows.push(stream.fetched.rows.into_iter().map(Some).collect());
    }

    // Where this page's rounds begin. A page boundary can fall in the middle of a round,
    // and the token says whose turn was owed; starting there is what makes the whole walk
    // alternate rather than each page alternating from the first stream again. A stream
    // the previous page exhausted is not here any more, and a fresh query names none, so
    // both fall back to the first stream — which is where an even round starts anyway.
    let start = resume_first
        .and_then(|owed| {
            (0..count).find(|&position| {
                sources[position] == owed.source && kinds[position] == owed.stream
            })
        })
        .unwrap_or(0);

    let mut taken = vec![0usize; count];
    let mut items: Vec<(SourceName, T)> = Vec::new();
    let mut round = 0usize;
    // The stream whose turn the budget cut short, so the next page can pick its rounds up
    // there. `None` once every stream has given every row it holds, which ends a round
    // evenly by construction.
    let mut owed: Option<usize> = None;
    'page: loop {
        let mut progressed = false;
        for step in 0..count {
            let position = (start + step) % count;
            if rows[position].len() <= round {
                continue;
            }
            progressed = true;
            if items.len() as u64 >= u64::from(budget) {
                owed = Some(position);
                break 'page;
            }
            let row = rows[position][round]
                .take()
                .expect("a round-robin takes each row once");
            items.push((sources[position].clone(), row.item));
            taken[position] += 1;
        }
        if !progressed {
            break;
        }
        round += 1;
    }

    let mut states = Vec::new();
    for position in 0..count {
        // The first row this page did not hand back, or the page after the last one
        // pulled. A stream with neither is exhausted and leaves the token entirely.
        let resume = match rows[position]
            .get(taken[position])
            .and_then(|row| row.as_ref())
        {
            Some(row) => Some(row.resume.clone()),
            None => afters[position].as_ref().map(|cursor| Resume {
                cursor: Some(cursor.clone()),
                skip: 0,
            }),
        };
        if let Some(resume) = resume {
            states.push(StreamState {
                source: sources[position].clone(),
                stream: kinds[position],
                resume,
            });
        }
    }

    // Only when that stream still has somewhere to resume from: a stream this page cut
    // short has, by construction, but saying so here means the answer cannot name one the
    // next page has nothing to give.
    let next = owed
        .map(|position| Owed {
            source: sources[position].clone(),
            stream: kinds[position],
        })
        .filter(|owed| {
            states
                .iter()
                .any(|state| state.source == owed.source && state.stream == owed.stream)
        });

    (items, states, next)
}
