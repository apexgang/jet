import type { Delivery, DeliveryOperation } from "$lib/jet/delivery";

export function operationLabel(kind: DeliveryOperation["kind"]): string {
  switch (kind) {
    case "branch": return "Create branch";
    case "commit": return "Commit checkpoint";
    case "push": return "Push branch";
    case "draft_pull_request": return "Create or update draft PR";
  }
}
export function deliverySummary(rows: Delivery[]): string {
  if (rows.some((row) => row.status === "outcome_unknown" && !row.acknowledged))
    return "An operation has an unknown outcome. Inspect Git or GitHub before acknowledging it. Do not repeat it.";
  if (rows.some((row) => row.status === "failed") && rows.some((row) => row.status === "completed"))
    return "Some steps completed and others failed. Completed changes remain in place.";
  if (rows.some((row) => row.status === "pending"))
    return "Delivery is queued or running. Completion has not been confirmed.";
  return rows.length ? "Each step below shows its recorded result." : "No delivery operations yet.";
}
export function statusLabel(row: Delivery): string {
  switch (row.status) {
    case "pending": return "Queued or running";
    case "completed": return "Completed";
    case "failed": return "Failed";
    case "outcome_unknown": return row.acknowledged ? "Unknown · acknowledged" : "Outcome unknown";
  }
}
