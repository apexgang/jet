<script lang="ts">
  import type { SettingsSection } from "$lib/jet/settings-window";
  import type { SystemHealth } from "$lib/jet/system";
  import SectionState from "$lib/features/settings/SectionState.svelte";
  import { LANDED_SECTIONS, PANES, sectionHeadingId, withIssues } from "$lib/features/settings/model";
  import AppUpdatesBlock from "./AppUpdatesBlock.svelte";
  import LocalServiceBlock from "./LocalServiceBlock.svelte";
  import type { LocalServiceSession } from "./local-service.svelte";
  import { credentialStoreText, formatWhen, negotiatedLine, startsText, toolText, unixMs } from "./model";
  import type { SystemSession } from "./session.svelte";
  import type { AppUpdateSession } from "./updates.svelte";

  let {
    system,
    service,
    updates,
    planeLabel,
    onopen,
  }: {
    system: SystemSession;
    /** This computer's Jet service, whichever Plane is picked. */
    service: LocalServiceSession;
    /** This app's own updates. */
    updates: AppUpdateSession;
    /** The Plane picker's label, shown before health loads. */
    planeLabel: string;
    /** Shows another Settings section in this window. */
    onopen: (section: SettingsSection) => void;
  } = $props();

  const view = $derived(withIssues(system.health, ["capabilities"]));
  const checking = $derived(system.health.kind === "loading");

  function sectionTitle(section: SettingsSection): string {
    return PANES.flatMap((pane) => pane.sections).find((entry) => entry.id === section)?.title ?? "Settings";
  }

  function startedText(health: SystemHealth): string {
    const started = unixMs(health.service.startedAtUnixMs);
    const since = started === null ? "" : `, started ${formatWhen(started)}`;
    return `Jet service ${health.service.coreVersion} on ${health.planeLabel}${since}.`;
  }
</script>

<section class="settings-section" aria-labelledby={sectionHeadingId("versions")}>
  <h2 id={sectionHeadingId("versions")} tabindex="-1">Versions and capabilities</h2>
  <p>What the Jet service on this Plane runs and what it can do right now.</p>
  <SectionState
    state={view}
    title="Versions and capabilities"
    {planeLabel}
    onretry={() => void system.load()}
  >
    {#snippet children(health: SystemHealth)}
      <dl class="facts">
        <dt>Jet service</dt>
        <dd>
          {startedText(health)}
          <span class="quiet">{startsText(health.service.daemonStarts)}</span>
        </dd>
        <dt>This app</dt>
        <dd>
          This app {health.app.version}, supports protocol {health.app.supportedProtocol}.
          {#if negotiatedLine(health.protocol)}<span class="quiet">{negotiatedLine(health.protocol)}.</span>{/if}
        </dd>
        {#if health.platform !== null}
          <dt>Platform</dt>
          <dd>{health.platform}</dd>
        {/if}
        <dt>Secure storage</dt>
        <dd>{credentialStoreText(health.credentialStore)}</dd>
      </dl>

      {#if health.tools.length > 0}
        <h3>Tools</h3>
        <ul class="plain-list">
          {#each health.tools as tool (tool.tool)}
            <li><span>{tool.label}</span> <span class:quiet={tool.version !== null}>{toolText(tool)}</span></li>
          {/each}
        </ul>
      {/if}

      <h3>Harness packages</h3>
      {#if health.crafts.length === 0}
        <p>No Craft is installed on this Plane.</p>
      {:else}
        <ul class="plain-list">
          {#each health.crafts as craft (craft.id)}
            <li>
              <span>{craft.id} {craft.version}</span>
              {#if craft.harnesses.length > 0}<span class="quiet">{craft.harnesses.join(", ")}</span>{/if}
            </li>
          {/each}
        </ul>
      {/if}

      <h3>What needs attention</h3>
      {#if health.degraded.length === 0}
        <p>Nothing. This Plane reports no degraded capability.</p>
      {:else}
        <ul class="plain-list">
          {#each health.degraded as condition, index (`${condition.kind}-${index}`)}
            <li class="degraded">
              <span>{condition.label}</span>
              {#if condition.target && LANDED_SECTIONS.has(condition.target.section)}
                {@const section = condition.target.section}
                <button class="text-button" onclick={() => onopen(section)}>Open {sectionTitle(section)}</button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      <div class="actions">
        <button class="secondary-button" disabled={checking} onclick={() => void system.load(true)}>
          {checking ? "Checking…" : "Check again"}
        </button>
      </div>
    {/snippet}
  </SectionState>

  <LocalServiceBlock {service} />
  <AppUpdatesBlock {updates} />
</section>

<style>
  .facts {
    display: grid;
    grid-template-columns: minmax(110px, max-content) minmax(0, 1fr);
    gap: 6px 16px;
    margin: 0;
    font-size: 13px;
  }

  .facts dt {
    color: var(--muted);
  }

  .facts dd {
    display: grid;
    margin: 0;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .plain-list {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 13px;
  }

  .plain-list li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 4px 12px;
    overflow-wrap: anywhere;
  }

  .quiet {
    color: var(--muted);
  }

  .degraded span {
    font-weight: 600;
  }

  .actions {
    display: flex;
    gap: 8px;
  }
</style>
