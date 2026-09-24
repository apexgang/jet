import type { PublicError } from "$lib/jet/bridge";
import type {
  LocalServiceChannel,
  LocalServiceManager,
  LocalServicePhase,
  LocalServiceView,
} from "$lib/jet/local-service";
import type { AppUpdate, AppUpdateDisabledReason } from "$lib/jet/updates";

/**
 * Copy for the local Jet service and app updates (Wave 4 §A, §B). Setup says
 * what the shell is doing while it works; Settings › Versions says who
 * manages the service and which versions it keeps.
 */

/** What Setup shows while the shell works on the service, or null. */
export function provisioningText(phase: LocalServicePhase): string | null {
  switch (phase) {
    case "checking":
      return "Checking the Jet service on this computer…";
    case "installing":
      return "Setting up the Jet service on this computer…";
    case "updating":
      return "Updating the Jet service on this computer…";
    case "starting":
      return "Starting the Jet service…";
    default:
      return null;
  }
}

export type ServiceProblem = { title: string; detail: string; code: string | null };

/** Setup's copy when the local Plane can't be reached because of its service. */
export function serviceProblem(view: LocalServiceView): ServiceProblem | null {
  switch (view.phase) {
    case "failed":
      return {
        title: "The Jet service needs attention",
        detail: serviceErrorText(view.error),
        code: view.error?.code ?? null,
      };
    case "stopped":
      return {
        title: "The Jet service isn't running",
        detail:
          view.channel === "homebrew"
            ? "Homebrew installed it. Jet can ask Homebrew to start it."
            : "It's installed on this computer. Jet can start it again.",
        code: null,
      };
    case "not_installed":
      return {
        title: "The Jet service isn't installed",
        detail:
          "This build of Jet doesn't include the service. Start jetd from a Jet checkout, or install Jet from a release, then check again.",
        code: null,
      };
    default:
      return null;
  }
}

/** One sentence for a `service.*` failure; the shell's message otherwise. */
export function serviceErrorText(error: PublicError | null): string {
  if (error === null) return "Jet couldn't reach the Jet service on this computer.";
  switch (error.code) {
    case "service.channel_owned":
      return "Another installation of Jet manages the service on this computer, so this app left it alone.";
    case "service.drain_timeout":
      return "The running Jet service didn't stop in time, so nothing was changed. Try again when no task is finishing.";
    case "service.start_timeout":
      return "The Jet service was started but didn't answer in time.";
    case "service.systemd_unavailable":
      return "Your system's service manager didn't accept the Jet service.";
    case "service.homebrew_start_failed":
      return "Homebrew couldn't start the Jet service. Try brew services start apexgang/tap/jet in a terminal.";
    case "service.payload_invalid":
      return "The Jet service included with this app is damaged. Reinstall Jet.";
    case "service.busy":
      return "Another Jet window is already working on the service. Try again in a moment.";
    default:
      return error.message;
  }
}

export function channelText(channel: LocalServiceChannel | null): string {
  switch (channel) {
    case "gui":
      return "Managed by this app";
    case "homebrew":
      return "Managed by Homebrew";
    case "development":
      return "Development build";
    default:
      return "Not identified";
  }
}

export function managerText(manager: LocalServiceManager | null): string | null {
  switch (manager) {
    case "systemd":
      return "Starts when you log in (systemd user service)";
    case "autostart":
      return "Starts when you log in (desktop autostart)";
    case "brew_services":
      return "Started by brew services";
    default:
      return null;
  }
}

export function phaseText(phase: LocalServicePhase): string {
  switch (phase) {
    case "running":
      return "Running";
    case "stopped":
      return "Not running";
    case "not_installed":
      return "Not installed";
    case "failed":
      return "Needs attention";
    default:
      return provisioningText(phase) ?? "Checking…";
  }
}

/** A one-line confirmation of what the last pass changed, or null. */
export function actionText(view: LocalServiceView): string | null {
  const version = view.currentVersion ? ` ${view.currentVersion}` : "";
  switch (view.lastAction) {
    case "installed":
      return `The Jet service${version} is set up on this computer.`;
    case "updated":
      return `The Jet service was updated to${version || " a newer version"}.`;
    case "started":
      return "The Jet service started.";
    case "rolled_back":
      return `The Jet service went back to${version || " the earlier version"}.`;
    default:
      return null;
  }
}

/** The rollback review's consequences, in order. */
export function rollbackLines(current: string, previous: string): string[] {
  return [
    `The Jet service stops, then starts again with version ${previous}.`,
    "Tasks that are running keep running; the service reconnects to them.",
    `Version ${current} stays installed, so you can update again later.`,
  ];
}

export function updateDisabledText(reason: AppUpdateDisabledReason): string {
  switch (reason) {
    case "homebrew":
      return "Homebrew keeps this copy of Jet up to date. Update it with brew upgrade.";
    case "development_build":
      return "This is a development build, so it doesn't update itself.";
    case "unsupported_install":
      return "This copy of Jet can't update itself. Install it from a .deb, .rpm or AppImage release to get updates.";
  }
}

/** Download progress as a whole percentage, or null when the size is unknown. */
export function downloadPercent(downloaded: number, total: number | null): number | null {
  if (total === null || total <= 0) return null;
  return Math.min(100, Math.floor((downloaded / total) * 100));
}

/** The App updates block's status line. */
export function updateStatusText(update: AppUpdate): string {
  const state = update.state;
  switch (state.kind) {
    case "disabled":
      return updateDisabledText(state.reason);
    case "idle":
      return state.upToDate ? `Jet ${update.currentVersion} is up to date.` : `This is Jet ${update.currentVersion}.`;
    case "checking":
      return "Checking for updates…";
    case "available":
      return `Jet ${state.version} is available. You have ${update.currentVersion}.`;
    case "downloading": {
      const percent = downloadPercent(state.downloaded, state.total);
      return percent === null ? `Downloading Jet ${state.version}…` : `Downloading Jet ${state.version}: ${percent}%`;
    }
    case "ready":
      return `Jet ${state.version} is installed. Restart Jet to use it.`;
    case "failed":
      return updateErrorText(state.error);
  }
}

export function updateErrorText(error: PublicError): string {
  switch (error.code) {
    case "update.offline":
      return "Jet couldn't reach github.com to check for updates.";
    case "update.release_unavailable":
      return "The update information isn't available right now. Try again later.";
    case "update.signature_invalid":
      return "The downloaded update isn't signed by Jet, so it wasn't installed.";
    case "update.package_install_failed":
      return "The update wasn't installed. The password prompt was canceled, or the system refused the package.";
    default:
      return error.message;
  }
}
