import { useUiStore } from "../../stores/ui";
import { MIN_ZOOM, MAX_ZOOM, ZOOM_STEP, DEFAULT_ZOOM } from "../../lib/zoom";
import { Button } from "../ui/button";

export function ZoomControl() {
  const zoom = useUiStore((s) => s.zoom);
  const setZoom = useUiStore((s) => s.setZoom);

  const percent = Math.round(zoom * 100);
  const atMin = zoom <= MIN_ZOOM;
  const atMax = zoom >= MAX_ZOOM;
  const atDefault = zoom === DEFAULT_ZOOM;

  return (
    <div>
      <p className="text-sm text-muted mb-2">Zoom</p>
      <div className="flex items-center gap-2">
        <Button
          variant="muted"
          size="sm"
          onClick={() => setZoom(zoom - ZOOM_STEP)}
          disabled={atMin}
          aria-label="Decrease zoom"
        >
          −
        </Button>
        <span
          className="text-sm text-text tabular-nums text-center"
          style={{ width: "3.5rem" }}
          aria-live="polite"
        >
          {percent}%
        </span>
        <Button
          variant="muted"
          size="sm"
          onClick={() => setZoom(zoom + ZOOM_STEP)}
          disabled={atMax}
          aria-label="Increase zoom"
        >
          +
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setZoom(DEFAULT_ZOOM)}
          disabled={atDefault}
        >
          Reset
        </Button>
      </div>
    </div>
  );
}
