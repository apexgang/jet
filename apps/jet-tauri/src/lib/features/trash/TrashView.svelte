<script lang="ts">
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import TrashPlaneSection from "./TrashPlaneSection.svelte";

  let { session }: { session: DesktopSession } = $props();
</script>

<section class="trash-destination" aria-labelledby="trash-title">
  <header class="setup-header">
    <div>
      <h1 id="trash-title" tabindex="-1">Jet Trash</h1>
      <p>Tasks here are deleted on the date shown. Restore a task to keep it.</p>
    </div>
    <button class="secondary-button" onclick={() => session.trash.show()}>Refresh</button>
  </header>
  <div class="setup-content trash-body">
    {#each session.planes.planes as plane (plane.planeId)}
      <TrashPlaneSection {session} planeId={plane.planeId} planeLabel={plane.label} />
    {:else}
      <p class="trash-status" role="status">
        {session.planes.error ? "Jet can't list your Planes right now." : "Loading Planes…"}
      </p>
    {/each}
  </div>
</section>
