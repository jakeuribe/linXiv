import { CheckMenuItem, Menu } from "@tauri-apps/api/menu";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { isTauri } from "../api/client";

export type ContextMenuItem =
  | "separator"
  // `checked` present (even false) makes it a native check item.
  | { text: string; action: () => void; enabled?: boolean; checked?: boolean };

// Rust-side menu resources are only freed via close(); keep one live menu and
// close it before the next popup so repeated right-clicks don't accumulate.
// `generation` makes rapid right-clicks last-writer-wins: a click that loses
// the race discards its own menu instead of closing the newer one.
let lastMenu: Menu | null = null;
let generation = 0;

// popup() resolves when the menu is SHOWN (not dismissed), so the last menu
// can't be closed inline — the next pointer interaction anywhere frees it
// instead. Native menu clicks don't reach the DOM, so this fires on the first
// page interaction after the menu is gone; one bounded resource in between.
let sweeperArmed = false;
function armMenuSweeper() {
  if (sweeperArmed) return;
  sweeperArmed = true;
  document.addEventListener(
    "pointerdown",
    () => {
      void lastMenu?.close().catch(() => {});
      lastMenu = null;
    },
    { capture: true }
  );
}

/** Pop a native context menu at the cursor. Outside Tauri (browser dev) this
 *  is a no-op that lets the browser's default menu through. */
export function showContextMenu(
  e: React.MouseEvent,
  items: ContextMenuItem[]
): void {
  if (!isTauri) return;
  e.preventDefault();
  e.stopPropagation();
  armMenuSweeper();
  // Captured before the async hop: the menu pops where the mouse actually
  // clicked (webview-logical coords), not wherever the OS last showed one.
  const at = new LogicalPosition(e.clientX, e.clientY);
  const gen = ++generation;
  (async () => {
    const menu = await Menu.new({
      items: await Promise.all(
        items.map((item) =>
          item === "separator"
            ? { item: "Separator" as const }
            : item.checked !== undefined
              ? CheckMenuItem.new({
                  text: item.text,
                  checked: item.checked,
                  enabled: item.enabled ?? true,
                  action: item.action,
                })
              : { text: item.text, action: item.action, enabled: item.enabled ?? true }
        )
      ),
    });
    if (gen !== generation) {
      // A newer right-click superseded this one while Menu.new was in flight.
      void menu.close().catch(() => {});
      return;
    }
    const prev = lastMenu;
    lastMenu = menu;
    await prev?.close().catch(() => {});
    await menu.popup(at);
  })().catch((err) => {
    // A newer click closing this menu mid-flight is expected; anything else
    // (menu API systematically failing) must not fail silent — right-click
    // would be dead app-wide with no trace.
    if (gen === generation) console.error("native context menu failed:", err);
  });
}
