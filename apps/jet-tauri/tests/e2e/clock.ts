/**
 * Time for the desktop release journey. The journey measures wall-clock
 * intervals against the app's own `performance.timeOrigin`, so `now` is
 * epoch milliseconds. Tests pass a virtual clock whose `sleep` only moves
 * time forward.
 */
export type Clock = {
  /** Milliseconds since the Unix epoch. */
  now(): number;
  sleep(ms: number): Promise<void>;
};

export const realClock: Clock = {
  now: () => Date.now(),
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, Math.max(0, ms))),
};

/** A virtual clock: `sleep` advances time at once, after letting pending I/O run. */
export class VirtualClock implements Clock {
  private listeners: Array<(now: number) => void> = [];

  constructor(private current = Date.UTC(2026, 8, 24, 12)) {}

  now(): number {
    return this.current;
  }

  async sleep(ms: number): Promise<void> {
    // Loopback I/O (a fake WebDriver server) completes between steps.
    await new Promise((resolve) => setImmediate(resolve));
    this.advance(Math.max(0, ms));
  }

  advance(ms: number): void {
    this.current += ms;
    for (const listener of this.listeners) listener(this.current);
  }

  /** Called after every advance, e.g. to move fake CPU counters. */
  onAdvance(listener: (now: number) => void): void {
    this.listeners.push(listener);
  }
}

export class TimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TimeoutError";
  }
}

export type WaitOptions<T> = {
  timeoutMs: number;
  intervalMs?: number;
  /** Describes the last thing seen, for the failure message. */
  describe?: (last: T | undefined, error: unknown) => string;
};

/**
 * Polls `probe` until it returns something other than `undefined`. A probe
 * that throws is retried; its last error lands in the timeout message with
 * what `describe` says about the last value seen.
 */
export async function waitFor<T, R>(
  clock: Clock,
  what: string,
  probe: (seen: (value: T) => void) => Promise<R | undefined>,
  options: WaitOptions<T>,
): Promise<R> {
  const started = clock.now();
  const interval = options.intervalMs ?? 100;
  let last: T | undefined;
  let lastError: unknown = null;
  const seen = (value: T) => {
    last = value;
  };
  for (;;) {
    try {
      const result = await probe(seen);
      if (result !== undefined) return result;
      lastError = null;
    } catch (error) {
      lastError = error;
    }
    if (clock.now() - started >= options.timeoutMs) {
      const detail = options.describe?.(last, lastError) ?? (lastError ? errorText(lastError) : "nothing matched");
      throw new TimeoutError(`${what}: not within ${formatSeconds(options.timeoutMs)} (${detail})`);
    }
    await clock.sleep(interval);
  }
}

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function formatSeconds(ms: number): string {
  return `${Number((ms / 1000).toFixed(1))} s`;
}
