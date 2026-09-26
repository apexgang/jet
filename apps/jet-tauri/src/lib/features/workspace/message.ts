/** A deliberately inert subset of Markdown. No HTML, link navigation, or images.
 * ASVS 1.2.1: text is rendered through escaped Svelte interpolation only. */
export type MessageBlock = { kind: "paragraph" | "heading" | "code" | "list"; text: string; language?: string };
export function messageBlocks(text: string): MessageBlock[] {
  const blocks: MessageBlock[] = [];
  let buffer: string[] = [];
  let fenced = false;
  let language = "";
  const flush = (kind: MessageBlock["kind"] = "paragraph") => {
    if (buffer.length) blocks.push({ kind, text: buffer.join("\n"), ...(kind === "code" ? { language } : {}) });
    buffer = [];
  };
  for (const line of text.split("\n")) {
    if (line.startsWith("```")) {
      flush(fenced ? "code" : "paragraph");
      fenced = !fenced;
      if (fenced) language = line.slice(3).trim();
    } else if (fenced) buffer.push(line);
    else if (/^#{1,6} /.test(line)) {
      flush();
      blocks.push({ kind: "heading", text: line.replace(/^#{1,6} /, "") });
    } else if (/^[-*] /.test(line)) {
      flush();
      blocks.push({ kind: "list", text: line.slice(2) });
    } else if (!line.trim()) flush();
    else buffer.push(line);
  }
  flush(fenced ? "code" : "paragraph");
  return blocks;
}
