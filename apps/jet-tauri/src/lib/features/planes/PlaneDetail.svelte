<script lang="ts">
  import { tick, untrack } from "svelte";

  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import { focusLost } from "./focus";
  import PairingSection from "./PairingSection.svelte";
  import {
    featureLabel,
    featureName,
    formatFingerprint,
    identityPrefix,
    needsPairing,
    onlyForget,
    planeErrorCopy,
    planeStatus,
  } from "./model";

  let { session }: { session: DesktopSession } = $props();
  const planes = $derived(session.planes);
  const detailState = $derived(
    planes.detail?.planeId === planes.selectedPlaneId ? planes.detail : null,
  );
  const detail = $derived(detailState?.kind === "ready" ? detailState.detail : null);
  const plane = $derived(planes.selectedPlane ?? detail?.plane ?? null);
  const status = $derived(plane ? planeStatus(plane) : null);
  const connectionError = $derived(
    plane && (plane.connection.state === "failed" || plane.connection.state === "reconnecting")
      ? plane.connection.error
      : null,
  );
  const connectionIssues = $derived(
    detail?.issues.filter((issue) => issue.section === "connection" || issue.section === "status") ?? [],
  );
  const capabilitiesIssue = $derived(
    detail?.issues.find((issue) => issue.section === "capabilities") ?? null,
  );

  const identity = $derived(planes.snapshot?.identity ?? null);
  const pairingView = $derived(
    plane && planes.pairing.planeId === plane.planeId ? planes.pairing.view : null,
  );
  const forgetState = $derived(planes.forgetState);
  const forgetting = $derived(
    plane !== null && forgetState.kind !== "idle" && forgetState.planeId === plane.planeId ? forgetState : null,
  );
  /** Revoke-then-forget needs a current Pairing read that lists this computer. */
  const canRevokeFirst = $derived(
    plane?.connection.state === "online" &&
      pairingView !== null &&
      pairingView.mutations.allowed &&
      planes.pairing.state.kind === "ready" &&
      pairingView.clients.some((client) => client.isThisComputer),
  );
  const revokeBlockedReason = $derived(
    canRevokeFirst
      ? null
      : plane?.connection.state !== "online"
        ? `Revoking needs ${plane?.label ?? "the Plane"} to be connected.`
        : pairingView && !pairingView.mutations.allowed
          ? `Pairing changes are paused on ${plane?.label ?? "the Plane"}.`
          : `${plane?.label ?? "The Plane"} doesn't list this computer as paired.`,
  );

  let retrying = $state(false);
  let connectionHeading = $state<HTMLHeadingElement>();

  // Deep links land on their section heading. "pairing" and "clients" live
  // in PairingSection; "add" and "repair" open the wizard in PlanesPanel.
  $effect(() => {
    const request = planes.focusRequest;
    if (!request || request.section === "add" || request.section === "repair") return;
    untrack(() => {
      planes.focusRequest = null;
      void tick().then(() => {
        const id =
          request.section === "pairing"
            ? "plane-pairing-heading"
            : request.section === "clients"
              ? "plane-clients-heading"
              : null;
        const target = id ? document.getElementById(id) : null;
        (target ?? connectionHeading)?.focus();
      });
    });
  });

  let forgetHeading = $state<HTMLHeadingElement>();
  let forgetButton = $state<HTMLButtonElement>();
  let titleHeading = $state<HTMLHeadingElement>();
  let forgetShown = false;

  // The Forget choice replaces its button, or opens from the Connection
  // section further up: focus moves to its heading so it is announced. When
  // it closes without forgetting, focus returns to "Forget {label}…".
  $effect(() => {
    const shown = forgetting !== null;
    untrack(() => {
      if (shown && !forgetShown) void tick().then(() => forgetHeading?.focus());
      else if (!shown && forgetShown) {
        void tick().then(() => {
          if (focusLost()) forgetButton?.focus();
        });
      }
      forgetShown = shown;
    });
  });

  // Forgetting a Plane (or a revoke that forgets it) replaces this detail
  // with another Plane's: never leave keyboard focus on the page itself.
  $effect(() => {
    void plane?.planeId;
    untrack(() => {
      void tick().then(() => {
        if (focusLost()) titleHeading?.focus();
      });
    });
  });

  async function retry(): Promise<void> {
    if (!plane || retrying) return;
    retrying = true;
    try {
      await session.retryPlane(plane.planeId);
      await planes.loadDetail();
    } finally {
      retrying = false;
    }
  }

  function unauthorizedCopy(label: string): string {
    return `This computer isn't allowed on ${label}. It may have been disabled or revoked there, or never paired.`;
  }

  function keyCopy(key: string): string {
    switch (key) {
      case "present":
        return "Stored in your system keyring";
      case "not_created":
        return "Not created yet";
      case "session_only":
        return "Kept for this session only";
      case "unavailable":
        return "Keyring unavailable";
      case "locked":
        return "Keyring locked";
      case "unsupported":
        return "No supported secure storage on this computer";
      default:
        return "Checked when you pair with a remote Plane";
    }
  }
</script>

{#if !plane}
  <section class="plane-detail" aria-label="Plane detail">
    <p class="plane-muted">Choose a Plane to see its connection and features.</p>
  </section>
{:else}
  <section class="plane-detail" aria-labelledby="plane-detail-title">
    <h2 id="plane-detail-title" class="plane-detail-title" tabindex="-1" bind:this={titleHeading}>{plane.label}</h2>

    <section class="plane-section" aria-labelledby="plane-connection-heading">
      <h3 id="plane-connection-heading" tabindex="-1" bind:this={connectionHeading}>Connection</h3>
      <dl class="plane-facts">
        <div>
          <dt>State</dt>
          <dd>
            <span
              class:online={status?.tone === "ok"}
              class:failed={status?.tone === "danger"}
              class="status-dot"
              aria-hidden="true"
            ></span>
            {status?.text}
          </dd>
        </div>
        <div><dt>Core version</dt><dd>{plane.coreVersion ?? "Not read yet"}</dd></div>
        <div>
          <dt>Plane identity</dt>
          <dd>{identityPrefix(plane.planeIdentity) ? `Plane ${identityPrefix(plane.planeIdentity)}` : "Not read yet"}</dd>
        </div>
        {#if plane.kind === "remote"}
          <div><dt>SSH address</dt><dd>{plane.label}</dd></div>
        {/if}
      </dl>

      {#if plane.credential === "session"}
        <p class="plane-session-warning" role="note">
          This pairing ends when Jet quits. Set up secure storage to keep it.
        </p>
      {/if}

      {#if plane.security === "degraded"}
        <p class="plane-health" role="status">
          Security record can't be vouched for. Changes that need them are paused.
        </p>
      {/if}
      {#if plane.store === "read_only"}
        <p class="plane-health" role="status">Read-only recovery. Changes that need them are paused.</p>
      {/if}

      {#if connectionError}
        <div class="section-error plane-connection-error" role={plane.connection.state === "failed" ? "alert" : "status"}>
          <p>
            {#if connectionError.code === "connection.unauthorized"}
              {unauthorizedCopy(plane.label)}
            {:else if connectionError.code === "identity.session_ended"}
              This computer paired with {plane.label} for one session only. Pair again to reconnect.
            {:else if plane.connection.state === "reconnecting"}
              Reconnecting to {plane.label}…
            {:else}
              {planeErrorCopy(connectionError, plane.label)}
            {/if}
            <code>{connectionError.code}</code>
          </p>
        </div>
      {/if}
      {#each connectionIssues as issue (issue.section)}
        <p class="section-error">
          {planeErrorCopy(issue.error, plane.label)} <code>{issue.error.code}</code>
        </p>
      {/each}
      {#if detailState?.kind === "failed"}
        <p class="section-error">
          {planeErrorCopy(detailState.error, plane.label)} <code>{detailState.error.code}</code>
        </p>
      {/if}

      {#if plane.kind === "remote" && plane.connection.state === "failed" && onlyForget(connectionError)}
        <div class="plane-actions">
          <button class="secondary-button" onclick={() => planes.askForget(plane.planeId)}>Forget…</button>
        </div>
      {:else if plane.kind === "remote" && plane.connection.state === "failed" && needsPairing(connectionError)}
        <div class="plane-actions">
          <button class="primary-button" onclick={() => planes.startRepair(plane.planeId)}>Pair again</button>
          <button class="secondary-button" onclick={() => planes.askForget(plane.planeId)}>Forget…</button>
        </div>
      {:else if plane.connection.state !== "online" || connectionIssues.length > 0}
        <div class="plane-actions">
          <button class="secondary-button" disabled={retrying} onclick={retry}>
            {retrying ? "Retrying…" : "Retry"}
          </button>
        </div>
      {/if}
    </section>

    <section class="plane-section" aria-labelledby="plane-features-heading">
      <h3 id="plane-features-heading">Features</h3>
      <table class="plane-features">
        <thead>
          <tr><th scope="col">Feature</th><th scope="col">Status</th></tr>
        </thead>
        <tbody>
          {#each plane.features as feature (feature.feature)}
            <tr class:unsupported={feature.support === "unsupported"}>
              <th scope="row">{featureName(feature.feature)}</th>
              <td>{featureLabel(feature, plane.protocol)}</td>
            </tr>
          {/each}
        </tbody>
      </table>

      {#if detailState?.kind === "loading"}
        <p class="plane-muted" aria-live="polite">Checking {plane.label}…</p>
      {:else if capabilitiesIssue}
        <p class="section-error">
          {planeErrorCopy(capabilitiesIssue.error, plane.label)} <code>{capabilitiesIssue.error.code}</code>
        </p>
      {:else if detail}
        <dl class="plane-facts">
          {#if detail.platform}
            <div><dt>System</dt><dd>{detail.platform}</dd></div>
          {/if}
          <div>
            <dt>Harnesses</dt>
            <dd>{detail.harnesses.length > 0 ? detail.harnesses.join(", ") : "None installed"}</dd>
          </div>
          <div>
            <dt>Crafts</dt>
            <dd>
              {detail.crafts.length > 0
                ? detail.crafts.map(([craft, version]) => `${craft} ${version}`).join(", ")
                : "None installed"}
            </dd>
          </div>
          {#if detail.missingTools.length > 0}
            <div><dt>Missing tools</dt><dd>{detail.missingTools.join(", ")}</dd></div>
          {/if}
        </dl>
        {#if detail.degraded.length > 0}
          <ul class="plane-degraded" aria-label={`Conditions on ${plane.label}`}>
            {#each detail.degraded as condition, index (index)}
              <li>{condition}</li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    {#key plane.planeId}
      <PairingSection {session} {plane} />
    {/key}

    <section class="plane-section" aria-labelledby="plane-key-heading">
      <h3 id="plane-key-heading">This computer's pairing key</h3>
      <dl class="plane-facts">
        <div><dt>Key</dt><dd>{keyCopy(identity?.key ?? "unknown")}</dd></div>
        {#if identity?.fingerprint}
          <div><dt>Fingerprint</dt><dd><code>{formatFingerprint(identity.fingerprint)}</code></dd></div>
        {/if}
        {#if identity?.clientId}
          <div><dt>Client</dt><dd><code>{identityPrefix(identity.clientId) ?? identity.clientId}</code></dd></div>
        {/if}
      </dl>
    </section>

    {#if plane.kind === "remote"}
      <section class="plane-section" aria-labelledby="plane-forget-heading">
        <h3 id="plane-forget-heading" tabindex="-1" bind:this={forgetHeading}>Forget {plane.label}</h3>
        {#if !forgetting}
          <div class="plane-actions">
            <button bind:this={forgetButton} class="secondary-button" onclick={() => planes.askForget(plane.planeId)}>
              Forget {plane.label}…
            </button>
          </div>
        {:else}
          <p>
            Forget only removes {plane.label} from this computer's list and hides its tasks here. {plane.label} will
            still list this computer as paired until someone revokes it there.
          </p>
          {#if forgetting.kind === "failed"}
            <p class="section-error" role="alert">
              {planeErrorCopy(forgetting.error, plane.label)} <code>{forgetting.error.code}</code>
            </p>
          {/if}
          <div class="plane-actions">
            {#if canRevokeFirst}
              <button
                class="danger-button"
                disabled={forgetting.kind === "forgetting"}
                onclick={() => planes.revokeThenForget(plane.planeId)}
              >
                Revoke this computer's access on {plane.label}, then forget
              </button>
            {:else}
              <button
                class="danger-button"
                aria-disabled="true"
                aria-describedby="plane-revoke-reason"
                onclick={(event) => event.preventDefault()}
              >
                Revoke this computer's access on {plane.label}, then forget
              </button>
            {/if}
            <button
              class="secondary-button"
              disabled={forgetting.kind === "forgetting"}
              onclick={() => planes.forget(plane.planeId)}
            >
              {forgetting.kind === "forgetting" ? "Forgetting…" : "Forget only"}
            </button>
            <button class="text-button" disabled={forgetting.kind === "forgetting"} onclick={() => planes.cancelForget()}>
              Cancel
            </button>
          </div>
          {#if !canRevokeFirst}
            <p id="plane-revoke-reason" class="plane-muted">{revokeBlockedReason}</p>
          {/if}
        {/if}
      </section>
    {/if}
  </section>
{/if}

<style>
  .plane-detail {
    display: grid;
    gap: 18px;
    min-width: 0;
  }

  .plane-detail-title {
    margin: 0;
    font-size: 18px;
  }

  .plane-section {
    display: grid;
    gap: 10px;
    padding: 16px 18px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
  }

  .plane-section h3 {
    margin: 0;
    font-size: 14px;
  }

  .plane-section h3:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .plane-facts {
    display: grid;
    gap: 6px;
    margin: 0;
  }

  .plane-facts div {
    display: grid;
    grid-template-columns: 140px minmax(0, 1fr);
    gap: 12px;
  }

  .plane-facts dt {
    color: var(--muted);
  }

  .plane-facts dd {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .plane-health {
    margin: 0;
    color: var(--warning);
  }

  .plane-session-warning {
    margin: 0;
    color: var(--warning);
    font-weight: 600;
  }

  .plane-section > p {
    margin: 0;
  }

  .plane-actions {
    flex-wrap: wrap;
  }

  .plane-actions button[aria-disabled="true"] {
    opacity: 0.55;
  }

  .plane-connection-error p {
    margin: 0;
  }

  .plane-actions {
    display: flex;
    gap: 8px;
  }

  .plane-features {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  .plane-features th,
  .plane-features td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--border-soft);
    text-align: left;
  }

  .plane-features thead th {
    color: var(--muted);
    font-size: 11px;
    font-weight: 650;
    text-transform: uppercase;
  }

  .plane-features tbody th {
    font-weight: 500;
  }

  .plane-features tr.unsupported td {
    color: var(--warning);
  }

  .plane-degraded {
    margin: 0;
    padding-left: 18px;
    color: var(--warning);
  }

  .plane-muted {
    margin: 0;
    color: var(--muted);
  }
</style>
