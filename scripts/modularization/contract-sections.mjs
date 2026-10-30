import { createHash } from "node:crypto";

// A test that reads a markdown document locates one passage of it by literal needles: a divergence
// entry by its numbered heading (`split("\n## ")` then `starts_with("45.")`), or a block by a key it
// contains. Such a read depends on the heading-bounded section the needles resolve to, not on the
// rest of the file. Where the needles do not resolve to one place, the read keeps the whole file.

/** Markdown documents whose reads are narrowed to sections. */
export const SECTIONED_DOCS = [
  "docs/contract.md",
  "docs/contract-divergences.md",
  "docs/stop-guard.md",
];

// Separators a reader splits on to walk a document's structure; they name no passage.
const STRUCTURAL = /^[\s`#*>|-]*(?:json|yaml|sh|toml|text|rust)?[\s`]*$/;

/** Heading-bounded sections; a heading's section runs to the next heading of its level or higher. */
export function sections(text) {
  const lines = text.split("\n");
  const headings = [];
  let offset = 0;
  let fenced = false;
  for (const line of lines) {
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    const heading = !fenced && /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading)
      headings.push({ level: heading[1].length, title: heading[2].trim(), start: offset });
    offset += line.length + 1;
  }
  return headings.map((heading, index) => {
    const next = headings.slice(index + 1).find((candidate) => candidate.level <= heading.level);
    return {
      ...heading,
      key: `${"#".repeat(heading.level)} ${heading.title}`,
      end: next ? next.start : text.length,
    };
  });
}

/** The section a needle names in `text`, or null when it names none or several. */
function resolve(text, all, needle) {
  const title = needle.replace(/^[\n#\s]+/, "");
  if (title) {
    const headed = all.filter((section) => section.title.startsWith(title));
    if (headed.length === 1) return headed[0];
    if (headed.length > 1) return null;
  }
  const holders = new Set();
  for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + 1)) {
    // The innermost section holding the occurrence.
    const holder = all
      .filter((section) => section.start <= at && section.end > at)
      .sort((a, b) => a.end - a.start - (b.end - b.start))[0];
    holders.add(holder ?? null);
  }
  return holders.size === 1 ? [...holders][0] : null;
}

/** The sections a read's needles scope it to, or null when the read keeps the whole document. */
export function scope(text, needles) {
  const all = sections(text);
  const naming = needles.filter((needle) => !STRUCTURAL.test(needle));
  const resolved = naming.map((needle) => resolve(text, all, needle));
  const held = [...new Set(resolved.filter(Boolean))];
  if (!held.length) return null;
  // A needle naming no single section must still fall inside the scope the others name: it then
  // refines the read within that passage rather than pointing somewhere else.
  for (const [index, needle] of naming.entries())
    if (
      !resolved[index] &&
      !held.some((section) => text.slice(section.start, section.end).includes(needle))
    )
      return null;
  // A section nested in another held section adds nothing to what the read depends on.
  return held
    .filter(
      (section) =>
        !held.some(
          (other) => other !== section && other.start <= section.start && other.end >= section.end,
        ),
    )
    .sort((a, b) => a.start - b.start);
}

/** The input path a scoped read stands for, stable across the reads that share it. */
export function scopePath(path, needles, held) {
  const digest = createHash("sha256")
    .update(JSON.stringify([...needles].sort()))
    .digest("hex")
    .slice(0, 10);
  const slug = held[0].title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 40);
  return `${path}#${slug || "section"}-${digest}`;
}

/** Whether a change from `before` to `after` can change what a read with these needles sees. */
export function scopeChanged(before, after, needles) {
  const [old, now] = [before, after].map((text) => scope(text, needles));
  if (!old || !now || old.length !== now.length) return true;
  return old.some(
    (section, index) =>
      section.key !== now[index].key ||
      before.slice(section.start, section.end) !== after.slice(now[index].start, now[index].end),
  );
}
