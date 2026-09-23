// Adaptation audit prelude (wave 3.4 §8). Registered as the first page init
// script, before the bundled mocked IPC, so the mock knows which scene to
// answer. The scene comes from the page's own query string (`?scene=`), so
// one browser session can open every scene in turn. Only a known scene name
// is accepted.
(() => {
  const scenes = ["setup", "approval-run", "changes", "new-task", "planes", "schedules", "trash", "settings"];
  const requested = new URLSearchParams(window.location.search).get("scene");
  /** @type {Window & { __JET_ADAPTATION_SCENE__?: string }} */ (window).__JET_ADAPTATION_SCENE__ =
    requested !== null && scenes.includes(requested) ? requested : "approval-run";
})();
