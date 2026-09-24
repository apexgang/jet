/**
 * A small W3C WebDriver client over `fetch`: sessions, scripts, clicks,
 * windows and screenshots, which is all the desktop release journey needs.
 * It has no dependencies, so the journey runs with bun alone on a fresh
 * runner. `tauri-driver` forwards every request to WebKitWebDriver; only
 * `POST /session` is rewritten, from `tauri:options` to WebKitGTK's
 * browser options.
 *
 * https://www.w3.org/TR/webdriver2/
 */

/** The W3C web element identifier. */
export const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

export type ElementReference = { [ELEMENT_KEY]: string };

export function isElementReference(value: unknown): value is ElementReference {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as Record<string, unknown>)[ELEMENT_KEY] === "string"
  );
}

/**
 * A failed command. `error` is the W3C error code (`no such element`,
 * `javascript error`, …) or one of this client's own: `timeout` when no
 * answer came in time, `unreachable` when nothing listened, and
 * `invalid response` for a body that is not a WebDriver reply.
 */
export class WebDriverError extends Error {
  constructor(
    readonly command: string,
    readonly status: number,
    readonly error: string,
    message: string,
  ) {
    super(`${command}: ${error}${message ? `: ${message}` : ""}`);
    this.name = "WebDriverError";
  }
}

export type Fetch = (input: string, init: RequestInit) => Promise<Response>;

export type ClientOptions = {
  fetch?: Fetch;
  /** Per-request limit in milliseconds (default 30 s). */
  timeoutMs?: number;
  /** Makes the abort signal for one request; tests pass an aborted one. */
  timeoutSignal?: (ms: number) => AbortSignal;
};

export type Timeouts = { script?: number; pageLoad?: number; implicit?: number };

type Method = "GET" | "POST" | "DELETE";

export class WebDriver {
  private readonly fetch: Fetch;
  private readonly timeoutMs: number;
  private readonly timeoutSignal: (ms: number) => AbortSignal;

  constructor(
    readonly base: string,
    options: ClientOptions = {},
  ) {
    this.fetch = options.fetch ?? ((input, init) => fetch(input, init));
    this.timeoutMs = options.timeoutMs ?? 30_000;
    this.timeoutSignal = options.timeoutSignal ?? ((ms) => AbortSignal.timeout(ms));
  }

  /** `GET /status`: whether the driver can create a session now. */
  async status(): Promise<{ ready: boolean; message: string }> {
    const value = (await this.request("status", "GET", "/status")) as { ready?: unknown; message?: unknown };
    return { ready: value?.ready === true, message: typeof value?.message === "string" ? value.message : "" };
  }

  /**
   * `POST /session`. For tauri-driver, `capabilities` holds
   * `{"tauri:options": {"application": "/usr/bin/jet-tauri"}}`; creating the
   * session launches the app, so it gets its own, longer limit.
   */
  async newSession(alwaysMatch: Record<string, unknown>, timeoutMs = 120_000): Promise<Session> {
    const value = (await this.request("new session", "POST", "/session", { capabilities: { alwaysMatch } }, timeoutMs)) as {
      sessionId?: unknown;
      capabilities?: unknown;
    };
    if (typeof value?.sessionId !== "string" || value.sessionId === "") {
      throw new WebDriverError("new session", 200, "invalid response", "no sessionId in the reply");
    }
    return new Session(this, value.sessionId, (value.capabilities ?? {}) as Record<string, unknown>);
  }

  /** One command; resolves to the reply's `value`. */
  async request(command: string, method: Method, path: string, body?: unknown, timeoutMs = this.timeoutMs): Promise<unknown> {
    let response: Response;
    try {
      response = await this.fetch(`${this.base}${path}`, {
        method,
        headers: body === undefined ? {} : { "content-type": "application/json; charset=utf-8" },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: this.timeoutSignal(timeoutMs),
      });
    } catch (error) {
      const name = (error as { name?: string })?.name;
      if (name === "TimeoutError" || name === "AbortError") {
        throw new WebDriverError(command, 0, "timeout", `no answer from ${this.base} within ${timeoutMs} ms`);
      }
      throw new WebDriverError(command, 0, "unreachable", `${this.base}: ${causeText(error)}`);
    }
    const text = await response.text();
    let reply: unknown;
    try {
      reply = text === "" ? {} : JSON.parse(text);
    } catch {
      throw new WebDriverError(command, response.status, "invalid response", `not JSON: ${text.slice(0, 200)}`);
    }
    const value = (reply as { value?: unknown })?.value;
    const failure = value as { error?: unknown; message?: unknown } | null | undefined;
    if (!response.ok || (typeof failure === "object" && failure !== null && typeof failure.error === "string")) {
      const code = typeof failure?.error === "string" ? failure.error : `HTTP ${response.status}`;
      const message = typeof failure?.message === "string" ? failure.message : text.slice(0, 200);
      throw new WebDriverError(command, response.status, code, message);
    }
    return value ?? null;
  }
}

export class Session {
  constructor(
    private readonly driver: WebDriver,
    readonly id: string,
    readonly capabilities: Record<string, unknown>,
  ) {}

  private request(command: string, method: Method, path: string, body?: unknown, timeoutMs?: number): Promise<unknown> {
    return this.driver.request(command, method, `/session/${encodeURIComponent(this.id)}${path}`, body, timeoutMs);
  }

  async setTimeouts(timeouts: Timeouts): Promise<void> {
    await this.request("set timeouts", "POST", "/timeouts", timeouts);
  }

  /** `POST /execute/sync`: `script` is a function body; `args` arrive as `arguments`. */
  async execute<T>(script: string, args: unknown[] = []): Promise<T> {
    return (await this.request("execute script", "POST", "/execute/sync", { script, args })) as T;
  }

  /** Runs `script` and requires an element back (null means not found). */
  async element(what: string, script: string, args: unknown[] = []): Promise<ElementReference | null> {
    const value = await this.execute<unknown>(script, args);
    if (value === null || value === undefined) return null;
    if (!isElementReference(value)) {
      throw new WebDriverError("execute script", 200, "invalid response", `${what} is not an element`);
    }
    return value;
  }

  async click(element: ElementReference): Promise<void> {
    await this.request("element click", "POST", `/element/${encodeURIComponent(element[ELEMENT_KEY])}/click`, {});
  }

  async windowHandle(): Promise<string> {
    return String(await this.request("get window handle", "GET", "/window"));
  }

  async windowHandles(): Promise<string[]> {
    const value = await this.request("get window handles", "GET", "/window/handles");
    if (!Array.isArray(value) || value.some((handle) => typeof handle !== "string")) {
      throw new WebDriverError("get window handles", 200, "invalid response", "not a list of handles");
    }
    return value as string[];
  }

  async switchToWindow(handle: string): Promise<void> {
    await this.request("switch to window", "POST", "/window", { handle });
  }

  async title(): Promise<string> {
    return String(await this.request("get title", "GET", "/title") ?? "");
  }

  async url(): Promise<string> {
    return String(await this.request("get current url", "GET", "/url") ?? "");
  }

  /** `GET /screenshot`: the current window as PNG bytes. */
  async screenshot(): Promise<Uint8Array> {
    const value = await this.request("take screenshot", "GET", "/screenshot", undefined, 60_000);
    if (typeof value !== "string" || value === "") {
      throw new WebDriverError("take screenshot", 200, "invalid response", "no image data");
    }
    return new Uint8Array(Buffer.from(value, "base64"));
  }

  /** `DELETE /session`: WebKitWebDriver closes the app with it. */
  async delete(timeoutMs = 30_000): Promise<void> {
    await this.request("delete session", "DELETE", "", undefined, timeoutMs);
  }
}

function causeText(error: unknown): string {
  const cause = (error as { cause?: { code?: string; message?: string } })?.cause;
  if (cause?.code) return cause.code;
  if (cause?.message) return cause.message;
  return error instanceof Error ? error.message : String(error);
}
