type EscapeState = "text" | "escape" | "csi" | "osc" | "osc_escape";

/**
 * Incrementally decodes UTF-8 terminal bytes and removes control sequences
 * before they reach the webview. This is a bounded transcript, not a shell.
 */
export class TerminalTranscriptDecoder {
  private readonly decoder = new TextDecoder("utf-8");
  private escapeState: EscapeState = "text";

  push(bytes: number[]): string {
    return this.sanitize(this.decoder.decode(new Uint8Array(bytes), { stream: true }));
  }

  finish(): string {
    return this.sanitize(this.decoder.decode());
  }

  private sanitize(value: string): string {
    let result = "";
    for (const character of value) {
      const scalar = character.codePointAt(0) ?? 0;
      switch (this.escapeState) {
        case "text":
          if (scalar === 0x1b) {
            this.escapeState = "escape";
          } else if (scalar === 0x0a || scalar === 0x0d || scalar === 0x09 || scalar >= 0x20) {
            if (scalar !== 0x7f) result += character;
          }
          break;
        case "escape":
          if (character === "[") this.escapeState = "csi";
          else if (character === "]") this.escapeState = "osc";
          else this.escapeState = "text";
          break;
        case "csi":
          if (scalar >= 0x40 && scalar <= 0x7e) this.escapeState = "text";
          break;
        case "osc":
          if (scalar === 0x07) this.escapeState = "text";
          else if (scalar === 0x1b) this.escapeState = "osc_escape";
          break;
        case "osc_escape":
          if (character === "\\") this.escapeState = "text";
          else if (scalar !== 0x1b) this.escapeState = "osc";
          break;
      }
    }
    return result;
  }
}
