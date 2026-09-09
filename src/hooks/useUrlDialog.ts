import { useEffect, useRef } from "react";
import { useLocation, useNavigate, useSearchParams } from "react-router";

/** Dialog open-state as a URL search param: opening pushes a history entry,
 *  so the back button (in-app or browser) closes the dialog. */
export function useUrlDialog(param: string): {
  open: boolean;
  show: () => void;
  close: () => void;
} {
  const [searchParams, setSearchParams] = useSearchParams();
  const location = useLocation();
  const navigate = useNavigate();
  // close() must be idempotent: the Dialog's X button fires onClose twice
  // (Radix composes the child onClick with Root.onOpenChange), and a second
  // navigate(-1) would go one page too far back. Any location change re-arms.
  const closing = useRef(false);
  useEffect(() => {
    closing.current = false;
  }, [location]);
  return {
    open: searchParams.has(param),
    show: () =>
      setSearchParams((p) => {
        p.set(param, "1");
        return p;
      }),
    close: () => {
      if (closing.current) return;
      closing.current = true;
      // A deep-linked ?param=1 has no entry to pop; strip it in place.
      if (location.key !== "default") navigate(-1);
      else
        setSearchParams(
          (p) => {
            p.delete(param);
            return p;
          },
          { replace: true }
        );
    },
  };
}
