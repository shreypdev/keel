import { type Header, type HttpAdapter, HttpError, type HttpRequest, type HttpResponse } from "@undra/runtime";

/** Options of {@link reactNativeHttp}. */
export interface ReactNativeHttpOptions {
  /** The `fetch` to use; default React Native's global one. */
  readonly fetch?: typeof fetch;
}

/**
 * `scheme://[userinfo@]host[:port][/path?query#fragment]` with an `http` or `https` scheme and a host. React
 * Native's `URL` is a regular-expression shim that accepts anything (`new URL("not a url")` does not throw), so the
 * adapter checks the shape itself.
 */
const HTTP_URL = /^https?:\/\/(?:[^\s/?#@]*@)?(?:\[[0-9A-Fa-f:.]+\]|[^\s/?#:@[\]]+)(?::(\d{1,5}))?(?:[/?#]\S*)?$/i;

/** Whether `url` is an absolute `http:` or `https:` URL with a host (and a port in range, if it has one). */
export function isHttpUrl(url: string): boolean {
  const match = HTTP_URL.exec(url);
  if (match === null) return false;
  const port = match[1];
  return port === undefined || Number(port) <= 65_535;
}

function isAbort(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { name?: unknown }).name === "AbortError";
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The `Http` port over React Native's `fetch` (ADR-038 amendment B): `NSURLSession` on iOS and OkHttp on Android,
 * through React Native's networking module, so the core's requests use the same stack, client configuration and
 * developer tools as the app's own. `loadNative` installs it; pass another `HttpAdapter` as `adapters.http` to
 * replace it.
 *
 * Any HTTP status is a response, not an error (the core decides what a 404 means). Failures are the port's typed
 * {@link HttpError} (ADR-032): a URL that is not `http(s)://host...` is `InvalidUrl`; the request's `timeoutMs`
 * (which covers the body) is `Timeout`; an abort is `Cancelled`; anything else, including no connection, airplane
 * mode, a refused connection and a DNS failure (React Native's `TypeError: Network request failed`), is `Network`:
 * an offline device is a `Network` error, never "unavailable".
 *
 * Bodies cross React Native's networking module as base64 both ways (its limitation), so very large bodies cost a
 * copy and an encoding on the JS thread.
 */
export function reactNativeHttp(options: ReactNativeHttpOptions = {}): HttpAdapter {
  return {
    async request(req: HttpRequest): Promise<HttpResponse> {
      const doFetch = options.fetch ?? (globalThis as { fetch?: typeof fetch }).fetch;
      if (typeof doFetch !== "function") throw new HttpError.Network("fetch is not available on this platform");
      if (!isHttpUrl(req.url)) throw new HttpError.InvalidUrl(req.url);

      const controller = typeof AbortController === "function" ? new AbortController() : undefined;
      let timedOut = false;
      const timer =
        req.timeoutMs === null || controller === undefined
          ? undefined
          : setTimeout(() => {
              timedOut = true;
              controller.abort();
            }, req.timeoutMs);
      try {
        // Pairs, not a record: a repeated header name is two headers.
        const headers: [string, string][] = req.headers.map((h) => [h.name, h.value]);
        const init: RequestInit = { method: req.method.toUpperCase(), headers };
        if (controller !== undefined) init.signal = controller.signal;
        // A copy of exactly the view's bytes: React Native encodes the whole buffer of a view it is given.
        if (req.body !== null) init.body = req.body.slice() as unknown as BodyInit;
        const response = await doFetch(req.url, init);
        const body = new Uint8Array(await response.arrayBuffer());
        const responseHeaders: Header[] = [];
        response.headers.forEach((value, name) => {
          responseHeaders.push({ name, value });
        });
        return { status: response.status, headers: responseHeaders, body };
      } catch (error) {
        if (error instanceof HttpError) throw error;
        if (timedOut) throw new HttpError.Timeout();
        if (isAbort(error)) throw new HttpError.Cancelled();
        throw new HttpError.Network(message(error));
      } finally {
        if (timer !== undefined) clearTimeout(timer);
      }
    },
  };
}
