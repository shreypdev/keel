import { storePath } from "./names.js";
import { type FsAdapter, FsError, fsErrorFrom } from "./types.js";

/** Options of {@link opfsFs}. */
export interface OpfsFsOptions {
  /**
   * The directory to serve, or a function that opens it. Default: the directory
   * `undra/<namespace>/fs` of the origin private file system
   * (`navigator.storage.getDirectory()`), created on first use (SPEC 8, ADR-044 amendment A).
   */
  readonly root?: FileSystemDirectoryHandle | (() => Promise<FileSystemDirectoryHandle>);
  /** The namespace of the core the default directory is made for. Default `"_"`; ignored when `root` is given. */
  readonly namespace?: string | undefined;
}

/** Splits an adapter path into safe segments; `..` is refused so that a path cannot leave the root. */
export function splitPath(path: string): string[] {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") throw new FsError.Denied();
    parts.push(part);
  }
  return parts;
}

/** The text of `FsError.Unavailable` where the platform has no origin private file system (ADR-049). */
export const NEEDS_OPFS = "needs the origin private file system";

/**
 * The `Fs` port over the Origin Private File System. Paths are `/`-separated
 * and relative to the root; missing directories are created by `write`.
 * `list` returns the names of the entries (files and directories) in
 * ascending order. Errors are {@link FsError} (see `fsErrorFrom`): a missing
 * entry is `NotFound`, a refused permission `Denied`, an exhausted quota `Full`
 * (ADR-049), a platform without the origin private file system `Unavailable`,
 * anything else `Io`.
 */
export function opfsFs(options: OpfsFsOptions = {}): FsAdapter {
  // The default directory, `undra/<namespace>/fs`, named now so that a namespace that is not one is refused here, typed.
  const defaultPath = options.root === undefined ? storePath(options.namespace, "fs").split("/") : [];
  let root: Promise<FileSystemDirectoryHandle> | null = null;
  const openRoot = (): Promise<FileSystemDirectoryHandle> => {
    if (root === null) {
      const option = options.root;
      if (typeof option === "function") root = option();
      else if (option !== undefined) root = Promise.resolve(option);
      else {
        const storage = (globalThis as { navigator?: { storage?: StorageManager } }).navigator?.storage;
        if (storage === undefined || typeof storage.getDirectory !== "function") {
          root = Promise.reject(new FsError.Unavailable(NEEDS_OPFS));
        } else {
          root = storage.getDirectory().then(async (origin) => {
            let dir = origin;
            for (const part of defaultPath) dir = await dir.getDirectoryHandle(part, { create: true });
            return dir;
          });
        }
      }
      root.catch(() => {
        root = null;
      });
    }
    return root;
  };

  /** The directory holding the last segment of `parts`, and that segment. */
  const locate = async (path: string, create: boolean): Promise<{ dir: FileSystemDirectoryHandle; name: string }> => {
    const parts = splitPath(path);
    const name = parts.pop();
    if (name === undefined) throw new FsError.Io("the path is empty");
    let dir = await openRoot();
    for (const part of parts) dir = await dir.getDirectoryHandle(part, { create });
    return { dir, name };
  };

  const guard = async <T>(work: () => Promise<T>): Promise<T> => {
    try {
      return await work();
    } catch (error) {
      throw fsErrorFrom(error);
    }
  };

  return {
    read: (path) =>
      guard(async () => {
        const { dir, name } = await locate(path, false);
        const file = await (await dir.getFileHandle(name)).getFile();
        return new Uint8Array(await file.arrayBuffer());
      }),
    write: (path, data) =>
      guard(async () => {
        const { dir, name } = await locate(path, true);
        const handle = await dir.getFileHandle(name, { create: true });
        const writable = await handle.createWritable();
        try {
          await writable.write(data as unknown as FileSystemWriteChunkType);
        } finally {
          await writable.close();
        }
      }),
    delete: (path) =>
      guard(async () => {
        const { dir, name } = await locate(path, false);
        await dir.removeEntry(name, { recursive: true });
      }),
    list: (dirPath) =>
      guard(async () => {
        let dir = await openRoot();
        for (const part of splitPath(dirPath)) dir = await dir.getDirectoryHandle(part);
        const names: string[] = [];
        for await (const name of dir.keys()) names.push(name);
        return names.sort();
      }),
  };
}
