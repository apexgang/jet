<script lang="ts">
  import { tick } from "svelte";

  import type { SettingKeyId, SettingValue } from "$lib/jet/settings";
  import {
    MAX_SETTING_TEXT_BYTES,
    SETTING_PLACEMENT,
    clearLabel,
    emptyBindingLabel,
    isBindingKey,
    refusalText,
    sectionData,
    sourceLabel,
    storedAt,
    textFits,
    utf8Bytes,
    valueKind,
    valueText,
    type SettingRowScope,
  } from "./model";
  import type { SettingsSession } from "./session.svelte";
  import SettingsReviewDialog from "./SettingsReviewDialog.svelte";

  let {
    session,
    settingKey,
    scope,
  }: {
    session: SettingsSession;
    settingKey: SettingKeyId;
    scope: SettingRowScope;
  } = $props();

  const MAX_COUNT = 4_294_967_295;
  const id = $props.id();

  const placement = $derived(SETTING_PLACEMENT[settingKey]);
  const setting = $derived(session.setting(settingKey, scope));
  /** The control follows the Plane's value; a missing key keeps its usual control, disabled. */
  const kind = $derived(setting?.value.type ?? valueKind(settingKey));
  const row = $derived(session.row(settingKey, scope));
  const block = $derived(session.mutationBlock(scope));
  const projects = $derived(sectionData(session.work)?.projects ?? []);
  const bindings = $derived(sectionData(session.work)?.bindings ?? []);
  const sectionReady = $derived(session.sectionFor(scope)?.kind === "ready");
  const inFlight = $derived(
    row.kind === "checking" || row.kind === "confirm" || row.kind === "applying" || row.kind === "uncertain",
  );
  const locked = $derived(block !== null || inFlight || !setting);
  const scopeLabel = $derived(
    scope.type === "plane"
      ? "This Plane"
      : (projects.find((project) => project.id === scope.projectId)?.name ?? "This Project"),
  );

  /** What the control shows: the draft being sent or edited, else the Plane's value. */
  const shown = $derived.by((): SettingValue | null => {
    if (row.kind === "editing") return row.draft;
    if (row.kind === "checking" && row.draft) return row.draft;
    if (row.kind === "confirm" && row.review.after) return row.review.after;
    return setting?.value ?? null;
  });

  const draftText = $derived(shown?.type === "text" ? shown.value : "");
  const draftCount = $derived(shown?.type === "count" && Number.isFinite(shown.value) ? String(shown.value) : "");
  const bytes = $derived(utf8Bytes(draftText));
  const editing = $derived(row.kind === "editing");
  const countValid = $derived.by(() => {
    if (shown?.type !== "count") return false;
    return Number.isInteger(shown.value) && shown.value >= 0 && shown.value <= MAX_COUNT;
  });
  const textValid = $derived(shown?.type === "text" && textFits(settingKey, shown.value));

  let rowElement = $state<HTMLDivElement>();
  /**
   * A change started from this row. Its control is locked while the change
   * is in flight (and Save, Cancel or the clear button may go away), so
   * focus falls to the document; it comes back once the row settles.
   */
  let returnFocus = $state(false);

  function started(): void {
    returnFocus = true;
  }

  $effect(() => {
    if (!returnFocus || inFlight) return;
    returnFocus = false;
    void tick().then(() => {
      const active = document.activeElement;
      // Never take focus the user has moved elsewhere.
      if (active && active !== document.body && !rowElement?.contains(active)) return;
      const control = document.getElementById(`${id}-control`);
      if (control instanceof HTMLElement && !control.matches(":disabled")) control.focus();
    });
  });

  function setFlag(value: boolean): void {
    started();
    void session.change(settingKey, scope, { kind: "set", value: { type: "flag", value } });
  }

  function editText(value: string): void {
    session.edit(settingKey, scope, { type: "text", value });
  }

  function editCount(raw: string): void {
    const value = raw.trim() === "" ? Number.NaN : Number(raw);
    session.edit(settingKey, scope, { type: "count", value });
  }

  function save(): void {
    if (row.kind !== "editing" || shown === null || shown.type === "undisplayable") return;
    if (shown.type === "count" && !countValid) return;
    if (shown.type === "text" && !textValid) return;
    started();
    void session.change(settingKey, scope, { kind: "set", value: shown });
  }

  /** A binding key's `<select>` applies the chosen Account binding at once. */
  function chooseBinding(value: string): void {
    started();
    void session.change(settingKey, scope, { kind: "set", value: { type: "text", value } });
  }

  function useInherited(): void {
    started();
    void session.change(settingKey, scope, { kind: "clear" });
  }

  function onKey(event: KeyboardEvent): void {
    if (event.key === "Escape" && editing) {
      event.preventDefault();
      session.dismiss(settingKey, scope);
    } else if (event.key === "Enter" && !(event.target instanceof HTMLTextAreaElement)) {
      event.preventDefault();
      save();
    }
  }
</script>

<div class="setting-row" class:busy={inFlight} aria-busy={inFlight} bind:this={rowElement}>
  <div class="setting-main">
    {#if kind === "flag"}
      <label class="toggle" for={`${id}-control`}>
        <input
          id={`${id}-control`}
          type="checkbox"
          aria-describedby={`${id}-help`}
          checked={shown?.type === "flag" ? shown.value : false}
          disabled={locked || setting?.value.type !== "flag"}
          onchange={(event) => setFlag(event.currentTarget.checked)}
        />
        <span class="setting-label">{placement.label}</span>
      </label>
    {:else if kind === "undisplayable"}
      <span class="setting-label">{placement.label}</span>
      <p class="undisplayable">Value can't be displayed</p>
    {:else}
      <label class="setting-label" for={`${id}-control`}>{placement.label}</label>
      {#if kind === "count"}
        <div class="field">
          <input
            id={`${id}-control`}
            type="number"
            inputmode="numeric"
            min="0"
            max={MAX_COUNT}
            step="1"
            aria-describedby={`${id}-help`}
            value={draftCount}
            disabled={locked}
            oninput={(event) => editCount(event.currentTarget.value)}
            onkeydown={onKey}
          />
          {#if placement.unit}<span class="unit">{placement.unit}</span>{/if}
        </div>
        {#if editing && !countValid}
          <p class="field-error" role="alert">Enter a whole number from 0 to 4,294,967,295.</p>
        {/if}
      {:else if isBindingKey(settingKey)}
        <select
          id={`${id}-control`}
          aria-describedby={`${id}-help`}
          value={draftText}
          disabled={locked || shown?.type !== "text"}
          onchange={(event) => chooseBinding(event.currentTarget.value)}
        >
          <option value="">{emptyBindingLabel(settingKey)}</option>
          {#each bindings as binding (binding.id)}
            <option value={binding.id}>{binding.label} · {binding.provider}</option>
          {/each}
          {#if draftText !== "" && !bindings.some((binding) => binding.id === draftText)}
            <option value={draftText}>An account that's no longer connected</option>
          {/if}
        </select>
      {:else if settingKey === "git.message_instructions"}
        <textarea
          id={`${id}-control`}
          rows="4"
          aria-describedby={`${id}-help ${id}-bytes`}
          value={draftText}
          disabled={locked}
          oninput={(event) => editText(event.currentTarget.value)}
          onkeydown={onKey}
        ></textarea>
        <p class="byte-count" id={`${id}-bytes`} class:over={bytes > MAX_SETTING_TEXT_BYTES}>
          {bytes.toLocaleString("en-US")} of {MAX_SETTING_TEXT_BYTES.toLocaleString("en-US")} bytes
        </p>
      {:else}
        <input
          id={`${id}-control`}
          type="text"
          autocomplete="off"
          spellcheck="false"
          aria-describedby={`${id}-help`}
          value={draftText}
          disabled={locked}
          oninput={(event) => editText(event.currentTarget.value)}
          onkeydown={onKey}
        />
        {#if editing && !textValid}
          <p class="field-error" role="alert">
            Use at most {MAX_SETTING_TEXT_BYTES.toLocaleString("en-US")} bytes, without control characters.
          </p>
        {/if}
      {/if}
    {/if}
    <p class="help" id={`${id}-help`}>{placement.help}</p>
  </div>

  <div class="setting-side">
    {#if setting}
      <span class="source">{sourceLabel(setting.source, projects)}</span>
    {/if}
    {#if editing}
      <div class="actions">
        <button
          class="secondary-button"
          aria-label={`Cancel editing ${placement.label}`}
          onclick={() => {
            started();
            session.dismiss(settingKey, scope);
          }}
        >
          Cancel
        </button>
        <button
          class="primary-button"
          aria-label={`Save ${placement.label}`}
          disabled={block !== null || (shown?.type === "count" ? !countValid : !textValid)}
          onclick={save}
        >
          Save
        </button>
      </div>
    {:else if setting && storedAt(setting.source, scope) && !inFlight}
      <button
        class="text-button"
        aria-label={`${clearLabel(settingKey, scope)}: ${placement.label}`}
        disabled={block !== null}
        onclick={useInherited}
      >
        {clearLabel(settingKey, scope)}
      </button>
    {/if}
  </div>

  <div class="row-status" role="status">
    {#if !setting && sectionReady}
      <p>This setting needs a newer Jet service on {session.planeLabel}.</p>
    {:else if row.kind === "checking"}
      <p>Checking the current value…</p>
    {:else if row.kind === "applying"}
      <p>Saving…</p>
    {:else if row.kind === "uncertain"}
      <div class="row-notice">
        <p>Jet couldn't confirm this change. It may have been saved. <code>{row.error.code}</code></p>
        <div class="actions">
          <button
            class="text-button"
            aria-label={`Retry same change: ${placement.label}`}
            disabled={block !== null}
            onclick={() => {
              started();
              void session.retry(settingKey, scope);
            }}
          >
            Retry same change
          </button>
          <button
            class="text-button"
            aria-label={`Show current value: ${placement.label}`}
            onclick={() => void session.showCurrent(scope)}
          >
            Show current value
          </button>
        </div>
      </div>
    {:else if row.kind === "changed_elsewhere"}
      <div class="row-notice">
        <p>
          This was changed on {session.planeLabel} to {valueText(settingKey, row.current.value, bindings)}. Your edit wasn't saved.
        </p>
        <div class="actions">
          <button
            class="text-button"
            aria-label={`Use current: ${placement.label}`}
            onclick={() => {
              started();
              session.dismiss(settingKey, scope);
            }}
          >
            Use current
          </button>
          {#if row.draft === null || row.draft.type !== "undisplayable"}
            <button
              class="text-button"
              aria-label={`Apply my value again: ${placement.label}`}
              disabled={block !== null}
              onclick={() => {
                started();
                void session.applyAgain(settingKey, scope);
              }}
            >
              Apply my value again
            </button>
          {/if}
        </div>
      </div>
    {:else if row.kind === "refused"}
      <div class="row-notice critical">
        <p>{refusalText(row.error)} <code>{row.error.code}</code></p>
        <button
          class="text-button"
          aria-label={`Dismiss: ${placement.label}`}
          onclick={() => {
            started();
            session.dismiss(settingKey, scope);
          }}
        >
          Dismiss
        </button>
      </div>
    {/if}
  </div>
</div>

{#if row.kind === "confirm"}
  <SettingsReviewDialog
    review={row.review}
    {settingKey}
    {scopeLabel}
    planeLabel={session.planeLabel}
    {projects}
    {bindings}
    {block}
    onconfirm={() => {
      started();
      void session.confirm(settingKey, scope);
    }}
    oncancel={() => {
      started();
      session.dismiss(settingKey, scope);
    }}
  />
{/if}

<style>
  .setting-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 6px 16px;
    padding: 12px 0;
    border-top: 1px solid var(--border-soft);
  }

  .setting-main {
    display: grid;
    gap: 6px;
    min-width: 0;
  }

  .setting-label {
    color: var(--text);
    font-weight: 560;
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .toggle input {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
  }

  .field {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  input[type="number"],
  input[type="text"],
  select,
  textarea {
    min-width: 0;
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font: inherit;
  }

  input[type="number"] {
    width: 12ch;
  }

  input[type="text"],
  textarea {
    width: 100%;
    max-width: 480px;
  }

  textarea {
    resize: vertical;
  }

  input:disabled,
  select:disabled,
  textarea:disabled {
    opacity: 0.6;
  }

  .unit,
  .help,
  .byte-count,
  .source {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .byte-count.over,
  .field-error {
    margin: 0;
    color: var(--warning);
    font-size: 12px;
  }

  .undisplayable {
    margin: 0;
    color: var(--muted);
    font-style: italic;
  }

  .setting-side {
    display: grid;
    justify-items: end;
    align-content: start;
    gap: 6px;
  }

  .source {
    padding: 2px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .row-status {
    grid-column: 1 / -1;
    font-size: 12px;
  }

  .row-status:empty {
    display: none;
  }

  .row-status p {
    margin: 0;
    color: var(--muted);
  }

  .row-notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 8%, var(--raised));
  }

  .row-notice p {
    color: var(--text) !important;
  }

  .row-notice.critical {
    background: color-mix(in srgb, var(--danger) 10%, var(--raised));
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }

  @media (max-width: 760px) {
    .setting-row {
      grid-template-columns: minmax(0, 1fr);
    }

    .setting-side {
      justify-items: start;
    }
  }
</style>
