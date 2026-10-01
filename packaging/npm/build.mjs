#!/usr/bin/env node
// Generates the npm packages of the `undra` command from packaging/npm/templates/ and the release
// tarballs (packaging/pack-release.sh): no dependencies, Node 20 or newer.
//
//   node packaging/npm/build.mjs --artifacts <dir> --out <dir>
//
// Output, one directory per package under <out>:
//
//   cli-<platform>/   @undra/cli-<platform>   the `undra` binary (darwin-arm64, darwin-x64,
//                                              linux-x64, linux-arm64), restricted by os / cpu
//   cli/              @undra/cli              the launcher (bin/undra.js); the four platform
//                                              packages are optionalDependencies at this exact
//                                              version
//   undra/            undra                   the same launcher under the unscoped name, which
//                                              keeps `npm install -g undra` ours
//
// Publish the platform packages first and the two launchers last, so that a launcher never
// exists without the packages it depends on.

import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../..");
const templates = join(here, "templates");

// `machine` is what the binary's header must say: ELF e_machine, or the Mach-O cputype.
const PLATFORMS = [
  { id: "darwin-arm64", os: "darwin", cpu: "arm64", target: "aarch64-apple-darwin", label: "macOS on Apple silicon", format: "macho", machine: 0x0100000c },
  { id: "darwin-x64", os: "darwin", cpu: "x64", target: "x86_64-apple-darwin", label: "macOS on Intel", format: "macho", machine: 0x01000007 },
  { id: "linux-x64", os: "linux", cpu: "x64", target: "x86_64-unknown-linux-gnu", label: "Linux on x64 (glibc)", format: "elf", machine: 0x3e },
  { id: "linux-arm64", os: "linux", cpu: "arm64", target: "aarch64-unknown-linux-gnu", label: "Linux on arm64 (glibc)", format: "elf", machine: 0xb7 },
];

const USAGE = `Usage: node packaging/npm/build.mjs --artifacts <dir> --out <dir> [options]

Builds the @undra/cli-<platform> packages, @undra/cli and the unscoped undra package into <out>.

  --artifacts <dir>   directory with undra-v<version>-<target>.tar.gz for every platform
  --out <dir>         output directory (the package directories inside it are replaced)
  --version <semver>  the version to expect; default: the version in packaging/npm/templates
                      (which scripts/bump-version.sh keeps equal to the workspace version)
  --only <ids>        comma-separated platform ids to build (${PLATFORMS.map((p) => p.id).join(", ")});
                      for local tests only, a release builds all four
  -h, --help          print this text

Exit status: 0 on success, 1 on a failed build, 2 on a usage error.`;

class UsageError extends Error {}

function parseArgs(argv) {
  const args = { artifacts: undefined, out: undefined, version: undefined, only: undefined };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new UsageError(`${arg} needs a value`);
      return argv[++i];
    };
    switch (arg) {
      case "-h":
      case "--help":
        process.stdout.write(`${USAGE}\n`);
        process.exit(0);
        break;
      case "--artifacts":
        args.artifacts = value();
        break;
      case "--out":
        args.out = value();
        break;
      case "--version":
        args.version = value();
        break;
      case "--only":
        args.only = value().split(",").filter(Boolean);
        break;
      default:
        throw new UsageError(`unknown argument: ${arg}`);
    }
  }
  if (!args.artifacts || !args.out) throw new UsageError("--artifacts and --out are required");
  return args;
}

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
const writeJson = (path, value) => writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
const fill = (text, values) => text.replace(/@@([A-Z]+)@@/g, (_, key) => {
  if (!(key in values)) throw new Error(`template placeholder @@${key}@@ has no value`);
  return values[key];
});

// The bytes of a release binary must be what its platform package claims: the matrix builds four
// targets, and shipping the wrong one under a label would only show up on a user's machine.
function checkBinary(platform, path) {
  const head = readFileSync(path).subarray(0, 32);
  if (platform.format === "elf") {
    const ok = head.length >= 20 && head.readUInt32BE(0) === 0x7f454c46 && head.readUInt16LE(18) === platform.machine;
    if (!ok) throw new Error(`${path} is not an ELF executable for ${platform.target}`);
  } else {
    const ok = head.length >= 8 && head.readUInt32LE(0) === 0xfeedfacf && head.readUInt32LE(4) === platform.machine;
    if (!ok) throw new Error(`${path} is not a Mach-O executable for ${platform.target}`);
  }
}

function extractBinary(tarball, scratch) {
  mkdirSync(scratch, { recursive: true });
  execFileSync("tar", ["-xzf", tarball, "-C", scratch, "undra"], { stdio: ["ignore", "inherit", "inherit"] });
  const binary = join(scratch, "undra");
  if (!statSync(binary).isFile()) throw new Error(`${tarball} has no undra executable`);
  return binary;
}

function copyLicenses(dest) {
  for (const name of ["LICENSE-MIT", "LICENSE-APACHE"]) {
    copyFileSync(join(repoRoot, name), join(dest, name));
  }
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const cliTemplate = readJson(join(templates, "cli/package.json"));
  const platformTemplate = readJson(join(templates, "platform/package.json"));
  if (cliTemplate.version !== platformTemplate.version) {
    throw new Error(
      `the templates disagree: cli ${cliTemplate.version}, platform ${platformTemplate.version}; run scripts/bump-version.sh`,
    );
  }
  const version = cliTemplate.version;
  if (args.version && args.version !== version) {
    throw new Error(`expected version ${args.version} but packaging/npm/templates say ${version}; run scripts/bump-version.sh ${args.version}`);
  }

  const selected = args.only
    ? args.only.map((id) => {
        const found = PLATFORMS.find((p) => p.id === id);
        if (!found) throw new UsageError(`unknown platform id ${id}`);
        return found;
      })
    : PLATFORMS;

  const artifacts = resolve(args.artifacts);
  const out = resolve(args.out);
  const scratch = mkdtempSync(join(tmpdir(), "undra-npm-"));
  try {
    // Find and check every binary before writing anything.
    const binaries = new Map();
    for (const platform of selected) {
      const tarball = join(artifacts, `undra-v${version}-${platform.target}.tar.gz`);
      if (!existsSync(tarball)) {
        const have = existsSync(artifacts) ? readdirSync(artifacts).join(", ") : "(directory missing)";
        throw new Error(`missing ${tarball}; the directory has: ${have || "(nothing)"}`);
      }
      const binary = extractBinary(tarball, join(scratch, platform.id));
      checkBinary(platform, binary);
      binaries.set(platform.id, binary);
    }

    mkdirSync(out, { recursive: true });
    const built = [];
    const emit = (dir, pkg, prepare) => {
      const dest = join(out, dir);
      rmSync(dest, { recursive: true, force: true });
      mkdirSync(dest, { recursive: true });
      writeJson(join(dest, "package.json"), pkg);
      copyLicenses(dest);
      prepare(dest);
      built.push({ name: pkg.name, dir: dest });
    };

    for (const platform of selected) {
      const values = { PLATFORM: platform.id, LABEL: platform.label, OS: platform.os, CPU: platform.cpu };
      const pkg = JSON.parse(fill(JSON.stringify(platformTemplate), values));
      // npm skips a package whose libc does not match: musl (Alpine) cannot run these binaries.
      if (platform.os === "linux") pkg.libc = ["glibc"];
      emit(`cli-${platform.id}`, pkg, (dest) => {
        copyFileSync(binaries.get(platform.id), join(dest, "undra"));
        chmodSync(join(dest, "undra"), 0o755);
        writeFileSync(join(dest, "README.md"), fill(readFileSync(join(templates, "platform/README.md"), "utf8"), values));
      });
    }

    // The launcher always names all four platform packages, at this exact version, so npm
    // installs the one that matches and ignores the rest; `--only` does not change that.
    const optionalDependencies = Object.fromEntries(PLATFORMS.map((p) => [`@undra/cli-${p.id}`, version]));
    for (const [dir, name] of [["cli", "@undra/cli"], ["undra", "undra"]]) {
      emit(dir, { ...cliTemplate, name, optionalDependencies }, (dest) => {
        mkdirSync(join(dest, "bin"));
        copyFileSync(join(templates, "cli/bin/undra.js"), join(dest, "bin/undra.js"));
        chmodSync(join(dest, "bin/undra.js"), 0o755);
        copyFileSync(join(templates, "cli/README.md"), join(dest, "README.md"));
      });
    }

    process.stdout.write(`Built ${built.length} packages at version ${version} in ${out}\n`);
    for (const { name, dir } of built) process.stdout.write(`  ${name}@${version}  ${dir}\n`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

try {
  main();
} catch (error) {
  if (error instanceof UsageError) {
    process.stderr.write(`build.mjs: ${error.message}\n\n${USAGE}\n`);
    process.exit(2);
  }
  process.stderr.write(`build.mjs: ${error.message}\n`);
  process.exit(1);
}
