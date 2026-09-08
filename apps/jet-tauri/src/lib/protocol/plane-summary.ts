// Views over the generated client protocol models. The models are not
// decoders: validate a frame against jet-v1.schema.json, then read it here.
import type { PlaneStatus, ServerHello } from "./JetModels";

/** How this client labels a Plane before any Conversation is loaded. */
export function planeLabel(status: PlaneStatus): string {
  return `${status.plane_id} (jetd ${status.core_version})`;
}

/** The negotiated protocol, or null while the connection is unauthenticated. */
export function negotiatedProtocol(hello: ServerHello): string | null {
  return hello.kind === "welcome" ? `${hello.protocol}.${hello.minor}` : null;
}
