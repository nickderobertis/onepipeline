import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const root = fileURLToPath(new URL("../../", import.meta.url));
const project = JSON.parse(readFileSync(join(root, ".github/project.json")));
const publish = parse(readFileSync(join(root, ".github/workflows/release.yml"), "utf8")).jobs
  .publish.steps;
const installers = {
  bootstrap: project.targets.bootstrap.command,
  publish: publish.find((step) => /\bonevcs-cli@/.test(step.run ?? "")).run,
};
for (const [name, command] of Object.entries(installers)) {
  for (const [fault, value] of Object.entries({
    missing: null,
    empty: "",
    range: "^0.57.4",
    tag: "latest",
    multiple: "0.57.4\n0.58.0",
  })) {
    test(`${name} refuses a ${fault} onevcs pin before calling npm`, () => {
      const dir = mkdtempSync(
        join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "onevcs-pin-"),
      );
      try {
        mkdirSync(join(dir, ".github"));
        for (const file of ["bootstrap.sh", "read-onevcs-cli-version.sh"]) {
          const source = join(root, ".github", file);
          copyFileSync(source, join(dir, ".github", file));
        }
        if (value !== null) writeFileSync(join(dir, ".github/onevcs-cli-version"), value);
        const cache = join(dir, "npm-cache");
        const result = spawnSync(
          "bash",
          ["--noprofile", "--norc", "-eo", "pipefail", "-c", command],
          {
            cwd: dir,
            env: {
              ...process.env,
              npm_config_offline: "true",
              npm_config_cache: cache,
              RUNNER_TEMP: dir,
              GITHUB_PATH: join(dir, "github-path"),
            },
            encoding: "utf8",
          },
        );
        assert.equal(result.status, 2, result.stdout + result.stderr);
        assert.match(result.stderr, /onevcs pin/);
        assert.match(result.stderr, /ACTION: .*rerun bootstrap/);
        assert.equal(existsSync(cache), false, "npm was called before the pin was validated");
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  }
}
for (const ending of ["\n", "\r\n"]) {
  test(`the onevcs pin reader accepts an exact version with ${JSON.stringify(ending)} line endings`, () => {
    const dir = mkdtempSync(
      join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "onevcs-pin-"),
    );
    try {
      mkdirSync(join(dir, ".github"));
      writeFileSync(join(dir, ".github/onevcs-cli-version"), `0.57.4${ending}`);
      const result = spawnSync("bash", [join(root, ".github/read-onevcs-cli-version.sh")], {
        cwd: dir,
        encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "0.57.4\n");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
for (const pin of ["01.2.3", "1.02.3", "1.2.03", "1.2", "1.2.3-rc.1", ""]) {
  test(`the onevcs pin reader refuses ${JSON.stringify(pin)} and names how to recover`, () => {
    const dir = mkdtempSync(
      join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "onevcs-pin-"),
    );
    try {
      mkdirSync(join(dir, ".github"));
      writeFileSync(join(dir, ".github/onevcs-cli-version"), `${pin}\n`);
      const result = spawnSync("bash", [join(root, ".github/read-onevcs-cli-version.sh")], {
        cwd: dir,
        encoding: "utf8",
      });
      assert.equal(result.status, 2, result.stdout);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /ACTION: write a published exact stable version/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
