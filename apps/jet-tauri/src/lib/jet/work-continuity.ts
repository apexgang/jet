export function shouldLoadNextWorkPage(
  loadedIds: ReadonlySet<string>,
  targetCount: number,
  selectedId: string | null,
  hasNextPage: boolean,
): boolean {
  if (!hasNextPage) return false;
  return loadedIds.size < targetCount || (selectedId !== null && !loadedIds.has(selectedId));
}
