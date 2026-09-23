<script lang="ts">
  import { onMount, untrack } from "svelte";

  import type { AgentsView, CraftView } from "$lib/jet/agents";
  import { LOCAL_PLANE } from "$lib/jet/planes";
  import type { SettingsSnapshot } from "$lib/jet/settings";
  import AccountDetail from "./AccountDetail.svelte";
  import { craftTitle, formatCount } from "./agents-model";
  import ConsentRow from "./ConsentRow.svelte";
  import CraftDisableDialog from "./CraftDisableDialog.svelte";
  import CraftInstallDialog from "./CraftInstallDialog.svelte";
  import ExtensionsSection from "./ExtensionsSection.svelte";
  import { blockText, sectionData, withIssues } from "./model";
  import OperationStatus from "./OperationStatus.svelte";
  import SectionState from "./SectionState.svelte";
  import type { SettingsSession } from "./session.svelte";
  import SettingRow from "./SettingRow.svelte";
  import SettingsDialog from "./SettingsDialog.svelte";
  import UsageHistoryTable from "./UsageHistoryTable.svelte";

  let {
    session,
    onopenpermissions,
  }: {
    session: SettingsSession;
    /** Opens Safety › Permissions (Developer Mode). */
    onopenpermissions: () => void;
  } = $props();

  const agents = $derived(session.agents);
  const PLANE = { type: "plane" } as const;
  const id = $props.id();
  /** The Connect button a bind review returns focus to. */
  const connectId = (provider: string) => `${id}-connect-${provider}`;

  const view = $derived(sectionData(agents.view));
  const block = $derived(session.agentsBlock());
  const accounts = $derived(view?.accounts ?? []);
  const developerMode = $derived.by(() => {
    const value = session.setting("craft.developer_mode", PLANE)?.value;
    return value?.type === "flag" ? value.value : null;
  });
  const bindReview = $derived(
    agents.operation.kind === "confirm" && agents.operation.review.subject.kind === "bind"
      ? agents.operation.review.subject
      : null,
  );
  const usageIssue = $derived(
    agents.view.kind === "ready" ? agents.view.issues.find((issue) => issue.section === "usage") : undefined,
  );

  let disabling = $state<CraftView | null>(null);
  /** Open account disclosures; a detail loads only while open. */
  let openAccounts = $state<Record<string, boolean>>({});
  let installing = $state(false);

  // Each Plane's pane loads once it is shown; later reloads come from focus,
  // events and Check again.
  $effect(() => {
    void agents.planeId;
    untrack(() => void agents.ensureLoaded());
  });

  function showNewerUsage(): void {
    session.usageNewer = false;
    void agents.load();
    void agents.loadHistory();
  }

  onMount(() => {
    // Harnesses have no events: they are read again when the window regains focus.
    const onFocus = () => {
      if (agents.idle) void agents.load();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  });
</script>

<section class="settings-section" aria-labelledby="section-harnesses">
  <h2 id="section-harnesses" tabindex="-1">Harnesses</h2>
  <p>The coding agents this Plane can run, and the Craft package that teaches Jet each one.</p>
  <SectionState
    state={withIssues(agents.view, ["capabilities"])}
    title="Harnesses"
    planeLabel={session.planeLabel}
    onretry={() => void agents.load()}
  >
    {#snippet children(data: AgentsView)}
      {#if data.crafts.length === 0 && !data.issues.some((issue) => issue.section === "capabilities")}
        <p>No Harness is installed on {session.planeLabel}.</p>
      {/if}
      <ul class="crafts">
        {#each data.crafts as craft (craft.craftId)}
          <li class="craft">
            <div class="craft-main">
              <strong>{craftTitle(craft)}</strong>
              <details>
                <summary>Details</summary>
                <p>Craft package: {craft.craftId} · {craft.version}</p>
                {#if craft.harnesses.length > 0}
                  <p>Harness identifiers: {craft.harnesses.map((harness) => harness.id).join(", ")}</p>
                {/if}
              </details>
            </div>
            <button
              class="text-button danger"
              aria-label={`Disable ${craftTitle(craft)}…`}
              disabled={block !== null || !agents.idle}
              onclick={() => (disabling = craft)}
            >
              Disable…
            </button>
          </li>
        {/each}
      </ul>
      {#if data.degraded.length > 0}
        <ul class="degraded" aria-label="Plane limitations">
          {#each data.degraded as condition (condition)}<li>{condition}</li>{/each}
        </ul>
      {/if}
      <div class="section-actions">
        <button
          id={`${id}-add-craft`}
          class="secondary-button"
          disabled={block !== null || !agents.idle}
          onclick={() => (installing = true)}
        >
          Add a Craft…
        </button>
      </div>
      <OperationStatus
        {agents}
        matches={(target) => target.action === "disable" || target.action === "install"}
        planeLabel={session.planeLabel}
        oncheck={() => void agents.load()}
      />
      <p class="note">
        Models are chosen by each Harness. Jet can't list them yet. Jet can't turn a disabled Harness back on for a
        Plane yet.
      </p>
    {/snippet}
  </SectionState>
</section>

<ExtensionsSection {session} />

<section class="settings-section" aria-labelledby="section-accounts">
  <h2 id="section-accounts" tabindex="-1">Accounts</h2>
  <p>Sign-ins Jet uses for tasks on this Plane. Jet stores a reference, never the credential.</p>
  <SectionState
    state={withIssues(agents.view, ["accounts"])}
    title="Accounts"
    planeLabel={session.planeLabel}
    onretry={() => void agents.load(true)}
  >
    {#snippet children(data: AgentsView)}
      {#if agents.accountsChanged}
        <div class="notice" role="status">
          <p>Accounts changed on {session.planeLabel} since you opened this page.</p>
          <button class="text-button" onclick={() => void agents.load()}>Show current values</button>
        </div>
      {/if}
      {#if data.credentialStore}
        <div class="store" class:warning={data.credentialStore.state !== "available"}>
          <p>
            {data.credentialStore.label}.
            {#if data.credentialStore.state === "locked"}Unlock your system keyring, then check again.{/if}
          </p>
          <button class="text-button" disabled={agents.view.kind === "loading"} onclick={() => void agents.load(true)}>
            Check again
          </button>
        </div>
      {/if}
      {#if data.accounts.length === 0}
        {#if !data.issues.some((issue) => issue.section === "accounts")}
          <p>No accounts connected on {session.planeLabel}.</p>
        {/if}
      {:else}
        <ul class="accounts">
          {#each data.accounts as account (account.id)}
            <li>
              <details
                open={openAccounts[account.id] ?? false}
                ontoggle={(event) => (openAccounts = { ...openAccounts, [account.id]: event.currentTarget.open })}
              >
                <summary>
                  <span class="account-name">{account.label}</span>
                  <span class="account-provider">{account.provider}</span>
                  <span class="account-state" class:warning={account.state !== "ready" && account.state !== "resolved_at_use"}>
                    {account.stateLabel}
                  </span>
                </summary>
                {#if openAccounts[account.id]}
                  <AccountDetail {session} {account} />
                {/if}
              </details>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="section-actions">
        {#each data.bindOptions as option (option.provider)}
          <button
            id={connectId(option.provider)}
            class="secondary-button"
            disabled={block !== null || !agents.idle}
            onclick={() => void agents.prepareBind(option.provider)}
          >
            Connect {option.harness}
          </button>
        {/each}
      </div>
      <OperationStatus
        {agents}
        matches={(target) => target.action === "bind" || target.action === "unbind"}
        planeLabel={session.planeLabel}
        oncheck={() => void agents.load(true)}
      />
      <p class="note">
        Jet connects a Harness through its own sign-in. Signing in with your system keyring, a credential helper, or
        for one session isn't available in this app.
      </p>
    {/snippet}
  </SectionState>
</section>

<section class="settings-section" aria-labelledby="section-usage">
  <h2 id="section-usage" tabindex="-1">Usage</h2>
  <p>Tokens and requests reported by the providers of this Plane's accounts.</p>
  {#if session.usageNewer}
    <div class="notice" role="status">
      <p>Newer usage is available.</p>
      <button class="text-button" onclick={showNewerUsage}>Show newer usage</button>
    </div>
  {/if}
  {#if usageIssue && usageIssue.error.category === "incompatible"}
    <p class="notice" role="status">
      Usage isn't available on {session.planeLabel} yet. Other settings still work.
      <code>{usageIssue.error.code}</code>
    </p>
  {:else}
    <SectionState
      state={withIssues(agents.view, ["usage"])}
      title="Usage"
      planeLabel={session.planeLabel}
      onretry={() => void agents.load()}
    >
      {#snippet children(data: AgentsView)}
        {#if data.usage}
          {@const usage = data.usage}
          <dl class="totals">
            <div><dt>Input</dt><dd>{formatCount(usage.tokens.input)}</dd></div>
            <div><dt>Cached input</dt><dd>{formatCount(usage.tokens.cachedInput)}</dd></div>
            <div><dt>Output</dt><dd>{formatCount(usage.tokens.output)}</dd></div>
            <div><dt>Reasoning tokens</dt><dd>{formatCount(usage.tokens.reasoning)}</dd></div>
          </dl>
          {#if usage.measurements !== "0"}
            <p>
              {formatCount(usage.measurements)}
              {usage.measurements === "1" ? "measurement" : "measurements"}{#if usage.estimated !== "0"}, {formatCount(
                  usage.estimated,
                )} estimated{/if}
            </p>
          {/if}
        {/if}
      {/snippet}
    </SectionState>
    <h3>History</h3>
    <UsageHistoryTable {agents} {accounts} planeLabel={session.planeLabel} />
  {/if}
</section>

<section class="settings-section" aria-labelledby="section-utility">
  <h2 id="section-utility" tabindex="-1">Utility</h2>
  <p>Short background work Jet does with one of your accounts, like naming tasks.</p>
  <SectionState
    state={session.plane}
    title="Utility"
    planeLabel={session.planeLabel}
    onretry={() => void session.showCurrent(PLANE)}
  >
    {#snippet children(_snapshot: SettingsSnapshot)}
      <div class="setting-rows">
        <SettingRow {session} settingKey="utility.automatic_naming" scope={PLANE} />
        <SettingRow {session} settingKey="utility.account_binding" scope={PLANE} />
        <ConsentRow
          {session}
          bindingKey="utility.account_binding"
          missingText={(label) => `Without this, Jet won't send task content to ${label} for naming and summaries.`}
        />
        <SettingRow {session} settingKey="utility.autodelete_compilation" scope={PLANE} />
      </div>
      <p class="note">
        The account and permission to send it task content are saved as two separate changes. If one of them isn't
        saved, this page shows exactly what was.
      </p>
    {/snippet}
  </SectionState>
</section>

{#if bindReview}
  <SettingsDialog
    title={`Connect ${bindReview.harness}?`}
    lead={`Jet will use ${bindReview.harness}'s own sign-in on ${session.planeLabel}. Jet doesn't store the credential.`}
    focus={block !== null ? "cancel" : "primary"}
    returnFocus={[connectId(bindReview.provider), "section-accounts"]}
    oncancel={() => agents.dismiss()}
  >
    <dl class="removal-facts">
      <div><dt>Plane</dt><dd>{session.planeLabel}</dd></div>
      <div><dt>Harness</dt><dd>{bindReview.harness}</dd></div>
      <div><dt>Provider</dt><dd>{bindReview.provider}</dd></div>
    </dl>
    {#if block !== null}
      <p class="dialog-note" role="status">{blockText(block, session.planeLabel)}</p>
    {/if}
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel onclick={() => agents.dismiss()}>Cancel</button>
      <button
        type="button"
        class="primary-button"
        data-dialog-primary
        disabled={block !== null}
        onclick={() => void agents.confirm()}
      >
        Connect
      </button>
    {/snippet}
  </SettingsDialog>
{/if}

{#if disabling}
  <CraftDisableDialog
    {agents}
    craft={disabling}
    planeLabel={session.planeLabel}
    {block}
    onclose={() => (disabling = null)}
  />
{/if}

{#if installing}
  <CraftInstallDialog
    {agents}
    planeLabel={session.planeLabel}
    isLocalPlane={agents.planeId === LOCAL_PLANE}
    {developerMode}
    {block}
    returnFocus={[`${id}-add-craft`, "section-harnesses"]}
    onclose={() => (installing = false)}
    onopenpermissions={() => {
      agents.dismiss();
      installing = false;
      onopenpermissions();
    }}
  />
{/if}

<style>
  .crafts,
  .accounts,
  .degraded {
    display: grid;
    gap: 0;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .craft {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 0;
    border-top: 1px solid var(--border-soft);
  }

  .craft-main {
    display: grid;
    gap: 4px;
    min-width: 0;
  }

  details summary {
    color: var(--muted);
    font-size: 12px;
    cursor: pointer;
  }

  details p {
    margin: 4px 0 0;
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .accounts li {
    padding: 10px 0;
    border-top: 1px solid var(--border-soft);
  }

  .accounts summary {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 12px;
    color: var(--text);
    font-size: 13px;
  }

  .account-name {
    font-weight: 600;
  }

  .account-provider,
  .account-state {
    color: var(--muted);
    font-size: 12px;
  }

  .warning {
    color: var(--warning) !important;
  }

  .degraded li {
    color: var(--warning);
    font-size: 12px;
  }

  .store {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .store p {
    margin: 0;
    font-size: 13px;
  }

  .section-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .note {
    font-size: 12px;
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
    color: var(--text) !important;
  }

  .totals {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 8px;
    margin: 0;
  }

  .totals div {
    display: grid;
    gap: 2px;
  }

  .totals dt {
    color: var(--muted);
    font-size: 12px;
  }

  .totals dd {
    margin: 0;
    font-size: 15px;
    font-variant-numeric: tabular-nums;
  }

  .setting-rows {
    display: grid;
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
