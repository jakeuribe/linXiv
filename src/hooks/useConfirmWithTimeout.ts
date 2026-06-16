import { useState, useRef, useEffect } from "react";

/**
 * Two-step "arm to confirm" guard for irreversible actions. The first call to
 * `arm()` flips `confirm` true for `timeoutMs`; acting again while `confirm` is
 * true is the real confirmation. It auto-disarms after the timeout (and on
 * unmount) so a button left armed can never fire a destructive action later.
 * Pair with `onBlur={disarm}` so navigating away cancels the armed state.
 */
export function useConfirmWithTimeout(timeoutMs = 3000) {
  const [confirm, setConfirm] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (timerRef.current) clearTimeout(timerRef.current);
  }, []);

  function arm() {
    setConfirm(true);
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => setConfirm(false), timeoutMs);
  }

  function disarm() {
    if (timerRef.current) clearTimeout(timerRef.current);
    setConfirm(false);
  }

  return { confirm, arm, disarm };
}
