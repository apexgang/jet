<script lang="ts">
  import type { CraftPreview } from "$lib/jet/agents";
  import type { AgentsSession } from "./agents-session.svelte";
  import { brokerPermissionText, hostAccessText, releaseProblem } from "./agents-model";
  import { blockText } from "./model";
  import SettingsDialog from "./SettingsDialog.svelte";

  let {
    agents,
    planeLabel,
    isLocalPlane,
    developerMode,
    block,
    returnFocus = [],
    onclose,
    onopenpermissions,
  }: {
    agents: AgentsSession;
    planeLabel: string;
    /** Local files exist only for the Plane on this computer. */
    isLocalPlane: boolean;
    /** `craft.developer_mode` on this Plane; `null` when unknown. */
    developerMode: boolean | null;
    /** Changes are paused (read-only Recovery or a stale view). */
    block: "read_only" | "stale" | null;
    /** Where focus goes when the dialog closes and its trigger is gone. */
    returnFocus?: readonly string[];
    onclose: () => void;
    /** Opens Safety › Permissions, where Developer Mode lives. */
    onopenpermissions: () => void;
  } = $props();

  const TARGET = { action: "install" } as const;
  const id = $props.id();

  let kind = $state<"github_release" | "local">("github_release");
  let repository = $state("");
  let tag = $state("");
  let attempted = $state(false);

  const operation = $derived(agents.operationFor(TARGET));
  const busy = $derived(operation.kind === "preparing" || operation.kind === "applying");
  const preview = $derived(
    operation.kind === "confirm" && operation.review.subject.kind === "install_craft"
      ? operation.review.subject.preview
      : null,
  );
  const localAllowed = $derived(isLocalPlane && developerMode === true);
  const problem = $derived(kind === "github_release" ? releaseProblem(repository, tag) : null);
  const picked = $derived(agents.localPick.kind === "picked" ? agents.localPick.source : null);

  function cancel(): void {
    if (busy || agents.localPick.kind === "picking") return;
    agents.dismiss();
    onclose();
  }

  async function check(): Promise<void> {
    if (block !== null) return;
    attempted = true;
    if (kind === "github_release") {
      if (problem) return;
      await agents.discover({ type: "github_release", repository: repository.trim(), tag: tag.trim() });
    } else if (picked) {
      await agents.discover({ type: "local", source_token: picked.sourceToken });
    }
  }

  async function install(): Promise<void> {
    if (block !== null) return;
    await agents.confirm();
    const outcome = agents.operationFor(TARGET).kind;
    // Queued or uncertain: the Harnesses section reports it.
    if (outcome === "done" || outcome === "uncertain") onclose();
  }

  function editAgain(): void {
    // The native review simply expires; nothing was sent.
    agents.dismiss();
  }

  function trustText(trust: CraftPreview["trust"]): string {
    return trust === "developer_source"
      ? "Built from local files. Jet can't verify it. Runs with your user account's permissions."
      : "Runs with your user account's permissions.";
  }
</script>

<SettingsDialog
  title={preview ? `Install ${preview.craftId} ${preview.version}?` : "Add a Craft"}
  lead={preview
    ? `Review what this Craft can do on ${planeLabel} before installing it.`
    : `Jet checks the release before anything is installed on ${planeLabel}.`}
  {returnFocus}
  oncancel={cancel}
>
  {#if preview}
    <dl class="removal-facts">
      <div><dt>Plane</dt><dd>{planeLabel}</dd></div>
      <div><dt>Craft</dt><dd>{preview.craftId} · {preview.version}</dd></div>
      <div><dt>Source</dt><dd>{preview.source === "local" ? "Local files" : preview.repository}</dd></div>
      <div><dt>Publisher claim</dt><dd>{preview.publisherClaim} <small>(Not verified by Jet)</small></dd></div>
      <div><dt>Commit</dt><dd class="mono">{preview.commit}</dd></div>
      <div><dt>Artifact SHA-256</dt><dd class="mono selectable">{preview.artifactSha256}</dd></div>
      {#if preview.enabledFeatures.length > 0}
        <div><dt>Features</dt><dd>{preview.enabledFeatures.join(", ")}</dd></div>
      {/if}
      <div>
        <dt>Jet access</dt>
        <dd>
          {preview.brokerPermissions.length === 0
            ? "None"
            : preview.brokerPermissions.map(brokerPermissionText).join(", ")}
        </dd>
      </div>
      <div>
        <dt>Computer access</dt>
        <dd>
          {#if preview.hostAccess.length === 0}
            None declared
          {:else}
            <ul class="access">
              {#each preview.hostAccess as access, index (index)}
                <li>{hostAccessText(access)}</li>
              {/each}
            </ul>
          {/if}
        </dd>
      </div>
      <div><dt>Trust</dt><dd>{trustText(preview.trust)}</dd></div>
    </dl>
  {:else}
    <fieldset class="sources">
      <legend>Where the Craft comes from</legend>
      <label class="choice">
        <input type="radio" name={`${id}-kind`} value="github_release" bind:group={kind} disabled={busy} />
        <span>A GitHub release</span>
      </label>
      {#if localAllowed}
        <label class="choice">
          <input type="radio" name={`${id}-kind`} value="local" bind:group={kind} disabled={busy} />
          <span>Use local files (Developer Mode)</span>
        </label>
      {/if}
    </fieldset>

    {#if !localAllowed}
      <div class="dialog-note local-note">
        {#if !isLocalPlane}
          Local files can only be added to the Plane on this computer.
        {:else}
          Local files need Developer Mode on this Plane.
          <button type="button" class="text-button" onclick={onopenpermissions}>Open Permissions</button>
        {/if}
      </div>
    {/if}

    {#if kind === "github_release"}
      <label for={`${id}-repository`}>GitHub repository (owner/name)</label>
      <input
        id={`${id}-repository`}
        type="text"
        autocomplete="off"
        spellcheck="false"
        placeholder="owner/name"
        bind:value={repository}
        disabled={busy}
      />
      <label for={`${id}-tag`}>Release tag</label>
      <input
        id={`${id}-tag`}
        type="text"
        autocomplete="off"
        spellcheck="false"
        bind:value={tag}
        disabled={busy}
      />
      {#if attempted && problem}<p class="dialog-alert" role="alert">{problem}</p>{/if}
    {:else}
      <div class="local-files">
        <button
          type="button"
          class="secondary-button"
          disabled={busy || agents.localPick.kind === "picking"}
          onclick={() => void agents.pickLocalSource()}
        >
          {agents.localPick.kind === "picking" ? "Choosing…" : picked ? "Choose other files…" : "Choose files…"}
        </button>
        {#if picked}
          <dl class="removal-facts">
            <div><dt>Specification</dt><dd>{picked.specificationName}</dd></div>
            <div><dt>Built Craft</dt><dd>{picked.artifactName}</dd></div>
          </dl>
        {:else if agents.localPick.kind === "failed"}
          <p class="dialog-alert" role="alert">
            {agents.localPick.error.message} <code>{agents.localPick.error.code}</code>
          </p>
        {:else if operation.kind === "refused"}
          <!-- A check uses up the chosen files, even when it fails. -->
          <p class="dialog-note">Choose the files again to check them.</p>
        {:else}
          <p class="dialog-note">Choose craft-spec.toml, then the built Craft executable.</p>
        {/if}
      </div>
    {/if}
  {/if}

  {#if block !== null}
    <p class="dialog-note" role="status">{blockText(block, planeLabel)}</p>
  {/if}

  {#if operation.kind === "refused"}
    <p class="dialog-alert" role="alert">{operation.error.message} <code>{operation.error.code}</code></p>
  {/if}

  {#snippet footer()}
    <button type="button" class="secondary-button" data-dialog-cancel disabled={busy} onclick={cancel}>Cancel</button>
    {#if preview}
      <button type="button" class="secondary-button" disabled={busy} onclick={editAgain}>Back</button>
      <button
        type="button"
        class="primary-button"
        data-dialog-primary
        disabled={busy || block !== null}
        onclick={() => void install()}
      >
        {operation.kind === "applying" ? "Installing…" : `Install ${preview.craftId} ${preview.version}`}
      </button>
    {:else}
      <button
        type="button"
        class="primary-button"
        data-dialog-primary
        disabled={busy || block !== null || (kind === "local" && !picked)}
        onclick={() => void check()}
      >
        {operation.kind === "preparing" ? "Checking…" : "Check"}
      </button>
    {/if}
  {/snippet}
</SettingsDialog>

<style>
  .sources {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    border: 0;
  }

  legend {
    margin-bottom: 6px;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
  }

  .choice {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text) !important;
    font-size: 13px !important;
    font-weight: 500 !important;
  }

  .choice input[type="radio"] {
    width: auto;
    margin: 0;
    accent-color: var(--accent);
  }

  .local-note {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .local-files {
    display: grid;
    gap: 10px;
    justify-items: start;
  }

  .mono {
    font-family: ui-monospace, "SFMono-Regular", Menlo, monospace;
  }

  .selectable {
    user-select: text;
  }

  .access {
    margin: 0;
    padding-left: 16px;
  }

  small {
    color: var(--muted);
  }
</style>
