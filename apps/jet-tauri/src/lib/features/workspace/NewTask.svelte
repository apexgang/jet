<script lang="ts">
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import Composer from "./Composer.svelte";
  import Mark from "./Mark.svelte";
  let { session }: { session: DesktopSession } = $props();
  const examples = [
    { title: "Understand a Project", prompt: "Explain what this Project does and how its main parts fit together. Start with a plain-language overview." },
    { title: "Make a change", prompt: "I would like to change " },
    { title: "Find a problem", prompt: "Help me investigate a problem in this Project. Here is what happens: " },
  ];
</script>
<div class="new-work">
  <div class="new-work-content">
    <Mark />
    <h2>What are you working on?</h2>
    <p class="new-work-intro">Describe what you need. You can review the changes as you go.</p>
    {#if !session.canStartTask}
      <div class="start-requirements" role="status">
        <span>{!session.selectedPlaneOnline ? "Connect Jet to start working on this computer." : !session.selectedProject ? "Add a Project to give your work a home." : "Choose an installed Harness to start."}</span>
        <button onclick={() => session.select("project")}>{session.setupSnapshot ? "Open setup" : "Set up Jet"}<span aria-hidden="true"> →</span></button>
      </div>
    {/if}
    <Composer {session} starting />
    <div class="starting-points" aria-label="Ideas for a task">
      <span>Start with an idea</span>
      {#each examples as example}
        <button disabled={session.draft.trim().length > 0} onclick={() => { session.draft = example.prompt; session.composerFocusRequest += 1; }}>
          {example.title}<span aria-hidden="true">↗</span>
        </button>
      {/each}
    </div>
    <p class="work-reassurance">Your tasks stay here, ready to continue.</p>
  </div>
</div>
