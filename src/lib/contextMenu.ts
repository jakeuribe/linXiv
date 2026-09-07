import { invoke } from "@tauri-apps/api/core";
import { CheckMenuItem, Menu } from "@tauri-apps/api/menu";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { isTauri } from "../api/client";
import { useUiStore } from "../stores/ui";

export type ContextMenuItem =
  | "separator"
  // `checked` present (even false) makes it a native check item.
  | { text: string; action: () => void; enabled?: boolean; checked?: boolean };

// Every menu command is serialized through this chain. popup() holds tauri's
// resources-table mutex until the menu is DISMISSED, while close() is a sync
// command served on the GTK main thread — inside the popup's own nested event
// loop. close() during an open popup therefore deadlocks the whole app (main
// thread parks on the mutex, the popup holds it until the main thread returns;
// observed live in eu-stack). Queueing behind the previous popup's resolution
// guarantees the mutex is free before any close/build/popup runs.
let chain: Promise<void> = Promise.resolve();
// Check items are their own Rust-side resources; menu.close() frees only the
// menu, so they're tracked and closed alongside it or they leak per click.
type LiveMenu = { menu: Menu; extras: CheckMenuItem[] };
let lastMenu: LiveMenu | null = null;
// Rapid right-clicks are last-writer-wins: superseded queued popups no-op.
let generation = 0;

async function closeMenu(m: LiveMenu | null): Promise<void> {
  if (!m) return;
  await Promise.all(
    [m.menu, ...m.extras].map((r) => r.close().catch(() => {}))
  );
}

// Steps must never reject (each catches internally) so one failure can't
// wedge the chain and kill right-click app-wide.
function enqueue(step: () => Promise<void>) {
  chain = chain.then(step);
}

// Rust-side menu resources are only freed via close(); the next pointer
// interaction after a menu is gone frees the last one, so at most one
// dismissed menu is ever kept alive in between.
let sweeperArmed = false;
function armMenuSweeper() {
  if (sweeperArmed) return;
  sweeperArmed = true;
  document.addEventListener(
    "pointerdown",
    () => {
      enqueue(async () => {
        await closeMenu(lastMenu);
        lastMenu = null;
      });
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
  // clicked, not wherever the OS last showed one. clientX/Y are in zoomed CSS
  // px (interface zoom is webview-native zoom); window-logical px = client × zoom.
  const zoom = useUiStore.getState().zoom;
  const clickX = e.clientX * zoom;
  const clickY = e.clientY * zoom;
  const gen = ++generation;
  enqueue(async () => {
    if (gen !== generation) return; // superseded while queued
    const extras: CheckMenuItem[] = [];
    let menu: Menu | null = null;
    try {
      // Popups anchor to the toplevel window, whose origin on Linux (CSD)
      // includes titlebar/shadow chrome outside the webview — Rust measures
      // that per popup (it changes on maximize). (0,0) on other platforms.
      const offset = invoke<[number, number]>("menu_popup_offset").catch(
        (err) => {
          console.warn("menu_popup_offset failed; menu may misplace:", err);
          return [0, 0] as [number, number];
        }
      );
      // Built sequentially so a failure can't race a still-pending item into
      // `extras` after the catch has already swept it.
      const menuItems: Array<
        CheckMenuItem | { item: "Separator" } | { text: string; action: () => void; enabled: boolean }
      > = [];
      for (const item of items) {
        if (item === "separator") {
          menuItems.push({ item: "Separator" });
        } else if (item.checked !== undefined) {
          const check = await CheckMenuItem.new({
            text: item.text,
            checked: item.checked,
            enabled: item.enabled ?? true,
            action: item.action,
          });
          extras.push(check);
          menuItems.push(check);
        } else {
          menuItems.push({ text: item.text, action: item.action, enabled: item.enabled ?? true });
        }
      }
      menu = await Menu.new({ items: menuItems });
      const built: LiveMenu = { menu, extras };
      await closeMenu(lastMenu);
      lastMenu = null;
      const [dx, dy] = await offset;
      if (gen !== generation) {
        // Superseded while building; no popup is in flight, so closing here
        // is safe. No awaits between this check and popup(), so it's final.
        await closeMenu(built);
        return;
      }
      lastMenu = built;
      // Resolves when the menu is dismissed, which is what keeps the chain
      // (and the resources-table mutex) honest.
      await menu.popup(new LogicalPosition(clickX + dx, clickY + dy));
    } catch (err) {
      // Free whatever was built before the failure — nothing else holds it.
      await Promise.all([menu, ...extras].map((r) => r?.close().catch(() => {})));
      if (lastMenu?.menu === menu) lastMenu = null;
      // Menu API systematically failing must not fail silent — right-click
      // would be dead app-wide with no trace.
      console.error("native context menu failed:", err);
    }
  });
}
