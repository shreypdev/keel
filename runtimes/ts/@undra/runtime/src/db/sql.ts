/*
 * What an adapter that cannot ask SQLite (`node:sqlite` exposes neither
 * `sqlite3_bind_parameter_count` nor the tail of `sqlite3_prepare`) needs to know about a
 * statement's text, by SQLite's own lexical rules (tokenize.c): its parameters, numbered as
 * `sqlite3_bind_parameter_count` / `sqlite3_bind_parameter_index` number them, and whether a
 * remainder holds nothing but whitespace, comments and semicolons. Internal to `@undra/runtime/db`.
 */

/** The parameters of one statement. */
export interface SqlParameters {
  /** What `sqlite3_bind_parameter_count` answers: the largest index used. */
  readonly count: number;
  /** The indices that have a `:name`, `@name` or `$name`, with that name (prefix included). */
  readonly named: ReadonlyMap<number, string>;
}

const isSpace = (c: number): boolean => c === 0x20 || c === 0x09 || c === 0x0a || c === 0x0c || c === 0x0d;
const isDigit = (c: number): boolean => c >= 0x30 && c <= 0x39;
/** An identifier character: ASCII letters, digits, `_`, `$`, and everything beyond ASCII. */
const isIdChar = (c: number): boolean =>
  (c >= 0x61 && c <= 0x7a) || (c >= 0x41 && c <= 0x5a) || isDigit(c) || c === 0x5f || c === 0x24 || c >= 0x80;

/**
 * Walks `sql` token by token and calls `visit` for each parameter and each piece of SQL that is
 * not trivia; returns nothing. Strings, quoted identifiers and comments are skipped whole (an
 * unterminated one runs to the end, where SQLite reports it).
 */
function scan(sql: string, visit: (token: { readonly kind: "parameter" | "other"; readonly text: string }) => void): void {
  const n = sql.length;
  let i = 0;
  while (i < n) {
    const c = sql.charCodeAt(i);
    if (isSpace(c) || c === 0x3b /* ; */) {
      i++;
    } else if (c === 0x2d && sql.charCodeAt(i + 1) === 0x2d) {
      const end = sql.indexOf("\n", i + 2);
      i = end < 0 ? n : end + 1;
    } else if (c === 0x2f && sql.charCodeAt(i + 1) === 0x2a) {
      const end = sql.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
    } else if (c === 0x27 || c === 0x22 || c === 0x60) {
      // 'string', "identifier", `identifier`: a doubled quote is the quote itself.
      let j = i + 1;
      for (;;) {
        const end = sql.indexOf(sql[i] as string, j);
        if (end < 0) {
          j = n;
          break;
        }
        if (sql.charCodeAt(end + 1) === c) {
          j = end + 2;
          continue;
        }
        j = end + 1;
        break;
      }
      visit({ kind: "other", text: sql.slice(i, j) });
      i = j;
    } else if (c === 0x5b /* [ */) {
      const end = sql.indexOf("]", i + 1);
      const j = end < 0 ? n : end + 1;
      visit({ kind: "other", text: sql.slice(i, j) });
      i = j;
    } else if (c === 0x3f /* ? */) {
      let j = i + 1;
      while (j < n && isDigit(sql.charCodeAt(j))) j++;
      visit({ kind: "parameter", text: sql.slice(i, j) });
      i = j;
    } else if ((c === 0x3a || c === 0x40 || c === 0x24) && i + 1 < n && isIdChar(sql.charCodeAt(i + 1))) {
      // :name, @name, $name (TCL-style `$a::b(c)` included).
      let j = i + 1;
      for (;;) {
        while (j < n && isIdChar(sql.charCodeAt(j))) j++;
        if (c === 0x24 && sql.charCodeAt(j) === 0x3a && sql.charCodeAt(j + 1) === 0x3a) {
          j += 2;
          continue;
        }
        break;
      }
      if (c === 0x24 && sql.charCodeAt(j) === 0x28 /* ( */) {
        const end = sql.indexOf(")", j);
        j = end < 0 ? n : end + 1;
      }
      visit({ kind: "parameter", text: sql.slice(i, j) });
      i = j;
    } else if (isIdChar(c)) {
      let j = i + 1;
      while (j < n && isIdChar(sql.charCodeAt(j))) j++;
      visit({ kind: "other", text: sql.slice(i, j) });
      i = j;
    } else {
      visit({ kind: "other", text: sql[i] as string });
      i++;
    }
  }
}

/**
 * The parameters of the statement `sql`, numbered as SQLite numbers them: `?` takes the next
 * index, `?NNN` is index NNN, a `:name` / `@name` / `$name` takes the next index the first time
 * it appears and the same one after that. `count` is the largest index.
 */
export function scanParameters(sql: string): SqlParameters {
  let count = 0;
  const byName = new Map<string, number>();
  const named = new Map<number, string>();
  scan(sql, ({ kind, text }) => {
    if (kind !== "parameter") return;
    if (text.startsWith("?")) {
      if (text.length === 1) {
        count++;
      } else {
        count = Math.max(count, Number(text.slice(1)));
      }
      return;
    }
    const seen = byName.get(text);
    if (seen !== undefined) return;
    count++;
    byName.set(text, count);
    named.set(count, text);
  });
  return { count, named };
}

/** Whether `sql` holds no statement: only whitespace, comments and semicolons. */
export function isTrivia(sql: string): boolean {
  let trivia = true;
  scan(sql, () => {
    trivia = false;
  });
  return trivia;
}
