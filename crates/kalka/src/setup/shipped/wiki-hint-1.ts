// Adds the project wiki's code lines for the prompt before the model runs (wiki setup --hook).
import { execFileSync } from "node:child_process";
import path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.on("before_agent_start", (event, ctx) => {
    if (event.messageType !== "user" || !event.prompt.trim()) return;
    let text = "";
    try {
      text = execFileSync("<cmd>", ["hint", "--dir", path.join(ctx.cwd, ".wiki"), "--", event.prompt], {
        encoding: "utf8",
        timeout: 3000,
      });
    } catch {
      return;
    }
    if (!text.trim()) return;
    return { message: { customType: "wiki-hint", content: text, display: false } };
  });
}
