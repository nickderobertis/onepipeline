//! Free space on the filesystems a host's runs live in, for the two views that
//! report what is running and said nothing about what it is running *in*.
//!
//! What this exists for is one measured incident, recorded in entry 86 of
//! `docs/contract-divergences.md`: a host at 197G/197G reported two unrelated
//! test failures, and nothing a supervisor read named the disk. Two roots are
//! measured — the runs root every view reads, and the directory the linked
//! `onevcs` cuts every lifecycle worktree and clone under, which is the one that
//! filled — and a filesystem holding both is one line naming both, because two
//! lines for one device read as two answers about two resources.
//!
//! Everything here **reads**, and a reading is never acted on: the space is
//! host-wide on a host several managers share, and clearing it belongs to
//! whoever owns what is filling it.

use std::path::{Path, PathBuf};

/// The indentation both views give the lines inside a block, so a reading sits
/// beside the engine's own lines rather than as a second document.
const INDENT: &str = "  ";

/// What the line opens with, which is what a reader greps for.
const LEAD: &str = "free space:";

/// The label a root is named under on the line.
const RUNS_ROOT: &str = "the runs root";
const WORKSPACES: &str = "the lifecycle workspaces under";

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The `free space:` lines both views carry: one per distinct filesystem among
/// the runs root and the linked `onevcs`'s workspaces root, each with its
/// terminator.
///
/// A root that does not exist yet is measured at its nearest existing ancestor
/// and the line says which — a fresh checkout whose `runs/` has never been
/// written is not a host whose free space is unknown. A filesystem that cannot
/// be read says so on a line of its own rather than being left out, and so does
/// a workspaces root the linked sibling could not resolve: silence is the one
/// answer a reading of the resource whose exhaustion stops everything may not
/// give.
pub(crate) fn lines(runs_root: &Path) -> String {
    let mut asked: Vec<(&str, PathBuf)> = vec![(RUNS_ROOT, runs_root.to_path_buf())];
    match workspaces_root() {
        Ok(root) => asked.push((WORKSPACES, root)),
        Err(why) => {
            return format!(
                "{}{INDENT}{LEAD} could not be read for the lifecycle workspaces, because the \
                 linked onevcs could not resolve its state root: {}, so what is free there is \
                 unknown\n",
                lines_of(&asked),
                crate::views::one_line(&why)
            )
        }
    }
    lines_of(&asked)
}

/// One line per distinct filesystem among `asked`, in the order first named.
fn lines_of(asked: &[(&str, PathBuf)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    // Which line each filesystem already has, keyed on what tells two apart
    // rather than on the path: the runs root and the workspaces are on one
    // device on the host this was written for and need not be on another.
    let mut at: Vec<(Identity, usize)> = Vec::new();
    for (label, path) in asked {
        let named = format!(
            "{label} {}",
            crate::views::one_line(&path.display().to_string())
        );
        match Filesystem::holding(path) {
            Err(why) => lines.push(format!(
                "{INDENT}{LEAD} could not be read for {named}: {}, so what is free there is \
                 unknown\n",
                crate::views::one_line(&why)
            )),
            Ok(filesystem) => {
                let named = match &filesystem.measured_at {
                    Some(ancestor) => format!(
                        "{named} (measured at {}, the nearest directory that exists)",
                        crate::views::one_line(&ancestor.display().to_string())
                    ),
                    None => named,
                };
                if let Some((_, index)) = at
                    .iter()
                    .find(|(identity, _)| *identity == filesystem.identity)
                {
                    let line = &mut lines[*index];
                    line.truncate(line.len().saturating_sub(1));
                    line.push_str(&format!(" and {named}\n"));
                    continue;
                }
                at.push((filesystem.identity.clone(), lines.len()));
                lines.push(format!(
                    "{INDENT}{LEAD} {} on the filesystem holding {named}\n",
                    filesystem.reading()
                ));
            }
        }
    }
    lines.concat()
}

/// One filesystem, as measured for one root.
#[derive(Debug)]
struct Filesystem {
    /// What tells this filesystem from another.
    identity: Identity,
    /// The ancestor the root was measured at, when the root itself is not there.
    measured_at: Option<PathBuf>,
    /// Bytes an unprivileged writer may still use.
    free: u64,
    /// The filesystem's size, in bytes.
    total: u64,
}

impl Filesystem {
    /// The filesystem `path` sits on — or, where `path` is not there yet, the one
    /// its nearest existing ancestor sits on.
    ///
    /// The walk up is the answer rather than a fallback: a root nothing has
    /// written yet still sits on a filesystem, and that filesystem's free space
    /// is the number a supervisor wants.
    fn holding(path: &Path) -> Result<Self, String> {
        let mut measured = path.to_path_buf();
        loop {
            match measure(&measured) {
                // Windows answers `NotFound` for a component under a file where
                // Unix answers "not a directory", so the walk can reach a file:
                // that is the same refusal, and the file is no directory a root
                // could be created in.
                Ok(_) if measured != path && !measured.is_dir() => {
                    return Err(format!("{}: not a directory", measured.display()))
                }
                Ok((identity, free, total)) => {
                    return Ok(Self {
                        identity,
                        measured_at: (measured != path).then_some(measured),
                        free,
                        total,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let Some(parent) = measured.parent().map(Path::to_path_buf) else {
                        return Err(format!("{}: {error}", path.display()));
                    };
                    // A relative root whose parent is the empty path is measured
                    // at the working directory, which is what it is relative to.
                    measured = if parent.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        parent
                    };
                }
                Err(error) => return Err(format!("{}: {error}", measured.display())),
            }
        }
    }

    /// `41.2 GiB of 196.9 GiB (21% free)`.
    fn reading(&self) -> String {
        // Integer arithmetic for the share, so a host at 197G/197G reads `0%`
        // rather than rounding up to something that is not full.
        let share = if self.total == 0 {
            0
        } else {
            u128::from(self.free) * 100 / u128::from(self.total)
        };
        format!(
            "{:.1} GiB of {:.1} GiB ({share}% free)",
            // Precision is a rendering: the lines are read by a person and a
            // byte count carries nothing a tenth of a gibibyte does not.
            self.free as f64 / GIB,
            self.total as f64 / GIB
        )
    }
}

/// What tells one filesystem from another on this host.
///
/// The device on Unix and the volume's mount path on Windows: two roots on one
/// filesystem must produce one line, and the free space alone would collapse
/// two genuinely different filesystems that happen to match.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity(String);

/// The directory the linked `onevcs` cuts every per-run clone and isolated
/// worktree under, resolved as that library resolves its own state root —
/// `ONEVCS_HOME` when it is set to something non-empty, `~/.onevcs` otherwise.
///
/// The state root is reached through the sibling's published resolution rather
/// than restated: `onevcs::workspaces::default_path` is that root's own sizing
/// file. The `workspaces` name beside it is the one piece the sibling does not
/// publish, so it is held to the sibling's behaviour instead:
/// `status_and_host_report_free_space_on_the_filesystem_holding_both_roots`
/// drives a real lifecycle node and fails when the checkout the linked `onevcs`
/// cut for it is not under the root this names.
fn workspaces_root() -> Result<PathBuf, String> {
    let sizing = onevcs::workspaces::default_path().map_err(|error| error.to_string())?;
    let root = sizing
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", sizing.display()))?;
    Ok(root.join("workspaces"))
}

/// Ask the host what filesystem `path` is on and what is left of it.
///
/// `NotFound` is the one refusal the caller walks past; any other is the host
/// declining to say, and is reported as that.
#[cfg(unix)]
fn measure(path: &Path) -> std::io::Result<(Identity, u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let device = std::fs::metadata(path)?.dev();
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated path and `stat` is writable
    // storage of exactly the type `statvfs` fills; the value is read only after
    // the call reports success.
    let status = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if status != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a zero return is the kernel's promise that the whole struct was
    // written.
    let stat = unsafe { stat.assume_init() };
    // Widened through `u128` because the fields' widths differ by platform, and
    // narrowed back to the bound a byte count has to fit.
    let block = u128::from(stat.f_frsize);
    let free = u64::try_from(u128::from(stat.f_bavail) * block).unwrap_or(u64::MAX);
    let total = u64::try_from(u128::from(stat.f_blocks) * block).unwrap_or(u64::MAX);
    Ok((Identity(device.to_string()), free, total))
}

/// Ask the host what volume `path` is on and what is left of it.
///
/// A path that is not there answers `NotFound` from the metadata read, so the
/// caller walks up exactly as it does on Unix.
#[cfg(windows)]
fn measure(path: &Path) -> std::io::Result<(Identity, u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumePathNameW};

    std::fs::metadata(path)?;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut volume = vec![0u16; wide.len().max(261)];
    // SAFETY: `wide` is NUL-terminated and `volume` is a writable buffer whose
    // length is passed beside it.
    let named = unsafe {
        GetVolumePathNameW(
            wide.as_ptr(),
            volume.as_mut_ptr(),
            u32::try_from(volume.len()).unwrap_or(u32::MAX),
        )
    };
    if named == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let end = volume.iter().position(|&c| c == 0).unwrap_or(volume.len());
    let mount = String::from_utf16_lossy(&volume[..end]);
    let mut free = 0u64;
    let mut total = 0u64;
    let mut total_free = 0u64;
    // SAFETY: `volume` is NUL-terminated and the three out-parameters are
    // writable `u64`s.
    let read = unsafe {
        GetDiskFreeSpaceExW(
            volume.as_ptr(),
            &raw mut free,
            &raw mut total,
            &raw mut total_free,
        )
    };
    if read == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((Identity(mount), free, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both roots on one filesystem — a scratch directory and one beside it —
    /// are one line naming both, and a root that is not there yet is measured
    /// at its nearest existing ancestor and says so.
    #[test]
    fn one_filesystem_is_one_line_and_a_missing_root_is_measured_at_its_ancestor() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-freespace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        let runs = root.join("runs");
        let workspaces = root.join("not-there-yet").join("workspaces");
        let rendered = lines_of(&[(RUNS_ROOT, runs.clone()), (WORKSPACES, workspaces.clone())]);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(rendered.lines().count(), 1, "{rendered}");
        assert!(rendered.starts_with("  free space: "), "{rendered}");
        assert!(rendered.contains(" GiB of "), "{rendered}");
        assert!(
            rendered.contains("% free) on the filesystem holding the runs root"),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!(
                "{} (measured at {}, the nearest directory that exists)",
                runs.display(),
                root.display()
            )),
            "{rendered}"
        );
        assert!(rendered.contains(&format!("and the lifecycle workspaces under {} (measured at {}, the nearest directory that exists)", workspaces.display(), root.display())), "{rendered}");
        assert!(rendered.ends_with('\n'), "{rendered}");
    }

    /// A root whose path cannot be a filesystem path at all is said to be
    /// unreadable on its own line, and the other root is still measured.
    #[test]
    fn a_root_that_cannot_be_read_says_so_and_the_other_is_still_measured() {
        // A file where a directory is wanted: every ancestor walk stops at it and
        // asking the host about a component *under* a file is refused with
        // something other than `NotFound`.
        let root =
            std::env::temp_dir().join(format!("onepipeline-freespace-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        let file = root.join("a-file");
        std::fs::write(&file, "not a directory").expect("a file");
        let under = file.join("runs");
        let rendered = lines_of(&[(RUNS_ROOT, under.clone()), (WORKSPACES, root.clone())]);
        let _ = std::fs::remove_dir_all(&root);

        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2, "{rendered}");
        assert!(
            lines[0].starts_with(&format!(
                "  free space: could not be read for the runs root {}",
                under.display()
            )),
            "{rendered}"
        );
        assert!(
            lines[0].ends_with("so what is free there is unknown"),
            "{rendered}"
        );
        assert!(
            lines[1].contains("on the filesystem holding the lifecycle workspaces under"),
            "{rendered}"
        );
    }

    /// A full filesystem reads `0%`, never a rounded-up share.
    #[test]
    fn the_share_is_integer_and_a_full_filesystem_reads_zero() {
        let full = Filesystem {
            identity: Identity("1".into()),
            measured_at: None,
            free: 1024 * 1024,
            total: 197 * 1024 * 1024 * 1024,
        };
        assert_eq!(full.reading(), "0.0 GiB of 197.0 GiB (0% free)");
        let empty = Filesystem {
            identity: Identity("1".into()),
            measured_at: None,
            free: 0,
            total: 0,
        };
        assert_eq!(empty.reading(), "0.0 GiB of 0.0 GiB (0% free)");
    }
}
