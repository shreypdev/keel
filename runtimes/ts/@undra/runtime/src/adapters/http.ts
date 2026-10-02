import { type Header, type HttpAdapter, HttpError, type HttpRequest, type HttpResponse } from "./types.js";

/** Options of {@link fetchHttp}. */
export interface FetchHttpOptions {
  /** The `fetch` to use; default the global one (browsers, Node 18+, Deno, Bun). */
  readonly fetch?: typeof fetch;
}

function isAbort(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { name?: unknown }).name === "AbortError";
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The `Http` port over `fetch`. Only `http:` and `https:` URLs are accepted.
 * Any HTTP status is a response, not an error; failures map to
 * {@link HttpError}: an unparsable URL to `InvalidUrl`, the request's
 * `timeoutMs` (covering the body download) to `Timeout`, any other abort to
 * `Cancelled`, everything else to `Network`.
 *
 * Browsers hide some headers (`Set-Cookie`, and everything not CORS-exposed);
 * that is what `fetch` gives, and what the core sees.
 */
export function fetchHttp(options: FetchHttpOptions = {}): HttpAdapter {
  return {
    async request(req: HttpRequest): Promise<HttpResponse> {
      const doFetch = options.fetch ?? (globalThis as { fetch?: typeof fetch }).fetch;
      if (typeof doFetch !== "function") throw new HttpError.Network("fetch is not available on this platform");
      let url: URL;
      try {
        url = new URL(req.url);
      } catch {
        throw new HttpError.InvalidUrl(req.url);
      }
      if (url.protocol !== "http:" && url.protocol !== "https:") throw new HttpError.InvalidUrl(req.url);

      const controller = new AbortController();
      let timedOut = false;
      const timer =
        req.timeoutMs === null
          ? undefined
          : setTimeout(() => {
              timedOut = true;
              controller.abort();
            }, req.timeoutMs);
      try {
        const headers = new Headers();
        for (const h of req.headers) headers.append(h.name, h.value);
        const init: RequestInit = {
          method: req.method.toUpperCase(),
          headers,
          signal: controller.signal,
        };
        if (req.body !== null) init.body = req.body as unknown as BodyInit;
        const response = await doFetch(url, init);
        const body = new Uint8Array(await response.arrayBuffer());
        const responseHeaders: Header[] = [];
        response.headers.forEach((value, name) => {
          responseHeaders.push({ name, value });
        });
        return { status: response.status, headers: responseHeaders, body };
      } catch (error) {
        if (timedOut) throw new HttpError.Timeout();
        if (isAbort(error)) throw new HttpError.Cancelled();
        throw new HttpError.Network(message(error));
      } finally {
        if (timer !== undefined) clearTimeout(timer);
      }
    },
  };
}
