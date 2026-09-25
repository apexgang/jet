import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, describe, expect, it } from "vitest";

import { ELEMENT_KEY, WebDriver, WebDriverError, isElementReference } from "./e2e/webdriver";

type Seen = { method: string; path: string; body: unknown };
type Handler = (request: Seen) => { status?: number; body: string | object };

let server: Server | null = null;

/** A loopback WebDriver server that records requests and answers from `handler`. */
async function serve(handler: Handler): Promise<{ base: string; seen: Seen[] }> {
  const seen: Seen[] = [];
  server = createServer((request: IncomingMessage, response: ServerResponse) => {
    let raw = "";
    request.on("data", (chunk: Buffer) => (raw += chunk.toString("utf8")));
    request.on("end", () => {
      const entry = { method: request.method ?? "", path: request.url ?? "", body: raw === "" ? undefined : JSON.parse(raw) };
      seen.push(entry);
      const reply = handler(entry);
      response.writeHead(reply.status ?? 200, { "content-type": "application/json" });
      response.end(typeof reply.body === "string" ? reply.body : JSON.stringify(reply.body));
    });
  });
  await new Promise<void>((resolve) => server!.listen(0, "127.0.0.1", () => resolve()));
  const { port } = server.address() as AddressInfo;
  return { base: `http://127.0.0.1:${port}`, seen };
}

afterEach(async () => {
  const current = server;
  server = null;
  if (current) await new Promise((resolve) => current.close(resolve));
});

/** Answers the commands a session sends, like WebKitWebDriver behind tauri-driver. */
function driverLike(request: Seen): { status?: number; body: object } {
  const route = `${request.method} ${request.path}`;
  switch (route) {
    case "POST /session":
      return { body: { value: { sessionId: "s 1", capabilities: { browserName: "MiniBrowser" } } } };
    case "POST /session/s%201/execute/sync":
      return { body: { value: { echoed: (request.body as { args: unknown[] }).args } } };
    case "GET /session/s%201/window":
      return { body: { value: "main" } };
    case "GET /session/s%201/window/handles":
      return { body: { value: ["main", "settings"] } };
    case "GET /session/s%201/title":
      return { body: { value: "Jet Settings" } };
    case "GET /session/s%201/screenshot":
      return { body: { value: Buffer.from([0x89, 0x50, 0x4e, 0x47]).toString("base64") } };
    case "POST /session/s%201/window":
      return (request.body as { handle: string }).handle === "gone"
        ? { status: 404, body: { value: { error: "no such window", message: "window gone", stacktrace: "" } } }
        : { body: { value: null } };
    default:
      return { body: { value: null } };
  }
}

describe("the journey's WebDriver client", () => {
  it("asks tauri-driver for the app and speaks W3C to the session", async () => {
    const { base, seen } = await serve(driverLike);
    const driver = new WebDriver(base);
    const session = await driver.newSession({ "tauri:options": { application: "/usr/bin/jet-tauri" } });
    expect(session.id).toBe("s 1");
    expect(seen[0]).toEqual({
      method: "POST",
      path: "/session",
      body: { capabilities: { alwaysMatch: { "tauri:options": { application: "/usr/bin/jet-tauri" } } } },
    });

    await session.setTimeouts({ script: 30_000, pageLoad: 60_000, implicit: 0 });
    expect(seen[1]).toEqual({ method: "POST", path: "/session/s%201/timeouts", body: { script: 30_000, pageLoad: 60_000, implicit: 0 } });

    expect(await session.execute("return 1", ["main", { x: 1 }])).toEqual({ echoed: ["main", { x: 1 }] });
    expect(seen[2].body).toEqual({ script: "return 1", args: ["main", { x: 1 }] });

    expect(await session.windowHandle()).toBe("main");
    expect(await session.windowHandles()).toEqual(["main", "settings"]);
    await session.switchToWindow("settings");
    expect(seen.at(-1)).toEqual({ method: "POST", path: "/session/s%201/window", body: { handle: "settings" } });
    expect(await session.title()).toBe("Jet Settings");
    expect([...(await session.screenshot())]).toEqual([0x89, 0x50, 0x4e, 0x47]);

    await session.click({ [ELEMENT_KEY]: "e/1" });
    expect(seen.at(-1)).toEqual({ method: "POST", path: "/session/s%201/element/e%2F1/click", body: {} });

    await session.delete();
    expect(seen.at(-1)).toEqual({ method: "DELETE", path: "/session/s%201", body: undefined });
  });

  it("returns element references and refuses other values where an element is required", async () => {
    let answer: unknown = { [ELEMENT_KEY]: "node-7" };
    const { base } = await serve(() => ({ body: { value: answer } }));
    const session = await new WebDriver(base).newSession({}).catch(() => null);
    // The first reply above has no sessionId.
    expect(session).toBeNull();

    answer = { sessionId: "s", capabilities: {} };
    const live = await new WebDriver(base).newSession({});
    answer = { [ELEMENT_KEY]: "node-7" };
    expect(await live.element("the Settings button", "return x")).toEqual({ [ELEMENT_KEY]: "node-7" });
    answer = null;
    expect(await live.element("the Settings button", "return x")).toBeNull();
    answer = "a string";
    await expect(live.element("the Settings button", "return x")).rejects.toThrow(/the Settings button is not an element/);
    expect(isElementReference({ [ELEMENT_KEY]: "x" })).toBe(true);
    expect(isElementReference({ ELEMENT: "x" })).toBe(false);
  });

  it("turns W3C errors into WebDriverError with the command and code", async () => {
    const { base } = await serve(driverLike);
    const session = await new WebDriver(base).newSession({});
    const error = await session.switchToWindow("gone").catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(WebDriverError);
    expect(error).toMatchObject({ command: "switch to window", status: 404, error: "no such window" });
    expect((error as Error).message).toBe("switch to window: no such window: window gone");
  });

  it("treats an error value in a 200 reply as a failure", async () => {
    const { base } = await serve(() => ({ body: { value: { error: "javascript error", message: "boom" } } }));
    await expect(new WebDriver(base).status()).rejects.toMatchObject({ error: "javascript error", status: 200 });
  });

  it("says when a reply is not JSON or a session has no id", async () => {
    const { base } = await serve(() => ({ status: 502, body: "<html>bad gateway</html>" }));
    await expect(new WebDriver(base).status()).rejects.toMatchObject({ error: "invalid response", status: 502 });
  });

  it("reports an unreachable driver and a request that timed out", async () => {
    const { base } = await serve(driverLike);
    const closed = server!;
    server = null;
    await new Promise((resolve) => closed.close(resolve));
    await expect(new WebDriver(base).status()).rejects.toMatchObject({ error: "unreachable", status: 0 });

    const { base: live } = await serve(driverLike);
    const aborted = () => AbortSignal.abort(new DOMException("The operation timed out.", "TimeoutError"));
    const error = await new WebDriver(live, { timeoutSignal: aborted, timeoutMs: 1234 }).status().catch((caught: unknown) => caught);
    expect(error).toMatchObject({ error: "timeout", command: "status" });
    expect((error as Error).message).toContain("within 1234 ms");
  });

  it("reads /status readiness", async () => {
    const { base } = await serve(() => ({ body: { value: { ready: true, message: "ready" } } }));
    expect(await new WebDriver(base).status()).toEqual({ ready: true, message: "ready" });
  });
});
