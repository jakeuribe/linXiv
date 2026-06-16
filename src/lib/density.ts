export type Density = "comfortable" | "compact";

export const DEFAULT_DENSITY: Density = "comfortable";

export function normalizeDensity(value: unknown): Density {
  return value === "compact" ? "compact" : DEFAULT_DENSITY;
}

/**
 * Apply the interface density by marking the document root. CSS density vars
 * (--card-pad, --card-radius, --row-pad-y) key off this marker, so the restyled
 * surfaces reflow live.
 */
export function applyDensity(density: Density): void {
  document.documentElement.dataset.density = density;
}
