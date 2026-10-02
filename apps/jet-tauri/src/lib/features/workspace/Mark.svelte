<script lang="ts">
  let { small = false }: { small?: boolean } = $props();
</script>
<svg class:small class="jet-mark" viewBox="0 0 32 32" aria-hidden="true">
  <path d="M5 22 14 7h6L11 22H5Zm10 3L24 10h4L19 25h-4Z" fill="currentColor" />
</svg>
<style>
  .jet-mark { width: 32px; height: 32px; flex: none; color: var(--accent-text); }
  .small { width: 24px; height: 24px; }
</style>
