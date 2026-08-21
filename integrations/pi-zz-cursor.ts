import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { CustomEditor, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { EditorTheme, TUI } from "@earendil-works/pi-tui";

type AppKeybindings = {
  matches(data: string, action: string): boolean;
};

class ZzCursorEditor extends CustomEditor {
  private opening = false;

  constructor(
    tui: TUI,
    theme: EditorTheme,
    private readonly appKeybindings: AppKeybindings,
  ) {
    super(tui, theme, appKeybindings as never);
  }

  override handleInput(data: string): void {
    if (this.appKeybindings.matches(data, "app.editor.external")) {
      if (!this.opening) void this.openZz();
      return;
    }
    super.handleInput(data);
  }

  private cursorByte(): number {
    const { line, col } = this.getCursor();
    const lines = this.getLines();
    const rawPrefix = [...lines.slice(0, line), (lines[line] ?? "").slice(0, col)].join("\n");
    return Buffer.byteLength(this.expandPasteMarkers(rawPrefix), "utf8");
  }

  private async openZz(): Promise<void> {
    this.opening = true;
    const directory = mkdtempSync(join(tmpdir(), "pi-zz-"));
    const file = join(directory, "prompt.md");
    const content = this.getExpandedText();
    const cursorByte = this.cursorByte();
    writeFileSync(file, content, "utf8");
    this.tui.stop();
    try {
      const status = await new Promise<number | null>((resolve) => {
        const child = spawn(process.env.ZZ_BIN || "zz", [file], {
          env: { ...process.env, ZZ_CURSOR_BYTE: String(cursorByte) },
          stdio: "inherit",
        });
        child.on("error", () => resolve(null));
        child.on("close", resolve);
      });
      if (status === 0) {
        this.setText(readFileSync(file, "utf8").replace(/\n$/, ""));
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
      this.tui.start();
      this.tui.requestRender(true);
      this.opening = false;
    }
  }
}

export default function zzCursorIntegration(pi: ExtensionAPI): void {
  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui") return;
    ctx.ui.setEditorComponent(
      (tui, theme, keybindings) => new ZzCursorEditor(tui, theme, keybindings),
    );
  });
}
