# SDE - the Swift `Fs` stays in its root; the Swift `Kv` lists only sealed entries (wt/swift-fs, 2026-10-01)

Two findings against the Swift runtime, both found by the rn-adapters piece and its review
(`rn-adapters.md` finding 1; `.10x/reviews/2026-10-02-rn-adapters-review.md` I1). No wire, schema or
generated-shape change: no ADR. SPEC section 8's `Fs` rule was already the contract (any `..` or
symbolic link on a path is `Denied`, a link is removed and never followed by `delete`); the Swift
adapter did not meet it.

## 1. `FsAdapter` followed symbolic links out of its root

`resolve` refused `..` and then handed `FileManager` a URL built by appending the components, which
follows every link on the way. Repro (root `<tmp>/root`, link `root/out -> <tmp>/outside`, file
`<tmp>/outside/secret.txt`): `read("out/secret.txt")` returned the bytes, `write("out/new.txt")`
created `<tmp>/outside/new.txt`; `delete` and `list` through the link worked too.

Fix: every operation walks from a descriptor of the root, one component at a time, each opened with
`openat(O_RDONLY | O_DIRECTORY | O_NOFOLLOW)` (the portable C++ `FsRoot` in
`runtimes/rn/@undra/react-native/cpp/UndraStores.cpp`, ported, same errno mapping). A link met as
any component, last or middle, is `Denied`; one swapped in between two steps is met by the next
`openat`, so no check is separated from its use. The last component of a read is opened
`O_NOFOLLOW | O_NONBLOCK` and `fstat`ed (a directory or a FIFO is an `Io` error, never blocks). A
write seals with `renameat` over the name, which replaces a link put there after the check instead
of writing through it. `delete` of a link unlinks the link; of a directory removes the tree without
following links, with a stack of names and bounded descriptors (as C++). The root is opened as given
(it is the app's configuration; `/var` is a link on a Mac). New `Adapters/PosixFiles.swift` holds
the owned descriptor, the read/write loops and the atomic write (shared with `Kv`).

What else changed in `Fs`, all to match the C++ store that shares the root on iOS:

* A path is split on the `/` byte, not on `Character`: in Swift `"/" + U+0301` is one `Character`,
  so a splitter by `Character` would not see that separator (tested).
* A NUL byte is an `Io` error (the C++ answer; a C string would end there and `notes\0/x` would have
  meant `notes`). A leading `/` is still ignored (SPEC), so an absolute path names a path under the
  root, never one outside (tested with a real outside file).
* `write` is atomic through a temporary `.undra-tmp-<16 hex>` (Foundation's `.atomic` did its own
  thing, unnamed), 0644 under the umask; created directories are 0700 (were the Foundation default,
  0755). `list` hides the `.undra-tmp-` prefix, as C++ does. Any other dot file is an ordinary name.
* `FsAdapter.map` (Foundation errors) stays: creating the root still goes through `FileManager`.

Not changed: `list` still sorts with Swift's `sorted()` (the C++ store sorts bytes; differs only
for non-ASCII names, as before).

## 2. `KvAdapter.list` read leftover temporary files

The directory is shared with the C++ store, which writes `<name>.<16 hex>.tmp` and renames; a
process killed between the write and the rename leaves the temporary behind, a complete copy of an
entry (or a cut one). The Swift `list` read every file, so it listed that key again (and after its
deletion). Foundation's `.atomic` also gave Swift no name of its own for the pattern.

Fix, with the naming defined once (`FileKeyValueBackend.temporaryName(for:)` / `temporarySuffix`,
`isEntryName`):

* `set` writes `<name>.<16 hex>.tmp` (the C++ store's pattern), flushes, renames, flushes the
  directory; 0600 (the C++ store's mode). So a sealed entry file is whole or absent.
* `list` considers only a sealed entry: a file named `<16 hex>-<8 hex>` whose header holds a key
  whose `fileName(for:)` is that name. Temporaries, dot files, other names, directories, damaged
  headers and unreadable files are skipped (an unreadable one no longer fails the whole listing).
* `get` of a file with a damaged header (empty, under 4 bytes, a key longer than the file, not
  UTF-8) is "no value", consistent with `list` and self-healing (the next `set` replaces it).

Two things this does not and cannot do:

* A *value* cut short at a sealed name is not detectable: the entry format (the shared layout, wire
  frozen) ends the value at the end of the file, with no length or checksum. Only the atomic rename
  rules it out, and a leftover temporary never has the sealed name.
* Nothing sweeps stale temporaries (the RN review's alternative); they are invisible, not removed.

## Parity notes for the integrator

* **Damaged entry in `get`**: Swift now answers "no value"; the C++ store answers "unavailable"
  (logged: `the entry file of '<key>' is damaged`). Both are typed outcomes of a port with no error
  channel. A shell of the same app on the other runtime sees the difference only on a damaged file.
  "No value" was chosen so an app does not meet an "unavailable" on every launch until it happens
  to overwrite the key; if the grid should be identical, the C++ side is one `return true` away.
* **`list` is stricter in Swift** than C++ (name shape and name-matches-key, where C++ skips dot
  files and `.tmp` and accepts any other file with a valid header). Only the stores write there.
* Not run: the iOS simulator/device (the Swift suite is macOS), and the RN module's `stores_test`
  was not touched (no C++ change).

## Tests

`FsConfinementTests.swift` (22): the repro through read / write / delete / list; a link as the last
component (file link, dangling link), in the middle, leading inside the root; delete of a link and
of a tree holding one; a root that is itself a link; `..` in nine spellings for all four operations;
absolute paths; NUL bytes; byte-wise splitting; a multibyte name round trip; a directory where a
file is expected and the other way round, a FIFO, a missing root, an oversized name; atomic write
and its 0644; list hiding `.undra-tmp-`; a thread swapping a directory for a link 400 times while
the test writes, reads and lists through it (nothing lands outside); the refusals as typed
`FsError` through the port. Checked by mutation: without `O_NOFOLLOW` (and the link check) the
link tests and the swap test fail.

`AdapterTests.swift` (`KeyValueAdapterTests`, +7): the temp-name pattern and `isEntryName`
vectors; `set` leaves only the entry, 0600; a planted complete and a cut temporary, a dot file and
a ghost key's temporary are not listed, a deleted key does not come back; an entry under another
entry-shaped name; an entry-named directory; six damaged entry files read as no value and a `set`
heals one; the same through the port.

Counts: `swift test` 511 (main had 482: +22, +7), 0 failures, no warnings; the Swift contract column
18/18 pass. `docs/ERRORS.md` unchanged (no row changes).
