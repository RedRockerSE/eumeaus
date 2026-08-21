// Native OS file/directory pickers (exploratory test §1/§2: every path
// input in the GUI used to be type-the-correct-path-only). Thin wrappers
// over @tauri-apps/plugin-dialog so screens don't import it directly —
// same "centralize the Tauri surface" reasoning as api.ts.
//
// Every field that gets one of these keeps its text input too: a picker
// fills the field, it doesn't replace typing (a user who already knows
// the path shouldn't be forced through a dialog).

import { open } from "@tauri-apps/plugin-dialog";

export async function pickDirectory(): Promise<string | null> {
  const result = await open({ directory: true, multiple: false });
  return typeof result === "string" ? result : null;
}

export async function pickCaseFile(): Promise<string | null> {
  const result = await open({
    directory: false,
    multiple: false,
    filters: [{ name: "Eumeaus case", extensions: ["eum"] }],
  });
  return typeof result === "string" ? result : null;
}
