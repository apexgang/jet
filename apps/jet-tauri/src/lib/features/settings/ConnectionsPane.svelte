<script lang="ts">
  import { onMount } from "svelte";

  import { planeStatus } from "$lib/features/planes/model";
  import { publicError } from "$lib/jet/errors";
  import {
    LOCAL_PLANE,
    listPlanes,
    loadPlaneDetail,
    type ClientIdentity,
    type PlaneDetail,
    type PlaneHealth,
    type PlanesSnapshot,
  } from "$lib/jet/planes";
  import SectionState from "./SectionState.svelte";
  import { sectionData, sectionStateFor, type SectionState as State } from "./model";

  const LOCAL_LABEL = "This computer";

  let local = $state<State<PlaneDetail>>({ kind: "loading", last: null });
  let planes = $state<State<PlanesSnapshot>>({ kind: "loading", last: null });
  let mounted = false;
  let localRequest = 0;
  let planesRequest = 0;

  /** Status and capabilities of the local Jet service. Read-only. */
  async function loadLocal() {
    const request = ++localRequest;
    const last = sectionData(local);
    local = { kind: "loading", last };
    try {
      const detail = await loadPlaneDetail(LOCAL_PLANE);
      if (!mounted || request !== localRequest) return;
      const unreachable = detail.issues.find((issue) => issue.section === "connection");
      local = unreachable
        ? sectionStateFor(unreachable.error, detail)
        : { kind: "ready", data: detail, freshness: "live", issues: detail.issues };
    } catch (error: unknown) {
      if (!mounted || request !== localRequest) return;
      local = sectionStateFor(publicError(error), last);
    }
  }

  /** The native Plane registry. It never connects to a Plane. */
  async function loadPlanes() {
    const request = ++planesRequest;
    const last = sectionData(planes);
    planes = { kind: "loading", last };
    try {
      const snapshot = await listPlanes();
      if (!mounted || request !== planesRequest) return;
      planes = { kind: "ready", data: snapshot, freshness: "live", issues: [] };
    } catch (error: unknown) {
      if (!mounted || request !== planesRequest) return;
      planes = sectionStateFor(publicError(error), last);
    }
  }

  function reload() {
    void loadLocal();
    void loadPlanes();
  }

  function securityText(security: PlaneHealth["security"]): string {
    switch (security) {
      case "trusted":
        return "Audit is healthy";
      case "degraded":
        return "Audit needs repair";
      default:
        return "Not reported by this Jet service";
    }
  }

  function storeText(store: PlaneHealth["store"]): string {
    switch (store) {
      case "serving":
        return "Serving";
      case "read_only":
        return "Read-only recovery";
      default:
        return "Not reported by this Jet service";
    }
  }

  function keyText(key: ClientIdentity["key"]): string {
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

  onMount(() => {
    mounted = true;
    reload();
    // The registry and the local service change outside this window.
    const onFocus = () => reload();
    window.addEventListener("focus", onFocus);
    return () => {
      mounted = false;
      window.removeEventListener("focus", onFocus);
    };
  });
</script>

<p class="pane-help">
  Manage Planes, pairing and paired computers from Planes in the main window. This page only shows
  their state.
</p>

<section class="settings-section" aria-labelledby="section-local-service">
  <h2 id="section-local-service" tabindex="-1">Local service</h2>
  <SectionState state={local} title="Local service" planeLabel={LOCAL_LABEL} onretry={() => void loadLocal()}>
    {#snippet children(detail: PlaneDetail)}
      <dl class="facts">
        <div>
          <dt>Jet service</dt>
          <dd>
            {local.kind === "ready" ? "Running on this computer" : local.kind === "loading" ? "Checking…" : "Not reachable"}
          </dd>
        </div>
        <div><dt>Version</dt><dd>{detail.plane.coreVersion ?? "Not reported"}</dd></div>
        {#if detail.platform}
          <div><dt>Platform</dt><dd>{detail.platform}</dd></div>
        {/if}
        <div><dt>Audit</dt><dd>{securityText(detail.plane.security)}</dd></div>
        <div><dt>Storage</dt><dd>{storeText(detail.plane.store)}</dd></div>
      </dl>
    {/snippet}
  </SectionState>
</section>

<section class="settings-section" aria-labelledby="section-planes">
  <h2 id="section-planes" tabindex="-1">Planes</h2>
  <SectionState state={planes} title="Planes" planeLabel={LOCAL_LABEL} onretry={() => void loadPlanes()}>
    {#snippet children(snapshot: PlanesSnapshot)}
      <ul class="plane-list" aria-label="Registered Planes">
        {#each snapshot.planes as plane (plane.planeId)}
          <li>
            <span class="plane-label">{plane.label}</span>
            <span class="plane-kind">{plane.kind === "local" ? "This computer" : "Remote over SSH"}</span>
            <span class="plane-state">{planeStatus(plane).text}</span>
          </li>
        {/each}
      </ul>
      <p class="hint">Connection states are the ones the main window last saw.</p>
      <dl class="facts">
        <div>
          <dt>This computer's pairing key</dt>
          <dd>
            {keyText(snapshot.identity.key)}
            {#if snapshot.identity.fingerprint}<code>{snapshot.identity.fingerprint}</code>{/if}
          </dd>
        </div>
        <div><dt>Client</dt><dd><code>{snapshot.identity.clientId.slice(0, 8)}</code></dd></div>
      </dl>
    {/snippet}
  </SectionState>
</section>

<style>
  .pane-help,
  .hint {
    margin: 0;
    color: var(--muted);
  }

  .facts {
    display: grid;
    gap: 6px;
    margin: 0;
  }

  .facts div {
    display: grid;
    grid-template-columns: minmax(120px, 200px) minmax(0, 1fr);
    gap: 12px;
  }

  .facts dt {
    color: var(--muted);
  }

  .facts dd {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    min-width: 0;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .plane-list {
    display: grid;
    gap: 0;
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .plane-list li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: 12px;
    padding: 10px 12px;
  }

  .plane-list li + li {
    border-top: 1px solid var(--border-soft);
  }

  .plane-label {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .plane-kind,
  .plane-state {
    color: var(--muted);
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
