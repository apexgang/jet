<script lang="ts">
  import { untrack } from "svelte";

  import AutodeleteRules from "$lib/features/autodelete/AutodeleteRules.svelte";
  import SchedulesDestination from "$lib/features/schedules/SchedulesDestination.svelte";
  import type { SettingsSnapshot, WorkContext } from "$lib/jet/settings";
  import type { SettingsSection } from "$lib/jet/settings-window";
  import ConsentRow from "./ConsentRow.svelte";
  import SectionState from "./SectionState.svelte";
  import SettingRow from "./SettingRow.svelte";
  import { planeRows, projectRows, sectionData, withIssues } from "./model";
  import type { SettingsSession } from "./session.svelte";

  let {
    session,
    onopen,
  }: {
    session: SettingsSession;
    /** Shows another Settings section in this window. */
    onopen: (section: SettingsSection) => void;
  } = $props();

  // Auto-delete rules load once per Plane selection while this pane is shown.
  $effect(() => {
    void session.autodelete.selection;
    untrack(() => void session.autodelete.ensureLoaded());
  });

  const projects = $derived(sectionData(session.work)?.projects ?? []);
  const projectsState = $derived(withIssues(session.work, ["projects"]));
  const graceDays = $derived.by(() => {
    const value = session.setting("retention.trash_grace_days", { type: "plane" })?.value;
    return value?.type === "count" ? value.value : null;
  });
  const accountsIssue = $derived(
    session.work.kind === "ready" ? session.work.issues.find((issue) => issue.section === "accounts") : undefined,
  );
  const PLANE = { type: "plane" } as const;
  const projectScope = $derived(
    session.projectId ? { type: "project" as const, projectId: session.projectId } : null,
  );
  const pickerId = $props.id();
</script>

<section class="settings-section" aria-labelledby="section-projects">
  <h2 id="section-projects" tabindex="-1">Projects</h2>
  <p>Defaults for tasks in one Project. A task can still change them for itself.</p>
  <SectionState
    state={projectsState}
    title="Projects"
    planeLabel={session.planeLabel}
    onretry={() => void session.showCurrent(null)}
  >
    {#snippet children(context: WorkContext)}
      {#if context.projects.length === 0}
        {#if !context.issues.some((issue) => issue.section === "projects")}
          <p>Add a Project from Projects in the main window to set its defaults.</p>
        {/if}
      {:else}
        <div class="project-picker">
          <label for={pickerId}>Project</label>
          <select
            id={pickerId}
            value={session.projectId ?? ""}
            onchange={(event) => void session.selectProject(event.currentTarget.value)}
          >
            {#each projects as project (project.id)}
              <option value={project.id}>{project.name}</option>
            {/each}
          </select>
        </div>
        {#if session.project && projectScope}
          {@const scope = projectScope}
          <SectionState
            state={session.project}
            title="Project defaults"
            planeLabel={session.planeLabel}
            onretry={() => void session.showCurrent(scope)}
          >
            {#snippet children(_snapshot: SettingsSnapshot)}
              <div class="setting-rows">
                {#each projectRows() as key (key)}
                  <SettingRow {session} settingKey={key} {scope} />
                {/each}
              </div>
            {/snippet}
          </SectionState>
        {/if}
      {/if}
    {/snippet}
  </SectionState>
</section>

<section class="settings-section" aria-labelledby="section-delivery">
  <h2 id="section-delivery" tabindex="-1">Delivery</h2>
  <p>How Jet writes commit messages and pull requests on this Plane.</p>
  <SectionState
    state={session.plane}
    title="Delivery"
    planeLabel={session.planeLabel}
    onretry={() => void session.showCurrent({ type: "plane" })}
  >
    {#snippet children(_snapshot: SettingsSnapshot)}
      <div class="setting-rows">
        {#each planeRows("delivery") as key (key)}
          <SettingRow {session} settingKey={key} scope={{ type: "plane" }} />
        {/each}
      </div>
    {/snippet}
  </SectionState>
</section>

<section class="settings-section" aria-labelledby="section-reviews">
  <h2 id="section-reviews" tabindex="-1">Reviews</h2>
  <p>Jet can decide eligible approval requests for you on this Plane. Requests it can't decide wait for you.</p>
  <SectionState
    state={session.plane}
    title="Reviews"
    planeLabel={session.planeLabel}
    onretry={() => void session.showCurrent(PLANE)}
  >
    {#snippet children(_snapshot: SettingsSnapshot)}
      {#if accountsIssue}
        <p class="notice" role="status">
          Jet couldn't list this Plane's accounts, so reviewer accounts show without names.
          <code>{accountsIssue.error.code}</code>
        </p>
      {/if}
      <div class="setting-rows">
        <SettingRow {session} settingKey="review.automatic" scope={PLANE} />
        <SettingRow {session} settingKey="review.account_binding" scope={PLANE} />
        <ConsentRow
          {session}
          bindingKey="review.account_binding"
          missingText={(label) => `Without this, reviews by ${label} wait for you.`}
        />
      </div>
      <p class="note">
        The reviewer account and permission to send it task content are saved as two separate changes. If one of them
        isn't saved, this page shows exactly what was.
      </p>
    {/snippet}
  </SectionState>
</section>

<section class="settings-section" aria-labelledby="section-schedules">
  <h2 id="section-schedules" tabindex="-1">Schedules</h2>
  <SchedulesDestination changed={session.schedulesChanged} />
</section>

<section class="settings-section" aria-labelledby="section-retention">
  <h2 id="section-retention" tabindex="-1">Retention</h2>
  <SectionState
    state={session.plane}
    title="Retention"
    planeLabel={session.planeLabel}
    onretry={() => void session.showCurrent({ type: "plane" })}
  >
    {#snippet children(_snapshot: SettingsSnapshot)}
      <div class="setting-rows">
        {#each planeRows("retention") as key (key)}
          <SettingRow {session} settingKey={key} scope={{ type: "plane" }} />
        {/each}
      </div>
    {/snippet}
  </SectionState>
  <h3>Auto-delete</h3>
  <AutodeleteRules autodelete={session.autodelete} planeLabel={session.planeLabel} {graceDays} {onopen} />
</section>

<style>
  .project-picker {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  .project-picker label {
    color: var(--muted);
    font-size: 13px;
  }

  .project-picker select {
    min-width: 200px;
    max-width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font: inherit;
  }

  .setting-rows {
    display: grid;
  }

  .note,
  .notice {
    font-size: 12px;
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
