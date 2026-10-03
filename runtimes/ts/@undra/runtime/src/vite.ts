import { spawn } from "node:child_process";
import { accessSync, constants, existsSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, isAbsolute, join, resolve, sep } from "node:path";

/*
 * `@undra/runtime/vite`: the Vite plugin that makes `undra build --platform web` part of the web app's
 * own build. Node only (it spawns the `undra` command-line tool) and free of run-time dependencies: the
 * parts of Vite it touches are described below as structural types, so it type-checks against any Vite
 * without importing one.
 *
 *   import { undra } from "@undra/runtime/vite";
 *   export default defineConfig({ plugins: [undra(), react()] });
 *
 * - `vite build` and `vite dev` both build the core first (`buildStart`), so there is no manual step.
 *   A failing build stops `vite build` and is reported (not fatal) under `vite dev`, where the next
 *   save is the retry.
 * - Under `vite dev` the plugin watches the core's `src/**`, its manifests and `Cargo.lock`, rebuilds on
 *   a change (one build at a time, the first one included; a burst of saves counted once) and reloads the
 *   page onto the new core. A failed build keeps the old core and shows the error in Vite's overlay.
 * - A page that runs the core `undra dev` serves (`?undra=ws://...` in its URL, or `VITE_UNDRA_DEV_URL`) is
 *   not reloaded: `undra dev` has already moved it to the rebuilt core with its state (ADR-053), and a reload
 *   would start it from scratch. The page decides, in a small module the plugin adds to it under `vite dev`
 *   (`virtual:undra/dev-reload`); a page without it (no index.html of Vite's) gets the plain reload.
 * - Under Vitest (mode `test`) it builds nothing unless `inTests` is set.
 * - `undra` is looked up as `UNDRA_BIN`, then on `PATH`, then where the installers put it. When it is
 *   not found the error says how to install it, in the shape of the CLI's own errors (C0003).
 */

/** The part of Vite's logger the plugin uses. */
export interface ViteLoggerLike {
  info(message: string): void;
  warn(message: string): void;
  error(message: string): void;
}

/** The part of Vite's resolved configuration the plugin uses. */
export interface ViteConfigLike {
  readonly root: string;
  readonly command: "build" | "serve";
  /** Vite's mode; Vitest runs Vite in mode `test`. */
  readonly mode?: string;
  readonly logger: ViteLoggerLike;
}

/** The part of Vite's file watcher the plugin uses (Vite's is a chokidar `FSWatcher`). */
export interface ViteWatcherLike {
  add(paths: string | readonly string[]): unknown;
  on(event: string, listener: (path: string) => void): unknown;
}

/** What the plugin sends to the page: a full reload, the rebuilt core's event for its client module, or an error for Vite's overlay. */
export type VitePayload =
  | { readonly type: "full-reload"; readonly path?: string }
  | { readonly type: "custom"; readonly event: typeof CORE_REBUILT_EVENT; readonly data: { readonly file: string } }
  | { readonly type: "error"; readonly err: { readonly message: string; readonly stack: string } };

/** The event the plugin sends when the core was rebuilt; its client module ({@link DEV_RELOAD_ID}) decides whether to reload. */
export const CORE_REBUILT_EVENT = "undra:core-rebuilt";

/** The module the plugin adds to the page under `vite dev`: it reloads the page onto a rebuilt core unless `undra dev` serves the page's core. */
export const DEV_RELOAD_ID = "virtual:undra/dev-reload";

/** The tag Vite puts into index.html for {@link UndraVitePlugin.transformIndexHtml}. */
export interface HtmlTagLike {
  readonly tag: "script";
  readonly attrs: { readonly type: "module"; readonly src: string };
  readonly injectTo: "head";
}

/**
 * The client module: whether this page runs the core `undra dev` serves is what the template's `startUndra` reads
 * (`?undra=` in the URL, else `VITE_UNDRA_DEV_URL`; a production build reads neither). Such a page stays: `undra dev`
 * has moved it to the rebuilt core with its state. Any other page loads the rebuilt wasm.
 */
export const DEV_RELOAD_SOURCE = `// Added by the undra() Vite plugin under vite dev (@undra/runtime/vite).
const url = new URLSearchParams(location.search).get("undra") ?? import.meta.env.VITE_UNDRA_DEV_URL;
const servedByUndraDev = typeof url === "string" && url.length > 0;
if (import.meta.hot) {
  import.meta.hot.on(${JSON.stringify(CORE_REBUILT_EVENT)}, () => {
    if (servedByUndraDev) {
      console.info("[undra] the core was rebuilt; this page runs the core undra dev serves, which kept its state, so it is not reloaded");
      return;
    }
    location.reload();
  });
}
`;

/** The part of Vite's dev server the plugin uses. */
export interface ViteDevServerLike {
  readonly watcher: ViteWatcherLike;
  readonly ws: { send(payload: VitePayload): void };
}

/** The Vite plugin `undra()` returns: assignable to Vite's `Plugin`. */
export interface UndraVitePlugin {
  readonly name: "undra";
  readonly enforce: "pre";
  config(config: { readonly build?: { readonly target?: unknown } }): { readonly build: { readonly target: string } } | undefined;
  configResolved(config: ViteConfigLike): void;
  buildStart(): Promise<void>;
  configureServer(server: ViteDevServerLike): void;
  resolveId(id: string): string | undefined;
  load(id: string): string | undefined;
  transformIndexHtml(): HtmlTagLike[];
  handleHotUpdate(context: { readonly file: string }): [] | undefined;
}

/** Options of {@link undra}. Every one has a default that fits a project made by `undra init`. */
export interface UndraPluginOptions {
  /** A directory of the Undra project (the one with `undra.toml`, or any directory below it). Default: Vite's root. */
  readonly projectDir?: string;
  /** The `undra` executable. Default: `UNDRA_BIN`, else `undra` from `PATH` or where the installers put it. */
  readonly command?: string;
  /** More files or directories (absolute, or relative to the project) whose change rebuilds the core under `vite dev`. */
  readonly watch?: readonly string[];
  /** Do not build (the core was built in an earlier step). Default: true when `UNDRA_SKIP_BUILD` is `1`. */
  readonly skip?: boolean;
  /**
   * Build under Vitest too. Default false: Vitest runs Vite in mode `test`, and a test run uses the core
   * that is already built rather than compiling it (which needs the Rust toolchain) on every run.
   */
  readonly inTests?: boolean;
  /** How long a burst of saves is allowed to settle before the core is rebuilt, in milliseconds. Default 150. */
  readonly debounceMs?: number;
}

/** The command that installs `undra` (docs/RELEASING.md). */
export const UNDRA_INSTALL_COMMAND = "curl -fsSL https://shreypdev.github.io/undra/install.sh | sh";

const DOCS = "https://shreypdev.github.io/undra/docs/errors.html";

/** A failure of `undra build`: `C0003` when `undra` was not found, `C0004` when it ran and failed. */
export class UndraBuildError extends Error {
  override readonly name = "UndraBuildError";

  constructor(
    message: string,
    /** The CLI error code this failure has: `C0003` (not installed) or `C0004` (the build failed). */
    readonly code: "C0003" | "C0004",
    /** The exit status of `undra`, when it ran. */
    readonly exitCode: number | null = null,
  ) {
    super(message);
  }
}

/** The teaching error for a missing `undra`, in the shape of the CLI's errors (`error[undra::C0003]`). */
export function undraNotFoundMessage(searched: string): string {
  return [
    "error[undra::C0003]: `undra` was not found",
    `  = note: the Vite plugin of vite.config.ts runs \`undra build --platform web\` to compile the Rust core to wasm, and ${searched}`,
    `  = help: ${UNDRA_INSTALL_COMMAND}`,
    "          then restart Vite so it sees the new PATH, or set UNDRA_BIN to the executable; `undra doctor` checks the rest of the toolchain",
    `  = docs: ${DOCS}#C0003`,
  ].join("\n");
}

/** The directories (besides `PATH`) the installers put `undra` in. */
export function installDirs(home: string): string[] {
  return [join(home, ".undra", "bin"), join(home, ".cargo", "bin"), "/opt/homebrew/bin", "/usr/local/bin"];
}

function isExecutable(path: string): boolean {
  try {
    if (!statSync(path).isFile()) return false;
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Finds `undra`: `UNDRA_BIN` when set, else the first executable named `undra` on `PATH` or in
 * the directories the installers use. `null` when there is none.
 */
export function findUndra(env: Readonly<Record<string, string | undefined>>, home: string): string | null {
  const explicit = env["UNDRA_BIN"];
  if (explicit !== undefined && explicit.trim() !== "") return explicit;
  const dirs = [...(env["PATH"] ?? "").split(delimiter), ...installDirs(home)].filter((d) => d !== "");
  for (const dir of dirs) {
    const candidate = join(dir, "undra");
    if (isExecutable(candidate)) return candidate;
  }
  return null;
}

/** Where the project and its core are: the directory of `undra.toml` found from `start` upwards, and the core crate. */
export interface CoreLayout {
  /** The directory of `undra.toml`. */
  readonly projectRoot: string;
  /** The core crate's directory (`[core] path`, default `core`). */
  readonly coreDir: string;
  /** Where `undra build` writes (`[paths] build`, default `build`). */
  readonly buildDir: string;
}

/** Reads the string `key` of `[table]` out of the text of an `undra.toml`; `fallback` when it has none. */
function tomlString(toml: string, wanted: string, key: string, fallback: string): string {
  let table = "";
  const pattern = new RegExp(`^${key}\\s*=\\s*(?:"([^"]*)"|'([^']*)')`);
  for (const raw of toml.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("[")) {
      table = line.replace(/^\[+|\]+.*$/g, "").trim();
    } else if (table === wanted) {
      const match = pattern.exec(line);
      const value = match?.[1] ?? match?.[2];
      if (value !== undefined && value !== "") return value;
    }
  }
  return fallback;
}

/** Reads `[core] path` out of the text of an `undra.toml`; `core` when it has none. */
export function corePathOf(toml: string): string {
  return tomlString(toml, "core", "path", "core");
}

/** Reads `[paths] build` out of the text of an `undra.toml`; `build` when it has none. */
export function buildPathOf(toml: string): string {
  return tomlString(toml, "paths", "build", "build");
}

/** Finds the project from `start` upwards; `null` when no directory has an `undra.toml`. */
export function findCoreLayout(start: string): CoreLayout | null {
  let dir = resolve(start);
  for (;;) {
    const file = join(dir, "undra.toml");
    if (existsSync(file)) {
      const toml = readFileSync(file, "utf8");
      return { projectRoot: dir, coreDir: resolve(dir, corePathOf(toml)), buildDir: resolve(dir, buildPathOf(toml)) };
    }
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/** What a change to `file` has to be under to rebuild the core: the core's sources and the manifests. */
export function watchTargets(layout: CoreLayout, extra: readonly string[], base: string): { dirs: string[]; files: string[] } {
  const dirs = [join(layout.coreDir, "src")];
  const files = [
    join(layout.coreDir, "Cargo.toml"),
    join(layout.projectRoot, "Cargo.toml"),
    join(layout.projectRoot, "Cargo.lock"),
    join(layout.projectRoot, "undra.toml"),
  ];
  for (const entry of extra) {
    const path = isAbsolute(entry) ? entry : resolve(base, entry);
    // A path that is not there yet is taken for a directory when it has no extension.
    if (existsSync(path) ? statSync(path).isDirectory() : !/\.[^/\\]+$/.test(path)) dirs.push(path);
    else files.push(path);
  }
  return { dirs, files };
}

/** Whether `file` is one of `files` or below one of `dirs`. */
export function isWatched(file: string, targets: { dirs: readonly string[]; files: readonly string[] }): boolean {
  const path = resolve(file);
  return targets.files.some((f) => resolve(f) === path) || targets.dirs.some((d) => path.startsWith(resolve(d) + sep));
}

interface RunResult {
  readonly code: number | null;
  /** The tail of what the build printed on stderr, for an error message. */
  readonly stderr: string;
}

/** Runs `command args` in `cwd`, forwarding its output to this process, and keeps the end of its stderr. */
function runCommand(command: string, args: readonly string[], cwd: string): Promise<RunResult> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, [...args], { cwd, stdio: ["ignore", "pipe", "pipe"] });
    let tail = "";
    child.stdout.on("data", (chunk: Buffer) => process.stdout.write(chunk));
    child.stderr.on("data", (chunk: Buffer) => {
      process.stderr.write(chunk);
      tail = (tail + chunk.toString("utf8")).slice(-4000);
    });
    child.on("error", reject);
    child.on("close", (code) => resolvePromise({ code, stderr: tail }));
  });
}

/**
 * The Vite plugin that builds the Rust core for the web before Vite starts and, under `vite dev`,
 * whenever the core's sources change. It also sets `build.target` to `es2022` when the app sets none: the
 * runtime is ES2022, and Vite 6's default target (es2020) would turn its class fields into helper calls.
 *
 * @example
 * ```ts
 * import { undra } from "@undra/runtime/vite";
 * export default defineConfig({ plugins: [undra()] });
 * ```
 */
export function undra(options: UndraPluginOptions = {}): UndraVitePlugin {
  let config: ViteConfigLike | undefined;
  // One `undra build` at a time, the first build included: a change that arrives during a build is
  // remembered (the latest one) and built once after it.
  let running = false;
  let again: string | undefined;
  let rebuild: ((file: string) => Promise<void>) | undefined;
  // Whether a page has loaded the client module: then a rebuilt core is an event the page decides on, else a plain reload.
  let clientInstalled = false;
  // What `undra build` writes (`[paths] build` of undra.toml): a change there is the plugin's to announce, not Vite's.
  let buildDir: string | undefined;

  const skip = (): boolean =>
    (options.skip ?? process.env["UNDRA_SKIP_BUILD"] === "1") || (options.inTests !== true && config?.mode === "test");
  const projectDir = (): string => resolve(options.projectDir ?? config?.root ?? process.cwd());

  /** Runs `undra -C <project> build --platform web`; throws an {@link UndraBuildError}. */
  async function build(): Promise<void> {
    const command = options.command ?? findUndra(process.env, homedir());
    if (command === null) {
      throw new UndraBuildError(
        undraNotFoundMessage("it is not on PATH or in ~/.undra/bin, ~/.cargo/bin or Homebrew's directories"),
        "C0003",
      );
    }
    const args = ["-C", projectDir(), "build", "--platform", "web"];
    let result: RunResult;
    try {
      result = await runCommand(command, args, projectDir());
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") {
        throw new UndraBuildError(undraNotFoundMessage(`\`${command}\` does not exist`), "C0003");
      }
      throw e;
    }
    if (result.code !== 0) {
      const status = result.code === null ? "was killed" : `exited with status ${String(result.code)}`;
      throw new UndraBuildError(
        [
          `error[undra::C0004]: \`undra build --platform web\` failed (${status})`,
          "  = note: the Vite plugin of vite.config.ts runs it so the page always has the core it is built from; its own output is above",
          "  = help: fix what it reported and save again; `undra doctor` checks the toolchain",
          `  = docs: ${DOCS}#C0004`,
        ].join("\n") + (result.stderr === "" ? "" : `\n\n${result.stderr.trim()}`),
        "C0004",
        result.code,
      );
    }
  }

  return {
    name: "undra",
    enforce: "pre",

    // The runtime is ES2022 (class fields, private methods): Vite 6 and older build for es2020 unless told otherwise, which
    // rewrites such syntax into helper calls. The app's own `build.target` wins.
    config(userConfig) {
      return userConfig.build?.target === undefined ? { build: { target: "es2022" } } : undefined;
    },

    configResolved(resolved) {
      config = resolved;
    },

    async buildStart() {
      if (skip()) return;
      running = true;
      try {
        await build();
      } catch (e) {
        // Under `vite dev` a failed build is not the end: the next save of the core is the retry.
        if (config?.command === "serve" && e instanceof UndraBuildError) {
          config.logger.error(e.message);
          return;
        }
        throw e;
      } finally {
        running = false;
        const changed = again;
        if (changed !== undefined && rebuild !== undefined) void rebuild(changed);
      }
    },

    configureServer(server) {
      if (skip()) return;
      const layout = findCoreLayout(projectDir());
      const logger = config?.logger;
      if (layout === null) {
        logger?.warn(`undra: no undra.toml from ${projectDir()} upwards, so the core's sources are not watched`);
        return;
      }
      const targets = watchTargets(layout, options.watch ?? [], projectDir());
      server.watcher.add([...targets.dirs, ...targets.files]);
      buildDir = layout.buildDir;

      const debounce = options.debounceMs ?? 150;
      let timer: ReturnType<typeof setTimeout> | undefined;

      rebuild = async (file: string): Promise<void> => {
        if (running) {
          again = file;
          return;
        }
        running = true;
        let next: string | undefined = file;
        try {
          while (next !== undefined) {
            const changed: string = next;
            again = undefined;
            logger?.info(`undra: ${changed} changed, rebuilding the core`);
            try {
              await build();
              server.ws.send(
                clientInstalled ? { type: "custom", event: CORE_REBUILT_EVENT, data: { file: changed } } : { type: "full-reload" },
              );
            } catch (e) {
              const message = e instanceof Error ? e.message : String(e);
              logger?.error(message);
              server.ws.send({ type: "error", err: { message, stack: "" } });
            }
            next = again;
          }
        } finally {
          running = false;
        }
      };

      const onChange = (file: string): void => {
        if (!isWatched(file, targets)) return;
        if (timer !== undefined) clearTimeout(timer);
        timer = setTimeout(() => {
          timer = undefined;
          void rebuild?.(file);
        }, debounce);
      };
      for (const event of ["change", "add", "unlink"]) server.watcher.on(event, onChange);
    },

    resolveId(id) {
      return id === DEV_RELOAD_ID ? `\0${DEV_RELOAD_ID}` : undefined;
    },

    load(id) {
      if (id !== `\0${DEV_RELOAD_ID}`) return undefined;
      clientInstalled = true;
      return DEV_RELOAD_SOURCE;
    },

    // Under `vite dev` (and only while the plugin builds the core) the page gets the client module.
    transformIndexHtml() {
      if (config?.command !== "serve" || skip()) return [];
      return [{ tag: "script", attrs: { type: "module", src: `/@id/${DEV_RELOAD_ID}` }, injectTo: "head" }];
    },

    // The wasm `undra build` rewrites is imported by the page (`?url`): left to Vite, its change reloads every page, the ones
    // `undra dev` serves included. The plugin announces a rebuilt core itself, once, after the build.
    handleHotUpdate({ file }) {
      if (buildDir === undefined || skip()) return undefined;
      return resolve(file).startsWith(resolve(buildDir) + sep) ? [] : undefined;
    },
  };
}
