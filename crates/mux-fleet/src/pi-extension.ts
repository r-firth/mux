// Mux's fleet extension for pi. Installed and removed by Mux
// (`muxctl hooks install pi`, `muxctl hooks uninstall pi`); a new Mux
// overwrites it, so keep your own extensions in files beside this one.
//
// It tells the Mux that owns this terminal pane what pi is doing: the
// prompt, the tool in hand, that a turn is done. Outside a Mux pane it
// does nothing at all. It never prints, never changes a prompt or a
// tool call, and never holds pi up.
//
// mux-fleet-extension 1
// @ts-nocheck

import { spawn } from "node:child_process";

const pane = process.env.MUX_PANE_ID;
const muxctl = process.env.MUX_CTL;

function report(event: string, ctx: any, fields: Record<string, unknown> = {}): void {
  if (!pane || !muxctl) return;
  try {
    const manager = ctx?.sessionManager;
    const input = JSON.stringify({
      session_id: manager?.getSessionId?.(),
      transcript_path: manager?.getSessionFile?.(),
      cwd: ctx?.cwd,
      ...fields,
    });
    const child = spawn(muxctl, ["hook", "pi", event], {
      stdio: ["pipe", "ignore", "ignore"],
    });
    child.on("error", () => {});
    child.stdin.on("error", () => {});
    child.stdin.end(input);
    child.unref();
  } catch {
    // Mux not hearing of a turn is never pi's trouble.
  }
}

// The words of the last thing the assistant said in a prompt's messages.
function lastWords(messages: any): string | undefined {
  if (!Array.isArray(messages)) return undefined;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message?.role !== "assistant") continue;
    const content = message.content;
    const text =
      typeof content === "string"
        ? content
        : Array.isArray(content)
          ? content
              .filter((part) => part?.type === "text" && typeof part.text === "string")
              .map((part) => part.text)
              .join("\n")
          : "";
    if (text.trim()) return text.trim().slice(0, 2000);
  }
  return undefined;
}

export default function (pi: any) {
  if (!pane || !muxctl) return;

  pi.on("session_start", (_event: any, ctx: any) => {
    report("SessionStart", ctx);
  });
  pi.on("before_agent_start", (event: any, ctx: any) => {
    report("UserPromptSubmit", ctx, {
      prompt: typeof event?.prompt === "string" ? event.prompt.slice(0, 2000) : undefined,
    });
  });
  pi.on("tool_execution_start", (event: any, ctx: any) => {
    report("PreToolUse", ctx, { tool_name: event?.toolName, tool_input: event?.args });
  });
  pi.on("tool_execution_end", (_event: any, ctx: any) => {
    report("PostToolUse", ctx);
  });
  pi.on("agent_end", (event: any, ctx: any) => {
    report("Stop", ctx, { last_assistant_message: lastWords(event?.messages) });
  });
  pi.on("session_shutdown", (event: any, ctx: any) => {
    // A reload, or a move to another session, starts again at once.
    if (event?.reason === undefined || event.reason === "quit") {
      report("SessionEnd", ctx);
    }
  });
}
