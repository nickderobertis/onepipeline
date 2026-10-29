import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { codeOnly } from "./graph.mjs";
import { cargoManifest, dependencyDefinitions } from "./manifest-projection.mjs";
import { declarations } from "./source-projection.mjs";

// The split names files exactly or as a directory's whole tree (dir/**); nothing else.
const claims = (pattern, file) =>
  pattern.endsWith("/**") ? file.startsWith(pattern.slice(0, -2)) : file === pattern;
const record = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const strings = (value) => Array.isArray(value) && value.every((item) => typeof item === "string");
const crateKeys = ["files", "items", "modules", "selection", "interface"];
const selectionKeys = ["everySuite", "via", "doubles", "capabilities", "helpers"];

/** Top-level items of a Rust file: declarations not nested inside another declaration. */
function topLevel(text) {
  const decls = declarations(text);
  return decls.filter(
    (decl) =>
      !decls.some((other) => other !== decl && other.start < decl.start && other.end >= decl.end),
  );
}

/**
 * The reviewed split of crates/testfakes: one crate per fake process or fake protocol, with one shared primitive
 * codec. Every file, library item, public module, binary and manifest dependency of the package must be owned, and
 * every fake a test names must be a declared binary; anything else refuses rather than being dropped or shared.
 */
export function testFakes(root = ".", path = "scripts/modularization/test-fakes.json") {
  const read = (file) => readFileSync(`${root}/${file}`, "utf8");
  const spec = JSON.parse(read(path));
  if (
    !record(spec) ||
    spec.schema !== 1 ||
    Object.keys(spec).some(
      (key) => !["schema", "package", "library", "crates", "wired", "historical"].includes(key),
    ) ||
    typeof spec.package !== "string" ||
    typeof spec.library !== "string" ||
    !record(spec.crates) ||
    !Array.isArray(spec.wired) ||
    !Array.isArray(spec.historical)
  )
    throw new Error(
      "invalid test-fake split: expected schema 1 with package, library, crates, wired, historical",
    );
  const manifestPath = `${spec.package}/Cargo.toml`;
  const manifest = cargoManifest(read(manifestPath));
  const tracked = execFileSync("git", ["ls-files", spec.package], { cwd: root, encoding: "utf8" })
    .split("\n")
    .filter(Boolean)
    .filter((file) => file !== manifestPath && file !== spec.library);
  // Cargo builds every src/bin/*.rs as a binary unless autobins is off, beside the declared [[bin]]s.
  const binPaths = Object.fromEntries([
    ...(manifest.package?.autobins === false
      ? []
      : tracked
          .map((file) => new RegExp(`^${spec.package}/src/bin/([\\w-]+)\\.rs$`).exec(file))
          .filter(Boolean)
          .map((match) => [match[1], match[0]])),
    ...(manifest.bin ?? []).map((bin) => [bin.name, `${spec.package}/${bin.path}`]),
  ]);
  const bins = Object.keys(binPaths);
  const library = read(spec.library);
  const libraryItems = topLevel(library);
  const libraryModules = [...codeOnly(library).matchAll(/^pub mod (\w+);/gm)].map(
    (match) => match[1],
  );
  const fileOwner = new Map();
  const itemOwner = new Map();
  const moduleOwner = new Map();
  const doubleOwner = new Map();
  for (const [name, entry] of Object.entries(spec.crates)) {
    if (
      !/^[a-z][a-z0-9-]*$/.test(name) ||
      !record(entry) ||
      Object.keys(entry).some((key) => !crateKeys.includes(key)) ||
      !strings(entry.files) ||
      !strings(entry.items) ||
      !strings(entry.modules) ||
      typeof entry.interface !== "string" ||
      !entry.interface.trim() ||
      !record(entry.selection) ||
      Object.keys(entry.selection).some((key) => !selectionKeys.includes(key)) ||
      !Object.keys(entry.selection).length
    )
      throw new Error(`invalid test-fake crate ${name}`);
    const { selection } = entry;
    for (const key of ["doubles", "capabilities", "helpers"])
      if (selection[key] !== undefined && (!strings(selection[key]) || !selection[key].length))
        throw new Error(`invalid test-fake selection ${name}.${key}`);
    for (const key of ["everySuite", "via"])
      if (
        selection[key] !== undefined &&
        (typeof selection[key] !== "string" || !selection[key].trim())
      )
        throw new Error(`invalid test-fake selection ${name}.${key}: expected a stated reason`);
    for (const pattern of entry.files) {
      const matches = tracked.filter((file) => claims(pattern, file));
      if (!matches.length && !pattern.endsWith("/**"))
        throw new Error(`test-fake crate ${name} names a missing file ${pattern}`);
      for (const file of matches) {
        if (fileOwner.has(file))
          throw new Error(`test-fake file ${file} is owned by ${fileOwner.get(file)} and ${name}`);
        fileOwner.set(file, name);
      }
    }
    for (const item of entry.items) {
      if (!libraryItems.some((decl) => decl.name === item))
        throw new Error(`test-fake crate ${name} names a missing library item ${item}`);
      if (itemOwner.has(item)) throw new Error(`test-fake library item ${item} is owned twice`);
      itemOwner.set(item, name);
    }
    for (const module of entry.modules) {
      if (!libraryModules.includes(module))
        throw new Error(`test-fake crate ${name} names a missing library module ${module}`);
      if (moduleOwner.has(module))
        throw new Error(`test-fake library module ${module} is owned twice`);
      moduleOwner.set(module, name);
    }
    for (const double of selection.doubles ?? []) {
      if (!bins.includes(double))
        throw new Error(`test-fake crate ${name} names an unknown binary ${double}`);
      if (doubleOwner.has(double)) throw new Error(`test-fake binary ${double} is owned twice`);
      if (fileOwner.get(binPaths[double]) !== name)
        throw new Error(
          `test-fake binary ${double} is selected by ${name} but its source is not in that crate`,
        );
      doubleOwner.set(double, name);
    }
  }
  for (const file of tracked)
    if (!fileOwner.has(file)) throw new Error(`unowned test-fake file ${file}`);
  for (const bin of bins)
    if (!doubleOwner.has(bin)) throw new Error(`unowned test-fake binary ${bin}`);
  for (const module of libraryModules)
    if (!moduleOwner.has(module)) throw new Error(`unowned test-fake library module ${module}`);
  for (const decl of libraryItems)
    if (!decl.private && !itemOwner.has(decl.name))
      throw new Error(`unowned test-fake library item ${decl.name}`);
  for (const wired of spec.wired)
    if (
      !record(wired) ||
      Object.keys(wired).some((key) => !["helper", "doubles", "reason"].includes(key)) ||
      typeof wired.helper !== "string" ||
      !/^\w+::\w+$/.test(wired.helper) ||
      !strings(wired.doubles) ||
      wired.doubles.some((double) => !doubleOwner.has(double)) ||
      typeof wired.reason !== "string" ||
      !wired.reason.trim()
    )
      throw new Error("invalid wired test-fake site");
  const historicalOwner = new Map();
  for (const entry of spec.historical) {
    if (
      !record(entry) ||
      Object.keys(entry).some((key) => !["file", "crate", "reason"].includes(key)) ||
      typeof entry.file !== "string" ||
      !entry.file.startsWith(`${spec.package}/`) ||
      tracked.includes(entry.file) ||
      !Object.hasOwn(spec.crates, entry.crate) ||
      typeof entry.reason !== "string" ||
      !entry.reason.trim()
    )
      throw new Error("invalid historical test-fake file");
    historicalOwner.set(entry.file, entry.crate);
  }
  // A fake a test names that no crate builds would select nothing; refuse it.
  const testFiles = execFileSync("git", ["ls-files", "tests"], { cwd: root, encoding: "utf8" })
    .split("\n")
    .filter((file) => file.endsWith(".rs"));
  // Raw text: erasing literals would erase the very names checked.
  for (const file of testFiles)
    for (const match of read(file).matchAll(/\bdouble\(\s*"([^"]*)"\s*\)/g))
      if (!doubleOwner.has(match[1]))
        throw new Error(`unresolved test fake ${match[1]} (read from ${file})`);
  // Code each crate owns: its files, and its library items with their private helpers.
  const fileCode = {};
  const libraryCode = {};
  for (const [name, entry] of Object.entries(spec.crates)) {
    const selected = new Set(entry.items);
    for (let grew = true; grew; ) {
      grew = false;
      for (const decl of libraryItems)
        if (
          decl.private &&
          !selected.has(decl.name) &&
          !itemOwner.has(decl.name) &&
          libraryItems.some(
            (user) => selected.has(user.name) && new RegExp(`\\b${decl.name}\\b`).test(user.code),
          )
        ) {
          selected.add(decl.name);
          grew = true;
        }
    }
    fileCode[name] = [...fileOwner]
      .filter(([, owner]) => owner === name)
      .map(([file]) => codeOnly(read(file)))
      .join("\n");
    libraryCode[name] = libraryItems
      .filter((decl) => selected.has(decl.name))
      .map((decl) => decl.code)
      .join("\n");
  }
  const dependencies = dependencyDefinitions(manifest);
  const crates = {};
  for (const [name, entry] of Object.entries(spec.crates)) {
    const text = `${fileCode[name]}\n${libraryCode[name]}`;
    const deps = new Set();
    // A file reaches the library through its crate path; a library item reaches a sibling item by bare name.
    const reaches = (symbol) =>
      new RegExp(`\\b(?:fake|onepipeline_testfakes|crate)::${symbol}\\b`).test(text) ||
      new RegExp(`(?<![\\w:])${symbol}\\b`).test(libraryCode[name]);
    for (const [item, owner] of itemOwner) if (owner !== name && reaches(item)) deps.add(owner);
    for (const [module, owner] of moduleOwner)
      if (
        owner !== name &&
        (reaches(module) || new RegExp(`\\bonepipeline_testfakes::${module}::`).test(text))
      )
        deps.add(owner);
    crates[name] = {
      files: entry.files,
      paths: [
        ...entry.files,
        ...[...historicalOwner].filter(([, owner]) => owner === name).map(([file]) => file),
        `moved/${name}/**`,
        `resolved/${name}/**`,
      ],
      deps: [...deps].sort(),
      external: [
        ...new Set(
          dependencies
            .filter((row) => new RegExp(`\\b${row.alias.replaceAll("-", "_")}::`).test(text))
            .map((row) => row.packageName),
        ),
      ].sort(),
      items: entry.items,
      modules: entry.modules,
      selection: entry.selection,
      interface: entry.interface,
    };
  }
  for (const row of dependencies)
    if (!Object.values(crates).some((crate) => crate.external.includes(row.packageName)))
      throw new Error(`unattributed test-fake dependency ${row.alias} (read from ${manifestPath})`);
  const visit = (name, chain = []) => {
    if (chain.includes(name))
      throw new Error(`test-fake crate cycle: ${[...chain, name].join(" -> ")}`);
    for (const dep of crates[name].deps) visit(dep, [...chain, name]);
  };
  for (const name of Object.keys(crates)) visit(name);
  return {
    package: spec.package,
    manifestPath,
    library: spec.library,
    crates,
    wired: spec.wired,
    historical: spec.historical,
    itemOwner,
    moduleOwner,
    doubleOwner,
    binPaths,
  };
}

/** Owners of the shared library's items and module declarations, as the source projection reads them. */
export function testFakeOwners(fakes) {
  return {
    [fakes.library]: new Map([
      ...fakes.itemOwner,
      ...[...fakes.moduleOwner].map(([module, owner]) => [`mod:${module}`, owner]),
    ]),
  };
}

/**
 * A historical change to the package manifest, charged to the crates it reaches: a dependency to the crates linking
 * it, a [[bin]] entry to the crate building that binary, and any other change (package, lib, features, or a
 * dependency no crate links any more) to every test-fake crate.
 */
export function testFakeManifestChanges(fakes, before, after) {
  const parse = (text) => (text.trim() ? cargoManifest(text) : {});
  const [a, b] = [parse(before), parse(after)];
  const owners = new Set();
  const all = Object.keys(fakes.crates);
  const rows = (manifest) =>
    Object.fromEntries(
      dependencyDefinitions(manifest).map((row) => [`${row.scope}:${row.alias}`, row]),
    );
  const [ra, rb] = [rows(a), rows(b)];
  for (const key of new Set([...Object.keys(ra), ...Object.keys(rb)])) {
    if (JSON.stringify(ra[key]?.value) === JSON.stringify(rb[key]?.value)) continue;
    const packageName = (rb[key] ?? ra[key]).packageName;
    const users = all.filter((name) => fakes.crates[name].external.includes(packageName));
    for (const name of users.length ? users : all) owners.add(name);
  }
  const bins = (manifest) => Object.fromEntries((manifest.bin ?? []).map((bin) => [bin.name, bin]));
  const [ba, bb] = [bins(a), bins(b)];
  for (const name of new Set([...Object.keys(ba), ...Object.keys(bb)]))
    if (JSON.stringify(ba[name]) !== JSON.stringify(bb[name]))
      for (const owner of [
        fakes.doubleOwner.get(name) ??
          fakes.historical.find((entry) => entry.file.endsWith(`/${name}.rs`))?.crate,
      ].flatMap((found) => (found ? [found] : all)))
        owners.add(owner);
  const others = (manifest) => {
    const {
      dependencies,
      "dev-dependencies": dev,
      "build-dependencies": build,
      target,
      bin,
      ...rest
    } = manifest;
    return JSON.stringify({ ...rest, target: Object.keys(target ?? {}).sort() });
  };
  if (others(a) !== others(b)) for (const name of all) owners.add(name);
  return [...owners].sort().map((name) => `moved/${name}/${fakes.manifestPath}`);
}
