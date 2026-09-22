import type { PublicError, PublicRecoveryAction } from "./bridge";

export function publicError(error: unknown): PublicError {
  const candidate = publicErrorCandidate(error);
  return {
    category: typeof candidate.category === "string" ? candidate.category : "internal",
    code: typeof candidate.code === "string" ? candidate.code : "client.request_failed",
    message:
      typeof candidate.message === "string"
        ? candidate.message
        : "Jet could not complete the request.",
    retryable: candidate.retryable === true,
    recoveryActions: Array.isArray(candidate.recoveryActions)
      ? candidate.recoveryActions.filter(isPublicRecoveryAction)
      : [],
    restart: candidate.restart && typeof candidate.restart === "object"
      ? candidate.restart
      : null,
    revisionConflict:
      candidate.revisionConflict && typeof candidate.revisionConflict === "object"
        ? candidate.revisionConflict
        : null,
  };
}

function isPublicRecoveryAction(value: unknown): value is PublicRecoveryAction {
  if (!value || typeof value !== "object" || !("type" in value)) return false;
  const type = (value as { type?: unknown }).type;
  if (type === "refresh_file" || type === "refresh_conversation" || type === "refresh_run") {
    return true;
  }
  return type === "resume_events" && typeof (value as { after?: unknown }).after === "string";
}

function publicErrorCandidate(error: unknown): Partial<PublicError> {
  let candidate = error;
  if (typeof candidate === "string") {
    try {
      candidate = JSON.parse(candidate) as unknown;
    } catch {
      return {};
    }
  }

  if (candidate && typeof candidate === "object") {
    const record = candidate as Record<string, unknown>;
    if ("category" in record || "code" in record || "retryable" in record) {
      return record as Partial<PublicError>;
    }
    for (const key of ["error", "data", "cause"] as const) {
      if (key in record && record[key] !== candidate) {
        const nested = publicErrorCandidate(record[key]);
        if (nested.code || nested.category) return nested;
      }
    }
  }
  return {};
}
