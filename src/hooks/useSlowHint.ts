import { useState, useEffect } from "react";

/**
 * True once `active` has stayed true for `delayMs` — for explaining a wait that
 * is sometimes instant and sometimes long, without flashing the explanation at
 * everyone whose operation was fast. Resets as soon as `active` goes false.
 */
export function useSlowHint(active: boolean, delayMs = 1500) {
  const [slow, setSlow] = useState(false);

  useEffect(() => {
    if (!active) {
      setSlow(false);
      return;
    }
    const timer = setTimeout(() => setSlow(true), delayMs);
    return () => clearTimeout(timer);
  }, [active, delayMs]);

  return slow;
}
