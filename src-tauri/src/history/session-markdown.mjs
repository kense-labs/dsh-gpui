/*!
MIT License

Copyright (c) 2026 DeepSeek

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

// DeepSeek Harness fcc0ad46, packages/session-query/session-log-export/src/markdown.ts
// Source SHA-256 df31523d62cd8a9d43602d586d1758fbc870a98302f008bdecdea258065d4841
// TypeScript types stripped by esbuild. No runtime dependencies.

const UNTITLED_SESSION_MARKDOWN_TITLE = "Untitled Session";
function formatSessionMarkdown(events, sourcePath) {
  const title = events.findLast((event) => event.type === "session/title")?.data.title;
  const sections = [`# ${normalizedTitle(title)}`];
  if (sourcePath !== void 0 && sourcePath !== "") {
    sections.push(`Session file:

${fenced(sourcePath, "text", false)}`);
  }
  for (const turn of collectTurns(events)) {
    const finalIndex = finalReplyIndex(turn.segments);
    const end = finalIndex < 0 ? turn.segments.length : finalIndex;
    let pending = [];
    for (const segment of turn.segments.slice(0, end)) {
      if (segment.kind === "user") {
        if (pending.length > 0) {
          sections.push(collapsibleSection(pending));
          pending = [];
        }
        sections.push(quote(segment.text));
      } else {
        pending.push(segment);
      }
    }
    if (pending.length > 0) sections.push(collapsibleSection(pending));
    const final = turn.segments[finalIndex];
    const finalText = final?.kind === "assistant" ? closeUnfinishedFence(trimBlankEdgeLines(final.text)) : "";
    if (finalText !== "") sections.push(finalText);
  }
  return `${sections.join("\n\n").trimEnd()}
`;
}
function normalizedTitle(title) {
  const line = title?.replace(/\r\n?|\n/gu, " ").trim() ?? "";
  return line === "" ? UNTITLED_SESSION_MARKDOWN_TITLE : line;
}
function collectTurns(events) {
  const turns = [];
  let current;
  const begin = () => {
    current = { segments: [], calls: /* @__PURE__ */ new Map() };
    turns.push(current);
    return current;
  };
  const active = () => current ?? begin();
  for (const event of events) {
    switch (event.type) {
      case "turn/start":
        begin();
        break;
      case "turn/end":
        current = void 0;
        break;
      case "user/message": {
        if (event.surfaceOp !== "append" || event.data.source.kind !== "user") break;
        const text = visibleContent(event.data.content);
        if (text !== "") active().segments.push({ kind: "user", text });
        break;
      }
      case "assistant/message": {
        if (event.surfaceOp !== "append") break;
        const message = event.data.message;
        const text = assistantText(message);
        const toolCalls = message.content.some((block) => block.type === "tool-call");
        if (trimBlankEdgeLines(text) !== "") active().segments.push({ kind: "assistant", text, toolCalls });
        break;
      }
      case "tool/call": {
        const turn = active();
        const entry = {
          kind: "tool",
          name: event.data.name,
          arguments: event.data.arguments,
          result: void 0
        };
        turn.segments.push(entry);
        turn.calls.set(String(event.data.callId), entry);
        break;
      }
      case "tool/result": {
        if (event.surfaceOp !== "append") break;
        const turn = active();
        const callId = String(event.data.message.toolCallId);
        let entry = turn.calls.get(callId);
        if (entry === void 0) {
          entry = { kind: "tool", name: "unknown", arguments: "", result: void 0 };
          turn.segments.push(entry);
          turn.calls.set(callId, entry);
        }
        entry.result = toolResult(event.data);
        break;
      }
      default:
        break;
    }
  }
  return turns.filter((turn) => turn.segments.length > 0);
}
function finalReplyIndex(segments) {
  const index = segments.length - 1;
  const entry = segments[index];
  return entry?.kind === "assistant" && !entry.toolCalls ? index : -1;
}
function collapsibleSection(entries) {
  const summary = entries.length === 1 ? "1 previous message" : `${String(entries.length)} previous messages`;
  const body = entries.map(renderEntry).join("\n\n");
  return `<details><summary>${summary}</summary>

${body}

</details>`;
}
function renderEntry(entry) {
  if (entry.kind === "assistant") {
    const text = escapeWrapperTagsOutsideFences(
      closeUnfinishedFence(trimBlankEdgeLines(entry.text))
    );
    return `**Assistant**

${text}`;
  }
  const heading = `**Tool: \`${escapeWrapperTags(entry.name)}\`**`;
  const argumentsText = entry.arguments === "" ? "(no arguments)" : fenced(jsonArguments(entry.arguments), "json");
  const result = entry.result;
  const resultHeading = result?.isError === true ? "**Result (error)**" : "**Result**";
  let resultBody = "";
  if (result === void 0) {
    resultBody = "(no result recorded)";
  } else if (result.reason !== void 0 && result.reason !== "") {
    resultBody = `Error: ${escapeWrapperTags(result.reason)}`;
    if (result.text !== "") resultBody += `

${fenced(result.text)}`;
  } else if (result.text === "") {
    resultBody = "(no output)";
  } else {
    resultBody = fenced(result.text);
  }
  return `${heading}

${argumentsText}

${resultHeading}

${resultBody}`;
}
function visibleContent(content) {
  const parts = [];
  for (const block of content) {
    switch (block.type) {
      case "text":
        if (block.text !== "") parts.push(block.text);
        break;
      case "image":
        parts.push(imageLabel(block.attachment));
        break;
      case "file":
        parts.push(fileLabel(block.attachment));
        break;
      default:
        break;
    }
  }
  return parts.join("\n\n");
}
function assistantText(message) {
  return visibleContent(message.content);
}
function toolResult(data) {
  return {
    text: visibleContent(data.message.content),
    isError: data.message.isError === true,
    reason: data.error?.reason
  };
}
function quote(text) {
  const normalized = text.replace(/\r\n?/gu, "\n").replace(/\n+$/u, "");
  return normalized.split("\n").map((line) => line === "" ? ">" : `> ${line}`).join("\n");
}
function imageLabel(ref) {
  return `[Image: ${labelName(ref.name)} (${String(ref.width)}\xD7${String(ref.height)}, ${String(ref.bytes)} bytes)]`;
}
function fileLabel(ref) {
  return `[File: ${labelName(ref.name)} (${String(ref.bytes)} bytes)]`;
}
function labelName(name) {
  const normalized = name?.replace(/\s+/gu, " ").replace(/\]/gu, "\\]").trim() ?? "";
  return normalized === "" ? "attachment" : normalized;
}
function jsonArguments(argumentsText) {
  let parsed;
  try {
    parsed = JSON.parse(argumentsText);
  } catch {
    return argumentsText;
  }
  return JSON.stringify(parsed, null, 2);
}
function fenced(text, language = "", trimTrailing = true) {
  const content = trimTrailing ? text.replace(/\n+$/u, "") : text;
  let longest = 0;
  for (const match of content.matchAll(/`+/gu)) longest = Math.max(longest, match[0].length);
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}${language}
${content}
${fence}`;
}
function trimBlankEdgeLines(text) {
  const lines = text.replace(/\r\n?/gu, "\n").split("\n");
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start]?.trim() === "") start += 1;
  while (end > start && lines[end - 1]?.trim() === "") end -= 1;
  return lines.slice(start, end).join("\n");
}
function scanMarkdown(markdown) {
  const lines = markdown.replace(/\r\n?/gu, "\n").split("\n");
  const scanned = [];
  let fence;
  let indented = false;
  for (const line of lines) {
    if (fence !== void 0) {
      scanned.push({ text: line, code: true });
      if (closesFence(line, fence)) fence = void 0;
      continue;
    }
    const opening = openingFence(line);
    if (opening !== void 0) {
      fence = opening;
      indented = false;
      scanned.push({ text: line, code: true });
      continue;
    }
    if (/^(?: {4}|\t)/u.test(line)) {
      indented = true;
      scanned.push({ text: line, code: true });
      continue;
    }
    scanned.push({ text: line, code: indented && line.trim() === "" });
    if (line.trim() !== "") indented = false;
  }
  return { lines: scanned, openFence: fence };
}
function openingFence(line) {
  const opening = /^ {0,3}(`{3,}|~{3,})(.*)$/u.exec(line);
  if (opening === null) return void 0;
  const marker = String(opening[1]);
  const info = String(opening[2]);
  if (marker.startsWith("`") && info.includes("`")) return void 0;
  return { character: marker.slice(0, 1), length: marker.length };
}
function closesFence(line, fence) {
  const closing = new RegExp(`^ {0,3}${fence.character}{${String(fence.length)},}\\s*$`, "u");
  return closing.test(line);
}
function closeUnfinishedFence(markdown) {
  const scan = scanMarkdown(markdown);
  if (scan.openFence === void 0) return markdown;
  const content = markdown.replace(/\n+$/u, "");
  return `${content}
${scan.openFence.character.repeat(scan.openFence.length)}`;
}
function escapeWrapperTagsOutsideFences(markdown) {
  return scanMarkdown(markdown).lines.map((line) => line.code ? line.text : escapeWrapperTags(line.text)).join("\n");
}
function escapeWrapperTags(text) {
  return text.replace(/<(?=\/?(?:details|summary)(?=[\s/>]))/giu, "&lt;");
}
export {
  formatSessionMarkdown
};
