/**
 * Returns focus after a modal or inline flow ends. The invoking control is
 * preferred, but flows often remove it (a revoked row, Pair again once the
 * Plane is online, Setup's button once Planes opens), so focus falls back to
 * the first stable anchor that exists instead of dropping to `<body>`.
 */
export function restoreFocus(preferred: HTMLElement | null, fallbacks: readonly string[]): void {
  if (preferred?.isConnected && !preferred.matches(":disabled")) {
    preferred.focus();
    return;
  }
  for (const selector of fallbacks) {
    const anchor = document.querySelector<HTMLElement>(selector);
    if (anchor) {
      anchor.focus();
      return;
    }
  }
}

/** Whether keyboard focus was lost to the page itself. */
export function focusLost(): boolean {
  const active = document.activeElement;
  return active === null || active === document.body;
}

/** Fallbacks inside the Planes destination, most specific first. */
export const CLIENTS_ANCHORS = ["#plane-clients-heading", "#plane-pairing-heading", "#plane-detail-title", "#planes-title"];
export const CONNECTION_ANCHORS = ["#plane-connection-heading", "#plane-detail-title", "#planes-add-button", "#planes-title"];
export const ADD_ANCHORS = ["#planes-add-button", "#plane-connection-heading", "#planes-title"];
