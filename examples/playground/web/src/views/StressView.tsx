import { type Signal, UndraCallError, UndraCore } from "@undra/runtime";
import { useSignal, useUndra } from "@undra/runtime/react";
import { Stress, StressError, type StressMode } from "@playground/core";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { type StatsChannel, STATS_INTERVAL_MS, timerResolutionUs, toStressMessage } from "../embed-stats";
import {
  FrameMonitor,
  StressMeter,
  type StressSnapshot,
  browserFrameSource,
  formatCount,
  formatDuration,
  formatMerge,
  formatStep,
  pauseWhenHidden,
} from "../stress-stats";

/** The generator rates the screen offers, in updates a second. */
export const RATES = [1_000, 10_000, 50_000, 100_000] as const;

/** The rate the generator starts at when the page does not say. */
export const DEFAULT_RATE = 10_000;

/** What `burst` commits. */
const BURST = 1_000;

const MODES = [
  { id: "firehose", label: "Firehose", note: "writes `value`: merged once per frame" },
  { id: "progress", label: "Progress", note: "writes `progress`: every entry applied" },
] as const satisfies readonly { readonly id: StressMode; readonly label: string; readonly note: string }[];

/** `10,000` as `10k`, `1,000,000` as `1M`. */
const shortRate = (rate: number): string => {
  if (rate >= 1_000_000 && rate % 1_000_000 === 0) return `${rate / 1_000_000}M`;
  return rate >= 1000 && rate % 1000 === 0 ? `${rate / 1000}k` : rate.toLocaleString("en-US");
};

/**
 * The stress screen: the core generates updates by itself (a task paced by the `Timer` port, one
 * transaction per update) and this page reports what its runtime did with them, measured in this
 * browser: how many change-sets arrived, how many entries were applied after the mirror merged them,
 * how long each drain held the main thread, and how many frames were dropped.
 *
 * `value` is an ordinary signal, so the mirror merges it: thousands of updates a frame are applied
 * once. `progress` is declared `#[undra(no_coalesce)]`: every update is applied on its own, and the
 * "applied" counters beside the two numbers show the difference. The store belongs to this view
 * (`useUndra`): leaving it stops the generator and releases the store.
 *
 * Embedded in the landing page, the screen also posts what it shows to its parent, as the
 * `undra-stats` message, through `channel`; `?rate=`, `?mode=` and `?autostart=1` steer it.
 */
export function StressView({
  channel,
  initialRate = DEFAULT_RATE,
  initialMode = "firehose",
  autostart = false,
}: {
  readonly channel?: StatsChannel | undefined;
  readonly initialRate?: number | undefined;
  readonly initialMode?: StressMode | undefined;
  readonly autostart?: boolean | undefined;
}) {
  const stress = useUndra(Stress);
  if (stress === undefined) return <h2>Stress</h2>;
  return <StressPanel stress={stress} channel={channel} initialRate={initialRate} initialMode={initialMode} autostart={autostart} />;
}

function StressPanel({
  stress,
  channel,
  initialRate,
  initialMode,
  autostart,
}: {
  readonly stress: Stress;
  readonly channel: StatsChannel | undefined;
  readonly initialRate: number;
  readonly initialMode: StressMode;
  readonly autostart: boolean;
}) {
  const running = useSignal(stress.running);
  const [mode, setMode] = useState<StressMode>(initialMode);
  const [rate, setRate] = useState(initialRate);
  const [snapshot, setSnapshot] = useState<StressSnapshot | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const meter = useRef<StressMeter | null>(null);
  // What the effect that posts the stats reads without being restarted by it.
  const latest = useRef({ mode, rate, running });
  latest.current = { mode, rate, running };

  // The meter: watches the mirror and the frames while the screen is open, and publishes what it
  // measured every 500 ms, to the tiles and (embedded) to the landing page, from the same snapshot.
  useEffect(() => {
    const frames = new FrameMonitor({ source: browserFrameSource() });
    const stopVisibility = pauseWhenHidden(frames);
    const measuring = new StressMeter({
      mirror: UndraCore.shared.mirror,
      generated: () => Number(stress.generated.peek()),
      frames,
      timerResolutionUs: timerResolutionUs(),
      applies: { value: stress.value, progress: stress.progress },
    });
    measuring.start();
    meter.current = measuring;
    const release = channel?.claim();
    const publish = (): void => {
      const next = measuring.snapshot();
      setSnapshot(next);
      const { mode: m, rate: r, running: on } = latest.current;
      channel?.post(toStressMessage(next, { mode: m, targetRate: r, running: on }));
    };
    publish();
    const timer = setInterval(publish, STATS_INTERVAL_MS);
    return () => {
      clearInterval(timer);
      release?.();
      measuring.stop();
      stopVisibility();
      meter.current = null;
    };
  }, [stress, channel]);

  /** Starts the generator (or retunes the running one) and starts the measurement over when it was stopped. */
  const start = async (nextMode: StressMode, nextRate: number, fresh: boolean): Promise<void> => {
    setProblem(null);
    if (fresh) meter.current?.reset();
    try {
      await stress.start(nextMode, nextRate);
    } catch (error) {
      setProblem(error instanceof StressError || error instanceof UndraCallError ? error.message : String(error));
    }
  };

  // `?autostart=1`, and the cleanup that matters: leaving the screen stops the generator. React
  // unmounts `StressView` (whose `useUndra` releases the store) before this panel, so the store may
  // already be closed here; then the core has ended its generator with it, and a `stop` on the
  // released handle would only be refused (and reported as a failure).
  useEffect(() => {
    if (autostart) void start(initialMode, initialRate, true);
    return () => {
      if (!stress.closed) void stress.stop(); // a command: it never rejects
    };
    // Once per store: the controls below call `start` themselves.
  }, [stress]);

  const chooseMode = (next: StressMode): void => {
    setMode(next);
    if (running) void start(next, rate, false);
  };
  const chooseRate = (next: number): void => {
    setRate(next);
    if (running) void start(mode, next, false);
  };
  const rates: readonly number[] = RATES.includes(rate as (typeof RATES)[number]) ? RATES : [...RATES, rate].sort((a, b) => a - b);

  const step = snapshot?.timerResolutionUs ?? 0;
  const s = snapshot;

  return (
    <>
      <h2>
        Stress{" "}
        <span className="badge" data-testid="stress-state">
          {running ? "running" : "stopped"}
        </span>
      </h2>
      <div className="row wrap">
        <div className="chips" role="group" aria-label="What the generator writes">
          {MODES.map(({ id, label, note }) => (
            <button key={id} aria-pressed={mode === id} title={note} data-testid={`stress-mode-${id}`} onClick={() => chooseMode(id)}>
              {label}
            </button>
          ))}
        </div>
        <div className="chips" role="group" aria-label="Target rate, updates per second">
          {rates.map((r) => (
            <button key={r} aria-pressed={rate === r} data-testid={`stress-rate-${r}`} onClick={() => chooseRate(r)}>
              {shortRate(r)}/s
            </button>
          ))}
        </div>
      </div>
      <div className="row wrap">
        {running ? (
          <button className="primary" data-testid="stress-stop" onClick={() => void stress.stop()}>
            Stop
          </button>
        ) : (
          <button className="primary" data-testid="stress-start" onClick={() => void start(mode, rate, true)}>
            Start
          </button>
        )}
        <button data-testid="stress-burst" onClick={() => void stress.burst(mode, BURST)}>
          Burst {BURST.toLocaleString("en-US")}
        </button>
      </div>
      {problem !== null && (
        <p className="error" role="alert">
          {problem}
        </p>
      )}

      <div className="stress-values">
        <ValueTile label="value" kind="merged" testId="stress-value" applies={s?.applies["value"] ?? 0}>
          <Latest signal={stress.value} />
        </ValueTile>
        <ValueTile label="progress" kind="no_coalesce" testId="stress-progress" applies={s?.applies["progress"] ?? 0}>
          <Latest signal={stress.progress} />
        </ValueTile>
      </div>

      <dl className="tiles" aria-label="Measured in this browser">
        <Tile label="Generated" unit="/ s" value={s ? formatCount(s.generatedPerSec) : "–"} note={`target ${formatCount(rate)} · ${s ? formatCount(s.generatedTotal) : 0} so far`} testId="stress-generated" />
        <Tile
          label="Received"
          unit="/ s"
          value={s ? formatCount(s.receivedPerSec) : "–"}
          note={
            s !== null && s.compactions > 0
              ? `change-sets, one per update · backlog folded ${formatCount(s.compactions)} times before a frame came`
              : "change-sets: one per update, one per 10 ms for the counter"
          }
          testId="stress-received"
        />
        <Tile label="Applied" unit="/ s" value={s ? formatCount(s.appliedPerSec) : "–"} note={s ? `entries after merging, ${formatMerge(s.mergeRatio)}` : "entries after merging"} testId="stress-applied" />
        <Tile label="Drains" unit="/ s" value={s ? formatCount(s.drainsPerSec) : "–"} note="once per frame" testId="stress-drains" />
        <Tile
          label="Drain p50 / p99"
          value={s ? `${formatDuration(s.drainP50Us, step)} / ${formatDuration(s.drainP99Us, step)}` : "–"}
          note={step > 0 ? `clock step ${formatStep(step)}` : "per drain"}
          testId="stress-drain"
        />
        <Tile label="Per change-set" unit="ns" value={s ? formatCount(s.nsPerChangeSet) : "–"} note="drain time ÷ change-sets; the parse on arrival is outside the drain" testId="stress-per-changeset" />
        <Tile
          label="Dropped frames"
          value={s ? formatCount(s.droppedFrames) : "–"}
          note={s ? `${formatCount(s.droppedFramesRecent)} in 5 s · longest ${s.longestFrameMs.toFixed(1)} ms` : "–"}
          testId="stress-dropped"
        />
        <Tile
          label="JS heap"
          unit={s?.heapMb == null ? "" : "MB"}
          value={s?.heapMb == null ? "n/a" : s.heapMb.toFixed(1)}
          note="Chrome only"
          testId="stress-heap"
        />
      </dl>
      <p className="note stress-note">
        Measured in your browser. The core is Rust compiled to WebAssembly, running on this page&apos;s main thread, and generates the
        updates itself on a timer. Each update is one transaction: the mirror receives one change-set for it and merges what arrives between
        two frames. Browsers round <code>performance.now()</code>, so a single drain below the clock step reads as &ldquo;under&rdquo; it; the
        per-change-set average is exact.
      </p>
    </>
  );
}

/** The latest value of a signal. Its own component, so a change re-renders this number and nothing else. */
function Latest<T extends bigint | number>({ signal }: { readonly signal: Signal<T> }) {
  return <>{useSignal(signal).toLocaleString("en-US")}</>;
}

/** One of the two signals the generator writes: its latest value (`children`) and how many times it was applied. */
function ValueTile({
  label,
  kind,
  testId,
  applies,
  children,
}: {
  readonly label: string;
  readonly kind: string;
  readonly testId: string;
  readonly applies: number;
  readonly children: ReactNode;
}) {
  return (
    <div className="value-tile">
      <div className="value-label">
        <code>{label}</code> <span className="badge">{kind}</span>
      </div>
      <div className="value-number" data-testid={testId} aria-live="off">
        {children}
      </div>
      <div className="value-note">
        applied <b data-testid={`${testId}-applies`}>{applies.toLocaleString("en-US")}</b> times
      </div>
    </div>
  );
}

function Tile({
  label,
  value,
  unit,
  note,
  testId,
}: {
  readonly label: string;
  readonly value: string;
  readonly unit?: string;
  readonly note: string;
  readonly testId: string;
}) {
  return (
    <div className="tile">
      <dt>{label}</dt>
      <dd>
        <span className="tile-value" data-testid={testId}>
          {value}
        </span>
        {unit ? <small>{unit}</small> : null}
      </dd>
      <dd className="tile-note">{note}</dd>
    </div>
  );
}
