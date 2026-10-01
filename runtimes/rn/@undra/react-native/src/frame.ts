import type { UndraNativeModule } from "./native.js";

/**
 * A frame that never comes (the app went to the background after the request) is replaced by a
 * timer after this long, as the browser schedule does (docs/SPEC.md section 11.1).
 */
const FRAME_BACKSTOP_MS = 100;
/** Without a native vsync source, a frame is approximated by a timer of this length. */
const FALLBACK_FRAME_MS = 16;

/** What {@link nativeFrameScheduler} needs to know about the app. */
export interface FrameSchedulerOptions {
  /** Whether the app is in the foreground (React Native's `AppState.currentState === "active"`). */
  readonly isActive: () => boolean;
  /** Receives failures of a scheduled function. Default: rethrown from a timer. */
  readonly onError?: (error: unknown) => void;
}

/**
 * The mirror's `schedule` for React Native (ADR-038, decision 6; the `AttachOptions.mirror.schedule`
 * of `UndraCore`): runs each function once, at the next display frame of the native vsync source
 * (`CADisplayLink` on iOS, `AChoreographer` on Android) while the app is active, with a 100 ms timer
 * as a backstop, and in a zero-delay timer while it is not (no frames are produced then).
 *
 * React Native's own `requestAnimationFrame` is a zero-delay timer in 0.87's bridgeless mode, so
 * it would drain after every macrotask instead of once per frame.
 */
export function nativeFrameScheduler(native: UndraNativeModule, options: FrameSchedulerOptions): (fn: () => void) => void {
  let waiting: Array<() => void> = [];
  let armed = false;
  let backstop: ReturnType<typeof setTimeout> | undefined;
  const fail =
    options.onError ??
    ((error: unknown) => {
      setTimeout(() => {
        throw error;
      }, 0);
    });

  const run = (): void => {
    if (!armed) return;
    armed = false;
    if (backstop !== undefined) {
      clearTimeout(backstop);
      backstop = undefined;
    }
    const due = waiting;
    waiting = [];
    for (const fn of due) {
      try {
        fn();
      } catch (error) {
        fail(error);
      }
    }
  };
  native.frame = run;

  return (fn) => {
    if (!options.isActive()) {
      setTimeout(fn, 0);
      return;
    }
    waiting.push(fn);
    if (armed) return;
    armed = true;
    let hasFrames = false;
    try {
      hasFrames = native.requestFrame();
    } catch (error) {
      fail(error);
    }
    backstop = setTimeout(run, hasFrames ? FRAME_BACKSTOP_MS : FALLBACK_FRAME_MS);
  };
}
