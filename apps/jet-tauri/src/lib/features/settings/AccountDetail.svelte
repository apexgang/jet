<script lang="ts">
  import { onMount, untrack } from "svelte";

  import { AUTO_CONTINUE_LIMITS, type AccountDetail, type AccountView } from "$lib/jet/agents";
  import {
    credentialSourceText,
    draftFor,
    policyFromDraft,
    policyText,
    quotaText,
    retryDraft,
    type AutoContinueDraft,
  } from "./agents-model";
  import { sectionData, utf8Bytes, withIssues } from "./model";
  import OperationStatus from "./OperationStatus.svelte";
  import SectionState from "./SectionState.svelte";
  import type { SettingsSession } from "./session.svelte";
  import SettingsDialog from "./SettingsDialog.svelte";

  let { session, account }: { session: SettingsSession; account: AccountView } = $props();

  const agents = $derived(session.agents);
  const id = $props.id();

  const detailState = $derived(agents.details[account.id] ?? { kind: "loading" as const, last: null });
  const detail = $derived(sectionData(detailState));
  const block = $derived(session.agentsBlock());
  const changed = $derived(agents.changedDetails.includes(account.id));
  const autoTarget = $derived({ action: "auto_continue" as const, bindingId: account.id });
  const autoOperation = $derived(agents.operationFor(autoTarget));
  const review = $derived(
    autoOperation.kind === "confirm" && autoOperation.review.subject.kind === "auto_continue"
      ? autoOperation.review.subject
      : null,
  );

  const unbinding = $derived.by(() => {
    const kind = agents.operationFor({ action: "unbind", bindingId: account.id }).kind;
    return kind === "preparing" || kind === "applying" || kind === "confirm";
  });

  /** The form; `null` shows the Plane's policy until the user edits. */
  let draft = $state<AutoContinueDraft | null>(null);
  let attempted = $state(false);
  let removing = $state(false);

  const shown = $derived(draft ?? draftFor(detail?.autoContinue?.policy ?? null));
  const checked = $derived(policyFromDraft(shown));
  const messageBytes = $derived(shown.mode === "retry" ? utf8Bytes(shown.message) : 0);
  const unreadableMessage = $derived(
    draft === null && detail?.autoContinue?.policy.mode === "retry" && detail.autoContinue.policy.message === null,
  );

  function edit(next: AutoContinueDraft): void {
    draft = next;
  }

  function editField(field: "delaySeconds" | "maxDelaySeconds" | "maxRetries" | "message", value: string): void {
    if (shown.mode !== "retry") return;
    draft = { ...shown, [field]: value };
  }

  async function save(): Promise<void> {
    attempted = true;
    if ("problem" in checked) return;
    await agents.prepareAutoContinue(account.id, checked.policy);
  }

  async function confirmPolicy(): Promise<void> {
    await agents.confirm();
    const outcome = agents.operationFor(autoTarget).kind;
    if (outcome === "done") {
      draft = null;
      attempted = false;
    }
  }

  function cancelReview(): void {
    agents.dismiss();
  }

  async function remove(): Promise<void> {
    await agents.unbind(account.id);
    removing = false;
  }

  onMount(() => {
    untrack(() => void agents.loadDetail(account.id));
    return () => agents.closeDetail(account.id);
  });
</script>

<div class="account-detail">
  <p class="fact">{credentialSourceText(account.credentialSource)}{account.providerAccount ? ` · ${account.providerAccount}` : ""}</p>

  {#if changed}
    <div class="notice" role="status">
      <p>Changed on {session.planeLabel} since you opened this account.</p>
      <button class="text-button" onclick={() => void agents.loadDetail(account.id)}>Show current values</button>
    </div>
  {/if}

  <h4>Usage limits</h4>
  <SectionState
    state={withIssues(detailState, ["quota"])}
    title="Usage limits"
    planeLabel={session.planeLabel}
    onretry={() => void agents.loadDetail(account.id)}
  >
    {#snippet children(value: AccountDetail)}
      {#if value.quotaWindows.length === 0 && !value.issues.some((issue) => issue.section === "quota")}
        <p>The provider hasn't reported usage limits for this account.</p>
      {:else}
        <ul class="quota">
          {#each value.quotaWindows as window, index (index)}
            <li class:warning={window.freshness !== "fresh"}>{quotaText(window)}</li>
          {/each}
        </ul>
      {/if}
    {/snippet}
  </SectionState>

  <h4 id={`${id}-auto`}>When a usage limit is reached</h4>
  {#if detail && detail.autoContinue === null && detailState.kind === "ready"}
    {@const issue = detailState.issues.find((entry) => entry.section === "auto_continue")}
    {#if issue}
      <p class="notice" role="status">
        {issue.error.category === "incompatible"
          ? `Continuing automatically isn't available on ${session.planeLabel} yet.`
          : issue.error.message}
        <code>{issue.error.code}</code>
      </p>
    {/if}
  {:else if detail?.autoContinue}
    {@const retry = detail.autoContinue.retry}
    <fieldset class="policy" aria-labelledby={`${id}-auto`} disabled={block !== null || !agents.idle}>
      <label class="choice">
        <input
          type="radio"
          name={`${id}-mode`}
          checked={shown.mode === "off"}
          onchange={() => edit({ mode: "off" })}
        />
        <span>Wait for me</span>
      </label>
      <label class="choice">
        <input
          type="radio"
          name={`${id}-mode`}
          checked={shown.mode === "retry"}
          onchange={() => edit(detail.autoContinue?.policy.mode === "retry" ? draftFor(detail.autoContinue.policy) : retryDraft())}
        />
        <span>Continue automatically</span>
      </label>
      {#if shown.mode === "retry"}
        <div class="fields">
          <label for={`${id}-delay`}>First wait (seconds)</label>
          <input
            id={`${id}-delay`}
            type="number"
            inputmode="decimal"
            min="0.001"
            max={AUTO_CONTINUE_LIMITS.maxDelayMs / 1000}
            step="any"
            value={shown.delaySeconds}
            oninput={(event) => editField("delaySeconds", event.currentTarget.value)}
          />
          <label for={`${id}-max`}>Longest wait (seconds)</label>
          <input
            id={`${id}-max`}
            type="number"
            inputmode="decimal"
            min="0.001"
            max={AUTO_CONTINUE_LIMITS.maxDelayMs / 1000}
            step="any"
            value={shown.maxDelaySeconds}
            oninput={(event) => editField("maxDelaySeconds", event.currentTarget.value)}
          />
          <label for={`${id}-retries`}>Retries</label>
          <input
            id={`${id}-retries`}
            type="number"
            inputmode="numeric"
            min={AUTO_CONTINUE_LIMITS.minRetries}
            max={AUTO_CONTINUE_LIMITS.maxRetries}
            step="1"
            value={shown.maxRetries}
            oninput={(event) => editField("maxRetries", event.currentTarget.value)}
          />
          <label for={`${id}-message`}>Message Jet sends to continue</label>
          <textarea
            id={`${id}-message`}
            rows="2"
            aria-describedby={`${id}-bytes`}
            value={shown.message}
            oninput={(event) => editField("message", event.currentTarget.value)}
          ></textarea>
          <p class="hint" id={`${id}-bytes`}>
            {messageBytes.toLocaleString("en-US")} of {AUTO_CONTINUE_LIMITS.maxMessageBytes.toLocaleString("en-US")} bytes.
            A reset time from the provider comes first; otherwise each wait doubles up to the longest wait.
          </p>
        </div>
      {/if}
      {#if unreadableMessage}
        <p class="hint">The current message can't be displayed. Write a new one to change this.</p>
      {/if}
      {#if attempted && "problem" in checked}
        <p class="field-error" role="alert">{checked.problem}</p>
      {/if}
      {#if draft !== null}
        <div class="actions">
          <button type="button" class="secondary-button" onclick={() => ((draft = null), (attempted = false))}>
            Cancel
          </button>
          <button type="button" class="primary-button" onclick={() => void save()}>Save</button>
        </div>
      {/if}
    </fieldset>
    {#if retry}
      <p class="hint">
        Last automatic retry: {retry.status.replace("_", " ")}, {retry.retryCount}
        {retry.retryCount === 1 ? "retry" : "retries"} so far.
      </p>
    {/if}
    <OperationStatus
      {agents}
      matches={(target) => target.action === "auto_continue" && target.bindingId === account.id}
      planeLabel={session.planeLabel}
      oncheck={() => void agents.loadDetail(account.id)}
    />
  {/if}

  <div class="remove">
    <button
      class="text-button danger"
      disabled={block !== null || !agents.idle}
      onclick={() => (removing = true)}
    >
      Remove account…
    </button>
  </div>
</div>

{#if review}
  <SettingsDialog
    title="Change what happens when a usage limit is reached?"
    lead={`This applies to ${account.label} on ${session.planeLabel}.`}
    oncancel={cancelReview}
  >
    <dl class="removal-facts">
      <div><dt>Now</dt><dd>{policyText(review.before)}</dd></div>
      <div><dt>After</dt><dd>{policyText(review.after)}</dd></div>
      {#if review.after.mode === "retry" && review.after.message !== null}
        <div><dt>Message</dt><dd>{review.after.message}</dd></div>
      {/if}
    </dl>
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel onclick={cancelReview}>Cancel</button>
      <button type="button" class="primary-button" data-dialog-primary onclick={() => void confirmPolicy()}>Save</button>
    {/snippet}
  </SettingsDialog>
{/if}

{#if removing}
  <SettingsDialog
    title={`Remove ${account.label}?`}
    lead={`Jet stops using ${account.label} on ${session.planeLabel}. Tasks already running keep their account.`}
    focus="cancel"
    oncancel={() => {
      if (!unbinding) removing = false;
    }}
  >
    <dl class="removal-facts">
      <div><dt>Account</dt><dd>{account.label}</dd></div>
      <div><dt>Provider</dt><dd>{account.provider}</dd></div>
      <div><dt>Sign-in</dt><dd>{credentialSourceText(account.credentialSource)}</dd></div>
    </dl>
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel disabled={unbinding} onclick={() => (removing = false)}>
        Cancel
      </button>
      <button type="button" class="danger-button" data-dialog-primary disabled={unbinding} onclick={() => void remove()}>
        {unbinding ? "Removing…" : "Remove account"}
      </button>
    {/snippet}
  </SettingsDialog>
{/if}

<style>
  .account-detail {
    display: grid;
    gap: 8px;
    padding: 4px 0 8px;
  }

  h4 {
    margin: 6px 0 0;
    font-size: 12px;
    font-weight: 650;
  }

  .fact,
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .quota {
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
  }

  .quota .warning {
    color: var(--warning);
  }

  .policy {
    display: grid;
    gap: 8px;
    margin: 0;
    padding: 0;
    border: 0;
  }

  .choice {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
  }

  .choice input {
    accent-color: var(--accent);
  }

  .fields {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 8px 12px;
    align-items: center;
    padding-left: 24px;
  }

  .fields label {
    color: var(--muted);
    font-size: 12px;
  }

  .fields input,
  .fields textarea {
    min-width: 0;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font: inherit;
  }

  .fields input {
    width: 14ch;
  }

  .fields .hint {
    grid-column: 1 / -1;
  }

  .field-error {
    margin: 0;
    color: var(--warning);
    font-size: 12px;
  }

  .actions {
    display: flex;
    gap: 6px;
  }

  .notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 13px;
  }

  .notice p {
    margin: 0;
  }

  .remove {
    padding-top: 4px;
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }

  @media (max-width: 760px) {
    .fields {
      grid-template-columns: minmax(0, 1fr);
      padding-left: 0;
    }
  }
</style>
