<script lang="ts">
  import { onMount } from "svelte";

  import {
    openPlaneFeed,
    type ConnectionSnapshot,
    type PlaneUpdate,
    type PublicError,
  } from "$lib/jet/bridge";
  import { firstLaunchFixture } from "$lib/jet/fixture";

  type ViewState = "connecting" | "online" | "reconnecting" | "failed";

  let connectionState = $state<ViewState>("connecting");
  let connection = $state<ConnectionSnapshot | null>(null);
  let failure = $state<PublicError | null>(null);
  let recentEvents = $state<Array<{ sequence: string; kind: string }>>([]);

  function receive(update: PlaneUpdate) {
    switch (update.type) {
      case "connected":
        connection = update.connection;
        connectionState = "online";
        failure = null;
        break;
      case "resumed":
        connectionState = "online";
        failure = null;
        break;
      case "event":
        connectionState = "online";
        recentEvents = [
          { sequence: update.sequence, kind: update.kind },
          ...recentEvents,
        ].slice(0, 4);
        break;
      case "reconnecting":
        connectionState = "reconnecting";
        failure = update.error;
        break;
      case "failed":
        connectionState = "failed";
        failure = update.error;
        break;
    }
  }

  onMount(() => {
    void openPlaneFeed(receive)
      .then((snapshot) => {
        connection = snapshot.state === "online" ? snapshot : null;
        connectionState = snapshot.state;
      })
      .catch((error: unknown) => {
        const candidate = error as Partial<PublicError>;
        failure = {
          category: typeof candidate.category === "string" ? candidate.category : "offline",
          code: typeof candidate.code === "string" ? candidate.code : "transport.offline",
          message:
            typeof candidate.message === "string"
              ? candidate.message
              : "Jet could not reach this Plane.",
          retryable: candidate.retryable === true,
        };
        connectionState = candidate.retryable === false ? "failed" : "reconnecting";
      });
  });

  const statusLabel = $derived(
    connectionState === "online"
      ? "Connected"
      : connectionState === "connecting"
        ? "Connecting"
        : connectionState === "reconnecting"
          ? "Reconnecting"
          : "Unavailable",
  );
</script>

<svelte:head>
  <title>Jet</title>
  <meta
    name="description"
    content="Jet desktop transport and presentation foundation"
  />
</svelte:head>

<main>
  <header>
    <div class="brand" aria-label="Jet">
      <span class="mark" aria-hidden="true"></span>
      <span>Jet</span>
    </div>
    <div class:online={connectionState === "online"} class="status" role="status" aria-live="polite">
      <span class="status-dot" aria-hidden="true"></span>
      {statusLabel}
    </div>
  </header>

  <section class="hero" aria-labelledby="foundation-title">
    <p class="eyebrow">Desktop foundation · Wave 0.3</p>
    <h1 id="foundation-title">A narrow, native Plane connection.</h1>
    <p class="lede">
      The webview receives a bounded status snapshot and ordered event labels. Native paths,
      identities, credentials, proofs, command bodies, and event payloads stay in Rust.
    </p>
  </section>

  <section class="grid" aria-label="Foundation status">
    <article class="card connection-card">
      <div class="card-heading">
        <div>
          <p class="label">Local Plane</p>
          <h2>{firstLaunchFixture.planeName}</h2>
        </div>
        <span class:online={connectionState === "online"} class="pill">{statusLabel}</span>
      </div>

      {#if connection?.state === "online"}
        <dl>
          <div>
            <dt>Core</dt>
            <dd>{connection.coreVersion ?? "Unknown"}</dd>
          </div>
          <div>
            <dt>Cursor</dt>
            <dd>{connection.cursor ?? "0"}</dd>
          </div>
          <div>
            <dt>Daemon starts</dt>
            <dd>{connection.daemonStarts ?? "0"}</dd>
          </div>
        </dl>
      {:else}
        <p class="empty">{failure?.message ?? firstLaunchFixture.notice.message}</p>
      {/if}
    </article>

    <article class="card fixture-card">
      <p class="label">Sanitized shared fixture</p>
      <h2>{firstLaunchFixture.notice.title}</h2>
      <p>{firstLaunchFixture.summary}</p>
      <div class="fixture-meta">
        <span>{firstLaunchFixture.id}</span>
        <span>{firstLaunchFixture.state}</span>
        <span>{firstLaunchFixture.notice.tone}</span>
      </div>
    </article>
  </section>

  <section class="events" aria-labelledby="events-title">
    <div>
      <p class="label">Redacted stream</p>
      <h2 id="events-title">Recent Plane activity</h2>
    </div>
    {#if recentEvents.length > 0}
      <ol>
        {#each recentEvents as event (event.sequence)}
          <li><span>{event.kind}</span><small>#{event.sequence}</small></li>
        {/each}
      </ol>
    {:else}
      <p class="empty">Event payloads remain native. Safe event labels will appear here.</p>
    {/if}
  </section>
</main>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(html) {
    color-scheme: light dark;
    --accent: #29b6f6;
    --background: #0d1115;
    --surface: #12181d;
    --surface-subtle: #0d1115;
    --border: #293138;
    --border-soft: #20282e;
    --text: #edf3f6;
    --muted: #9aa6ae;
    --quiet: #73808a;
    --pill: #aeb9c1;
    --warning: #f0a84b;
    --success: #50c878;
    --success-text: #8bdfa7;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    background: var(--background);
    color: var(--text);
    font-synthesis: none;
    text-rendering: optimizeLegibility;
  }

  :global(body) {
    margin: 0;
    min-width: 320px;
    min-height: 100vh;
    background: var(--background);
  }

  main {
    width: min(100%, 1080px);
    min-height: 100vh;
    margin: 0 auto;
    padding: 24px 32px 48px;
  }

  header,
  .card-heading,
  .events,
  li {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  header {
    min-height: 40px;
  }

  .brand,
  .status,
  .fixture-meta {
    display: flex;
    align-items: center;
    gap: 9px;
  }

  .brand {
    font-weight: 700;
    letter-spacing: -0.02em;
  }

  .mark,
  .status-dot {
    display: inline-block;
    border-radius: 999px;
  }

  .mark {
    width: 15px;
    height: 15px;
    background: #29b6f6;
  }

  .status {
    color: var(--muted);
    font-size: 13px;
  }

  .status-dot {
    width: 7px;
    height: 7px;
    background: var(--warning);
  }

  .status.online .status-dot {
    background: var(--success);
  }

  .hero {
    max-width: 710px;
    padding: 94px 0 52px;
  }

  .eyebrow,
  .label {
    margin: 0 0 12px;
    color: var(--accent);
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.11em;
    text-transform: uppercase;
  }

  h1,
  h2,
  p {
    margin-top: 0;
  }

  h1 {
    max-width: 650px;
    margin-bottom: 20px;
    font-size: clamp(42px, 7vw, 72px);
    line-height: 0.98;
    letter-spacing: -0.055em;
  }

  h2 {
    margin-bottom: 12px;
    font-size: 19px;
    letter-spacing: -0.025em;
  }

  .lede,
  .card > p,
  .empty {
    color: var(--muted);
    line-height: 1.65;
  }

  .lede {
    max-width: 650px;
    font-size: 16px;
  }

  .grid {
    display: grid;
    grid-template-columns: 1.15fr 0.85fr;
    border: 1px solid var(--border);
  }

  .card {
    min-height: 238px;
    padding: 26px;
    background: var(--surface);
  }

  .card + .card {
    border-left: 1px solid var(--border);
  }

  .pill,
  .fixture-meta span {
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--pill);
    font-size: 11px;
  }

  .pill {
    padding: 5px 9px;
  }

  .pill.online {
    border-color: rgba(80, 200, 120, 0.45);
    color: var(--success-text);
  }

  dl {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin: 50px 0 0;
  }

  dt {
    margin-bottom: 6px;
    color: var(--quiet);
    font-size: 11px;
    text-transform: uppercase;
  }

  dd {
    margin: 0;
    overflow: hidden;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 13px;
    text-overflow: ellipsis;
  }

  .fixture-meta {
    flex-wrap: wrap;
    margin-top: 28px;
  }

  .fixture-meta span {
    padding: 4px 8px;
  }

  .events {
    align-items: flex-start;
    gap: 32px;
    min-height: 128px;
    padding: 26px;
    border: solid var(--border);
    border-width: 0 1px 1px;
    background: var(--surface-subtle);
  }

  .events ol {
    width: min(100%, 430px);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    gap: 24px;
    padding: 7px 0;
    border-bottom: 1px solid var(--border-soft);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
  }

  li small {
    color: var(--quiet);
  }

  @media (prefers-color-scheme: light) {
    :global(html) {
      --accent: #006e9f;
      --background: #f4f6f7;
      --surface: #ffffff;
      --surface-subtle: #f8f9fa;
      --border: #d5dde1;
      --border-soft: #e5e9ec;
      --text: #152028;
      --muted: #52616b;
      --quiet: #687780;
      --pill: #40515c;
      --warning: #b86a00;
      --success: #16803d;
      --success-text: #176d37;
    }
  }

  @media (max-width: 720px) {
    main {
      padding-inline: 20px;
    }

    .hero {
      padding-top: 64px;
    }

    .grid {
      grid-template-columns: 1fr;
    }

    .card + .card {
      border-top: 1px solid var(--border);
      border-left: 0;
    }

    .events {
      display: block;
    }

    .events ol {
      margin-top: 20px;
    }
  }
</style>
