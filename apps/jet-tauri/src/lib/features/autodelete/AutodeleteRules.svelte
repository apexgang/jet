<script lang="ts">
  import type { SettingsSection } from "$lib/jet/settings-window";
  import type { AutodeleteRule, AutodeleteRules } from "$lib/jet/retention";
  import SectionState from "$lib/features/settings/SectionState.svelte";
  import SettingsDialog from "$lib/features/settings/SettingsDialog.svelte";
  import { blockText, landedTarget } from "$lib/features/settings/model";
  import { formatDate, protectionLine } from "$lib/features/trash/model";
  import {
    CHANGED_TEXT,
    COMPILING_TEXT,
    DELETE_TEXT,
    NEW_RULE,
    NO_BINDING_TEXT,
    PENDING_TEXT,
    STILL_PREPARING_TEXT,
    approveText,
    approvedText,
    attributionText,
    byteCounter,
    candidatesHeading,
    changeRefusalText,
    draftText,
    draftingText,
    everywhereText,
    introText,
    parseDays,
    pendingLabel,
    promptBytes,
    promptFits,
    refusalCopy,
    PROMPT_LIMIT,
  } from "./model";
  import type { AutodeleteSession } from "./session.svelte";

  let {
    autodelete,
    planeLabel,
    graceDays,
    onopen,
  }: {
    autodelete: AutodeleteSession;
    planeLabel: string;
    /** `retention.trash_grace_days` as the Retention rows read it. */
    graceDays: number | null;
    /** Shows another Settings section in this window. */
    onopen: (section: SettingsSection) => void;
  } = $props();

  const id = $props.id();
  const now = $derived.by(() => {
    void autodelete.rules;
    return Date.now();
  });
  const block = $derived(autodelete.block);
  const editor = $derived(autodelete.editor);
  const dialog = $derived(autodelete.dialog);
  const sending = $derived(editor.kind === "sending");
  const agentsLinked = landedTarget("utility") !== null;
  const composeBytes = $derived(promptBytes(autodelete.compose));
  const composeFits = $derived(promptFits(autodelete.compose));
  const dialogRule = $derived(dialog.kind === "closed" ? null : autodelete.rule(dialog.ruleId));

  function sendingFor(slot: string): boolean {
    return editor.kind === "sending" && editor.slot === slot;
  }

  function errorFor(slot: string) {
    return editor.kind === "error" && editor.slot === slot ? editor.error : null;
  }

  function ruleText(rule: AutodeleteRule): string {
    return rule.prompt === "" ? "Rule text unavailable" : rule.prompt;
  }

  function nameOf(conversationId: string): string {
    if (!(conversationId in autodelete.names)) return "Loading name…";
    return autodelete.names[conversationId] ?? "Name unavailable";
  }
</script>

<svelte:window onfocus={() => void autodelete.reloadIfLoaded()} />

<div class="autodelete">
  <p>{introText(graceDays)}</p>

  {#snippet agentsLink()}
    {#if agentsLinked}
      <button type="button" class="text-button" onclick={() => onopen("utility")}>Open Agents settings</button>
    {/if}
  {/snippet}

  {#snippet pendingNotice(slot: string)}
    {@const change = autodelete.pending[slot]}
    {#if change}
      <div class="notice pending" role="status">
        <p>{PENDING_TEXT} <span class="quiet">({pendingLabel(change)})</span></p>
        <button
          type="button"
          class="secondary-button"
          disabled={sending || block === "read_only"}
          onclick={() => void autodelete.retry(slot)}
        >
          {sendingFor(slot) ? "Sending…" : "Try again"}
        </button>
      </div>
    {/if}
  {/snippet}

  {#snippet refusal(slot: string)}
    {@const error = errorFor(slot)}
    {#if error}
      <div class="notice critical" role="alert">
        <p>{changeRefusalText(error)} <code>{error.code}</code></p>
        {#if error.code === "utility.disabled"}{@render agentsLink()}{/if}
        <button type="button" class="text-button" onclick={() => autodelete.dismissError()}>Dismiss</button>
      </div>
    {/if}
  {/snippet}

  <SectionState state={autodelete.rules} title="Auto-delete" {planeLabel} onretry={() => void autodelete.refresh()}>
    {#snippet children(view: AutodeleteRules)}
      <div class="compose">
        <p class="disclosure" class:off={view.drafting.enabled === false}>
          {draftingText(view.drafting.enabled)}
          {#if view.drafting.enabled === false}{@render agentsLink()}{/if}
        </p>
        {#if view.drafting.enabled === true && view.drafting.bindingConfigured === false}
          <p class="disclosure off">{NO_BINDING_TEXT} {@render agentsLink()}</p>
        {/if}
        <label for={`${id}-rule`}>Rule</label>
        <textarea
          id={`${id}-rule`}
          rows="3"
          placeholder="For example: tasks I haven't touched in two months"
          aria-describedby={`${id}-bytes`}
          bind:value={autodelete.compose}
          disabled={!autodelete.canChange(NEW_RULE)}
        ></textarea>
        <div class="compose-actions">
          <span id={`${id}-bytes`} class="quiet" class:over={composeBytes > PROMPT_LIMIT}>
            {byteCounter(autodelete.compose)}
          </span>
          <button
            type="button"
            class="primary-button"
            disabled={!composeFits || !autodelete.canChange(NEW_RULE)}
            onclick={() => void autodelete.createDraft()}
          >
            {sendingFor(NEW_RULE) ? "Creating…" : "Create draft"}
          </button>
        </div>
        {@render pendingNotice(NEW_RULE)}
        {@render refusal(NEW_RULE)}
        {#if block !== null && view.rules.length + Object.keys(autodelete.pending).length > 0}
          <p class="quiet" role="status">{blockText(block, planeLabel)}</p>
        {/if}
      </div>

      <div class="list-heading">
        <h4>Rules on {planeLabel}</h4>
        <button type="button" class="text-button" onclick={() => void autodelete.refresh()}>Refresh</button>
      </div>
      {#if autodelete.stillPreparing}
        <p class="quiet" role="status">{STILL_PREPARING_TEXT}</p>
      {/if}

      {#if view.rules.length === 0}
        <p class="quiet">No auto-delete rules on {planeLabel}.</p>
      {:else}
        <ul class="rules">
          {#each view.rules as rule (rule.ruleId)}
            {@const locked = !autodelete.canChange(rule.ruleId)}
            <li>
              <article class="rule" aria-labelledby={`${id}-${rule.ruleId}`}>
                <h5 id={`${id}-${rule.ruleId}`} class="prompt">{ruleText(rule)}</h5>

                {#if rule.state.kind === "compiling"}
                  <p role="status">{COMPILING_TEXT}</p>
                {:else if rule.state.kind === "refused"}
                  {@const copy = refusalCopy(rule.state.reason)}
                  <p>{copy.text} {#if copy.drafting}{@render agentsLink()}{/if}</p>
                {:else if rule.state.kind === "draft"}
                  <p>{draftText(rule.state.inactiveDays)}</p>
                {:else}
                  {@const since = formatDate(rule.state.approvedAtUnixMs, now)}
                  <p>{approvedText(rule, rule.state.approvedAtUnixMs, now)}</p>
                  <p class="quiet">
                    {draftText(rule.state.inactiveDays)} Approved <time datetime={since.iso}>{since.absolute}</time>.
                  </p>
                {/if}
                {#if attributionText(rule.attribution)}
                  <p class="quiet">{attributionText(rule.attribution)}</p>
                {/if}

                {#if editor.kind === "wording" && editor.ruleId === rule.ruleId}
                  <div class="edit">
                    <label for={`${id}-${rule.ruleId}-wording`}>Rule</label>
                    <textarea
                      id={`${id}-${rule.ruleId}-wording`}
                      rows="3"
                      aria-describedby={`${id}-${rule.ruleId}-bytes`}
                      value={editor.prompt}
                      oninput={(event) => autodelete.setEditorText(event.currentTarget.value)}
                    ></textarea>
                    <span id={`${id}-${rule.ruleId}-bytes`} class="quiet">{byteCounter(editor.prompt)}</span>
                    <p class="quiet">Saving new wording drafts the rule again and clears its approval.</p>
                    <div class="actions">
                      <button
                        type="button"
                        class="primary-button"
                        disabled={!promptFits(editor.prompt) || block !== null}
                        onclick={() => void autodelete.saveEdit()}>Save wording</button
                      >
                      <button type="button" class="secondary-button" onclick={() => autodelete.cancelEdit()}>Cancel</button>
                    </div>
                  </div>
                {:else if editor.kind === "days" && editor.ruleId === rule.ruleId}
                  <div class="edit">
                    <label for={`${id}-${rule.ruleId}-days`}>Days with no activity</label>
                    <input
                      id={`${id}-${rule.ruleId}-days`}
                      type="number"
                      inputmode="numeric"
                      min="1"
                      max="36500"
                      value={editor.days}
                      oninput={(event) => autodelete.setEditorText(event.currentTarget.value)}
                    />
                    <p class="quiet">
                      The rule becomes a draft of exactly this many days. You approve it again before it runs.
                    </p>
                    <div class="actions">
                      <button
                        type="button"
                        class="primary-button"
                        disabled={parseDays(editor.days) === null || block !== null}
                        onclick={() => void autodelete.saveEdit()}>Save days</button
                      >
                      <button type="button" class="secondary-button" onclick={() => autodelete.cancelEdit()}>Cancel</button>
                    </div>
                  </div>
                {:else}
                  <div class="actions">
                    {#if rule.state.kind === "draft"}
                      <button
                        type="button"
                        class="primary-button"
                        disabled={locked}
                        onclick={() => autodelete.openApprove(rule.ruleId)}>Approve rule</button
                      >
                    {/if}
                    {#if rule.state.kind !== "compiling"}
                      <button
                        type="button"
                        class="secondary-button"
                        disabled={locked}
                        onclick={() => autodelete.editDays(rule.ruleId)}>Change days</button
                      >
                    {/if}
                    {#if rule.state.kind === "refused" && rule.prompt !== ""}
                      <button
                        type="button"
                        class="secondary-button"
                        disabled={locked}
                        onclick={() => autodelete.editWording(rule.ruleId)}>Edit wording</button
                      >
                    {/if}
                    {#if rule.state.kind === "approved" && rule.state.everywhereToken !== null}
                      <button
                        type="button"
                        class="secondary-button"
                        disabled={locked}
                        onclick={() => autodelete.openEverywhere(rule.ruleId)}>Also delete everywhere…</button
                      >
                    {/if}
                    <button
                      type="button"
                      class="text-button danger"
                      disabled={locked}
                      onclick={() => autodelete.openDelete(rule.ruleId)}>Delete rule…</button
                    >
                    {#if sendingFor(rule.ruleId)}<span class="quiet" role="status">Sending…</span>{/if}
                  </div>
                {/if}

                {@render pendingNotice(rule.ruleId)}
                {@render refusal(rule.ruleId)}

                {#if rule.state.kind === "draft" || rule.state.kind === "approved"}
                  <details
                    class="candidates"
                    ontoggle={(event) => {
                      if (event.currentTarget.open) void autodelete.resolveNames(rule.ruleId);
                    }}
                  >
                    <summary>{candidatesHeading(rule)}</summary>
                    {#if rule.candidates.length === 0}
                      <p class="quiet">No task matches today.</p>
                    {:else}
                      <ul>
                        {#each rule.candidates as candidate (candidate.conversationId)}
                          {@const active = formatDate(candidate.lastActiveAtUnixMs, now)}
                          <li>
                            <span class="candidate-name">{nameOf(candidate.conversationId)}</span>
                            <span class="quiet">
                              Last active <time datetime={active.iso}>{active.absolute}</time>
                            </span>
                            {#if candidate.protections.length === 0}
                              <span class="quiet">Would move to Jet Trash</span>
                            {:else}
                              {#each candidate.protections as protection (protection.kind)}
                                <span class="quiet">{protectionLine(protection.kind)}</span>
                              {/each}
                            {/if}
                          </li>
                        {/each}
                      </ul>
                    {/if}
                  </details>
                {/if}
              </article>
            </li>
          {/each}
        </ul>
      {/if}
    {/snippet}
  </SectionState>
</div>

{#if dialog.kind === "approve"}
  <SettingsDialog
    title="Approve this rule?"
    lead={dialogRule ? ruleText(dialogRule) : undefined}
    returnFocus={["section-retention"]}
    oncancel={() => autodelete.closeDialog()}
  >
    <p>{approveText(dialog.days, planeLabel)}</p>
    {#if dialogRule}
      <p class="dialog-note">{candidatesHeading(dialogRule)}.</p>
    {/if}
    {#if block !== null}<p class="dialog-note" role="status">{blockText(block, planeLabel)}</p>{/if}
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel disabled={sending} onclick={() => autodelete.closeDialog()}>
        Cancel
      </button>
      <button
        type="button"
        class="primary-button"
        data-dialog-primary
        disabled={sending || block !== null}
        onclick={() => void autodelete.confirmDialog()}
      >
        {sending ? "Approving…" : "Approve rule"}
      </button>
    {/snippet}
  </SettingsDialog>
{:else if dialog.kind === "everywhere"}
  <SettingsDialog
    title="Allow delete everywhere?"
    focus="cancel"
    returnFocus={["section-retention"]}
    oncancel={() => autodelete.closeDialog()}
  >
    <p class="dialog-alert">{everywhereText(planeLabel)}</p>
    {#if block !== null}<p class="dialog-note" role="status">{blockText(block, planeLabel)}</p>{/if}
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel disabled={sending} onclick={() => autodelete.closeDialog()}>
        Cancel
      </button>
      <button
        type="button"
        class="danger-button"
        data-dialog-primary
        disabled={sending || block !== null}
        onclick={() => void autodelete.confirmDialog()}
      >
        {sending ? "Allowing…" : "Allow delete everywhere"}
      </button>
    {/snippet}
  </SettingsDialog>
{:else if dialog.kind === "delete"}
  <SettingsDialog
    title="Delete this rule?"
    lead={dialogRule ? ruleText(dialogRule) : undefined}
    focus="cancel"
    returnFocus={["section-retention"]}
    oncancel={() => autodelete.closeDialog()}
  >
    <p>{DELETE_TEXT}</p>
    {#if block !== null}<p class="dialog-note" role="status">{blockText(block, planeLabel)}</p>{/if}
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel disabled={sending} onclick={() => autodelete.closeDialog()}>
        Cancel
      </button>
      <button
        type="button"
        class="danger-button"
        data-dialog-primary
        disabled={sending || block !== null}
        onclick={() => void autodelete.confirmDialog()}
      >
        {sending ? "Deleting…" : "Delete rule"}
      </button>
    {/snippet}
  </SettingsDialog>
{:else if dialog.kind === "changed"}
  <SettingsDialog title="This rule changed" returnFocus={["section-retention"]} oncancel={() => autodelete.closeDialog()}>
    <p role="alert">{CHANGED_TEXT}</p>
    {#snippet footer()}
      <button type="button" class="primary-button" data-dialog-primary onclick={() => autodelete.closeDialog()}>
        Review the rule
      </button>
    {/snippet}
  </SettingsDialog>
{/if}

<style>
  .autodelete,
  .compose,
  .edit {
    display: grid;
    gap: 8px;
  }

  .autodelete p {
    margin: 0;
  }

  label {
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
  }

  textarea,
  input[type="number"] {
    width: 100%;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font: inherit;
    resize: vertical;
  }

  input[type="number"] {
    max-width: 140px;
  }

  .compose-actions,
  .list-heading,
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .compose-actions,
  .list-heading {
    justify-content: space-between;
  }

  .list-heading {
    margin-top: 8px;
  }

  h4,
  h5 {
    margin: 0;
    font-size: 13px;
  }

  .prompt {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    font-weight: 600;
  }

  .rules {
    display: grid;
    gap: 10px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .rule {
    display: grid;
    gap: 6px;
    padding: 12px;
    border: 1px solid var(--border-soft);
    border-radius: 8px;
    background: var(--raised);
  }

  .disclosure {
    font-size: 12px;
  }

  .disclosure.off {
    color: var(--warning);
  }

  .quiet {
    color: var(--muted);
    font-size: 12px;
  }

  .over {
    color: var(--danger);
  }

  .danger {
    color: var(--danger);
  }

  .notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 8px;
    font-size: 12px;
  }

  .notice.pending {
    background: color-mix(in srgb, var(--warning) 12%, var(--raised));
  }

  .notice.critical {
    background: color-mix(in srgb, var(--danger) 10%, var(--raised));
  }

  .candidates summary {
    cursor: pointer;
    color: var(--muted);
    font-size: 12px;
  }

  .candidates ul {
    display: grid;
    gap: 6px;
    margin: 8px 0 0;
    padding: 0;
    list-style: none;
  }

  .candidates li {
    display: grid;
    gap: 2px;
  }

  .candidate-name {
    overflow-wrap: anywhere;
    font-size: 13px;
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
