#!/usr/bin/env node
"use strict";

// The launcher of the `undra` npm package. The executable itself lives in a platform package
// (`@undra/cli-<platform>-<arch>`, an optional dependency npm picks by `os` and `cpu`); this file
// finds it and runs it with the caller's arguments, standard streams and exit code.

const { execFileSync } = require("node:child_process");
const { constants } = require("node:os");

const INSTALLER = "https://shreypdev.github.io/undra/install.sh";
const platform = `${process.platform}-${process.arch}`;
const packageName = `@undra/cli-${platform}`;

function fail(message) {
  process.stderr.write(`undra: ${message}\n`);
  process.exit(1);
}

// Alpine and other musl distributions report no glibc version. Only asked when something is
// already wrong, to say why: the report is slow to build.
function isMusl() {
  if (process.platform !== "linux") return false;
  try {
    return !process.report.getReport().header.glibcVersionRuntime;
  } catch {
    return false;
  }
}

let binary;
try {
  binary = require.resolve(`${packageName}/undra`);
} catch {
  const supported = new Set(["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64"]);
  if (!supported.has(platform)) {
    fail(`there is no prebuilt binary for ${platform}; see ${INSTALLER} or build from source with cargo`);
  } else if (isMusl()) {
    fail(`the ${platform} binary needs glibc and this system uses musl (Alpine); build from source with cargo (${INSTALLER})`);
  } else {
    fail(`${packageName} is not installed (was @undra/cli installed with --omit=optional?); reinstall it, or use ${INSTALLER}`);
  }
}

try {
  execFileSync(binary, process.argv.slice(2), { stdio: "inherit" });
} catch (error) {
  if (error && typeof error.status === "number") {
    process.exit(error.status);
  }
  if (error && typeof error.signal === "string") {
    process.exit(128 + (constants.signals[error.signal] || 1));
  }
  fail(`could not run ${binary}: ${error && error.message ? error.message : error}`);
}
