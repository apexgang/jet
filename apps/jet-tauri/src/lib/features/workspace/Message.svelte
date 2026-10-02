<script lang="ts">
  import { messageBlocks } from "./message";
  let { text }: { text: string } = $props();
  const blocks = $derived(messageBlocks(text));
</script>
<div class="message-body">
  {#each blocks as block, index (index)}
    {#if block.kind === "code"}
      <div class="message-code">{#if block.language}<span>{block.language}</span>{/if}<pre><code>{block.text}</code></pre></div>
    {:else if block.kind === "heading"}<h3>{block.text}</h3>
    {:else if block.kind === "list"}<p class="message-list"><span aria-hidden="true">•</span>{block.text}</p>
    {:else}<p>{block.text}</p>{/if}
  {/each}
</div>
