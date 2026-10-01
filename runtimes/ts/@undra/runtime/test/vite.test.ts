import { EventEmitter } from "node:events";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
  UNDRA_INSTALL_COMMAND,
  UndraBuildError,
  corePathOf,
  findCoreLayout,
  findUndra,
  installDirs,
  isWatched,
  undra,
  undraNotFoundMessage,
  watchTargets,
  type ViteConfigLike,
  type ViteDevServerLike,
  type VitePayload,
} from "../src/vite.js";

/*
 * The Vite plugin that runs `undra build --platform web` (docs/DEV_LOOP.md, "no manual build step"). The
 * `undra` it runs is a script in a temporary directory: it records its arguments and exits with the
 * status the test chose, so nothing here compiles Rust.
 */

let dir: string;
let log: string;
// The scripts are written once for the whole file: a freshly written executable is scanned by the
// operating system the first time it runs, which costs about a second each.
let binDir: string;
let bin: string;
let slowBin: string;
const restore: Array<() => void> = [];

beforeAll(() => {
  binDir = mkdtempSync(join(tmpdir(), "undra-vite-bin-"));
  bin = join(binDir, "undra");
  writeFileSync(
    bin,
    `#!/bin/sh\necho "$@" >> "$FAKE_UNDRA_LOG"\nif [ -n "$FAKE_UNDRA_STDERR" ]; then echo "$FAKE_UNDRA_STDERR" >&2; fi\nexit \${FAKE_UNDRA_EXIT:-0}\n`,
  );
  slowBin = join(binDir, "slow-undra");
  // It records its start and end, so two builds at once would show.
  writeFileSync(slowBin, `#!/bin/sh\necho "start" >> "$FAKE_UNDRA_LOG"\nsleep 0.15\necho "end" >> "$FAKE_UNDRA_LOG"\n`);
  chmodSync(bin, 0o755);
  chmodSync(slowBin, 0o755);
});

afterAll(() => rmSync(binDir, { recursive: true, force: true }));

/** A project: `undra.toml` and `core/src/lib.rs`; `bin` appends its arguments to `log`. */
function makeProject(core = "core"): { root: string; bin: string; web: string } {
  const root = join(dir, "app");
  const web = join(root, "web");
  mkdirSync(join(root, core, "src"), { recursive: true });
  mkdirSync(web, { recursive: true });
  writeFileSync(join(root, "undra.toml"), `[project]\nname = "app"\nid = "com.example.app"\n\n[core]\npath = "${core}"\n`);
  writeFileSync(join(root, core, "src", "lib.rs"), "// core\n");
  writeFileSync(join(root, core, "Cargo.toml"), '[package]\nname = "app-core"\n');
  return { root, bin, web };
}

function setEnv(name: string, value: string | undefined): void {
  const before = process.env[name];
  if (value === undefined) delete process.env[name];
  else process.env[name] = value;
  restore.push(() => {
    if (before === undefined) delete process.env[name];
    else process.env[name] = before;
  });
}

function calls(): string[] {
  try {
    return readFileSync(log, "utf8")
      .trim()
      .split("\n")
      .filter((l) => l !== "");
  } catch {
    return [];
  }
}

class Logger {
  readonly infos: string[] = [];
  readonly warns: string[] = [];
  readonly errors: string[] = [];
  info(m: string): void {
    this.infos.push(m);
  }
  warn(m: string): void {
    this.warns.push(m);
  }
  error(m: string): void {
    this.errors.push(m);
  }
}

function fakeServer(): { server: ViteDevServerLike; watcher: EventEmitter; added: string[]; sent: VitePayload[] } {
  const watcher = new EventEmitter();
  const added: string[] = [];
  const sent: VitePayload[] = [];
  const server: ViteDevServerLike = {
    watcher: {
      add(paths) {
        added.push(...(typeof paths === "string" ? [paths] : paths));
      },
      on(event, listener) {
        watcher.on(event, listener);
      },
    },
    ws: { send: (payload) => void sent.push(payload) },
  };
  return { server, watcher, added, sent };
}

function config(
  root: string,
  command: "build" | "serve",
  logger = new Logger(),
  mode?: string,
): ViteConfigLike & { logger: Logger } {
  return mode === undefined ? { root, command, logger } : { root, command, logger, mode };
}

async function until(condition: () => boolean, what: string): Promise<void> {
  for (let i = 0; i < 400; i++) {
    if (condition()) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error(`timed out waiting for ${what}`);
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "undra-vite-"));
  log = join(dir, "calls.log");
  setEnv("FAKE_UNDRA_LOG", log);
  setEnv("UNDRA_SKIP_BUILD", undefined);
  setEnv("FAKE_UNDRA_EXIT", undefined);
  setEnv("FAKE_UNDRA_STDERR", undefined);
});

afterEach(() => {
  while (restore.length > 0) restore.pop()?.();
  rmSync(dir, { recursive: true, force: true });
  vi.restoreAllMocks();
});

describe("finding undra", () => {
  it("prefers UNDRA_BIN, then PATH, then where the installers put it", () => {
    expect(findUndra({ UNDRA_BIN: "/opt/custom/undra", PATH: binDir }, "/home/nobody")).toBe("/opt/custom/undra");
    expect(findUndra({ PATH: `/nonexistent:${binDir}` }, "/home/nobody")).toBe(bin);
    expect(findUndra({ PATH: "/nonexistent" }, "/home/nobody")).toBeNull();
    // ~/.undra/bin is where install.sh puts it; a GUI-launched tool has a short PATH.
    const home = join(dir, "home");
    mkdirSync(join(home, ".undra", "bin"), { recursive: true });
    writeFileSync(join(home, ".undra", "bin", "undra"), "#!/bin/sh\n");
    chmodSync(join(home, ".undra", "bin", "undra"), 0o755);
    expect(findUndra({ PATH: "" }, home)).toBe(join(home, ".undra", "bin", "undra"));
    expect(installDirs("/h")).toEqual(["/h/.undra/bin", "/h/.cargo/bin", "/opt/homebrew/bin", "/usr/local/bin"]);
  });

  it("does not take a file that is not executable, or a directory, for undra", () => {
    writeFileSync(join(dir, "undra"), "#!/bin/sh\n");
    chmodSync(join(dir, "undra"), 0o644);
    mkdirSync(join(dir, "bin", "undra"), { recursive: true });
    expect(findUndra({ PATH: `${dir}:${join(dir, "bin")}` }, "/home/nobody")).toBeNull();
  });

  it("teaches how to install it, in the shape of the CLI's errors", () => {
    const text = undraNotFoundMessage("it is not on PATH");
    expect(text).toMatch(/^error\[undra::C0003\]: `undra` was not found\n {2}= note: .*it is not on PATH\n {2}= help: /);
    expect(text).toContain(UNDRA_INSTALL_COMMAND);
    expect(text).toContain("https://shreypdev.github.io/undra/docs/errors.html#C0003");
    expect(text).toContain("UNDRA_BIN");
  });
});

describe("the project", () => {
  it("reads the core's path out of undra.toml and defaults to core", () => {
    expect(corePathOf('[project]\nname = "a"\n')).toBe("core");
    expect(corePathOf('[core]\npath = "crates/engine"\n')).toBe("crates/engine");
    expect(corePathOf('[project]\npath = "nope"\n[core]\n# a comment\npath = \'single\'\n')).toBe("single");
    expect(corePathOf('[core]\npackage = "x"\n[paths]\npath = "other"\n')).toBe("core");
  });

  it("finds undra.toml from a directory below it", () => {
    const { root, web } = makeProject("crates/engine");
    expect(findCoreLayout(web)).toEqual({ projectRoot: root, coreDir: join(root, "crates", "engine") });
    expect(findCoreLayout(dir)).toBeNull();
  });

  it("watches the core's sources and manifests and what the options add", () => {
    const { root } = makeProject();
    const layout = findCoreLayout(root);
    if (layout === null) throw new Error("no layout");
    const targets = watchTargets(layout, ["../shared/src", "extra.toml"], join(root, "web"));
    expect(targets.dirs).toEqual([join(root, "core", "src"), join(root, "shared", "src")]);
    expect(targets.files).toContain(join(root, "core", "Cargo.toml"));
    expect(targets.files).toContain(join(root, "Cargo.toml"));
    expect(targets.files).toContain(join(root, "undra.toml"));
    // `cargo update` changes what the core is built from (the shim follows the lock file).
    expect(targets.files).toContain(join(root, "Cargo.lock"));
    expect(targets.files).toContain(join(root, "web", "extra.toml"));
    expect(isWatched(join(root, "core", "src", "todo", "list.rs"), targets)).toBe(true);
    expect(isWatched(join(root, "core", "Cargo.toml"), targets)).toBe(true);
    expect(isWatched(join(root, "core", "target", "debug", "x.rs"), targets)).toBe(false);
    expect(isWatched(join(root, "core", "src-other", "x.rs"), targets)).toBe(false);
    expect(isWatched(join(root, "web", "src", "App.tsx"), targets)).toBe(false);
  });
});

describe("the plugin", () => {
  it("is a plugin that runs first and has the hooks Vite calls", () => {
    const plugin = undra();
    expect(plugin.name).toBe("undra");
    expect(plugin.enforce).toBe("pre");
    expect(typeof plugin.buildStart).toBe("function");
    expect(typeof plugin.configureServer).toBe("function");
  });

  it("builds the web core when Vite starts, from the project that holds the app", async () => {
    const { bin, web } = makeProject();
    const plugin = undra({ command: bin });
    plugin.configResolved(config(web, "build"));
    await plugin.buildStart();
    expect(calls()).toEqual([`-C ${web} build --platform web`]);
  });

  it("takes the project directory from the option when there is one", async () => {
    const { root, bin } = makeProject();
    const plugin = undra({ command: bin, projectDir: root });
    plugin.configResolved(config(join(dir, "elsewhere"), "build"));
    await plugin.buildStart();
    expect(calls()).toEqual([`-C ${root} build --platform web`]);
  });

  it("finds undra on PATH when no command is given", async () => {
    const { web } = makeProject();
    setEnv("PATH", `${binDir}:${process.env["PATH"] ?? ""}`);
    const plugin = undra();
    plugin.configResolved(config(web, "build"));
    await plugin.buildStart();
    expect(calls()).toHaveLength(1);
  });

  it("stops `vite build` with the build's own words when undra fails", async () => {
    const { bin, web } = makeProject();
    setEnv("FAKE_UNDRA_EXIT", "1");
    setEnv("FAKE_UNDRA_STDERR", "error[undra::C0011]: the Rust target wasm32-unknown-unknown is not installed");
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    const plugin = undra({ command: bin });
    plugin.configResolved(config(web, "build"));
    const failure = await plugin.buildStart().catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraBuildError);
    const error = failure as UndraBuildError;
    expect(error.code).toBe("C0004");
    expect(error.exitCode).toBe(1);
    expect(error.message).toContain("error[undra::C0004]: `undra build --platform web` failed (exited with status 1)");
    expect(error.message).toContain("C0011");
    expect(error.message).toContain("https://shreypdev.github.io/undra/docs/errors.html#C0004");
  });

  it("says how to install undra when it is not there, instead of a spawn error", async () => {
    const { web } = makeProject();
    setEnv("PATH", "/nonexistent");
    setEnv("UNDRA_BIN", undefined);
    setEnv("HOME", join(dir, "empty-home"));
    const plugin = undra();
    plugin.configResolved(config(web, "build"));
    const failure = (await plugin.buildStart().catch((e: unknown) => e)) as UndraBuildError;
    expect(failure).toBeInstanceOf(UndraBuildError);
    expect(failure.code).toBe("C0003");
    expect(failure.message).toContain("error[undra::C0003]: `undra` was not found");
    expect(failure.message).toContain(UNDRA_INSTALL_COMMAND);
  });

  it("names the command when an explicit one does not exist", async () => {
    const { web } = makeProject();
    const plugin = undra({ command: join(dir, "no-such-undra") });
    plugin.configResolved(config(web, "build"));
    const failure = (await plugin.buildStart().catch((e: unknown) => e)) as UndraBuildError;
    expect(failure.code).toBe("C0003");
    expect(failure.message).toContain("no-such-undra");
  });

  it("does not build when told to skip, by option or by UNDRA_SKIP_BUILD", async () => {
    const { bin, web } = makeProject();
    const byOption = undra({ command: bin, skip: true });
    byOption.configResolved(config(web, "build"));
    await byOption.buildStart();
    setEnv("UNDRA_SKIP_BUILD", "1");
    const byEnv = undra({ command: bin });
    byEnv.configResolved(config(web, "build"));
    await byEnv.buildStart();
    byEnv.configureServer(fakeServer().server);
    expect(calls()).toEqual([]);
    // The option wins over the variable.
    const forced = undra({ command: bin, skip: false });
    forced.configResolved(config(web, "build"));
    await forced.buildStart();
    expect(calls()).toHaveLength(1);
  });

  it("builds nothing under Vitest (mode test) unless asked to", async () => {
    // A test run uses the core that is already built; compiling it on every `vitest` would make a
    // web developer's tests need the Rust toolchain and take a release build's time.
    const { root, bin, web } = makeProject();
    const plugin = undra({ command: bin, debounceMs: 5 });
    plugin.configResolved(config(web, "serve", new Logger(), "test"));
    await plugin.buildStart();
    const fake = fakeServer();
    plugin.configureServer(fake.server);
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await new Promise((r) => setTimeout(r, 40));
    expect(calls()).toEqual([]);
    expect(fake.added).toEqual([]);
    const asked = undra({ command: bin, inTests: true });
    asked.configResolved(config(web, "serve", new Logger(), "test"));
    await asked.buildStart();
    expect(calls()).toHaveLength(1);
  });

  it("reports a failed build under `vite dev` instead of ending the server", async () => {
    const { bin, web } = makeProject();
    setEnv("FAKE_UNDRA_EXIT", "1");
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    const logger = new Logger();
    const plugin = undra({ command: bin });
    plugin.configResolved(config(web, "serve", logger));
    await expect(plugin.buildStart()).resolves.toBeUndefined();
    expect(logger.errors).toHaveLength(1);
    expect(logger.errors[0]).toContain("C0004");
  });
});

describe("under vite dev", () => {
  async function started(extra: { debounceMs?: number; watch?: string[] } = {}): Promise<{
    root: string;
    web: string;
    fake: ReturnType<typeof fakeServer>;
    logger: Logger;
  }> {
    const { root, bin, web } = makeProject();
    const logger = new Logger();
    const plugin = undra({ command: bin, debounceMs: 5, ...extra });
    plugin.configResolved(config(web, "serve", logger));
    await plugin.buildStart();
    const fake = fakeServer();
    plugin.configureServer(fake.server);
    return { root, web, fake, logger };
  }

  it("watches the core's sources and manifests, and nothing of the app", async () => {
    const { root, fake } = await started();
    expect(fake.added).toContain(join(root, "core", "src"));
    expect(fake.added).toContain(join(root, "core", "Cargo.toml"));
    expect(fake.added).not.toContain(join(root, "web"));
  });

  it("rebuilds and reloads the page when the core's source changes", async () => {
    const { root, web, fake, logger } = await started();
    expect(calls()).toHaveLength(1);
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await until(() => fake.sent.length === 1, "the reload");
    expect(calls()).toEqual([`-C ${web} build --platform web`, `-C ${web} build --platform web`]);
    expect(fake.sent).toEqual([{ type: "full-reload" }]);
    expect(logger.infos.join("\n")).toContain("lib.rs changed, rebuilding the core");
  });

  it("counts a burst of saves once", async () => {
    const { root, fake } = await started({ debounceMs: 40 });
    for (const name of ["a.rs", "b.rs", "lib.rs"]) fake.watcher.emit("change", join(root, "core", "src", name));
    fake.watcher.emit("add", join(root, "core", "src", "c.rs"));
    await until(() => fake.sent.length >= 1, "the reload");
    await new Promise((r) => setTimeout(r, 120));
    expect(calls()).toHaveLength(2);
    expect(fake.sent).toHaveLength(1);
  });

  it("builds again when a change arrives during a build, never two at once", async () => {
    const { root, web } = makeProject();
    const plugin = undra({ command: slowBin, debounceMs: 5, skip: false });
    plugin.configResolved(config(web, "serve"));
    const fake = fakeServer();
    plugin.configureServer(fake.server);
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await until(() => calls().length === 1, "the first build to start");
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await until(() => fake.sent.length === 2, "two reloads");
    expect(calls()).toEqual(["start", "end", "start", "end"]);
  });

  it("never runs a rebuild next to the first build: a save while Vite starts waits for it", async () => {
    const { root, web } = makeProject();
    const plugin = undra({ command: slowBin, debounceMs: 5, skip: false });
    plugin.configResolved(config(web, "serve"));
    const fake = fakeServer();
    // Vite calls configureServer while it creates the server, and buildStart when it listens.
    plugin.configureServer(fake.server);
    const first = plugin.buildStart();
    await until(() => calls().length === 1, "the first build to start");
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await first;
    await until(() => fake.sent.length === 1, "the reload after the second build");
    expect(calls()).toEqual(["start", "end", "start", "end"]);
  });

  it("ignores changes outside the core", async () => {
    const { root, web, fake } = await started();
    fake.watcher.emit("change", join(web, "src", "App.tsx"));
    fake.watcher.emit("change", join(root, "core", "target", "debug", "x.rs"));
    await new Promise((r) => setTimeout(r, 60));
    expect(calls()).toHaveLength(1);
    expect(fake.sent).toEqual([]);
  });

  it("shows a failed rebuild in the overlay and does not reload", async () => {
    const { root, fake, logger } = await started();
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    setEnv("FAKE_UNDRA_EXIT", "1");
    setEnv("FAKE_UNDRA_STDERR", "error[E0425]: cannot find value `x` in this scope");
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await until(() => fake.sent.length === 1, "the error");
    const sent = fake.sent[0];
    expect(sent?.type).toBe("error");
    if (sent?.type === "error") expect(sent.err.message).toContain("cannot find value");
    expect(logger.errors.join("\n")).toContain("C0004");
    // The next save, with the fix, reloads.
    setEnv("FAKE_UNDRA_EXIT", "0");
    fake.watcher.emit("change", join(root, "core", "src", "lib.rs"));
    await until(() => fake.sent.length === 2, "the reload");
    expect(fake.sent[1]).toEqual({ type: "full-reload" });
  });

  it("reads the core's location from undra.toml", async () => {
    const { root, bin, web } = makeProject("crates/engine");
    const plugin = undra({ command: bin, debounceMs: 5 });
    plugin.configResolved(config(web, "serve"));
    const fake = fakeServer();
    plugin.configureServer(fake.server);
    expect(fake.added).toContain(join(root, "crates", "engine", "src"));
    fake.watcher.emit("change", join(root, "crates", "engine", "src", "lib.rs"));
    await until(() => fake.sent.length === 1, "the reload");
  });

  it("watches what the options add", async () => {
    const { root, fake } = await started({ watch: ["../shared/src"] });
    expect(fake.added).toContain(join(root, "shared", "src"));
    fake.watcher.emit("change", join(root, "shared", "src", "util.rs"));
    await until(() => fake.sent.length === 1, "the reload");
  });

  it("says so, and builds nothing, when there is no undra.toml above the app", () => {
    const stray = join(dir, "stray");
    mkdirSync(stray, { recursive: true });
    const logger = new Logger();
    const plugin = undra({ command: "unused", debounceMs: 5 });
    plugin.configResolved(config(stray, "serve", logger));
    const fake = fakeServer();
    plugin.configureServer(fake.server);
    expect(fake.added).toEqual([]);
    expect(logger.warns.join("\n")).toContain("no undra.toml");
  });
});
