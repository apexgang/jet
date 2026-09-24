<script lang="ts">
  import { onMount } from "svelte";

  import { publicError } from "$lib/jet/errors";
  import { listPlanes, planeLabel, type Plane, type PlaneId } from "$lib/jet/planes";

  let {
    planeId,
    onselect,
  }: {
    planeId: PlaneId;
    /** Called with the chosen Plane and its label; also when only the label changed. */
    onselect: (planeId: PlaneId, label: string) => void;
  } = $props();

  const id = $props.id();
  let planes = $state<Plane[]>([]);
  let failure = $state<string | null>(null);
  let mounted = false;
  let request = 0;

  /** The registry is native; listing it never connects to a Plane. */
  async function load(): Promise<void> {
    const current = ++request;
    try {
      const snapshot = await listPlanes();
      if (!mounted || current !== request) return;
      planes = snapshot.planes;
      failure = null;
      onselect(planeId, planeLabel(snapshot, planeId));
    } catch (error: unknown) {
      if (!mounted || current !== request) return;
      failure = publicError(error).message;
    }
  }

  function choose(next: PlaneId): void {
    const plane = planes.find((candidate) => candidate.planeId === next);
    onselect(next, plane?.label ?? planeLabel(null, next));
  }

  onMount(() => {
    mounted = true;
    void load();
    const onFocus = () => void load();
    window.addEventListener("focus", onFocus);
    return () => {
      mounted = false;
      window.removeEventListener("focus", onFocus);
    };
  });
</script>

<div class="plane-picker">
  <label for={id}>Plane</label>
  <select {id} value={planeId} onchange={(event) => choose(event.currentTarget.value)}>
    {#if !planes.some((plane) => plane.planeId === planeId)}
      <option value={planeId}>{planeLabel(null, planeId)}</option>
    {/if}
    {#each planes as plane (plane.planeId)}
      <option value={plane.planeId}>{plane.label}</option>
    {/each}
  </select>
  {#if failure}<span class="picker-note" role="status">{failure}</span>{/if}
</div>

<style>
  .plane-picker {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  label {
    color: var(--muted);
    font-size: 13px;
  }

  select {
    min-width: 200px;
    max-width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font: inherit;
  }

  .picker-note {
    color: var(--muted);
    font-size: 12px;
  }
</style>
