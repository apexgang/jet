<script lang="ts">
  import type { SettingsSnapshot } from "$lib/jet/settings";
  import type { SettingsSection } from "$lib/jet/settings-window";
  import SectionState from "./SectionState.svelte";
  import SettingRow from "./SettingRow.svelte";
  import { planeRows } from "./model";
  import type { SettingsSession } from "./session.svelte";

  let { session }: { session: SettingsSession } = $props();

  const sections: ReadonlyArray<{ id: SettingsSection; title: string; help: string }> = [
    {
      id: "execution",
      title: "Execution",
      help: "How much work this Plane runs at once. The Plane enforces these limits.",
    },
    {
      id: "permissions",
      title: "Permissions",
      help: "What this Plane lets Harness packages do.",
    },
    {
      id: "storage",
      title: "Storage",
      help: "How much space task output may use on this Plane.",
    },
  ];

  const auditText = $derived.by(() => {
    switch (session.planeState?.security) {
      case "trusted":
        return "Audit is healthy.";
      case "degraded":
        return "Audit needs repair. This app can't repair the audit yet.";
      default:
        return "Audit state isn't reported by this Plane.";
    }
  });
</script>

{#each sections as section (section.id)}
  <section class="settings-section" aria-labelledby={`section-${section.id}`}>
    <h2 id={`section-${section.id}`} tabindex="-1">{section.title}</h2>
    <p>{section.help}</p>
    <SectionState
      state={session.plane}
      title={section.title}
      planeLabel={session.planeLabel}
      onretry={() => void session.showCurrent({ type: "plane" })}
    >
      {#snippet children(_snapshot: SettingsSnapshot)}
        <div class="setting-rows">
          {#each planeRows(section.id) as key (key)}
            <SettingRow {session} settingKey={key} scope={{ type: "plane" }} />
          {/each}
        </div>
      {/snippet}
    </SectionState>
  </section>
{/each}

<section class="settings-section" aria-labelledby="section-audit">
  <h2 id="section-audit" tabindex="-1">Audit</h2>
  <p class:warning={session.planeState?.security === "degraded"}>{auditText}</p>
  <SectionState
    state={session.plane}
    title="Audit"
    planeLabel={session.planeLabel}
    onretry={() => void session.showCurrent({ type: "plane" })}
  >
    {#snippet children(_snapshot: SettingsSnapshot)}
      <div class="setting-rows">
        {#each planeRows("audit") as key (key)}
          <SettingRow {session} settingKey={key} scope={{ type: "plane" }} />
        {/each}
      </div>
    {/snippet}
  </SectionState>
</section>

<style>
  .setting-rows {
    display: grid;
  }

  .warning {
    color: var(--warning) !important;
  }
</style>
